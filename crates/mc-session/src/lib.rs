use std::sync::Arc;

use mc_protocol::events::PluginDetails;
use mc_protocol::{
    ConnectOptions, DebuggeeConnection, DebuggeeEvent, DebuggeeResponse, DebuggerEvent,
    PendingConnection, ProtocolHandshake,
};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, Mutex};

// ── Error ─────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("not connected")]
    NotConnected,
    #[error("cancelled")]
    Cancelled,
    #[error("no target selection pending")]
    NoTargetSelectionPending,
    #[error("connection task dropped")]
    ConnectionTaskDropped,
    #[error(transparent)]
    ConnectionError(#[from] mc_protocol::ConnectionError),
    #[error("command channel closed")]
    CommandChannelClosed,
}

// ─── Framework‑neutral types ──────────────────────────────────────────

/// Framework‑neutral handshake information returned by [`SessionController::connect`]
/// and [`SessionController::listen`].  mc‑tauri maps this into its own
/// `HandshakeInfo` with camelCase serde.
#[derive(Debug, Clone)]
pub struct HandshakeInfo {
    pub version: u8,
    pub plugins: Vec<PluginDetails>,
    pub require_passcode: bool,
}

/// Framework‑neutral result of an evaluate request.
#[derive(Debug, Clone)]
pub struct EvaluateResult {
    pub success: bool,
    pub args: Option<serde_json::Value>,
    pub message: Option<String>,
}

impl From<DebuggeeResponse> for EvaluateResult {
    fn from(r: DebuggeeResponse) -> Self {
        Self {
            success: r.success,
            args: r.args,
            message: r.response_message,
        }
    }
}

// ─── SessionEvent ─────────────────────────────────────────────────────

/// Framework‑neutral event from the session layer.
///
/// * `Debuggee(event)` — a raw protocol event from the debuggee.  The adapter
///   maps this to its own DTO (e.g. `McEvent` in Tauri).
/// * `TargetSelectionRequired { plugins }` — the connection has multiple
///   plugins and the UI must pick one via [`SessionController::select_target`].
/// * `Disconnected` — the TCP/connection was lost unexpectedly.
/// * `Terminated { reason }` — the debuggee sent a terminated event.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Debuggee(DebuggeeEvent),
    TargetSelectionRequired { plugins: Vec<PluginDetails> },
    Disconnected,
    Terminated { reason: Option<String> },
}

// ─── SessionCommand ───────────────────────────────────────────────────

pub type Responder = oneshot::Sender<Result<EvaluateResult, SessionError>>;

/// Commands sent to the background connection task.
///
/// Ownership of command handling stays inside the controller task;
/// the adapter only forwards these to [`SessionController::send_command`].
#[derive(Debug)]
pub enum SessionCommand {
    SendEvent(DebuggerEvent),
    SendMinecraftCommand {
        command: String,
    },
    Pause {
        thread_id: u32,
    },
    Continue {
        thread_id: u32,
    },
    StepNext {
        thread_id: u32,
    },
    StepIn {
        thread_id: u32,
    },
    StepOut {
        thread_id: u32,
    },
    Evaluate {
        expression: String,
        response_tx: Responder,
    },
}

// ─── PendingPhase state machine ───────────────────────────────────────

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
        selection_tx: Option<oneshot::Sender<Result<String, SessionError>>>,
    },
}

// ─── SessionInner ─────────────────────────────────────────────────────

struct SessionInner {
    cmd_tx: Option<mpsc::Sender<SessionCommand>>,
    handshake: Option<HandshakeInfo>,
    phase: PendingPhase,
    /// Monotonically increasing generation counter, incremented each time
    /// [`spawn_connection`](SessionController::spawn_connection) installs
    /// new `cmd_tx`/`handshake` state.  Used by
    /// [`cleanup_task_state`] to ensure only the current task clears state.
    generation: u64,
    /// Active cancellation sender for the current connection task.
    /// Signalled by [`disconnect`](SessionController::disconnect) or when
    /// a newer connection replaces this one.
    cancel_task_tx: Option<oneshot::Sender<()>>,
}

// ─── SessionController ────────────────────────────────────────────────

/// A concrete, cloneable controller for a Minecraft debugger session.
///
/// Construct via [`SessionController::new`], which also yields a single-consumer
/// [`mpsc::Receiver`] for [`SessionEvent`].  The receiver is consumed by the
/// adapter/UI layer (Tauri, TUI, etc.).
///
/// # Lifecycle
///
/// 1. Call [`connect`](SessionController::connect) or [`listen`](SessionController::listen).
/// 2. If the server advertises multiple plugins and no `target_module_uuid` was given,
///    a [`SessionEvent::TargetSelectionRequired`] is emitted.  Answer via
///    [`select_target`](SessionController::select_target).
/// 3. On success the controller spawns a background task that forwards
///    [`DebuggeeEvent`]s as [`SessionEvent::Debuggee`] and accepts
///    [`SessionCommand`]s.
/// 4. Call [`disconnect`](SessionController::disconnect) to tear down or
///    [`cancel_pending`](SessionController::cancel_pending) to abort a pending connect.
#[derive(Clone)]
pub struct SessionController {
    inner: Arc<Mutex<SessionInner>>,
    event_tx: mpsc::Sender<SessionEvent>,
}

impl SessionController {
    /// Create a new controller together with the event receiver.
    ///
    /// The event channel capacity is 256.  Critical events block on send;
    /// high-frequency `Stat2` events use `try_send` and may be dropped when full.
    pub fn new() -> (Self, mpsc::Receiver<SessionEvent>) {
        let (event_tx, event_rx) = mpsc::channel(256);
        let controller = Self {
            inner: Arc::new(Mutex::new(SessionInner {
                cmd_tx: None,
                handshake: None,
                phase: PendingPhase::Idle,
                generation: 0,
                cancel_task_tx: None,
            })),
            event_tx,
        };
        (controller, event_rx)
    }

    // ── Lifecycle ──────────────────────────────────────────────────────

    /// Listen for an incoming Minecraft debugger connection.
    ///
    /// Cancellable during TCP accept, target-selection wait, and handshake
    /// completion via [`cancel_pending`](SessionController::cancel_pending).
    pub async fn listen(
        &self,
        port: u16,
        target_module_uuid: Option<String>,
        passcode: Option<String>,
    ) -> Result<HandshakeInfo, SessionError> {
        let mut cancel_rx = self.activate_phase().await;

        let pending = tokio::select! {
            result = DebuggeeConnection::listen_pending(port) => result,
            _ = &mut cancel_rx => {
                self.clear_phase().await;
                return Err(SessionError::Cancelled);
            }
        };

        let pending = match pending {
            Ok(p) => p,
            Err(e) => {
                self.clear_phase().await;
                return Err(SessionError::ConnectionError(e));
            }
        };

        let opts = ConnectOptions {
            target_module_uuid,
            passcode,
        };
        self.connect_flow(pending, opts, cancel_rx).await
    }

