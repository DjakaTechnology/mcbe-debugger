use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::{env, fs};

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
    pub source_map_status: WorkspaceMapStatus,
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
            source_map_status: WorkspaceMapStatus {
                enabled: false,
                map_path: None,
                error: None,
            },
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

#[derive(Debug, Deserialize)]
struct PackManifest {
    header: Option<PackHeader>,
    #[serde(default)]
    modules: Vec<PackModule>,
}

#[derive(Debug, Deserialize)]
struct PackHeader {
    uuid: String,
}

#[derive(Debug, Deserialize)]
struct PackModule {
    #[serde(rename = "type")]
    module_type: String,
    uuid: String,
    entry: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegolithConfig {
    packs: RegolithPacks,
}

#[derive(Debug, Deserialize)]
struct RegolithPacks {
    #[serde(rename = "behaviorPack")]
    behavior_pack: String,
    #[serde(rename = "resourcePack")]
    resource_pack: Option<String>,
}

#[derive(Debug, Clone)]
struct WorkspaceProject {
    root: PathBuf,
    behavior_pack_path: PathBuf,
    resource_pack_path: Option<PathBuf>,
    behavior_pack_uuid: Option<String>,
    resource_pack_uuid: Option<String>,
    script_module_uuids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    pub root: String,
    pub behavior_pack_path: String,
    pub resource_pack_path: Option<String>,
    pub behavior_pack_uuid: Option<String>,
    pub resource_pack_uuid: Option<String>,
    pub script_module_uuids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSelection {
    pub workspace: Option<WorkspaceInfo>,
    pub source_map_status: WorkspaceMapStatus,
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
            frame.source_path = Some(if original.path.exists() {
                original.path.to_string_lossy().into_owned()
            } else {
                original.source_reference
            });
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
    selected_module_uuid: RwLock<Option<String>>,
    manual_source_map_path: RwLock<Option<PathBuf>>,
    workspace: RwLock<Option<WorkspaceProject>>,
}

impl AppState {
    pub fn new() -> Self {
        let (controller, event_rx) = SessionController::new();
        Self {
            controller,
            event_rx: Mutex::new(Some(event_rx)),
            bridge_started: std::sync::atomic::AtomicBool::new(false),
            source_maps: Arc::new(RwLock::new(None)),
            selected_module_uuid: RwLock::new(None),
            manual_source_map_path: RwLock::new(None),
            workspace: RwLock::new(None),
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

/// Finds and loads the connected behavior pack's source map automatically.
///
/// `MOJANG_DIR` is checked first. On Windows, standard stable and Preview
/// `com.mojang` directories are fallback candidates. Pack manifests are matched
/// by the selected script-module UUID, then the module entry is resolved to
/// either `main.js.map` or `main.map.js` in the pack's scripts directory.
fn configure_source_maps_for_module(
    state: &AppState,
    module_uuid: Option<&str>,
) -> WorkspaceMapStatus {
    let Some(module_uuid) = module_uuid else {
        return replace_source_maps(
            state,
            None,
            WorkspaceMapStatus {
                enabled: false,
                map_path: None,
                error: Some("no connected script module is selected".into()),
            },
        );
    };

    let workspace = state
        .workspace
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(workspace) = workspace {
        let matches = workspace
            .script_module_uuids
            .iter()
            .any(|uuid| uuid.eq_ignore_ascii_case(module_uuid));
        if !matches {
            return replace_source_maps(
                state,
                None,
                WorkspaceMapStatus {
                    enabled: false,
                    map_path: None,
                    error: Some(format!(
                        "open workspace does not contain connected script module {module_uuid}"
                    )),
                },
            );
        }
    }

    let candidates = mojang_dir_candidates();
    if candidates.is_empty() {
        return replace_source_maps(
            state,
            None,
            WorkspaceMapStatus {
                enabled: false,
                map_path: None,
                error: Some(
                    "MOJANG_DIR is not set and no standard com.mojang directory was found".into(),
                ),
            },
        );
    }

    let mut failures = Vec::new();
    for mojang_dir in candidates {
        match find_source_map_for_module(&mojang_dir, module_uuid) {
            Ok((pack_root, map_path)) => {
                let source_base = source_base_for_module(state, module_uuid);
                let loaded = match source_base {
                    Some(project_root) => {
                        mc_source_maps::SourceMaps::from_map_file_with_source_base(
                            &map_path,
                            &pack_root,
                            project_root,
                        )
                    }
                    None => mc_source_maps::SourceMaps::from_map_file(&map_path, &pack_root),
                };
                return match loaded {
                    Ok(maps) => replace_source_maps(
                        state,
                        Some(maps),
                        WorkspaceMapStatus {
                            enabled: true,
                            map_path: Some(map_path.to_string_lossy().into_owned()),
                            error: None,
                        },
                    ),
                    Err(error) => replace_source_maps(
                        state,
                        None,
                        WorkspaceMapStatus {
                            enabled: false,
                            map_path: Some(map_path.to_string_lossy().into_owned()),
                            error: Some(error.to_string()),
                        },
                    ),
                };
            }
            Err(error) => failures.push(error),
        }
    }

    replace_source_maps(
        state,
        None,
        WorkspaceMapStatus {
            enabled: false,
            map_path: None,
            error: Some(failures.join("; ")),
        },
    )
}

/// Sets or clears a manual source-map override. Clearing returns to automatic
/// detection for the currently selected module.
pub fn set_source_map_path(
    state: &AppState,
    source_map_path: Option<String>,
) -> WorkspaceMapStatus {
    let source_map_path = source_map_path
        .map(|path| path.trim().to_owned())
        .filter(|path| !path.is_empty());
    *state
        .manual_source_map_path
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        source_map_path.as_deref().map(absolute_path);

    if let Some(path) = source_map_path {
        return configure_source_maps_from_path(state, &absolute_path(&path));
    }

    let selected = state
        .selected_module_uuid
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if selected.is_none() {
        return replace_source_maps(
            state,
            None,
            WorkspaceMapStatus {
                enabled: false,
                map_path: None,
                error: None,
            },
        );
    }
    configure_source_maps_for_module(state, selected.as_deref())
}

fn configure_source_maps_from_path(state: &AppState, input: &Path) -> WorkspaceMapStatus {
    let selected = state
        .selected_module_uuid
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let (map_path, generated_root) = match resolve_manual_map_path(input, selected.as_deref()) {
        Ok(location) => location,
        Err(error) => {
            return replace_source_maps(
                state,
                None,
                WorkspaceMapStatus {
                    enabled: false,
                    map_path: Some(input.to_string_lossy().into_owned()),
                    error: Some(error),
                },
            );
        }
    };
    let source_base = project_root_for_path(&map_path).or_else(|| {
        selected
            .as_deref()
            .and_then(|uuid| source_base_for_module(state, uuid))
    });
    let loaded = match source_base {
        Some(source_base) => mc_source_maps::SourceMaps::from_map_file_with_source_base(
            &map_path,
            &generated_root,
            source_base,
        ),
        None => mc_source_maps::SourceMaps::from_map_file(&map_path, &generated_root),
    };

    match loaded {
        Ok(maps) => replace_source_maps(
            state,
            Some(maps),
            WorkspaceMapStatus {
                enabled: true,
                map_path: Some(map_path.to_string_lossy().into_owned()),
                error: None,
            },
        ),
        Err(error) => replace_source_maps(
            state,
            None,
            WorkspaceMapStatus {
                enabled: false,
                map_path: Some(map_path.to_string_lossy().into_owned()),
                error: Some(error.to_string()),
            },
        ),
    }
}

fn resolve_manual_map_path(
    input: &Path,
    selected_module_uuid: Option<&str>,
) -> Result<(PathBuf, PathBuf), String> {
    if input.is_file() {
        return Ok((input.to_path_buf(), infer_generated_root(input)));
    }
    if !input.is_dir() {
        return Err(format!(
            "source-map path '{}' does not exist",
            input.display()
        ));
    }

    let pack_roots = [
        input.to_path_buf(),
        input.join("BP"),
        input.join("packs").join("BP"),
    ];
    for pack_root in pack_roots {
        if !pack_root.join("manifest.json").is_file() {
            continue;
        }
        let manifest = read_pack_manifest(&pack_root)?;
        let script_modules = manifest
            .modules
            .iter()
            .filter(|module| module.module_type == "script")
            .collect::<Vec<_>>();
        let selected_modules = if let Some(uuid) = selected_module_uuid {
            let matching = script_modules
                .into_iter()
                .filter(|module| module.uuid.eq_ignore_ascii_case(uuid))
                .collect::<Vec<_>>();
            if matching.is_empty() {
                return Err(format!(
                    "pack '{}' has no script module matching {uuid}",
                    pack_root.display()
                ));
            }
            matching
        } else {
            script_modules
        };
        let maps = selected_modules
            .into_iter()
            .flat_map(|module| map_candidates_for_entry(&pack_root, module.entry.as_deref()))
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        match maps.as_slice() {
            [map_path] => return Ok((map_path.clone(), pack_root)),
            [] => {}
            _ => {
                return Err(format!(
                    "multiple source maps match '{}'; select a map file directly",
                    pack_root.display()
                ));
            }
        }
    }

    let mut maps = Vec::new();
    collect_map_files(input, &mut maps);
    let scripts = input.join("scripts");
    if scripts.is_dir() {
        collect_map_files(&scripts, &mut maps);
    }
    maps.sort();
    maps.dedup();
    match maps.as_slice() {
        [map_path] => Ok((map_path.clone(), infer_generated_root(map_path))),
        [] => Err(format!(
            "no source map found at or directly below '{}'",
            input.display()
        )),
        _ => Err(format!(
            "multiple source maps found below '{}'; select a map file directly",
            input.display()
        )),
    }
}

fn map_candidates_for_entry(pack_root: &Path, entry: Option<&str>) -> Vec<PathBuf> {
    let Some(entry) = entry else {
        return Vec::new();
    };
    let generated_path = pack_root.join(portable_relative_path(entry));
    let mut candidates = vec![PathBuf::from(format!(
        "{}.map",
        generated_path.to_string_lossy()
    ))];
    if let Some(stem) = generated_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".js"))
    {
        candidates.push(generated_path.with_file_name(format!("{stem}.map.js")));
    }
    candidates
}

fn collect_map_files(directory: &Path, maps: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    maps.extend(
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.ends_with(".map") || name.ends_with(".map.js"))
            }),
    );
}

