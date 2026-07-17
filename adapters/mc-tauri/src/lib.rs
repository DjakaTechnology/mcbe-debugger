use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use mc_protocol::events::shared::StatDataModel;
use mc_session::{
    EvaluateResult, HandshakeInfo as SessionHandshakeInfo, SessionCommand, SessionController,
    SessionError, SessionEvent,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, Mutex};

/// Helper: convert a [`SessionError`] to its stable display string for
/// frontend-visible Tauri command error responses.
fn map_err<T>(r: Result<T, SessionError>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}

// ─── Tauri DTOs ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterInfo {
    pub protocol_version: u8,
    pub default_port: u16,
}

pub fn adapter_info() -> AdapterInfo {
    AdapterInfo {
        protocol_version: mc_protocol::ProtocolVersion::CURRENT.as_u8(),
        default_port: mc_protocol::DEFAULT_PORT,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub module_uuid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandshakeInfo {
    pub version: u8,
    pub plugins: Vec<PluginInfo>,
    pub require_passcode: bool,
}

impl From<SessionHandshakeInfo> for HandshakeInfo {
    fn from(hs: SessionHandshakeInfo) -> Self {
        Self {
            version: hs.version,
            plugins: hs
                .plugins
                .into_iter()
                .map(|p| PluginInfo {
                    name: p.name,
                    module_uuid: p.module_uuid,
                })
                .collect(),
            require_passcode: hs.require_passcode,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsePayload {
    pub success: bool,
    pub args: Option<serde_json::Value>,
    pub message: Option<String>,
}

/// Current desktop source-map configuration status.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMapStatus {
    pub enabled: bool,
    pub map_path: Option<String>,
    pub error: Option<String>,
}

/// A JavaScript stack frame extracted from a scripting-log message.
///
/// All DTO coordinates are one-based for display. A missing generated column
/// means the message only supplied a line number.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScriptFrame {
    pub function_name: Option<String>,
    pub generated_path: String,
    pub generated_line: u32,
    pub generated_column: Option<u32>,
    pub source_path: Option<String>,
    pub source_line: Option<u32>,
    pub source_column: Option<u32>,
    pub mapped: bool,
}

impl From<EvaluateResult> for ResponsePayload {
    fn from(r: EvaluateResult) -> Self {
        Self {
            success: r.success,
            args: r.args,
            message: r.message,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum McEvent {
    Protocol {
        version: u8,
        plugins: Vec<PluginInfo>,
        #[serde(rename = "requirePasscode")]
        require_passcode: bool,
    },
    Stopped {
        reason: String,
        thread: u32,
    },
    Thread {
        reason: String,
        thread: u32,
    },
    Print {
        message: String,
        #[serde(rename = "logLevel")]
        log_level: u8,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        frames: Vec<ScriptFrame>,
    },
    Notification {
        message: String,
        #[serde(rename = "logLevel")]
        log_level: u8,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        frames: Vec<ScriptFrame>,
    },
    Stat2 {
        tick: u64,
        stats: Vec<StatDataModel>,
    },
    ProfilerCapture {
        #[serde(rename = "captureBasePath")]
        capture_base_path: String,
    },
    Schema {
        count: usize,
    },
    Terminated {
        reason: Option<String>,
    },
    Unknown {
        #[serde(rename = "typeName")]
        type_name: String,
    },
}

impl From<mc_protocol::DebuggeeEvent> for McEvent {
    fn from(event: mc_protocol::DebuggeeEvent) -> Self {
        match event {
            mc_protocol::DebuggeeEvent::Protocol {
                version,
                plugins,
                require_passcode,
            } => Self::Protocol {
                version,
                plugins: plugins
                    .into_iter()
                    .map(|p| PluginInfo {
                        name: p.name,
                        module_uuid: p.module_uuid,
                    })
                    .collect(),
                require_passcode,
            },
            mc_protocol::DebuggeeEvent::Stopped { reason, thread } => {
                Self::Stopped { reason, thread }
            }
            mc_protocol::DebuggeeEvent::Thread { reason, thread } => {
                Self::Thread { reason, thread }
            }
            mc_protocol::DebuggeeEvent::Print { message, log_level } => Self::Print {
                frames: parse_script_frames(&message),
                message,
                log_level: log_level as u8,
            },
            mc_protocol::DebuggeeEvent::Notification { message, log_level } => Self::Notification {
                frames: parse_script_frames(&message),
                message,
                log_level: log_level as u8,
            },
            mc_protocol::DebuggeeEvent::Stat2 { tick, stats } => Self::Stat2 { tick, stats },
            mc_protocol::DebuggeeEvent::ProfilerCapture {
                capture_base_path, ..
            } => Self::ProfilerCapture { capture_base_path },
            mc_protocol::DebuggeeEvent::DebuggeeResponse { .. } => Self::Unknown {
                type_name: "debuggee-response".to_string(),
            },
            mc_protocol::DebuggeeEvent::Response { .. } => Self::Unknown {
                type_name: "response".to_string(),
            },
            mc_protocol::DebuggeeEvent::Schema { descriptors } => Self::Schema {
                count: descriptors.len(),
            },
            mc_protocol::DebuggeeEvent::Terminated { reason } => Self::Terminated { reason },
            mc_protocol::DebuggeeEvent::Unknown { type_name, .. } => Self::Unknown { type_name },
        }
    }
}

fn parse_script_frames(message: &str) -> Vec<ScriptFrame> {
    let mut frames = Vec::new();

    for line in message.lines() {
        let trimmed = line.trim_start();
        if let Some(frame) = trimmed.strip_prefix("at ").and_then(parse_stack_frame_body) {
            frames.push(frame);
            continue;
        }

        if !looks_like_inline_error(trimmed) {
            continue;
        }
        for (index, _) in trimmed.match_indices(" at ") {
            if let Some(frame) = parse_stack_frame_body(&trimmed[index + 4..]) {
                frames.push(frame);
            }
        }
    }

    frames
}

fn looks_like_inline_error(line: &str) -> bool {
    line.split_once(':').is_some_and(|(kind, _)| {
        let kind = kind.trim();
        kind.ends_with("Error") || kind.ends_with("Exception")
    })
}

fn parse_stack_frame_body(body: &str) -> Option<ScriptFrame> {
    let body = body.trim();
    let (function_name, location) = if let Some(without_paren) = body.strip_suffix(')') {
        let (function_name, location) = without_paren.rsplit_once(" (")?;
        let function_name = function_name.trim();
        if function_name.is_empty() || function_name.contains(" at ") {
            return None;
        }
        (Some(function_name.to_owned()), location)
    } else {
        (None, body)
    };

    let (generated_path, generated_line, generated_column) = parse_generated_location(location)?;
    Some(ScriptFrame {
        function_name,
        generated_path,
        generated_line,
        generated_column,
        source_path: None,
        source_line: None,
        source_column: None,
        mapped: false,
    })
}

fn parse_generated_location(location: &str) -> Option<(String, u32, Option<u32>)> {
    let mut components = location.rsplitn(3, ':');
    let final_number = components.next()?.parse::<u32>().ok()?;
    if final_number == 0 {
        return None;
    }
    let preceding = components.next()?;
    let remaining = components.next();

    let (path, line, column) = match (preceding.parse::<u32>(), remaining) {
        (Ok(line), Some(path)) if line > 0 => (path, line, Some(final_number)),
        _ => (preceding, final_number, None),
    };
    let path = normalize_script_path(path)?;
    Some((path, line, column))
}

fn normalize_script_path(path: &str) -> Option<String> {
    let normalized = path.trim().replace('\\', "/");
    let normalized = normalized.trim_start_matches('/');
    let normalized = normalized.strip_prefix("BP/").unwrap_or(normalized);
    match normalized {
        "main.js" | "scripts/main.js" => Some("/scripts/main.js".to_owned()),
        _ => None,
    }
}

fn enrich_frames(frames: &mut [ScriptFrame], source_maps: Option<&mc_source_maps::SourceMaps>) {
    let Some(source_maps) = source_maps else {
        return;
    };

    for frame in frames {
        let generated_line = frame.generated_line - 1;
        let generated_column = frame.generated_column.map_or(0, |column| column - 1);
        if let Ok(original) = source_maps.generated_to_original(
            &frame.generated_path,
            generated_line,
            generated_column,
        ) {
            frame.source_path = Some(original.path.to_string_lossy().into_owned());
            frame.source_line = Some(original.line.saturating_add(1));
            frame.source_column = Some(original.column.saturating_add(1));
            frame.mapped = true;
        }
    }
}

fn enrich_event(event: &mut McEvent, source_maps: Option<&mc_source_maps::SourceMaps>) {
    match event {
        McEvent::Print { frames, .. } | McEvent::Notification { frames, .. } => {
            enrich_frames(frames, source_maps);
        }
        _ => {}
    }
}

// ─── Event Bridge ─────────────────────────────────────────────────────

/// Bridge framework‑neutral [`SessionEvent`]s to Tauri events.
///
/// Emits exactly these Tauri event names, matching the existing frontend:
/// * `mc-event` — debuggee protocol events (mapped through [`McEvent`])
/// * `mc-target-selection-required` — plugin selection prompt
/// * `mc-disconnected` — unexpected connection loss
/// * `mc-terminated` — debuggee termination
type SharedSourceMaps = Arc<RwLock<Option<mc_source_maps::SourceMaps>>>;

async fn event_bridge(
    mut rx: mpsc::Receiver<SessionEvent>,
    app: AppHandle,
    source_maps: SharedSourceMaps,
) {
    while let Some(event) = rx.recv().await {
        match event {
            SessionEvent::Debuggee(debuggee_event) => {
                let mut event = McEvent::from(debuggee_event);
                {
                    let maps = source_maps
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    enrich_event(&mut event, maps.as_ref());
                }
                let _ = app.emit("mc-event", event);
            }
            SessionEvent::TargetSelectionRequired { plugins } => {
                let plugin_infos: Vec<PluginInfo> = plugins
                    .into_iter()
                    .map(|p| PluginInfo {
                        name: p.name,
                        module_uuid: p.module_uuid,
                    })
                    .collect();
                let _ = app.emit("mc-target-selection-required", &plugin_infos);
            }
            SessionEvent::Disconnected => {
                let _ = app.emit("mc-disconnected", ());
            }
            SessionEvent::Terminated { .. } => {
                let _ = app.emit("mc-terminated", ());
            }
        }
    }
}

// ─── AppState ─────────────────────────────────────────────────────────

/// Thin Tauri‑managed state that owns a [`SessionController`] and the event
/// bridge lifecycle.
pub struct AppState {
    controller: SessionController,
    event_rx: Mutex<Option<mpsc::Receiver<SessionEvent>>>,
    bridge_started: std::sync::atomic::AtomicBool,
    source_maps: SharedSourceMaps,
}

impl AppState {
    pub fn new() -> Self {
        let (controller, event_rx) = SessionController::new();
        Self {
            controller,
            event_rx: Mutex::new(Some(event_rx)),
            bridge_started: std::sync::atomic::AtomicBool::new(false),
            source_maps: Arc::new(RwLock::new(None)),
        }
    }

    /// Ensure the event bridge is running.  Safe to call multiple times;
    /// the bridge is started at most once.
    async fn ensure_bridge(&self, app: &AppHandle) {
        if self
            .bridge_started
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let mut lock = self.event_rx.lock().await;
        let rx = lock
            .take()
            .expect("ensure_bridge called after event_rx consumed");
        let app_handle = app.clone();
        let source_maps = Arc::clone(&self.source_maps);
        tokio::spawn(event_bridge(rx, app_handle, source_maps));
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Public free functions ────────────────────────────────────────────
//
// Signatures remain compatible with `apps/mc-desktop/src-tauri/src/lib.rs`.

/// Changes the workspace used for desktop scripting-log source mapping.
///
/// Empty values disable mapping. Load failures are represented in the returned
/// status and leave the event bridge running with mapping disabled.
pub fn set_workspace_root(state: &AppState, workspace_root: Option<String>) -> WorkspaceMapStatus {
    let Some(workspace_root) = workspace_root.filter(|root| !root.trim().is_empty()) else {
        *state
            .source_maps
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        return WorkspaceMapStatus {
            enabled: false,
            map_path: None,
            error: None,
        };
    };

    let workspace_root = absolute_path(Path::new(workspace_root.trim()));
    let map_path = workspace_root
        .join("BP")
        .join("scripts")
        .join("main.js.map");
    let result = mc_source_maps::SourceMaps::from_workspace(&workspace_root);
    let (maps, error) = match result {
        Ok(maps) => (Some(maps), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let enabled = maps.is_some();
    *state
        .source_maps
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = maps;

    WorkspaceMapStatus {
        enabled,
        map_path: Some(map_path.to_string_lossy().into_owned()),
        error,
    }
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

pub async fn listen_to_minecraft(
    state: &AppState,
    app: AppHandle,
    port: u16,
    target_module_uuid: Option<String>,
    passcode: Option<String>,
) -> Result<HandshakeInfo, String> {
    state.ensure_bridge(&app).await;
    let hs = map_err(
        state
            .controller
            .listen(port, target_module_uuid, passcode)
            .await,
    )?;
    Ok(HandshakeInfo::from(hs))
}

pub async fn connect_to_minecraft(
    state: &AppState,
    app: AppHandle,
    host: String,
    port: u16,
    target_module_uuid: Option<String>,
    passcode: Option<String>,
) -> Result<HandshakeInfo, String> {
    state.ensure_bridge(&app).await;
    let hs = map_err(
        state
            .controller
            .connect(host, port, target_module_uuid, passcode)
            .await,
    )?;
    Ok(HandshakeInfo::from(hs))
}

pub async fn disconnect(state: &AppState) -> Result<(), String> {
    map_err(state.controller.disconnect().await)
}

pub async fn cancel_pending_connect(state: &AppState) -> Result<(), String> {
    map_err(state.controller.cancel_pending().await)
}

pub async fn select_target_module(state: &AppState, module_uuid: String) -> Result<(), String> {
    map_err(state.controller.select_target(module_uuid).await)
}

pub async fn send_minecraft_command(state: &AppState, command: String) -> Result<(), String> {
    map_err(
        state
            .controller
            .send_command(SessionCommand::SendMinecraftCommand { command })
            .await,
    )
}

pub async fn get_handshake_info(state: &AppState) -> Result<Option<HandshakeInfo>, String> {
    let hs = map_err(state.controller.get_handshake_info().await)?;
    Ok(hs.map(HandshakeInfo::from))
}

pub async fn pause_thread(state: &AppState, thread_id: u32) -> Result<(), String> {
    map_err(
        state
            .controller
            .send_command(SessionCommand::Pause { thread_id })
            .await,
    )
}

pub async fn continue_thread(state: &AppState, thread_id: u32) -> Result<(), String> {
    map_err(
        state
            .controller
            .send_command(SessionCommand::Continue { thread_id })
            .await,
    )
}

pub async fn step_next(state: &AppState, thread_id: u32) -> Result<(), String> {
    map_err(
        state
            .controller
            .send_command(SessionCommand::StepNext { thread_id })
            .await,
    )
}

pub async fn step_in(state: &AppState, thread_id: u32) -> Result<(), String> {
    map_err(
        state
            .controller
            .send_command(SessionCommand::StepIn { thread_id })
            .await,
    )
}

pub async fn step_out(state: &AppState, thread_id: u32) -> Result<(), String> {
    map_err(
        state
            .controller
            .send_command(SessionCommand::StepOut { thread_id })
            .await,
    )
}

pub async fn evaluate(state: &AppState, expression: String) -> Result<ResponsePayload, String> {
    let result = map_err(state.controller.evaluate(expression).await)?;
    Ok(ResponsePayload::from(result))
}

// ─── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "mc-tauri-source-maps-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(root.join("BP").join("scripts")).expect("create test workspace");
            Self { root }
        }

        fn map_path(&self) -> PathBuf {
            self.root.join("BP").join("scripts").join("main.js.map")
        }

        fn write_valid_map(&self) {
            fs::write(
                self.map_path(),
                r#"{
                    "version": 3,
                    "file": "main.js",
                    "sourceRoot": "../../",
                    "sources": ["src/main.ts"],
                    "names": [],
                    "mappings": "AAAA"
                }"#,
            )
            .expect("write source map");
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn handshake_info_serializes_camel_case() {
        let info = HandshakeInfo {
            version: 7,
            plugins: vec![
                PluginInfo {
                    name: "bp.main".into(),
                    module_uuid: "abc-123".into(),
                },
                PluginInfo {
                    name: "bp.extra".into(),
                    module_uuid: "def-456".into(),
                },
            ],
            require_passcode: true,
        };

        let json = serde_json::to_value(&info).unwrap();
        let map = json.as_object().unwrap();

        // Top-level fields are camelCase
        assert!(
            map.contains_key("requirePasscode"),
            "should have requirePasscode"
        );
        assert!(map.contains_key("version"), "should have version");
        assert!(map.contains_key("plugins"), "should have plugins");

        // No snake_case top-level keys
        assert!(
            !map.contains_key("require_passcode"),
            "should NOT have require_passcode"
        );

        // Values
        assert_eq!(map["requirePasscode"], true);
        assert_eq!(map["version"], 7);

        // PluginInfo fields remain snake_case
        let plugins = map["plugins"].as_array().unwrap();
        assert_eq!(plugins.len(), 2);
        let p0 = plugins[0].as_object().unwrap();
        assert!(
            p0.contains_key("module_uuid"),
            "PluginInfo should have module_uuid (snake_case)"
        );
        assert_eq!(p0["module_uuid"], "abc-123");
        assert_eq!(p0["name"], "bp.main");
        // PluginInfo should NOT have camelCase keys
        assert!(
            !p0.contains_key("moduleUuid"),
            "PluginInfo should NOT have moduleUuid"
        );
    }

    // ── McEvent mapping tests ──────────────────────────────────────────

    #[test]
    fn mc_event_from_debuggee_protocol() {
        let event = mc_protocol::DebuggeeEvent::Protocol {
            version: 8,
            plugins: vec![mc_protocol::events::PluginDetails {
                name: "test".into(),
                module_uuid: "xyz".into(),
            }],
            require_passcode: false,
        };
        let mc: McEvent = event.into();
        let json = serde_json::to_value(&mc).unwrap();
        assert_eq!(json["kind"], "protocol");
        assert_eq!(json["version"], 8);
        assert_eq!(json["requirePasscode"], false);
        assert!(json.get("require_passcode").is_none());
    }

    #[test]
    fn mc_event_from_debuggee_stopped() {
        let event = mc_protocol::DebuggeeEvent::Stopped {
            reason: "breakpoint".into(),
            thread: 1,
        };
        let mc: McEvent = event.into();
        let json = serde_json::to_value(&mc).unwrap();
        assert_eq!(json["kind"], "stopped");
        assert_eq!(json["reason"], "breakpoint");
        assert_eq!(json["thread"], 1);
    }

    #[test]
    fn mc_event_from_debuggee_terminated() {
        let event = mc_protocol::DebuggeeEvent::Terminated {
            reason: Some("done".into()),
        };
        let mc: McEvent = event.into();
        let json = serde_json::to_value(&mc).unwrap();
        assert_eq!(json["kind"], "terminated");
        assert_eq!(json["reason"], "done");
    }

    #[test]
    fn mc_event_uses_frontend_camel_case_fields() {
        let cases = [
            (
                McEvent::Print {
                    message: "m".into(),
                    log_level: 2,
                    frames: Vec::new(),
                },
                "logLevel",
            ),
            (
                McEvent::Notification {
                    message: "n".into(),
                    log_level: 1,
                    frames: Vec::new(),
                },
                "logLevel",
            ),
            (
                McEvent::ProfilerCapture {
                    capture_base_path: "/tmp".into(),
                },
                "captureBasePath",
            ),
            (
                McEvent::Unknown {
                    type_name: "x".into(),
                },
                "typeName",
            ),
        ];
        for (event, field) in cases {
            let json = serde_json::to_value(event).unwrap();
            assert!(json.get(field).is_some(), "missing {field}: {json}");
        }
    }

    #[test]
    fn parses_inline_error_and_following_anonymous_frame_in_order() {
        let message = "Error: shield registration failed at registerShieldSystem (main.js:2394)\n    at <anonymous> (main.js:2456)";
        let frames = parse_script_frames(message);

        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[0],
            ScriptFrame {
                function_name: Some("registerShieldSystem".into()),
                generated_path: "/scripts/main.js".into(),
                generated_line: 2394,
                generated_column: None,
                source_path: None,
                source_line: None,
                source_column: None,
                mapped: false,
            }
        );
        assert_eq!(frames[1].function_name.as_deref(), Some("<anonymous>"));
        assert_eq!(frames[1].generated_line, 2456);
    }

    #[test]
    fn parses_supported_direct_parenthesized_and_column_forms() {
        let frames = parse_script_frames(
            "at main.js:2394\n at run (BP/scripts/main.js:2395)\n at other (/scripts/main.js:2396:17)\n at scripts\\main.js:2397",
        );

        assert_eq!(frames.len(), 4);
        assert_eq!(frames[0].function_name, None);
        assert_eq!(
            (frames[0].generated_line, frames[0].generated_column),
            (2394, None)
        );
        assert_eq!(frames[1].function_name.as_deref(), Some("run"));
        assert_eq!(
            (frames[1].generated_line, frames[1].generated_column),
            (2395, None)
        );
        assert_eq!(frames[2].function_name.as_deref(), Some("other"));
        assert_eq!(
            (frames[2].generated_line, frames[2].generated_column),
            (2396, Some(17))
        );
        assert!(frames
            .iter()
            .all(|frame| frame.generated_path == "/scripts/main.js"));
    }

    #[test]
    fn ignores_non_stack_prose_containing_a_script_location() {
        let message = "See main.js:2394 for details\nWe looked at main.js:2394 yesterday.";
        assert!(parse_script_frames(message).is_empty());
    }

    #[test]
    fn enrichment_converts_display_coordinates_through_zero_based_resolver() {
        let workspace = TestWorkspace::new();
        workspace.write_valid_map();
        let maps =
            mc_source_maps::SourceMaps::from_workspace(&workspace.root).expect("load source map");
        let message = "Error: failed at bootstrap (main.js:1)";
        let mut event: McEvent = mc_protocol::DebuggeeEvent::Print {
            message: message.into(),
            log_level: mc_protocol::events::LogLevel::Error,
        }
        .into();

        enrich_event(&mut event, Some(&maps));
        let McEvent::Print {
            message: emitted_message,
            frames,
            ..
        } = event
        else {
            panic!("expected print event");
        };
        assert_eq!(emitted_message, message);
        assert_eq!(frames.len(), 1);
        let frame = &frames[0];
        assert_eq!((frame.generated_line, frame.generated_column), (1, None));
        let expected_source = workspace
            .root
            .join("src")
            .join("main.ts")
            .to_string_lossy()
            .into_owned();
        assert_eq!(frame.source_path.as_deref(), Some(expected_source.as_str()));
        assert_eq!((frame.source_line, frame.source_column), (Some(1), Some(1)));
        assert!(frame.mapped);
    }

    #[test]
    fn absent_resolver_and_lookup_failure_keep_generated_unmapped_frames() {
        let mut absent = parse_script_frames("at main.js:1");
        enrich_frames(&mut absent, None);
        assert_eq!(absent.len(), 1);
        assert_eq!(absent[0].generated_column, None);
        assert_eq!(absent[0].source_path, None);
        assert!(!absent[0].mapped);

        let workspace = TestWorkspace::new();
        workspace.write_valid_map();
        let maps =
            mc_source_maps::SourceMaps::from_workspace(&workspace.root).expect("load source map");
        let mut missing_line = parse_script_frames("at main.js:2:3");
        enrich_frames(&mut missing_line, Some(&maps));
        assert_eq!(missing_line[0].generated_line, 2);
        assert_eq!(missing_line[0].source_line, None);
        assert!(!missing_line[0].mapped);
    }

    #[test]
    fn frame_serialization_is_camel_case_and_empty_frames_are_omitted() {
        let empty = serde_json::to_value(McEvent::Print {
            message: "plain".into(),
            log_level: 1,
            frames: Vec::new(),
        })
        .expect("serialize empty event");
        assert!(empty.get("frames").is_none());

        let event = McEvent::Notification {
            message: "at main.js:1:2".into(),
            log_level: 2,
            frames: parse_script_frames("at main.js:1:2"),
        };
        let json = serde_json::to_value(event).expect("serialize frame event");
        let frame = &json["frames"][0];
        for field in [
            "functionName",
            "generatedPath",
            "generatedLine",
            "generatedColumn",
            "sourcePath",
            "sourceLine",
            "sourceColumn",
            "mapped",
        ] {
            assert!(frame.get(field).is_some(), "missing {field}: {frame}");
        }
        assert!(frame.get("generated_path").is_none());
    }

    #[test]
    fn workspace_status_controls_cache_for_disabled_and_load_outcomes() {
        let state = AppState::new();

        let disabled = set_workspace_root(&state, Some("   ".into()));
        assert_eq!(
            disabled,
            WorkspaceMapStatus {
                enabled: false,
                map_path: None,
                error: None,
            }
        );
        assert!(state.source_maps.read().unwrap().is_none());

        let workspace = TestWorkspace::new();
        let expected_map_path = workspace
            .root
            .join("BP")
            .join("scripts")
            .join("main.js.map")
            .to_string_lossy()
            .into_owned();
        let missing =
            set_workspace_root(&state, Some(workspace.root.to_string_lossy().into_owned()));
        assert!(!missing.enabled);
        assert_eq!(
            missing.map_path.as_deref(),
            Some(expected_map_path.as_str())
        );
        assert!(missing.error.is_some());
        assert!(state.source_maps.read().unwrap().is_none());

        fs::write(workspace.map_path(), "{malformed").expect("write malformed map");
        let malformed =
            set_workspace_root(&state, Some(workspace.root.to_string_lossy().into_owned()));
        assert!(!malformed.enabled);
        assert_eq!(
            malformed.map_path.as_deref(),
            Some(expected_map_path.as_str())
        );
        assert!(malformed.error.is_some());
        assert!(state.source_maps.read().unwrap().is_none());

        workspace.write_valid_map();
        let valid = set_workspace_root(&state, Some(workspace.root.to_string_lossy().into_owned()));
        assert_eq!(
            valid,
            WorkspaceMapStatus {
                enabled: true,
                map_path: Some(expected_map_path),
                error: None,
            }
        );
        let maps = state.source_maps.read().unwrap();
        assert!(maps
            .as_ref()
            .expect("cached source map")
            .generated_to_original("/scripts/main.js", 0, 0)
            .is_ok());
        drop(maps);

        let cleared = set_workspace_root(&state, None);
        assert!(!cleared.enabled);
        assert_eq!(cleared.map_path, None);
        assert_eq!(cleared.error, None);
        assert!(state.source_maps.read().unwrap().is_none());
    }

    #[test]
    fn response_payload_from_evaluate_result() {
        let result = EvaluateResult {
            success: true,
            args: Some(serde_json::json!({"x": 1})),
            message: Some("ok".into()),
        };
        let payload: ResponsePayload = result.into();
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["args"]["x"], 1);
        assert_eq!(json["message"], "ok");
    }
}