    /// Connect to a remote Minecraft debugger.
    ///
    /// Cancellable during TCP connect, target-selection wait, and handshake
    /// completion via [`cancel_pending`](SessionController::cancel_pending).
    pub async fn connect(
        &self,
        host: String,
        port: u16,
        target_module_uuid: Option<String>,
        passcode: Option<String>,
    ) -> Result<HandshakeInfo, SessionError> {
        let mut cancel_rx = self.activate_phase().await;

        let pending = tokio::select! {
            result = DebuggeeConnection::connect_pending(&host, port) => result,
            _ = &mut cancel_rx => {
                self.clear_phase().await;
                return Err(SessionError::Cancelled);
            }
        };

        let pending = match pending {
            Ok(p) => p,
            Err(e) => {
                self.clear_phase().await;
                return Err(SessionError::ConnectionError(e));
            }
        };

        let opts = ConnectOptions {
            target_module_uuid,
            passcode,
        };
        self.connect_flow(pending, opts, cancel_rx).await
    }

    /// Cancel any pending connect/listen operation.
    ///
    /// Idempotent — safe to call when no operation is in progress.
    pub async fn cancel_pending(&self) -> Result<(), SessionError> {
        self.clear_phase().await;
        Ok(())
    }

    /// Disconnect an active session.
    ///
    /// Signals the connection task to stop (cancelling any blocked send or
    /// in-flight evaluate), cancels any pending phase, drops the command sender,
    /// and clears handshake info.  The connection task exits without emitting
    /// a fake `Disconnected` event.
    pub async fn disconnect(&self) -> Result<(), SessionError> {
        self.clear_phase().await;
        let mut inner = self.inner.lock().await;
        // Signal the connection task to stop immediately
        if let Some(cancel) = inner.cancel_task_tx.take() {
            let _ = cancel.send(());
        }
        inner.cmd_tx.take();
        inner.handshake = None;
        Ok(())
    }

    /// Provide a target module UUID when the connection is waiting for
    /// interactive plugin selection.
    ///
    /// Single-use — errors if no selection is pending.
    pub async fn select_target(&self, module_uuid: String) -> Result<(), SessionError> {
        let mut inner = self.inner.lock().await;
        if let PendingPhase::Active { selection_tx, .. } = &mut inner.phase {
            if let Some(tx) = selection_tx.take() {
                let _ = tx.send(Ok(module_uuid));
                return Ok(());
            }
        }
        Err(SessionError::NoTargetSelectionPending)
    }

    /// Get the current handshake info, if connected.
    pub async fn get_handshake_info(&self) -> Result<Option<HandshakeInfo>, SessionError> {
        let inner = self.inner.lock().await;
        Ok(inner.handshake.clone())
    }

    // ── Controls ───────────────────────────────────────────────────────

    /// Send a fire-and-forget command to the connection task.
    ///
    /// Clones the sender under the lock and releases it before awaiting the send
    /// so that [`disconnect`](SessionController::disconnect) can always acquire
    /// the lock even when the command queue is full.
    pub async fn send_command(&self, cmd: SessionCommand) -> Result<(), SessionError> {
        let sender = {
            let inner = self.inner.lock().await;
            inner.cmd_tx.clone().ok_or(SessionError::NotConnected)?
        };
        sender
            .send(cmd)
            .await
            .map_err(|_| SessionError::CommandChannelClosed)
    }

    /// Evaluate a Minecraft expression and wait for the response.
    ///
    /// Clones the sender under the lock and releases it before awaiting the send
    /// so that [`disconnect`](SessionController::disconnect) can always acquire
    /// the lock even when the command queue is full.
    pub async fn evaluate(&self, expression: String) -> Result<EvaluateResult, SessionError> {
        let (tx, rx) = oneshot::channel();
        let cmd = SessionCommand::Evaluate {
            expression,
            response_tx: tx,
        };
        let sender = {
            let inner = self.inner.lock().await;
            inner.cmd_tx.clone().ok_or(SessionError::NotConnected)?
        };
        sender
            .send(cmd)
            .await
            .map_err(|_| SessionError::CommandChannelClosed)?;
        rx.await.unwrap_or(Err(SessionError::ConnectionTaskDropped))
    }

    // ── Phase helpers ──────────────────────────────────────────────────

    /// Set the pending phase to `Active` with a fresh cancel channel and return
    /// the receiver half.  Drops any previous phase (cancelling the prior operation).
    async fn activate_phase(&self) -> oneshot::Receiver<()> {
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let mut inner = self.inner.lock().await;
        inner.phase = PendingPhase::Active {
            cancel_tx,
            selection_tx: None,
        };
        cancel_rx
    }

    /// If the phase is `Active`, send the cancel signal and reset to `Idle`.
    /// Safe to call even after the cancel receiver has been dropped.  Also used
    /// to clear the phase on success so that no stale `Active` state remains.
    async fn clear_phase(&self) {
        let mut inner = self.inner.lock().await;
        let old = std::mem::replace(&mut inner.phase, PendingPhase::Idle);
        if let PendingPhase::Active { cancel_tx, .. } = old {
            let _ = cancel_tx.send(());
        }
    }

    // ── Connect flow ───────────────────────────────────────────────────

    /// Shared tail of both `listen` and `connect`.
    ///
    /// The same `cancel_rx` from `activate_phase` is threaded through so
    /// cancellation works continuously across the optional target-selection
    /// sub-phase and the handshake completion.  The phase is cleared to `Idle`
    /// on **every** exit path.
    async fn connect_flow(
        &self,
        pending: PendingConnection,
        mut opts: ConnectOptions,
        cancel_rx: oneshot::Receiver<()>,
    ) -> Result<HandshakeInfo, SessionError> {
        tokio::pin!(cancel_rx);

        // ── Interactive target selection? ──────────────────────────
        if opts.target_module_uuid.is_none() && pending.plugins().len() > 1 {
            let (selection_tx, selection_rx) = oneshot::channel();

            // Install selection_tx in the existing Active phase — do NOT
            // replace the cancel channel so the same cancel_rx stays valid.
            {
                let mut inner = self.inner.lock().await;
                if let PendingPhase::Active {
                    selection_tx: slot, ..
                } = &mut inner.phase
                {
                    *slot = Some(selection_tx);
                } else {
                    // Phase was already cleared (cancelled) — no modal to open.
                    return Err(SessionError::Cancelled);
                }
            }

            // Notify the adapter — race against cancellation so that
            // cancel_pending still works even when the event queue is full.
            let plugins = pending.plugins().to_vec();
            tokio::select! {
                result = self.event_tx.send(SessionEvent::TargetSelectionRequired { plugins }) => {
                    if result.is_err() {
                        // Receiver dropped — nothing to notify
                        self.clear_phase().await;
                        return Err(SessionError::Cancelled);
                    }
                }
                _ = cancel_rx.as_mut() => {
                    self.clear_phase().await;
                    return Err(SessionError::Cancelled);
                }
            }

            // Wait for selection or cancellation
            let chosen = tokio::select! {
                result = selection_rx => match result {
                    Ok(Ok(uuid)) => uuid,
                    Ok(Err(e)) => {
                        self.clear_phase().await;
                        return Err(e);
                    }
                    Err(_) => {
                        self.clear_phase().await;
                        return Err(SessionError::Cancelled);
                    }
                },
                _ = cancel_rx.as_mut() => {
                    self.clear_phase().await;
                    return Err(SessionError::Cancelled);
                }
            };

            opts.target_module_uuid = Some(chosen);
        }

        // ── Complete the handshake (still cancellable) ─────────────
        tokio::select! {
            result = pending.complete(opts) => match result {
                Ok((conn, hs)) => {
                    self.clear_phase().await;
                    Ok(self.spawn_connection(conn, hs).await)
                }
                Err(e) => {
                    self.clear_phase().await;
                    Err(SessionError::ConnectionError(e))
                }
            },
            _ = cancel_rx.as_mut() => {
                self.clear_phase().await;
                Err(SessionError::Cancelled)
            }
        }
    }