fn infer_generated_root(map_path: &Path) -> PathBuf {
    map_path
        .parent()
        .and_then(|parent| {
            (parent.file_name().and_then(|name| name.to_str()) == Some("scripts"))
                .then(|| parent.parent())
                .flatten()
        })
        .or_else(|| map_path.parent())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn project_root_for_path(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| ancestor.join("config.json").is_file())
        .map(Path::to_path_buf)
}

fn absolute_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map(|current| current.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn replace_source_maps(
    state: &AppState,
    maps: Option<mc_source_maps::SourceMaps>,
    status: WorkspaceMapStatus,
) -> WorkspaceMapStatus {
    *state
        .source_maps
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = maps;
    status
}

fn mojang_dir_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(value) = env::var_os("MOJANG_DIR").filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(value));
    }

    #[cfg(windows)]
    if let Some(app_data) = env::var_os("APPDATA") {
        let app_data = PathBuf::from(app_data);
        candidates.push(
            app_data
                .join("Minecraft Bedrock")
                .join("Users")
                .join("Shared")
                .join("games")
                .join("com.mojang"),
        );
        candidates.push(
            app_data
                .join("Minecraft Bedrock Preview")
                .join("Users")
                .join("Shared")
                .join("games")
                .join("com.mojang"),
        );
    }

    candidates.retain(|path| path.is_dir());
    candidates.dedup();
    candidates
}

