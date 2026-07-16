use mc_protocol::{
    ConnectOptions, DebuggeeConnection, DebuggeeEvent, DebuggerEvent, DebuggeeResponse,
    ProtocolHandshake, ProtocolVersion, DEFAULT_PORT,
};
use mc_protocol::events::shared::StatDataModel;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterInfo {
    pub protocol_version: u8,
    pub default_port: u16,
}

pub fn adapter_info() -> AdapterInfo {
    AdapterInfo {
        protocol_version: ProtocolVersion::CURRENT.as_u8(),
        default_port: DEFAULT_PORT,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsePayload {
    pub success: bool,
    pub args: Option<serde_json::Value>,
    pub message: Option<String>,
}

impl From<DebuggeeResponse> for ResponsePayload {
    fn from(r: DebuggeeResponse) -> Self {
        Self {
            success: r.success,
            args: r.args,
            message: r.response_message,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum McEvent {
    Protocol {
        version: u8,
        plugins: Vec<PluginInfo>,
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
        log_level: u8,
    },
    Notification {
        message: String,
        log_level: u8,
    },
    Stat2 {
        tick: u64,
        stats: Vec<StatDataModel>,
    },
    ProfilerCapture {
        capture_base_path: String,
    },
    Schema {
        count: usize,
    },
    Terminated {
        reason: Option<String>,
    },
    Unknown {
        type_name: String,
    },
}

impl From<DebuggeeEvent> for McEvent {
    fn from(event: DebuggeeEvent) -> Self {
        match event {
            DebuggeeEvent::Protocol {
                version,
                plugins,
                require_passcode,
            } => Self::Protocol {
                version,
                plugins: plugins
                    .iter()
                    .map(|p| PluginInfo {
                        name: p.name.clone(),
                        module_uuid: p.module_uuid.clone(),
                    })
                    .collect(),
                require_passcode,
            },
            DebuggeeEvent::Stopped { reason, thread } => Self::Stopped { reason, thread },
            DebuggeeEvent::Thread { reason, thread } => Self::Thread { reason, thread },
            DebuggeeEvent::Print { message, log_level } => Self::Print {
                message,
                log_level: log_level as u8,
            },
            DebuggeeEvent::Notification { message, log_level } => Self::Notification {
                message,
                log_level: log_level as u8,
            },
            DebuggeeEvent::Stat2 { tick, stats } => Self::Stat2 { tick, stats },
            DebuggeeEvent::ProfilerCapture {
                capture_base_path, ..
            } => Self::ProfilerCapture { capture_base_path },
            DebuggeeEvent::DebuggeeResponse { .. } => Self::Unknown {
                type_name: "debuggee-response".to_string(),
            },
            DebuggeeEvent::Response { .. } => Self::Unknown {
                type_name: "response".to_string(),
            },
            DebuggeeEvent::Schema { descriptors } => Self::Schema {
                count: descriptors.len(),
            },
            DebuggeeEvent::Terminated { reason } => Self::Terminated { reason },
            DebuggeeEvent::Unknown { type_name, .. } => Self::Unknown { type_name },
        }
    }
}

type Responder = oneshot::Sender<Result<ResponsePayload, String>>;

enum Command {
    SendEvent(DebuggerEvent),
    SendMinecraftCommand { command: String },
    Pause { thread_id: u32 },
    Continue { thread_id: u32 },
    StepNext { thread_id: u32 },
    StepIn { thread_id: u32 },
    StepOut { thread_id: u32 },
    Evaluate {
        expression: String,
        response_tx: Responder,
    },
}

pub struct AppState {
    cmd_tx: Mutex<Option<mpsc::Sender<Command>>>,
    handshake: Mutex<Option<HandshakeInfo>>,
    pending_cancel: Mutex<Option<oneshot::Sender<()>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            cmd_tx: Mutex::new(None),
            handshake: Mutex::new(None),
            pending_cancel: Mutex::new(None),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn listen_to_minecraft(
    state: &AppState,
    app: AppHandle,
    port: u16,
    target_module_uuid: Option<String>,
    passcode: Option<String>,
) -> Result<HandshakeInfo, String> {
    let opts = ConnectOptions {
        target_module_uuid,
        passcode,
    };
    let result = run_cancellable(state, DebuggeeConnection::listen_with_options(port, opts)).await?;
    Ok(spawn_connection(state, app, result.0, result.1).await)
}

pub async fn connect_to_minecraft(
    state: &AppState,
    app: AppHandle,
    host: String,
    port: u16,
    target_module_uuid: Option<String>,
    passcode: Option<String>,
) -> Result<HandshakeInfo, String> {
    let opts = ConnectOptions {
        target_module_uuid,
        passcode,
    };
    let result = run_cancellable(
        state,
        DebuggeeConnection::connect_with_options(&host, port, opts),
    )
    .await?;
    Ok(spawn_connection(state, app, result.0, result.1).await)
}

async fn run_cancellable<F>(
    state: &AppState,
    future: F,
) -> Result<(DebuggeeConnection, mc_protocol::ProtocolHandshake), String>
where
    F: std::future::Future<Output = Result<(DebuggeeConnection, mc_protocol::ProtocolHandshake), mc_protocol::ConnectionError>>,
{
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    *state.pending_cancel.lock().await = Some(cancel_tx);

    let result = tokio::select! {
        result = future => Some(result),
        _ = cancel_rx => None,
    };

    *state.pending_cancel.lock().await = None;

    match result {
        Some(r) => r.map_err(|e| e.to_string()),
        None => Err("cancelled".to_string()),
    }
}

pub async fn cancel_pending_connect(state: &AppState) -> Result<(), String> {
    if let Some(tx) = state.pending_cancel.lock().await.take() {
        let _ = tx.send(());
    }
    Ok(())
}

pub async fn send_minecraft_command(
    state: &AppState,
    command: String,
) -> Result<(), String> {
    let guard = state.cmd_tx.lock().await;
    let sender = guard.as_ref().ok_or("not connected")?;
    sender
        .send(Command::SendMinecraftCommand { command })
        .await
        .map_err(|e| e.to_string())
}

async fn spawn_connection(
    state: &AppState,
    app: AppHandle,
    conn: DebuggeeConnection,
    hs: ProtocolHandshake,
) -> HandshakeInfo {
    let info = handshake_to_info(&hs);
    *state.handshake.lock().await = Some(info.clone());
    let (tx, rx) = mpsc::channel(16);
    let _ = tx.send(Command::SendEvent(DebuggerEvent::Resume)).await;
    *state.cmd_tx.lock().await = Some(tx);
    tokio::spawn(connection_task(conn, app, rx));
    info
}

pub async fn disconnect(state: &AppState) -> Result<(), String> {
    state.cmd_tx.lock().await.take();
    *state.handshake.lock().await = None;
    Ok(())
}

pub async fn get_handshake_info(state: &AppState) -> Result<Option<HandshakeInfo>, String> {
    Ok(state.handshake.lock().await.clone())
}

pub async fn pause_thread(state: &AppState, thread_id: u32) -> Result<(), String> {
    send_fire_and_forget(state, Command::Pause { thread_id }).await
}

pub async fn continue_thread(state: &AppState, thread_id: u32) -> Result<(), String> {
    send_fire_and_forget(state, Command::Continue { thread_id }).await
}

pub async fn step_next(state: &AppState, thread_id: u32) -> Result<(), String> {
    send_fire_and_forget(state, Command::StepNext { thread_id }).await
}

pub async fn step_in(state: &AppState, thread_id: u32) -> Result<(), String> {
    send_fire_and_forget(state, Command::StepIn { thread_id }).await
}

pub async fn step_out(state: &AppState, thread_id: u32) -> Result<(), String> {
    send_fire_and_forget(state, Command::StepOut { thread_id }).await
}

pub async fn evaluate(
    state: &AppState,
    expression: String,
) -> Result<ResponsePayload, String> {
    send_request(state, |tx| Command::Evaluate {
        expression,
        response_tx: tx,
    })
    .await
}

async fn send_fire_and_forget(state: &AppState, cmd: Command) -> Result<(), String> {
    let guard = state.cmd_tx.lock().await;
    let sender = guard.as_ref().ok_or("not connected")?;
    sender.send(cmd).await.map_err(|e| e.to_string())
}

async fn send_request<F>(state: &AppState, build: F) -> Result<ResponsePayload, String>
where
    F: FnOnce(Responder) -> Command,
{
    let (tx, rx) = oneshot::channel();
    let cmd = build(tx);
    {
        let guard = state.cmd_tx.lock().await;
        let sender = guard.as_ref().ok_or("not connected")?;
        sender.send(cmd).await.map_err(|e| e.to_string())?;
    }
    rx.await.map_err(|_| "connection task dropped".to_string())?
}

async fn connection_task(
    mut conn: DebuggeeConnection,
    app: AppHandle,
    mut cmd_rx: mpsc::Receiver<Command>,
) {
    loop {
        tokio::select! {
            event_result = conn.recv_event() => match event_result {
                Ok(event) => {
                    let is_terminated = matches!(event, DebuggeeEvent::Terminated { .. });
                    let _ = app.emit("mc-event", McEvent::from(event));
                    if is_terminated {
                        let _ = app.emit("mc-terminated", ());
                        return;
                    }
                }
                Err(_) => {
                    let _ = app.emit("mc-disconnected", ());
                    return;
                }
            },
            cmd = cmd_rx.recv() => match cmd {
                Some(Command::SendEvent(event)) => {
                    if conn.send_event(&event).await.is_err() {
                        let _ = app.emit("mc-disconnected", ());
                        return;
                    }
                }
                Some(Command::SendMinecraftCommand { command }) => {
                    if conn.send_minecraft_command(&command, "overworld")
                        .await
                        .is_err()
                    {
                        let _ = app.emit("mc-disconnected", ());
                        return;
                    }
                }
                Some(Command::Pause { thread_id }) => {
                    if conn.pause(thread_id).await.is_err() {
                        let _ = app.emit("mc-disconnected", ());
                        return;
                    }
                }
                Some(Command::Continue { thread_id }) => {
                    if conn.continue_thread(thread_id).await.is_err() {
                        let _ = app.emit("mc-disconnected", ());
                        return;
                    }
                }
                Some(Command::StepNext { thread_id }) => {
                    if conn.step_next(thread_id).await.is_err() {
                        let _ = app.emit("mc-disconnected", ());
                        return;
                    }
                }
                Some(Command::StepIn { thread_id }) => {
                    if conn.step_in(thread_id).await.is_err() {
                        let _ = app.emit("mc-disconnected", ());
                        return;
                    }
                }
                Some(Command::StepOut { thread_id }) => {
                    if conn.step_out(thread_id).await.is_err() {
                        let _ = app.emit("mc-disconnected", ());
                        return;
                    }
                }
                Some(Command::Evaluate {
                    expression,
                    response_tx,
                }) => {
                    let result = conn
                        .evaluate(&expression)
                        .await
                        .map(ResponsePayload::from)
                        .map_err(|e| e.to_string());
                    let _ = response_tx.send(result);
                }
                None => return,
            },
        }
    }
}

fn handshake_to_info(hs: &ProtocolHandshake) -> HandshakeInfo {
    HandshakeInfo {
        version: hs.version.as_u8(),
        plugins: hs
            .plugins
            .iter()
            .map(|p| PluginInfo {
                name: p.name.clone(),
                module_uuid: p.module_uuid.clone(),
            })
            .collect(),
        require_passcode: hs.require_passcode,
    }
}

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
        assert!(map.contains_key("requirePasscode"), "should have requirePasscode");
        assert!(map.contains_key("version"), "should have version");
        assert!(map.contains_key("plugins"), "should have plugins");

        // No snake_case top-level keys
        assert!(!map.contains_key("require_passcode"), "should NOT have require_passcode");

        // Values
        assert_eq!(map["requirePasscode"], true);
        assert_eq!(map["version"], 7);

        // PluginInfo fields remain snake_case
        let plugins = map["plugins"].as_array().unwrap();
        assert_eq!(plugins.len(), 2);
        let p0 = plugins[0].as_object().unwrap();
        assert!(p0.contains_key("module_uuid"), "PluginInfo should have module_uuid (snake_case)");
        assert_eq!(p0["module_uuid"], "abc-123");
        assert_eq!(p0["name"], "bp.main");
        // PluginInfo should NOT have camelCase keys
        assert!(!p0.contains_key("moduleUuid"), "PluginInfo should NOT have moduleUuid");
    }
}