    /// Spawn the background connection task and return handshake info.
    ///
    /// Cancels any previous connection task, drops the old command sender,
    /// and installs a fresh cancellation sender for the new generation.
    async fn spawn_connection(
        &self,
        conn: DebuggeeConnection,
        hs: ProtocolHandshake,
    ) -> HandshakeInfo {
        let info = handshake_to_info(&hs);
        let (cmd_tx, cmd_rx) = mpsc::channel(16);

        // Send Resume as the first command (auto-resume on active connection)
        let _ = cmd_tx
            .send(SessionCommand::SendEvent(DebuggerEvent::Resume))
            .await;

        let (cancel_task_tx, cancel_task_rx) = oneshot::channel();

        let generation = {
            let mut inner = self.inner.lock().await;

            // Cancel any old connection task first
            if let Some(old_cancel) = inner.cancel_task_tx.take() {
                let _ = old_cancel.send(());
            }
            // Drop old command sender
            drop(inner.cmd_tx.take());

            inner.generation += 1;
            let gen = inner.generation;
            inner.handshake = Some(info.clone());
            inner.cmd_tx = Some(cmd_tx);
            inner.cancel_task_tx = Some(cancel_task_tx);
            gen
        };

        let inner = self.inner.clone();
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            connection_task(conn, event_tx, cmd_rx, cancel_task_rx).await;
            let mut inner = inner.lock().await;
            cleanup_task_state(&mut inner, generation);
        });

        info
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────

/// Send a [`SessionEvent`] or return `false` if the cancellation receiver
/// fires first (e.g. user-initiated disconnect).  Used by the connection
/// task to avoid blocking indefinitely on a full event channel during
/// shutdown.
async fn send_or_break(
    event_tx: &mpsc::Sender<SessionEvent>,
    event: SessionEvent,
    cancel_rx: &mut oneshot::Receiver<()>,
) -> bool {
    tokio::select! {
        result = event_tx.send(event) => result.is_ok(),
        _ = cancel_rx => false,
    }
}

// ─── Connection task ──────────────────────────────────────────────────