pub fn set_workspace_root(
    state: &AppState,
    workspace_root: Option<String>,
) -> Result<WorkspaceSelection, String> {
    let workspace_root = workspace_root
        .map(|path| path.trim().to_owned())
        .filter(|path| !path.is_empty());
    let project = workspace_root
        .as_deref()
        .map(absolute_path)
        .map(load_workspace_project)
        .transpose()?;
    *state
        .workspace
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = project.clone();

    let source_map_status = refresh_source_maps(state);
    Ok(WorkspaceSelection {
        workspace: project.map(workspace_info),
        source_map_status,
    })
}

fn load_workspace_project(root: PathBuf) -> Result<WorkspaceProject, String> {
    let root = fs::canonicalize(&root).unwrap_or(root);
    let config_path = root.join("config.json");
    let config_bytes = fs::read(&config_path)
        .map_err(|error| format!("could not read '{}': {error}", config_path.display()))?;
    let config: RegolithConfig = serde_json::from_slice(&config_bytes).map_err(|error| {
        format!(
            "invalid Regolith config '{}': {error}",
            config_path.display()
        )
    })?;

    let behavior_pack_path = root.join(portable_relative_path(&config.packs.behavior_pack));
    let behavior_manifest = read_pack_manifest(&behavior_pack_path)?;
    let resource_pack_path = config
        .packs
        .resource_pack
        .as_deref()
        .map(|path| root.join(portable_relative_path(path)));
    let resource_manifest = resource_pack_path
        .as_deref()
        .map(read_pack_manifest)
        .transpose()?;
    let script_module_uuids = behavior_manifest
        .modules
        .iter()
        .filter(|module| module.module_type == "script")
        .map(|module| module.uuid.clone())
        .collect();

    Ok(WorkspaceProject {
        root,
        behavior_pack_path,
        resource_pack_path,
        behavior_pack_uuid: behavior_manifest.header.map(|header| header.uuid),
        resource_pack_uuid: resource_manifest
            .and_then(|manifest| manifest.header.map(|header| header.uuid)),
        script_module_uuids,
    })
}

