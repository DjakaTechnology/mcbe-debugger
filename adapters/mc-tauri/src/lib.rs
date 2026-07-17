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
    },
    Notification {
        message: String,
        #[serde(rename = "logLevel")]
        log_level: u8,
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
                message,
                log_level: log_level as u8,
            },
            mc_protocol::DebuggeeEvent::Notification { message, log_level } => Self::Notification {
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

// ─── Event Bridge ─────────────────────────────────────────────────────

/// Bridge framework‑neutral [`SessionEvent`]s to Tauri events.
///
/// Emits exactly these Tauri event names, matching the existing frontend:
/// * `mc-event` — debuggee protocol events (mapped through [`McEvent`])
/// * `mc-target-selection-required` — plugin selection prompt
/// * `mc-disconnected` — unexpected connection loss
/// * `mc-terminated` — debuggee termination
async fn event_bridge(mut rx: mpsc::Receiver<SessionEvent>, app: AppHandle) {
    while let Some(event) = rx.recv().await {
        match event {
            SessionEvent::Debuggee(debuggee_event) => {
                let _ = app.emit("mc-event", McEvent::from(debuggee_event));
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
}

impl AppState {
    pub fn new() -> Self {
        let (controller, event_rx) = SessionController::new();
        Self {
            controller,
            event_rx: Mutex::new(Some(event_rx)),
            bridge_started: std::sync::atomic::AtomicBool::new(false),
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
        tokio::spawn(event_bridge(rx, app_handle));
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Public free functions ────────────────────────────────────────────
//
// Signatures remain compatible with `apps/tauri-standalone/src-tauri/src/lib.rs`.

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
    use super::*;

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
                },
                "logLevel",
            ),
            (
                McEvent::Notification {
                    message: "n".into(),
                    log_level: 1,
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