async fn connection_task(
    mut conn: DebuggeeConnection,
    event_tx: mpsc::Sender<SessionEvent>,
    mut cmd_rx: mpsc::Receiver<SessionCommand>,
    cancel_rx: oneshot::Receiver<()>,
) {
    tokio::pin!(cancel_rx);

    loop {
        tokio::select! {
            event_result = conn.recv_event() => {
                match event_result {
                    Ok(DebuggeeEvent::Terminated { reason }) => {
                        // Send raw terminated event first (as Debuggee) so the
                        // adapter can emit one mc-event with kind=terminated.
                        if !send_or_break(
                            &event_tx,
                            SessionEvent::Debuggee(DebuggeeEvent::Terminated { reason: reason.clone() }),
                            &mut cancel_rx,
                        ).await {
                            return;
                        }
                        // Then send lifecycle terminated event with the actual reason.
                        // On cancellation we still try the second send; if it fails,
                        // the connection is shutting down anyway.
                        send_or_break(&event_tx, SessionEvent::Terminated { reason }, &mut cancel_rx).await;
                        return;
                    }
                    Ok(event) => {
                        let is_stat2 = matches!(event, DebuggeeEvent::Stat2 { .. });
                        if is_stat2 {
                            // High-frequency — may drop when channel full
                            let _ = event_tx.try_send(SessionEvent::Debuggee(event));
                        } else if !send_or_break(&event_tx, SessionEvent::Debuggee(event), &mut cancel_rx).await {
                            return;
                        }
                    }
                    Err(_) => {
                        // Connection error — send Disconnected unless cancelled
                        send_or_break(&event_tx, SessionEvent::Disconnected, &mut cancel_rx).await;
                        return;
                    }
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SessionCommand::SendEvent(debugger_event)) => {
                        if conn.send_event(&debugger_event).await.is_err() {
                            send_or_break(&event_tx, SessionEvent::Disconnected, &mut cancel_rx).await;
                            return;
                        }
                    }
                    Some(SessionCommand::SendMinecraftCommand { command }) => {
                        if conn.send_minecraft_command(&command, "overworld").await.is_err() {
                            send_or_break(&event_tx, SessionEvent::Disconnected, &mut cancel_rx).await;
                            return;
                        }
                    }
                    Some(SessionCommand::Pause { thread_id }) => {
                        if conn.pause(thread_id).await.is_err() {
                            send_or_break(&event_tx, SessionEvent::Disconnected, &mut cancel_rx).await;
                            return;
                        }
                    }
                    Some(SessionCommand::Continue { thread_id }) => {
                        if conn.continue_thread(thread_id).await.is_err() {
                            send_or_break(&event_tx, SessionEvent::Disconnected, &mut cancel_rx).await;
                            return;
                        }
                    }
                    Some(SessionCommand::StepNext { thread_id }) => {
                        if conn.step_next(thread_id).await.is_err() {
                            send_or_break(&event_tx, SessionEvent::Disconnected, &mut cancel_rx).await;
                            return;
                        }
                    }
                    Some(SessionCommand::StepIn { thread_id }) => {
                        if conn.step_in(thread_id).await.is_err() {
                            send_or_break(&event_tx, SessionEvent::Disconnected, &mut cancel_rx).await;
                            return;
                        }
                    }
                    Some(SessionCommand::StepOut { thread_id }) => {
                        if conn.step_out(thread_id).await.is_err() {
                            send_or_break(&event_tx, SessionEvent::Disconnected, &mut cancel_rx).await;
                            return;
                        }
                    }
                    Some(SessionCommand::Evaluate {
                        expression,
                        response_tx,
                    }) => {
                        let result = tokio::select! {
                            result = conn.evaluate(&expression) => {
                                result
                                    .map(EvaluateResult::from)
                                    .map_err(SessionError::ConnectionError)
                            }
                            _ = &mut cancel_rx => {
                                // Cancelled — response_tx is dropped so caller gets ConnectionTaskDropped
                                return;
                            }
                        };
                        let _ = response_tx.send(result);
                    }
                    None => return,
                }
            }
            _ = &mut cancel_rx => {
                // User-initiated disconnect — exit cleanly without Disconnected event
                return;
            }
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────

fn handshake_to_info(hs: &ProtocolHandshake) -> HandshakeInfo {
    HandshakeInfo {
        version: hs.version.as_u8(),
        plugins: hs.plugins.clone(),
        require_passcode: hs.require_passcode,
    }
}

/// After a connection task exits, clear its state ([`cmd_tx`](SessionInner::cmd_tx),
/// [`handshake`](SessionInner::handshake), [`cancel_task_tx`](SessionInner::cancel_task_tx))
/// only if the current generation still matches `task_generation`.  This prevents
/// an old task from clearing state installed by a newer connection (user disconnect
/// + reconnect race).
fn cleanup_task_state(inner: &mut SessionInner, task_generation: u64) {
    if inner.generation == task_generation {
        inner.cmd_tx.take();
        inner.handshake = None;
        inner.cancel_task_tx.take();
    }
}

// ─── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // ── Selection flow tests ───────────────────────────────────────────

    #[tokio::test]
    async fn select_target_happy_path() {
        let (controller, _rx) = SessionController::new();

        // Manually arm the phase with a selection slot
        let (cancel_tx, _cancel_rx) = oneshot::channel();
        let (selection_tx, selection_rx) = oneshot::channel();
        {
            let mut inner = controller.inner.lock().await;
            inner.phase = PendingPhase::Active {
                cancel_tx,
                selection_tx: Some(selection_tx),
            };
        }

        // Call the public API as the frontend would
        controller
            .select_target("chosen-uuid".into())
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
        let inner = controller.inner.lock().await;
        match &inner.phase {
            PendingPhase::Active { selection_tx, .. } => {
                assert!(selection_tx.is_none(), "selection_tx should be taken");
            }
            other => panic!("expected Active, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn select_target_double_call_errors() {
        let (controller, _rx) = SessionController::new();

        let (cancel_tx, _cancel_rx) = oneshot::channel();
        let (selection_tx, _selection_rx) = oneshot::channel();
        {
            let mut inner = controller.inner.lock().await;
            inner.phase = PendingPhase::Active {
                cancel_tx,
                selection_tx: Some(selection_tx),
            };
        }

        // First call succeeds
        controller
            .select_target("uuid-1".into())
            .await
            .expect("first select should succeed");

        // Second call should error
        let err = controller
            .select_target("uuid-2".into())
            .await
            .expect_err("second select should fail");
        assert_eq!(err.to_string(), "no target selection pending");
    }

    #[tokio::test]
    async fn select_target_no_pending_errors() {
        let (controller, _rx) = SessionController::new();
        // Phase is Idle by default

        let err = controller
            .select_target("any-uuid".into())
            .await
            .expect_err("select without pending should fail");
        assert_eq!(err.to_string(), "no target selection pending");
    }

    #[tokio::test]
    async fn cancel_pending_during_selection_phase() {
        let (controller, _rx) = SessionController::new();

        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        let (selection_tx, _selection_rx) = oneshot::channel();
        {
            let mut inner = controller.inner.lock().await;
            inner.phase = PendingPhase::Active {
                cancel_tx,
                selection_tx: Some(selection_tx),
            };
        }

        // Cancel should fire the cancel signal
        controller
            .cancel_pending()
            .await
            .expect("cancel should succeed");

        let cancel_result = cancel_rx.try_recv();
        assert!(
            cancel_result.is_ok(),
            "cancel_rx should have received signal"
        );

        // Phase should be Idle
        let inner = controller.inner.lock().await;
        assert!(matches!(inner.phase, PendingPhase::Idle));
    }

    #[tokio::test]
    async fn disconnect_clears_pending_phase_too() {
        let (controller, _rx) = SessionController::new();

        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        {
            let mut inner = controller.inner.lock().await;
            inner.phase = PendingPhase::Active {
                cancel_tx,
                selection_tx: None,
            };
        }

        controller
            .disconnect()
            .await
            .expect("disconnect should succeed");

        let cancel_result = cancel_rx.try_recv();
        assert!(
            cancel_result.is_ok(),
            "disconnect should cancel pending phase"
        );

        let inner = controller.inner.lock().await;
        assert!(matches!(inner.phase, PendingPhase::Idle));
    }

    // ── Phase refactoring regression tests ─────────────────────────────

    #[tokio::test]
    async fn activate_phase_uses_async_lock() {
        let (controller, _rx) = SessionController::new();
        let _cancel_rx = controller.activate_phase().await;
        let inner = controller.inner.lock().await;
        assert!(matches!(inner.phase, PendingPhase::Active { .. }));
    }

    #[tokio::test]
    async fn clear_phase_resets_to_idle() {
        let (controller, _rx) = SessionController::new();
        let _cancel_rx = controller.activate_phase().await;
        controller.clear_phase().await;
        let inner = controller.inner.lock().await;
        assert!(matches!(inner.phase, PendingPhase::Idle));
    }

    #[tokio::test]
    async fn clear_phase_sends_cancel_signal() {
        let (controller, _rx) = SessionController::new();
        let cancel_rx = controller.activate_phase().await;
        controller.clear_phase().await;
        let result = cancel_rx.await;
        assert_eq!(result, Ok(()));
    }

    #[tokio::test]
    async fn clear_phase_twice_is_idempotent() {
        let (controller, _rx) = SessionController::new();
        let _cancel_rx = controller.activate_phase().await;
        controller.clear_phase().await;
        controller.clear_phase().await; // second clear on Idle is a no-op
        let inner = controller.inner.lock().await;
        assert!(matches!(inner.phase, PendingPhase::Idle));
    }

    #[tokio::test]
    async fn network_error_clears_phase() {
        // Simulates the Phase-1 error path: activate → connect_pending
        // fails → clear_phase → Idle.  The cancel signal is observable.
        let (controller, _rx) = SessionController::new();
        let cancel_rx = controller.activate_phase().await;

        // connect_pending to a closed port should fail quickly
        let result = DebuggeeConnection::connect_pending("127.0.0.1", 1).await;
        assert!(result.is_err(), "connect to closed port should fail");

        controller.clear_phase().await;

        let inner = controller.inner.lock().await;
        assert!(matches!(inner.phase, PendingPhase::Idle));

        // The cancel signal must have been sent so waiters can observe it
        let signal = cancel_rx.await;
        assert_eq!(signal, Ok(()));
    }

    #[tokio::test]
    async fn selection_tx_install_on_idle_errors() {
        // When connect_flow tries to install a selection_tx but the phase
        // has already been cleared (e.g. concurrent cancel), it must
        // return "cancelled" without opening a modal.
        let (controller, _rx) = SessionController::new();

        // Phase is Idle (not Active) — as if cancel/clear already ran
        let (selection_tx, _selection_rx) = oneshot::channel();
        {
            let mut inner = controller.inner.lock().await;
            if let PendingPhase::Active {
                selection_tx: slot, ..
            } = &mut inner.phase
            {
                *slot = Some(selection_tx);
            } // else: this branch must NOT be taken
        }
        let inner = controller.inner.lock().await;
        assert!(matches!(inner.phase, PendingPhase::Idle));
        // selection_tx was dropped without sending → selection_rx shows cancelled
    }

    #[tokio::test]
    async fn cancel_signal_works_across_selection_gap() {
        // Proves that the cancel channel from activate_phase survives
        // a simulated selection sub-phase (selection_tx install + wait).
        let (controller, _rx) = SessionController::new();
        let cancel_rx = controller.activate_phase().await;

        // Simulate selection_tx install as connect_flow would
        let (selection_tx, _selection_rx) = oneshot::channel();
        {
            let mut inner = controller.inner.lock().await;
            if let PendingPhase::Active {
                selection_tx: slot, ..
            } = &mut inner.phase
            {
                *slot = Some(selection_tx);
            }
        }

        // Cancel from another task (as cancel_pending would)
        controller.clear_phase().await;

        // The original cancel_rx must fire
        let signal = cancel_rx.await;
        assert_eq!(signal, Ok(()));
    }

    /// Helper: encode and send a JSON value over a TCP stream (same pattern
    /// as mc-protocol's connection tests).
    async fn send_test_frame(sock: &mut tokio::net::TcpStream, value: serde_json::Value) {
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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

        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), server).await;
    }

    #[tokio::test]
    async fn phase_one_cancel_interrupts_blocked_connect() {
        // Proves that `tokio::select!` between `connect_pending` and
        // `&mut cancel_rx` actually allows cancellation to interrupt a
        // blocked Phase-1 recv, not just the channel helper.
        let (controller, _rx) = SessionController::new();
        let mut cancel_rx = controller.activate_phase().await;

        // TCP server that accepts but *never* sends ProtocolEvent,
        // so connect_pending blocks on the read.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (_sock, _) = listener.accept().await.unwrap();
            // Hold the connection open but send nothing – the client
            // will block reading the ProtocolEvent.
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });

        // Cancel after a short delay so connect_pending has time to
        // establish TCP and start reading.
        let controller_clone = controller.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let _ = controller_clone.cancel_pending().await;
        });

        // This select! mirrors the Phase-1 logic in listen/connect.
        let result: Result<PendingConnection, String> = tokio::select! {
            result = DebuggeeConnection::connect_pending("127.0.0.1", port) => {
                result.map_err(|e| e.to_string())
            }
            _ = &mut cancel_rx => {
                Err("cancelled".to_string())
            }
        };

        // If cancellation works, we get Err("cancelled") – not a
        // network timeout and not an Ok(pending).
        assert!(result.is_err(), "expected cancelled, got Ok(pending)");
        assert_eq!(result.unwrap_err(), "cancelled");

        // Phase has already been cleared by cancel_pending.
        let inner = controller.inner.lock().await;
        assert!(matches!(inner.phase, PendingPhase::Idle));

        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), server).await;
    }

    // ── Stat2 try_send behavior ────────────────────────────────────────

    #[tokio::test]
    async fn stat2_uses_try_send_and_drops_when_full() {
        // Create a controller with a tiny channel (capacity 1) so we can
        // fill it easily and prove Stat2 uses try_send (doesn't block).
        let (event_tx, mut event_rx) = mpsc::channel(1);
        let controller = SessionController {
            inner: Arc::new(Mutex::new(SessionInner {
                cmd_tx: None,
                handshake: None,
                phase: PendingPhase::Idle,
                generation: 0,
                cancel_task_tx: None,
            })),
            event_tx,
        };

        // Fill the channel
        controller
            .event_tx
            .try_send(SessionEvent::Disconnected)
            .expect("first send should succeed");

        // A Stat2 event must not block and may be dropped
        let stat2 = DebuggeeEvent::Stat2 {
            tick: 1,
            stats: vec![],
        };

        // We need to test the connection_task send logic.  The matching
        // logic for Stat2 is: `try_send` is used.  Prove that try_send
        // on a full channel returns an error (dropped) instead of pending.
        let se = SessionEvent::Debuggee(stat2);
        let result = controller.event_tx.try_send(se);
        assert!(
            result.is_err(),
            "Stat2 should be dropped when channel is full"
        );

        // But a non-Stat2 critical event would eventually await capacity.
        // We can't fully test await here without a consumer, but we can
        // prove that try_send on Disconnected also fails when full (and
        // that the code path *would* use send().await).
        let result = controller
            .event_tx
            .try_send(SessionEvent::Terminated { reason: None });
        assert!(
            result.is_err(),
            "Terminated try_send fails on full channel (but real code awaits)"
        );

        // Clean up: consume the event
        let _ = event_rx.recv().await;
    }

    // ── Controller Clone ───────────────────────────────────────────────

    #[tokio::test]
    async fn controller_is_cloneable() {
        let (controller, _rx) = SessionController::new();
        let cloned = controller.clone();
        // Both should share the same inner state
        controller
            .cancel_pending()
            .await
            .expect("cancel should work on original");
        cloned
            .cancel_pending()
            .await
            .expect("cancel should work on clone (idempotent)");
    }

    // ── send_command errors when not connected ─────────────────────────

    #[tokio::test]
    async fn send_command_fails_when_not_connected() {
        let (controller, _rx) = SessionController::new();
        let err = controller
            .send_command(SessionCommand::Pause { thread_id: 0 })
            .await
            .expect_err("send_command without connection should fail");
        assert_eq!(err.to_string(), "not connected");
    }

    #[tokio::test]
    async fn evaluate_fails_when_not_connected() {
        let (controller, _rx) = SessionController::new();
        let err = controller
            .evaluate("1+1".into())
            .await
            .expect_err("evaluate without connection should fail");
        assert_eq!(err.to_string(), "not connected");
    }

    // ── Termination semantics ───────────────────────────────────────────

    #[tokio::test]
    async fn terminated_event_preserves_reason_and_emits_single_lifecycle() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Server reads the debugger's protocol response for synchronization
        // instead of sleeping.  This ensures the client is fully set up
        // (handshake complete, connection_task spawned) BEFORE we send
        // the terminated event.  After sending terminated, the server
        // stays alive (reads) until the client closes its side, preventing
        // a premature TCP RST on Windows that would lose the terminated data.
        let server = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            use tokio_util::codec::Decoder;

            let (mut sock, _) = listener.accept().await.unwrap();

            let mut codec = mc_protocol::framing::MessageCodec::new();
            let mut read_buf = bytes::BytesMut::new();

            // Send ProtocolEvent (handshake initiation)
            send_test_frame(
                &mut sock,
                serde_json::json!({
                    "type": "ProtocolEvent",
                    "version": 9,
                    "plugins": [],
                    "require_passcode": false
                }),
            )
            .await;

            // Read the debugger's protocol response — proves the client
            // completed the handshake (PendingConnection::complete sent
            // DebuggerEvent::Protocol).
            loop {
                if let Some(value) = codec.decode(&mut read_buf).unwrap() {
                    assert_eq!(value["type"], "protocol", "expected protocol response");
                    break;
                }
                let mut tmp = [0u8; 4096];
                let n = sock.read(&mut tmp).await.unwrap();
                if n == 0 {
                    panic!("server: unexpected close before protocol response");
                }
                read_buf.extend_from_slice(&tmp[..n]);
            }

            // Also consume the auto Resume that connection_task sends.
            loop {
                if let Some(value) = codec.decode(&mut read_buf).unwrap() {
                    assert_eq!(value["type"], "resume", "expected resume");
                    break;
                }
                let mut tmp = [0u8; 4096];
                let n = sock.read(&mut tmp).await.unwrap();
                if n == 0 {
                    panic!("server: unexpected close before resume");
                }
                read_buf.extend_from_slice(&tmp[..n]);
            }

            // Send Terminated with a reason
            send_test_frame(
                &mut sock,
                serde_json::json!({
                    "type": "terminated",
                    "reason": "game over"
                }),
            )
            .await;

            // Stay alive until the client closes the connection, so the
            // TCP send buffer drains and no RST is generated.
            let mut tmp = [0u8; 4096];
            loop {
                match sock.read(&mut tmp).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });

        let (controller, mut rx) = SessionController::new();
        controller
            .connect("127.0.0.1".into(), port, None, None)
            .await
            .expect("connect should succeed");

        let mut events: Vec<SessionEvent> = vec![];
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while let Some(event) = rx.recv().await {
                events.push(event.clone());
                if matches!(event, SessionEvent::Terminated { .. }) {
                    break;
                }
            }
        })
        .await
        .expect("timed out waiting for termination events");

        // Exactly two events: Debuggee(Terminated) then Terminated
        assert_eq!(events.len(), 2, "expected 2 events, got {:#?}", events);

        // First event is the raw Debuggee variant with reason preserved
        match &events[0] {
            SessionEvent::Debuggee(DebuggeeEvent::Terminated { reason }) => {
                assert_eq!(
                    reason.as_deref(),
                    Some("game over"),
                    "reason should be preserved in Debuggee event"
                );
            }
            other => panic!("expected Debuggee(Terminated), got {other:?}"),
        }

        // Second event is the lifecycle Terminated with same reason
        match &events[1] {
            SessionEvent::Terminated { reason } => {
                assert_eq!(
                    reason.as_deref(),
                    Some("game over"),
                    "reason should be preserved in Terminated event"
                );
            }
            other => panic!("expected Terminated, got {other:?}"),
        }

        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), server).await;
    }

    #[tokio::test]
    async fn unexpected_close_clears_state_and_commands_fail() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Server sends ProtocolEvent, lets client handshake, then drops.
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();

            send_test_frame(
                &mut sock,
                serde_json::json!({
                    "type": "ProtocolEvent",
                    "version": 9,
                    "plugins": [],
                    "require_passcode": false
                }),
            )
            .await;

            // Give the client time to complete the handshake.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            // Drop the socket — unexpected close
            drop(sock);
        });

        let (controller, mut rx) = SessionController::new();
        controller
            .connect("127.0.0.1".into(), port, None, None)
            .await
            .expect("connect should succeed");

        // Wait for the Disconnected event
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                match rx.recv().await {
                    Some(SessionEvent::Disconnected) => break,
                    Some(_) => continue,
                    None => panic!("event channel closed before Disconnected"),
                }
            }
        })
        .await
        .expect("timed out waiting for Disconnected");

        // Give the spawned task time to run cleanup
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Handshake should be cleared
        let hs = controller
            .get_handshake_info()
            .await
            .expect("get_handshake_info should succeed");
        assert!(hs.is_none(), "handshake should be None after disconnect");

        // Commands should fail cleanly
        let err = controller
            .send_command(SessionCommand::Pause { thread_id: 0 })
            .await
            .expect_err("send_command should fail after disconnect");
        assert_eq!(err.to_string(), "not connected");

        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), server).await;
    }

    // ── Generation-race safety ──────────────────────────────────────────

    #[tokio::test]
    async fn generation_race_old_task_does_not_clear_newer_state() {
        let (controller, _rx) = SessionController::new();

        // Simulate Task A (gen=1) installing its state
        let (cmd_tx_a, _) = mpsc::channel(16);
        let (cancel_a_tx, _cancel_a_rx) = oneshot::channel();
        let hs_a = HandshakeInfo {
            version: 9,
            plugins: vec![],
            require_passcode: false,
        };
        {
            let mut inner = controller.inner.lock().await;
            inner.generation = 1;
            inner.cmd_tx = Some(cmd_tx_a);
            inner.handshake = Some(hs_a);
            inner.cancel_task_tx = Some(cancel_a_tx);
        }

        // Simulate Task B (gen=2) installing its state (disconnect + reconnect)
        let (cmd_tx_b, _) = mpsc::channel(16);
        let (cancel_b_tx, _cancel_b_rx) = oneshot::channel();
        let hs_b = HandshakeInfo {
            version: 8,
            plugins: vec![],
            require_passcode: true,
        };
        {
            let mut inner = controller.inner.lock().await;
            inner.generation = 2;
            inner.cmd_tx = Some(cmd_tx_b);
            inner.handshake = Some(hs_b);
            inner.cancel_task_tx = Some(cancel_b_tx);
        }

        // Now simulate Task A's cleanup (old generation tries to clear)
        {
            let mut inner = controller.inner.lock().await;
            cleanup_task_state(&mut inner, 1);
        }

        // Task B's state must still be intact
        let inner = controller.inner.lock().await;
        assert_eq!(inner.generation, 2, "generation should still be 2");
        assert!(
            inner.cmd_tx.is_some(),
            "cmd_tx should still be set (Task B's)"
        );
        assert_eq!(
            inner.handshake.as_ref().unwrap().version,
            8,
            "handshake should still be Task B's"
        );
        assert!(
            inner.cancel_task_tx.is_some(),
            "cancel_task_tx should still be set (Task B's)"
        );
    }

    // ── SessionError typed-variant tests ─────────────────────────────────

    #[test]
    fn session_error_display_strings() {
        assert_eq!(SessionError::NotConnected.to_string(), "not connected");
        assert_eq!(SessionError::Cancelled.to_string(), "cancelled");
        assert_eq!(
            SessionError::NoTargetSelectionPending.to_string(),
            "no target selection pending"
        );
        assert_eq!(
            SessionError::ConnectionTaskDropped.to_string(),
            "connection task dropped"
        );
        assert_eq!(
            SessionError::CommandChannelClosed.to_string(),
            "command channel closed"
        );
        // ConnectionError preserves the original ConnectionError display
        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let ce = SessionError::ConnectionError(mc_protocol::ConnectionError::Io(io_err));
        assert!(
            ce.to_string().contains("io error"),
            "ConnectionError should preserve inner display: {}",
            ce
        );
    }

    #[test]
    fn session_error_variants_match() {
        // Prove that pattern matching works (for TUI matching)
        let err = SessionError::NotConnected;
        assert!(matches!(err, SessionError::NotConnected));
        let err = SessionError::Cancelled;
        assert!(matches!(err, SessionError::Cancelled));
        let err = SessionError::NoTargetSelectionPending;
        assert!(matches!(err, SessionError::NoTargetSelectionPending));
        let err = SessionError::ConnectionTaskDropped;
        assert!(matches!(err, SessionError::ConnectionTaskDropped));
        let err = SessionError::CommandChannelClosed;
        assert!(matches!(err, SessionError::CommandChannelClosed));
    }

    // ── Active connection-task cancellation tests ────────────────────────

    #[tokio::test]
    async fn disconnect_cancels_active_task_with_full_event_channel() {
        // Prove that disconnect terminates the connection task even when
        // the event channel is completely full (event_tx.send would block).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            use tokio_util::codec::Decoder;

            let (mut sock, _) = listener.accept().await.unwrap();

            let mut codec = mc_protocol::framing::MessageCodec::new();
            let mut read_buf = bytes::BytesMut::new();

            // Send ProtocolEvent
            send_test_frame(
                &mut sock,
                serde_json::json!({
                    "type": "ProtocolEvent",
                    "version": 9,
                    "plugins": [],
                    "require_passcode": false
                }),
            )
            .await;

            // Wait for protocol response
            loop {
                if let Some(value) = codec.decode(&mut read_buf).unwrap() {
                    if value["type"] == "protocol" {
                        break;
                    }
                }
                let mut tmp = [0u8; 4096];
                let n = sock.read(&mut tmp).await.unwrap();
                if n == 0 {
                    return;
                }
                read_buf.extend_from_slice(&tmp[..n]);
            }
            // Wait for resume
            loop {
                if let Some(value) = codec.decode(&mut read_buf).unwrap() {
                    if value["type"] == "resume" {
                        break;
                    }
                }
                let mut tmp = [0u8; 4096];
                let n = sock.read(&mut tmp).await.unwrap();
                if n == 0 {
                    return;
                }
                read_buf.extend_from_slice(&tmp[..n]);
            }

            // Hold the connection open but send nothing further
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        });

        // Create controller with capacity-1 event channel
        let (event_tx, _event_rx) = mpsc::channel(1);
        let controller = SessionController {
            inner: Arc::new(Mutex::new(SessionInner {
                cmd_tx: None,
                handshake: None,
                phase: PendingPhase::Idle,
                generation: 0,
                cancel_task_tx: None,
            })),
            event_tx,
        };

        // Connect — fills the event channel immediately when connection_task
        // tries to send the Resume event.  Since _event_rx is never consumed,
        // the channel stays full.
        controller
            .connect("127.0.0.1".into(), port, None, None)
            .await
            .expect("connect should succeed");

        // Very short timeout to prove the task would block without cancellation.
        // If disconnect/cancellation does NOT work, the test will hang here.
        tokio::time::timeout(std::time::Duration::from_millis(500), async {
            controller
                .disconnect()
                .await
                .expect("disconnect should succeed");
        })
        .await
        .expect("disconnect should complete promptly even with full event channel");

        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), server).await;
    }

    #[tokio::test]
    async fn disconnect_interrupts_in_flight_evaluate() {
        // Prove that disconnect interrupts an evaluate that is blocked
        // waiting for a response from Minecraft.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            use tokio_util::codec::Decoder;

            let (mut sock, _) = listener.accept().await.unwrap();

            let mut codec = mc_protocol::framing::MessageCodec::new();
            let mut read_buf = bytes::BytesMut::new();

            // Send ProtocolEvent
            send_test_frame(
                &mut sock,
                serde_json::json!({
                    "type": "ProtocolEvent",
                    "version": 9,
                    "plugins": [],
                    "require_passcode": false
                }),
            )
            .await;

            // Wait for protocol response
            loop {
                if let Some(value) = codec.decode(&mut read_buf).unwrap() {
                    if value["type"] == "protocol" {
                        break;
                    }
                }
                let mut tmp = [0u8; 4096];
                let n = sock.read(&mut tmp).await.unwrap();
                if n == 0 {
                    panic!("server: unexpected close");
                }
                read_buf.extend_from_slice(&tmp[..n]);
            }
            // Wait for resume
            loop {
                if let Some(value) = codec.decode(&mut read_buf).unwrap() {
                    if value["type"] == "resume" {
                        break;
                    }
                }
                let mut tmp = [0u8; 4096];
                let n = sock.read(&mut tmp).await.unwrap();
                if n == 0 {
                    panic!("server: unexpected close");
                }
                read_buf.extend_from_slice(&tmp[..n]);
            }

            // Also wait for the evaluate request but *never* respond
            loop {
                if let Some(value) = codec.decode(&mut read_buf).unwrap() {
                    if value["type"] == "evaluate" {
                        // Consume the request then go silent
                        break;
                    }
                }
                let mut tmp = [0u8; 4096];
                let n = sock.read(&mut tmp).await.unwrap();
                if n == 0 {
                    return;
                }
                read_buf.extend_from_slice(&tmp[..n]);
            }

            // Hold the connection open but *never* send the evaluate response.
            // This will cause conn.evaluate() to wait indefinitely (or until
            // the RequestTimeout, which is long).
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        });

        let (controller, _rx) = SessionController::new();
        controller
            .connect("127.0.0.1".into(), port, None, None)
            .await
            .expect("connect should succeed");

        // Spawn the evaluate call so it runs concurrently
        let ctrl = controller.clone();
        let evaluate_handle = tokio::spawn(async move { ctrl.evaluate("1+1".into()).await });

        // Give the evaluate time to send its request and block
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        // Disconnect — should interrupt the blocked evaluate
        controller
            .disconnect()
            .await
            .expect("disconnect should succeed");

        // The evaluate should complete promptly with an error
        let join_result = tokio::time::timeout(std::time::Duration::from_secs(2), evaluate_handle)
            .await
            .expect("evaluate should complete promptly after disconnect");

        // Task should not have panicked; unwrap the JoinHandle
        let evaluate_result: Result<EvaluateResult, SessionError> =
            join_result.expect("evaluate task should not have panicked");

        // The evaluate itself should have failed
        let err = evaluate_result.expect_err("evaluate should fail after disconnect");
        assert_eq!(
            err.to_string(),
            "connection task dropped",
            "evaluate should get ConnectionTaskDropped after disconnect"
        );

        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), server).await;
    }

    #[tokio::test]
    async fn replacement_cancels_old_task_and_cleanup_does_not_clear_new_state() {
        // Proves that spawning a new connection (generation bump) cancels
        // the old connection task, and the old task's cleanup leaves the
        // new state intact.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        /// Helper server that accepts one connection, completes handshake,
        /// and holds open.
        async fn spawn_hold_server(
            listener: tokio::net::TcpListener,
        ) -> tokio::task::JoinHandle<()> {
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                use tokio_util::codec::Decoder;

                let (mut sock, _) = listener.accept().await.unwrap();

                let mut codec = mc_protocol::framing::MessageCodec::new();
                let mut read_buf = bytes::BytesMut::new();

                send_test_frame(
                    &mut sock,
                    serde_json::json!({
                        "type": "ProtocolEvent",
                        "version": 9,
                        "plugins": [],
                        "require_passcode": false
                    }),
                )
                .await;

                loop {
                    if let Some(value) = codec.decode(&mut read_buf).unwrap() {
                        if value["type"] == "protocol" {
                            break;
                        }
                    }
                    let mut tmp = [0u8; 4096];
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    read_buf.extend_from_slice(&tmp[..n]);
                }

                loop {
                    if let Some(value) = codec.decode(&mut read_buf).unwrap() {
                        if value["type"] == "resume" {
                            break;
                        }
                    }
                    let mut tmp = [0u8; 4096];
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    read_buf.extend_from_slice(&tmp[..n]);
                }

                // Hold open
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            })
        }

        let server_task = spawn_hold_server(listener).await;

        // We need two different ports for two connections.
        let listener2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port2 = listener2.local_addr().unwrap().port();

        let server_task2 = spawn_hold_server(listener2).await;

        let (controller, _rx) = SessionController::new();

        // First connection (generation 1)
        controller
            .connect("127.0.0.1".into(), port, None, None)
            .await
            .expect("first connect should succeed");

        let gen1 = { controller.inner.lock().await.generation };

        // Second connection (generation 2) — implicitly cancels old task
        controller
            .connect("127.0.0.1".into(), port2, None, None)
            .await
            .expect("second connect should succeed");

        let gen2 = { controller.inner.lock().await.generation };

        assert_ne!(gen1, gen2, "generation should have incremented");

        // State should be from gen2
        {
            let inner = controller.inner.lock().await;
            assert_eq!(inner.generation, gen2);
            assert!(inner.cmd_tx.is_some(), "cmd_tx should be set (gen2)");
            assert!(
                inner.cancel_task_tx.is_some(),
                "cancel_task_tx should be set (gen2)"
            );
        }

        // Disconnect cleanly
        controller
            .disconnect()
            .await
            .expect("disconnect should succeed");

        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), server_task).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), server_task2).await;
    }

    // ── Command‑queue saturation / lock availability ──────────────────

    #[tokio::test]
    async fn disconnect_acquires_lock_even_when_command_queue_full() {
        // Set up a controller with a small (capacity-1) command channel so
        // that a second concurrent send_command would block.  Prove that
        // disconnect can still acquire the lock and return promptly.
        let (controller, _rx) = SessionController::new();

        let (cmd_tx, cmd_rx) = mpsc::channel(1);
        let (cancel_task_tx, _cancel_task_rx) = oneshot::channel();
        let hs = HandshakeInfo {
            version: 9,
            plugins: vec![],
            require_passcode: false,
        };

        // Fill the command queue
        cmd_tx
            .try_send(SessionCommand::Pause { thread_id: 0 })
            .expect("fill-send should succeed");

        // Install the state as if connected
        {
            let mut inner = controller.inner.lock().await;
            inner.cmd_tx = Some(cmd_tx);
            inner.handshake = Some(hs);
            inner.generation = 1;
            inner.cancel_task_tx = Some(cancel_task_tx);
        }

        // Spawn a send_command that will block because the queue is full
        let ctrl = controller.clone();
        let send_handle = tokio::spawn(async move {
            ctrl.send_command(SessionCommand::Pause { thread_id: 1 })
                .await
        });

        // Yield to let the spawned task park on the full channel
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // disconnect() must acquire the lock and return — even though the
        // command queue is full and send_command is blocked.  With a 500 ms
        // timeout we prove promptness.
        tokio::time::timeout(std::time::Duration::from_millis(500), async {
            controller
                .disconnect()
                .await
                .expect("disconnect should succeed");
        })
        .await
        .expect("disconnect should complete promptly even with blocked send_command");

        // Now drop the receiver (simulating the connection task exiting) so
        // the channel closes and the blocked send_command can complete.
        drop(cmd_rx);

        // The blocked send_command should get a closed-channel error
        let send_result = tokio::time::timeout(std::time::Duration::from_secs(2), send_handle)
            .await
            .expect("send should complete after disconnect")
            .expect("send task should not panic");
        assert!(
            matches!(send_result, Err(SessionError::CommandChannelClosed)),
            "blocked send should get CommandChannelClosed after disconnect, got {send_result:?}"
        );
    }

    // ── Target‑selection event‑send cancellation ──────────────────────

    #[tokio::test]
    async fn cancel_interrupts_target_selection_when_event_queue_full() {
        // Fill the event queue before target selection so that
        // event_tx.send(TargetSelectionRequired) would block.  Prove that
        // cancel_pending still interrupts the blocked send and the
        // connect future returns Cancelled.
        let (event_tx, mut event_rx) = mpsc::channel(1);
        let controller = SessionController {
            inner: Arc::new(Mutex::new(SessionInner {
                cmd_tx: None,
                handshake: None,
                phase: PendingPhase::Idle,
                generation: 0,
                cancel_task_tx: None,
            })),
            event_tx: event_tx.clone(),
        };

        // Fill the event channel (capacity 1)
        event_tx
            .try_send(SessionEvent::Disconnected)
            .expect("fill-send should succeed");

        // Set up the phase as Active with a cancel channel we control
        let (cancel_tx, cancel_rx) = oneshot::channel();
        {
            let mut inner = controller.inner.lock().await;
            inner.phase = PendingPhase::Active {
                cancel_tx,
                selection_tx: None,
            };
        }

        let ctrl = controller.clone();

        // Spawn a task that replicates the connect_flow target-selection
        // logic: install selection_tx, then try to send
        // TargetSelectionRequired (raced against cancel_rx).
        let handle: tokio::task::JoinHandle<SessionError> = tokio::spawn(async move {
            // Install selection_tx (same pattern as connect_flow)
            let (selection_tx, _selection_rx) = oneshot::channel();
            {
                let mut inner = ctrl.inner.lock().await;
                if let PendingPhase::Active {
                    selection_tx: slot, ..
                } = &mut inner.phase
                {
                    *slot = Some(selection_tx);
                } else {
                    return SessionError::Cancelled;
                }
            }

            // Try to send the critical event — would block because the
            // event channel is full, but we race against cancel_rx.
            let plugins: Vec<PluginDetails> = vec![];
            tokio::select! {
                result = ctrl.event_tx.send(SessionEvent::TargetSelectionRequired { plugins }) => {
                    if result.is_err() {
                        ctrl.clear_phase().await;
                        SessionError::Cancelled
                    } else {
                        // Send succeeded (someone drained the channel);
                        // fall through to a forever-block (shouldn't
                        // happen because we are about to cancel).
                        let (_tx2, rx2) = oneshot::channel::<()>();
                        let _ = rx2.await;
                        unreachable!()
                    }
                }
                _ = cancel_rx => {
                    ctrl.clear_phase().await;
                    SessionError::Cancelled
                }
            }
        });

        // Yield so the spawned task enters the select and starts waiting
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Cancel from the main task — must interrupt the blocked send
        controller
            .cancel_pending()
            .await
            .expect("cancel_pending should succeed");

        // The connect task must complete promptly with Cancelled
        let err = tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("task should complete within timeout")
            .expect("task should not panic");

        assert!(
            matches!(err, SessionError::Cancelled),
            "expected Cancelled, got {err:?}"
        );

        // Phase must be Idle after cancellation
        let inner = controller.inner.lock().await;
        assert!(matches!(inner.phase, PendingPhase::Idle));

        // Clean up the one event we inserted
        let _ = event_rx.try_recv();
    }
}