fn read_pack_manifest(pack_path: &Path) -> Result<PackManifest, String> {
    let manifest_path = pack_path.join("manifest.json");
    let bytes = fs::read(&manifest_path)
        .map_err(|error| format!("could not read '{}': {error}", manifest_path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "invalid pack manifest '{}': {error}",
            manifest_path.display()
        )
    })
}

fn source_base_for_module(state: &AppState, module_uuid: &str) -> Option<PathBuf> {
    let workspace = state
        .workspace
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(workspace) = workspace {
        if workspace
            .script_module_uuids
            .iter()
            .any(|uuid| uuid.eq_ignore_ascii_case(module_uuid))
        {
            return Some(workspace.root);
        }
    }

    env::var_os("REGOLITH_PROJECT_ROOT")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .and_then(|root| load_workspace_project(root).ok())
        .filter(|project| {
            project
                .script_module_uuids
                .iter()
                .any(|uuid| uuid.eq_ignore_ascii_case(module_uuid))
        })
        .map(|project| project.root)
}

fn refresh_source_maps(state: &AppState) -> WorkspaceMapStatus {
    let manual = state
        .manual_source_map_path
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(path) = manual {
        return configure_source_maps_from_path(state, &path);
    }
    let selected = state
        .selected_module_uuid
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match selected {
        Some(uuid) => configure_source_maps_for_module(state, Some(&uuid)),
        None => replace_source_maps(
            state,
            None,
            WorkspaceMapStatus {
                enabled: false,
                map_path: None,
                error: None,
            },
        ),
    }
}

fn workspace_info(project: WorkspaceProject) -> WorkspaceInfo {
    WorkspaceInfo {
        root: project.root.to_string_lossy().into_owned(),
        behavior_pack_path: project.behavior_pack_path.to_string_lossy().into_owned(),
        resource_pack_path: project
            .resource_pack_path
            .map(|path| path.to_string_lossy().into_owned()),
        behavior_pack_uuid: project.behavior_pack_uuid,
        resource_pack_uuid: project.resource_pack_uuid,
        script_module_uuids: project.script_module_uuids,
    }
}

