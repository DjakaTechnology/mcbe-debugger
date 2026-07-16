use mc_protocol::{
    ConnectOptions, DebuggeeConnection, DebuggeeEvent, DebuggerEvent, DebuggeeResponse,
    PendingConnection, ProtocolHandshake, ProtocolVersion, DEFAULT_PORT,
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

/// Tracks the lifecycle of a pending connection from initial TCP connect/accept
/// through optional interactive target selection.
#[derive(Debug)]
enum PendingPhase {
    Idle,
    /// Either waiting for TCP to connect/accept, or waiting for the user to
    /// select a target module UUID.  The `selection_tx` half is `Some` only
    /// when we are in the target-selection sub-phase.
    Active {
        cancel_tx: oneshot::Sender<()>,
        selection_tx: Option<oneshot::Sender<Result<String, String>>>,
    },
}

pub struct AppState {
    cmd_tx: Mutex<Option<mpsc::Sender<Command>>>,
    handshake: Mutex<Option<HandshakeInfo>>,
    phase: Mutex<PendingPhase>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            cmd_tx: Mutex::new(None),
            handshake: Mutex::new(None),
            phase: Mutex::new(PendingPhase::Idle),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Phase helpers ─────────────────────────────────────────────────────

/// Set the pending phase to `Active` with a fresh cancel channel and return
/// the receiver half.  Drops any previous phase (which cancels the prior
/// operation if one was active).
///
/// The returned receiver lives for the entire connect/listen flow (TCP
/// phase, optional selection, and completion) so cancellation via
/// [`cancel_pending_connect`] or [`disconnect`] works continuously.
async fn activate_phase(state: &AppState) -> oneshot::Receiver<()> {
    let (cancel_tx, cancel_rx) = oneshot::channel();
    *state.phase.lock().await = PendingPhase::Active {
        cancel_tx,
        selection_tx: None,
    };
    cancel_rx
}

/// If the phase is `Active`, send the cancel signal and reset to `Idle`.
/// Safe to call even after the cancel receiver has been dropped (the send
/// error is silently ignored).  Also used to clear the phase on success so
/// that no stale `Active` state remains.
async fn clear_phase(state: &AppState) {
    let mut phase = state.phase.lock().await;
    let old = std::mem::replace(&mut *phase, PendingPhase::Idle);
    if let PendingPhase::Active { cancel_tx, .. } = old {
        let _ = cancel_tx.send(());
    }
}

// ── Public API ────────────────────────────────────────────────────────

pub async fn listen_to_minecraft(
    state: &AppState,
    app: AppHandle,
    port: u16,
    target_module_uuid: Option<String>,
    passcode: Option<String>,
) -> Result<HandshakeInfo, String> {
    // Single continuous cancel channel for the entire operation.
    let mut cancel_rx = activate_phase(state).await;

    // Phase 1: TCP accept + receive ProtocolEvent (cancellable).
    let pending = match tokio::select! {
        result = DebuggeeConnection::listen_pending(port) => result,
        _ = &mut cancel_rx => {
            clear_phase(state).await;
            return Err("cancelled".to_string());
        }
    } {
        Ok(p) => p,
        Err(e) => {
            clear_phase(state).await;
            return Err(e.to_string());
        }
    };

    let opts = ConnectOptions {
        target_module_uuid,
        passcode,
    };

    connect_flow(state, app, pending, opts, cancel_rx).await
}

pub async fn connect_to_minecraft(
    state: &AppState,
    app: AppHandle,
    host: String,
    port: u16,
    target_module_uuid: Option<String>,
    passcode: Option<String>,
) -> Result<HandshakeInfo, String> {
    // Single continuous cancel channel for the entire operation.
    let mut cancel_rx = activate_phase(state).await;

    // Phase 1: TCP connect + receive ProtocolEvent (cancellable).
    let pending = match tokio::select! {
        result = DebuggeeConnection::connect_pending(&host, port) => result,
        _ = &mut cancel_rx => {
            clear_phase(state).await;
            return Err("cancelled".to_string());
        }
    } {
        Ok(p) => p,
        Err(e) => {
            clear_phase(state).await;
            return Err(e.to_string());
        }
    };

    let opts = ConnectOptions {
        target_module_uuid,
        passcode,
    };

    connect_flow(state, app, pending, opts, cancel_rx).await
}

/// Shared tail of both `listen_to_minecraft` and `connect_to_minecraft`.
///
/// The same `cancel_rx` from [`activate_phase`] is threaded through so
/// cancellation works continuously across the optional target-selection
/// sub-phase and the handshake completion.  The phase is cleared to `Idle`
/// on **every** exit path (success, cancellation, network error, invalid
/// UUID, passcode error, selection sender dropped).
async fn connect_flow(
    state: &AppState,
    app: AppHandle,
    pending: PendingConnection,
    mut opts: ConnectOptions,
    cancel_rx: oneshot::Receiver<()>,
) -> Result<HandshakeInfo, String> {
    tokio::pin!(cancel_rx);

    // ── Interactive target selection? ──────────────────────────────
    if opts.target_module_uuid.is_none() && pending.plugins().len() > 1 {
        let (selection_tx, selection_rx) = oneshot::channel();

        // Install selection_tx in the existing Active phase — do NOT
        // replace the cancel channel so the same cancel_rx stays valid.
        {
            let mut phase = state.phase.lock().await;
            if let PendingPhase::Active {
                selection_tx: slot, ..
            } = &mut *phase
            {
                *slot = Some(selection_tx);
            } else {
                // Phase was already cleared (cancelled) — no modal to open.
                return Err("cancelled".to_string());
            }
        } // mutex released before emit

        // Notify the frontend
        let plugin_infos: Vec<PluginInfo> = pending
            .plugins()
            .iter()
            .map(|p| PluginInfo {
                name: p.name.clone(),
                module_uuid: p.module_uuid.clone(),
            })
            .collect();
        let _ = app.emit("mc-target-selection-required", &plugin_infos);

        // Wait for selection or cancellation
        let chosen = tokio::select! {
            result = selection_rx => match result {
                Ok(Ok(uuid)) => uuid,
                Ok(Err(e)) => {
                    clear_phase(state).await;
                    return Err(e);
                }
                Err(_) => {
                    clear_phase(state).await;
                    return Err("cancelled".to_string());
                }
            },
            _ = cancel_rx.as_mut() => {
                clear_phase(state).await;
                return Err("cancelled".to_string());
            }
        };

        opts.target_module_uuid = Some(chosen);
    }

    // ── Complete the handshake (still cancellable) ─────────────────
    tokio::select! {
        result = pending.complete(opts) => match result {
            Ok((conn, hs)) => {
                clear_phase(state).await;
                Ok(spawn_connection(state, app, conn, hs).await)
            }
            Err(e) => {
                clear_phase(state).await;
                Err(e.to_string())
            }
        },
        _ = cancel_rx.as_mut() => {
            clear_phase(state).await;
            Err("cancelled".to_string())
        }
    }
}

/// Let the frontend select a target module UUID when the handshake is
/// waiting for interactive selection.  Single-use; errors if no selection
/// is pending.
pub async fn select_target_module(
    state: &AppState,
    module_uuid: String,
) -> Result<(), String> {
    let mut phase = state.phase.lock().await;
    if let PendingPhase::Active { selection_tx, .. } = &mut *phase {
        if let Some(tx) = selection_tx.take() {
            let _ = tx.send(Ok(module_uuid));
            return Ok(());
        }
    }
    Err("no target selection pending".to_string())
}

pub async fn cancel_pending_connect(state: &AppState) -> Result<(), String> {
    clear_phase(state).await;
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
    clear_phase(state).await;
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
    use std::sync::Arc;
    use tokio::sync::oneshot;

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

    // ── Selection flow tests ────────────────────────────────────────────

    #[tokio::test]
    async fn select_target_module_happy_path() {
        let state = AppState::new();

        // Manually arm the phase with a selection slot
        let (cancel_tx, _cancel_rx) = oneshot::channel();
        let (selection_tx, selection_rx) = oneshot::channel();
        *state.phase.lock().await = PendingPhase::Active {
            cancel_tx,
            selection_tx: Some(selection_tx),
        };

        // Call the public API as the frontend would
        select_target_module(&state, "chosen-uuid".into())
            .await
            .expect("select should succeed");

        // The waiter should receive the UUID
        let result = selection_rx
            .await
            .expect("selection_rx should have been sent");
        match result {
            Ok(uuid) => assert_eq!(uuid, "chosen-uuid"),
            Err(e) => panic!("expected Ok, got Err({e})"),
        }

        // Phase should be Active but with selection_tx = None (single-use)
        let phase = state.phase.lock().await;
        match &*phase {
            PendingPhase::Active { selection_tx, .. } => {
                assert!(selection_tx.is_none(), "selection_tx should be taken");
            }
            other => panic!("expected Active, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn select_target_module_double_call_errors() {
        let state = AppState::new();

        let (cancel_tx, _cancel_rx) = oneshot::channel();
        let (selection_tx, _selection_rx) = oneshot::channel();
        *state.phase.lock().await = PendingPhase::Active {
            cancel_tx,
            selection_tx: Some(selection_tx),
        };

        // First call succeeds
        select_target_module(&state, "uuid-1".into())
            .await
            .expect("first select should succeed");

        // Second call should error
        let err = select_target_module(&state, "uuid-2".into())
            .await
            .expect_err("second select should fail");
        assert_eq!(err, "no target selection pending");
    }

    #[tokio::test]
    async fn select_target_module_no_pending_errors() {
        let state = AppState::new();
        // Phase is Idle by default

        let err = select_target_module(&state, "any-uuid".into())
            .await
            .expect_err("select without pending should fail");
        assert_eq!(err, "no target selection pending");
    }

    #[tokio::test]
    async fn cancel_pending_connect_during_selection_phase() {
        let state = AppState::new();

        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        let (selection_tx, _selection_rx) = oneshot::channel();
        *state.phase.lock().await = PendingPhase::Active {
            cancel_tx,
            selection_tx: Some(selection_tx),
        };

        // Cancel should fire the cancel signal
        cancel_pending_connect(&state)
            .await
            .expect("cancel should succeed");

        let cancel_result = cancel_rx.try_recv();
        assert!(cancel_result.is_ok(), "cancel_rx should have received signal");

        // Phase should be Idle
        let phase = state.phase.lock().await;
        assert!(matches!(*phase, PendingPhase::Idle));
    }

    #[tokio::test]
    async fn disconnect_clears_pending_phase_too() {
        let state = AppState::new();

        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        *state.phase.lock().await = PendingPhase::Active {
            cancel_tx,
            selection_tx: None,
        };

        disconnect(&state).await.expect("disconnect should succeed");

        let cancel_result = cancel_rx.try_recv();
        assert!(cancel_result.is_ok(), "disconnect should cancel pending phase");

        let phase = state.phase.lock().await;
        assert!(matches!(*phase, PendingPhase::Idle));
    }

    // ── Phase refactoring regression tests ──────────────────────────────

    #[tokio::test]
    async fn activate_phase_uses_async_lock() {
        let state = AppState::new();
        let _cancel_rx = activate_phase(&state).await;
        let phase = state.phase.lock().await;
        assert!(matches!(*phase, PendingPhase::Active { .. }));
    }

    #[tokio::test]
    async fn clear_phase_resets_to_idle() {
        let state = AppState::new();
        let _cancel_rx = activate_phase(&state).await;
        clear_phase(&state).await;
        let phase = state.phase.lock().await;
        assert!(matches!(*phase, PendingPhase::Idle));
    }

    #[tokio::test]
    async fn clear_phase_sends_cancel_signal() {
        let state = AppState::new();
        let cancel_rx = activate_phase(&state).await;
        clear_phase(&state).await;
        let result = cancel_rx.await;
        assert_eq!(result, Ok(()));
    }

    #[tokio::test]
    async fn clear_phase_twice_is_idempotent() {
        let state = AppState::new();
        let _cancel_rx = activate_phase(&state).await;
        clear_phase(&state).await;
        clear_phase(&state).await; // second clear on Idle is a no-op
        let phase = state.phase.lock().await;
        assert!(matches!(*phase, PendingPhase::Idle));
    }

    #[tokio::test]
    async fn network_error_clears_phase() {
        // Simulates the Phase-1 error path: activate → connect_pending
        // fails → clear_phase → Idle.  The cancel signal is observable.
        let state = AppState::new();
        let cancel_rx = activate_phase(&state).await;

        // connect_pending to a closed port should fail quickly
        let result = DebuggeeConnection::connect_pending("127.0.0.1", 1).await;
        assert!(result.is_err(), "connect to closed port should fail");

        clear_phase(&state).await;

        let phase = state.phase.lock().await;
        assert!(matches!(*phase, PendingPhase::Idle));

        // The cancel signal must have been sent so waiters can observe it
        let signal = cancel_rx.await;
        assert_eq!(signal, Ok(()));
    }

    #[tokio::test]
    async fn selection_tx_install_on_idle_errors() {
        // When connect_flow tries to install a selection_tx but the phase
        // has already been cleared (e.g. concurrent cancel), it must
        // return "cancelled" without opening a modal.
        let state = AppState::new();

        // Phase is Idle (not Active) — as if cancel/clear already ran
        let (selection_tx, _selection_rx) = oneshot::channel();
        {
            let mut phase = state.phase.lock().await;
            if let PendingPhase::Active {
                selection_tx: slot, ..
            } = &mut *phase
            {
                *slot = Some(selection_tx);
            } // else: this branch must NOT be taken
        }
        let phase = state.phase.lock().await;
        assert!(matches!(*phase, PendingPhase::Idle));
        // selection_tx was dropped without sending → selection_rx shows cancelled
    }

    #[tokio::test]
    async fn cancel_signal_works_across_selection_gap() {
        // Proves that the cancel channel from activate_phase survives
        // a simulated selection sub-phase (selection_tx install + wait).
        let state = AppState::new();
        let cancel_rx = activate_phase(&state).await;

        // Simulate selection_tx install as connect_flow would
        let (selection_tx, _selection_rx) = oneshot::channel();
        {
            let mut phase = state.phase.lock().await;
            if let PendingPhase::Active {
                selection_tx: slot, ..
            } = &mut *phase
            {
                *slot = Some(selection_tx);
            }
        }

        // Cancel from another task (as cancel_pending_connect would)
        clear_phase(&state).await;

        // The original cancel_rx must fire
        let signal = cancel_rx.await;
        assert_eq!(signal, Ok(()));
    }

    /// Helper: encode and send a JSON value over a TCP stream (same pattern
    /// as mc-protocol's connection tests).
    async fn send_test_frame(
        sock: &mut tokio::net::TcpStream,
        value: serde_json::Value,
    ) {
        use mc_protocol::framing::MessageCodec;
        use tokio::io::AsyncWriteExt;
        use tokio_util::codec::Encoder;
        let mut codec = MessageCodec::new();
        let mut buf = bytes::BytesMut::new();
        codec.encode(value, &mut buf).unwrap();
        sock.write_all(&buf).await.unwrap();
    }

    #[tokio::test]
    async fn completion_error_leaves_phase_idle() {
        // Requires a real TCP server to get a PendingConnection that
        // fails on complete (e.g. missing passcode).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            send_test_frame(
                &mut sock,
                serde_json::json!({
                    "type": "ProtocolEvent",
                    "version": 9,
                    "plugins": [],
                    "require_passcode": true,
                }),
            )
            .await;
        });

        // Use a separate scope so the pending connection is dropped cleanly
        let pending = DebuggeeConnection::connect_pending("127.0.0.1", port)
            .await
            .expect("pending connect should succeed");
        assert!(pending.require_passcode());

        // complete WITHOUT passcode → must fail
        let result = pending
            .complete(ConnectOptions {
                target_module_uuid: None,
                passcode: None,
            })
            .await;
        assert!(result.is_err(), "complete without passcode should fail");

        // Phase was never activated (no AppState involvement here) → Idle
        let state = AppState::new();
        let phase = state.phase.lock().await;
        assert!(matches!(*phase, PendingPhase::Idle));

        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), server).await;
    }

    #[tokio::test]
    async fn phase_one_cancel_interrupts_blocked_connect() {
        // Proves that `tokio::select!` between `connect_pending` and
        // `&mut cancel_rx` actually allows cancellation to interrupt a
        // blocked Phase-1 recv, not just the channel helper.
        let state = Arc::new(AppState::new());
        let mut cancel_rx = activate_phase(&state).await;

        // TCP server that accepts but *never* sends ProtocolEvent,
        // so connect_pending blocks on the read.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (_sock, _) = listener.accept().await.unwrap();
            // Hold the connection open but send nothing – the client
            // will block reading the ProtocolEvent.
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });

        // Cancel after a short delay so connect_pending has time to
        // establish TCP and start reading.
        let state2 = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let _ = cancel_pending_connect(&state2).await;
        });

        // This select! mirrors the Phase-1 logic in listen_to_minecraft.
        let result = tokio::select! {
            result = DebuggeeConnection::connect_pending("127.0.0.1", port) => {
                result.map_err(|e| e.to_string())
            }
            _ = &mut cancel_rx => {
                Err("cancelled".to_string())
            }
        };

        // If cancellation works, we get Err("cancelled") – not a
        // network timeout and not an Ok(pending).
        assert!(
            result.is_err(),
            "expected cancelled, got Ok(pending)"
        );
        assert_eq!(result.unwrap_err(), "cancelled");

        // Phase has already been cleared by cancel_pending_connect.
        let phase = state.phase.lock().await;
        assert!(matches!(*phase, PendingPhase::Idle));

        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), server).await;
    }
}