fn portable_relative_path(path: &str) -> PathBuf {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .fold(PathBuf::new(), |path, part| path.join(part))
}

fn find_source_map_for_module(
    mojang_dir: &Path,
    module_uuid: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let mut matching_pack_without_map = None;

    for collection in ["development_behavior_packs", "behavior_packs"] {
        let collection_root = mojang_dir.join(collection);
        let Ok(entries) = fs::read_dir(&collection_root) else {
            continue;
        };
        let mut pack_roots = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        pack_roots.sort();

        for pack_root in pack_roots {
            let manifest_path = pack_root.join("manifest.json");
            let Ok(manifest_bytes) = fs::read(&manifest_path) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_slice::<PackManifest>(&manifest_bytes) else {
                continue;
            };
            let Some(script_module) = manifest.modules.iter().find(|module| {
                module.module_type == "script" && module.uuid.eq_ignore_ascii_case(module_uuid)
            }) else {
                continue;
            };
            let generated_path = script_module
                .entry
                .as_deref()
                .map(portable_relative_path)
                .map(|entry| pack_root.join(entry));
            let map_candidates =
                map_candidates_for_entry(&pack_root, script_module.entry.as_deref());
            if let Some(map_path) = map_candidates.into_iter().find(|path| path.is_file()) {
                return Ok((pack_root, map_path));
            }
            matching_pack_without_map = generated_path;
        }
    }

    if let Some(generated_path) = matching_pack_without_map {
        Err(format!(
            "connected module {module_uuid} was found, but no source map exists beside '{}'",
            generated_path.display()
        ))
    } else {
        Err(format!(
            "connected module {module_uuid} was not found under '{}'",
            mojang_dir.display()
        ))
    }
}

fn selected_module_uuid(
    state: &AppState,
    requested: Option<String>,
    handshake: &SessionHandshakeInfo,
) -> Option<String> {
    requested
        .or_else(|| {
            state
                .selected_module_uuid
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
        .or_else(|| {
            (handshake.plugins.len() == 1).then(|| handshake.plugins[0].module_uuid.clone())
        })
}

fn handshake_with_source_maps(
    state: &AppState,
    handshake: SessionHandshakeInfo,
    requested: Option<String>,
) -> HandshakeInfo {
    let module_uuid = selected_module_uuid(state, requested, &handshake);
    let manual_path = state
        .manual_source_map_path
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let source_map_status = match manual_path {
        Some(path) => configure_source_maps_from_path(state, &path),
        None => configure_source_maps_for_module(state, module_uuid.as_deref()),
    };
    let mut info = HandshakeInfo::from(handshake);
    info.source_map_status = source_map_status;
    info
}

pub async fn listen_to_minecraft(
    state: &AppState,
    app: AppHandle,
    port: u16,
    target_module_uuid: Option<String>,
    passcode: Option<String>,
) -> Result<HandshakeInfo, String> {
    state.ensure_bridge(&app).await;
    *state
        .selected_module_uuid
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = target_module_uuid.clone();
    let requested = target_module_uuid.clone();
    let hs = map_err(
        state
            .controller
            .listen(port, target_module_uuid, passcode)
            .await,
    )?;
    Ok(handshake_with_source_maps(state, hs, requested))
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
    *state
        .selected_module_uuid
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = target_module_uuid.clone();
    let requested = target_module_uuid.clone();
    let hs = map_err(
        state
            .controller
            .connect(host, port, target_module_uuid, passcode)
            .await,
    )?;
    Ok(handshake_with_source_maps(state, hs, requested))
}

pub async fn disconnect(state: &AppState) -> Result<(), String> {
    map_err(state.controller.disconnect().await)
}

pub async fn cancel_pending_connect(state: &AppState) -> Result<(), String> {
    map_err(state.controller.cancel_pending().await)
}

pub async fn select_target_module(state: &AppState, module_uuid: String) -> Result<(), String> {
    *state
        .selected_module_uuid
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(module_uuid.clone());
    let result = map_err(state.controller.select_target(module_uuid).await);
    if result.is_err() {
        *state
            .selected_module_uuid
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
    result
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
    Ok(hs.map(|handshake| handshake_with_source_maps(state, handshake, None)))
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

        fn write_mojang_pack(&self, module_uuid: &str, map_file_name: &str) -> PathBuf {
            let pack_root = self
                .root
                .join("development_behavior_packs")
                .join("Shield BP");
            let scripts = pack_root.join("scripts");
            fs::create_dir_all(&scripts).expect("create Mojang pack scripts");
            fs::write(
                pack_root.join("manifest.json"),
                format!(
                    r#"{{
                        "format_version": 2,
                        "modules": [{{
                            "type": "script",
                            "uuid": "{module_uuid}",
                            "entry": "scripts/main.js"
                        }}]
                    }}"#
                ),
            )
            .expect("write pack manifest");
            fs::write(
                scripts.join(map_file_name),
                r#"{
                    "version": 3,
                    "file": "main.js",
                    "sources": ["../../src/main.ts"],
                    "names": [],
                    "mappings": "AAAA"
                }"#,
            )
            .expect("write pack source map");
            pack_root
        }

        fn write_regolith_project(&self, module_uuid: &str) -> PathBuf {
            let project_root = self.root.join("projects").join("shield");
            let behavior_pack = project_root.join("packs").join("BP");
            let resource_pack = project_root.join("packs").join("RP");
            fs::create_dir_all(&behavior_pack).expect("create Regolith behavior pack");
            fs::create_dir_all(&resource_pack).expect("create Regolith resource pack");
            fs::write(
                project_root.join("config.json"),
                r#"{
                    "packs": {
                        "behaviorPack": "./packs/BP",
                        "resourcePack": "./packs/RP"
                    },
                    "regolith": { "dataPath": "./data" }
                }"#,
            )
            .expect("write Regolith config");
            fs::write(
                behavior_pack.join("manifest.json"),
                format!(
                    r#"{{
                        "format_version": 2,
                        "header": {{ "uuid": "bp-header-uuid" }},
                        "modules": [{{
                            "type": "script",
                            "uuid": "{module_uuid}",
                            "entry": "scripts/main.js"
                        }}]
                    }}"#
                ),
            )
            .expect("write Regolith manifest");
            fs::write(
                resource_pack.join("manifest.json"),
                r#"{
                    "format_version": 2,
                    "header": { "uuid": "rp-header-uuid" },
                    "modules": []
                }"#,
            )
            .expect("write resource manifest");
            project_root
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
            source_map_status: WorkspaceMapStatus {
                enabled: true,
                map_path: Some("C:/pack/scripts/main.js.map".into()),
                error: None,
            },
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
        assert!(map.contains_key("sourceMapStatus"));

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
        assert_eq!(frame.source_path.as_deref(), Some("../../src/main.ts"));
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
    fn finds_connected_modules_main_js_map_under_mojang_dir() {
        let workspace = TestWorkspace::new();
        let module_uuid = "e05152e8-56fc-4e10-9f27-dc7796400bd7";
        let pack_root = workspace.write_mojang_pack(module_uuid, "main.js.map");

        let (found_pack, map_path) =
            find_source_map_for_module(&workspace.root, module_uuid).expect("find source map");
        assert_eq!(found_pack, pack_root);
        assert_eq!(map_path, pack_root.join("scripts").join("main.js.map"));

        let maps = mc_source_maps::SourceMaps::from_map_file(map_path, found_pack)
            .expect("load discovered source map");
        assert!(maps.generated_to_original("/scripts/main.js", 0, 0).is_ok());
        assert!(find_source_map_for_module(&workspace.root, "unknown-module").is_err());
    }

    #[test]
    fn accepts_map_js_filename_fallback() {
        let workspace = TestWorkspace::new();
        let module_uuid = "module-with-map-js";
        let pack_root = workspace.write_mojang_pack(module_uuid, "main.map.js");

        let (_, map_path) =
            find_source_map_for_module(&workspace.root, module_uuid).expect("find .map.js");
        assert_eq!(map_path, pack_root.join("scripts").join("main.map.js"));
        let maps = mc_source_maps::SourceMaps::from_map_file(&map_path, &pack_root)
            .expect("load .map.js source map");
        assert!(maps.generated_to_original("/scripts/main.js", 0, 0).is_ok());
    }

    #[test]
    fn opens_regolith_workspace_and_reports_pack_metadata() {
        let workspace = TestWorkspace::new();
        let module_uuid = "e05152e8-56fc-4e10-9f27-dc7796400bd7";
        let project_root = workspace.write_regolith_project(module_uuid);
        let state = AppState::new();

        let selection =
            set_workspace_root(&state, Some(project_root.to_string_lossy().into_owned()))
                .expect("open workspace");
        let info = selection.workspace.expect("workspace info");
        assert_eq!(info.behavior_pack_uuid.as_deref(), Some("bp-header-uuid"));
        assert_eq!(info.resource_pack_uuid.as_deref(), Some("rp-header-uuid"));
        assert_eq!(info.script_module_uuids, [module_uuid]);
        assert_eq!(
            source_base_for_module(&state, module_uuid),
            Some(fs::canonicalize(&project_root).unwrap())
        );
        assert!(source_base_for_module(&state, "different-module").is_none());
        let mismatch = configure_source_maps_for_module(&state, Some("different-module"));
        assert!(!mismatch.enabled);
        assert!(mismatch
            .error
            .as_deref()
            .is_some_and(|error| error.contains("does not contain connected script module")));

        assert!(set_workspace_root(&state, None)
            .expect("clear workspace")
            .workspace
            .is_none());
    }

    #[test]
    fn manual_source_map_override_loads_and_clears() {
        let workspace = TestWorkspace::new();
        let pack_root = workspace.write_mojang_pack("manual-module", "main.js.map");
        let map_path = pack_root.join("scripts").join("main.js.map");
        let state = AppState::new();

        let loaded = set_source_map_path(&state, Some(map_path.to_string_lossy().into_owned()));
        assert!(loaded.enabled);
        assert_eq!(
            loaded.map_path.as_deref(),
            Some(map_path.to_string_lossy().as_ref())
        );
        assert!(state.source_maps.read().unwrap().is_some());

        let cleared = set_source_map_path(&state, None);
        assert!(!cleared.enabled);
        assert_eq!(cleared.error, None);
        assert!(state.source_maps.read().unwrap().is_none());
    }

    #[test]
    fn manual_folder_uses_manifest_script_entry_instead_of_main_filename() {
        let workspace = TestWorkspace::new();
        let pack_root = workspace.root.join("custom-entry-pack");
        let generated = pack_root.join("dist").join("runtime.js");
        fs::create_dir_all(generated.parent().unwrap()).expect("create custom entry directory");
        fs::write(
            pack_root.join("manifest.json"),
            r#"{
                "format_version": 2,
                "modules": [{
                    "type": "script",
                    "uuid": "custom-entry-module",
                    "entry": "dist/runtime.js"
                }]
            }"#,
        )
        .expect("write custom manifest");
        let map_path = PathBuf::from(format!("{}.map", generated.to_string_lossy()));
        fs::write(
            &map_path,
            r#"{
                "version": 3,
                "file": "runtime.js",
                "sources": ["../src/runtime.ts"],
                "names": [],
                "mappings": "AAAA"
            }"#,
        )
        .expect("write custom source map");

        let (found_map, generated_root) =
            resolve_manual_map_path(&pack_root, Some("custom-entry-module"))
                .expect("resolve manifest entry map");
        assert_eq!(found_map, map_path);
        assert_eq!(generated_root, pack_root);
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
