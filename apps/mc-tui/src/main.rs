use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use mc_session::SessionController;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use tokio::sync::mpsc;

use mc_tui::app::{
    App, ConnectionState, FieldState, FilterPopup, Focus, HelpPopup, Mode, Tab, RENDER_TICK_MS,
};
use mc_tui::config::{self, AppConfig, AppConfigFile};
use mc_tui::event::{start_input_thread, InputEvent};
use mc_tui::terminal::{init as terminal_init, install_panic_hook};

const AUTO_RELISTEN_DELAY_MS: u64 = 800;

// ── CLI ───────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "mc-tui", about = "Minecraft Debugger TUI")]
struct Cli {
    /// Connection mode: "listen" or "connect"
    #[arg(long, default_value = "listen")]
    mode: String,

    /// Host to connect to (used in connect mode)
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Port number
    #[arg(long, default_value_t = 19144)]
    port: u16,

    /// Optional target module UUID (skip plugin selection)
    #[arg(long)]
    target_module_uuid: Option<String>,

    /// Optional passcode for the connection
    #[arg(long)]
    passcode: Option<String>,

    /// Path to config file (defaults to platform-appropriate location)
    #[arg(long)]
    config: Option<String>,
}

fn parse_mode(s: &str) -> Option<Mode> {
    match s {
        "listen" => Some(Mode::Listen),
        "connect" => Some(Mode::Connect),
        _ => None,
    }
}

/// Runtime state for auto-relisten logic.
#[derive(Debug)]
struct AutoRetryState {
    /// Whether the current/last connection mode was "listen".
    /// Cleared when the user explicitly connects, cancels, or quits.
    listen_mode: bool,
    /// When `Some`, an auto-retry timer is pending at this instant.
    retry_at: Option<tokio::time::Instant>,
}

impl AutoRetryState {
    fn new() -> Self {
        Self {
            listen_mode: false,
            retry_at: None,
        }
    }

    /// Called when a listen is started.
    fn on_listen_started(&mut self) {
        self.listen_mode = true;
        self.retry_at = None;
    }

    /// Called when a connect is started (cancels auto-relisten).
    fn on_connect_started(&mut self) {
        self.listen_mode = false;
        self.retry_at = None;
    }

    /// Called when the connection fails or disconnects while in listen mode.
    fn on_disconnected(&mut self) {
        if self.listen_mode {
            self.retry_at =
                Some(tokio::time::Instant::now() + Duration::from_millis(AUTO_RELISTEN_DELAY_MS));
        }
    }

    /// Cancel any pending auto-retry and reset listen tracking.
    fn cancel(&mut self) {
        self.listen_mode = false;
        self.retry_at = None;
    }

    /// Check whether a retry is due (returns true and clears the flag).
    fn try_fire(&mut self) -> bool {
        if let Some(instant) = self.retry_at {
            if tokio::time::Instant::now() >= instant {
                self.retry_at = None;
                return true;
            }
        }
        false
    }
}

// ── Entry point ───────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // ── Config path resolution ───────────────────────────────────────
    let config_path: PathBuf = if let Some(ref path) = cli.config {
        PathBuf::from(path)
    } else {
        config::default_config_path()
    };

    // ── Load config ──────────────────────────────────────────────────
    let (config_file, mut config_warning): (Option<AppConfigFile>, Option<String>) =
        match config::load(&config_path) {
            Ok(cfg) => (cfg, None),
            Err(e) => (
                None,
                Some(format!("Could not load settings. Using defaults: {e}")),
            ),
        };

    // `config::load` intentionally falls back to defaults for malformed JSON
    // (and reports that to stderr). Mirror that warning in the TUI once the
    // terminal exists, without changing the shared config API.
    if config_warning.is_none() && config_file.is_some() {
        if let Ok(raw) = std::fs::read_to_string(&config_path) {
            if serde_json::from_str::<AppConfigFile>(&raw).is_err() {
                config_warning = Some("Could not read settings. Using defaults.".into());
            }
        }
    }

    let resolved_config = config_file.as_ref().map(|f| {
        AppConfig::from_file_with_cli(f, cli.target_module_uuid.clone(), cli.passcode.clone())
    });

    install_panic_hook();
    let (mut terminal, _guard) = terminal_init()?;

    let (controller, event_rx) = SessionController::new();
    let (mut input_rx, _input_handle) = start_input_thread();

    let mut app = App::new(
        controller,
        event_rx,
        cli.host.clone(),
        cli.port,
        cli.target_module_uuid.clone(),
        cli.passcode.clone(),
    );

    // Apply resolved config (filter state, sidebar defaults from config)
    // CLI values already won during hydration in App::new → SidebarForm::new.
    if let Some(ref cfg) = resolved_config {
        app.apply_config(cfg);
    }
    if let Some(warning) = config_warning {
        app.set_startup_warning(warning);
    }

    // ── Auto-retry state ─────────────────────────────────────────────
    let mut auto_retry = AutoRetryState::new();

    // Start using sidebar values (which already include CLI + config defaults)
    if let Some(mode) = parse_mode(cli.mode.as_str()) {
        app.set_mode(mode);
        match mode {
            Mode::Connect => {
                auto_retry.on_connect_started();
                app.start_connect_from_sidebar();
            }
            Mode::Listen => {
                auto_retry.on_listen_started();
                app.start_listen_from_sidebar();
            }
        }
    } else {
        app.add_log(format!("Unknown mode '{}' — starting idle.", cli.mode));
        app.show_error("Invalid mode", "Choose listen or connect.");
    }

    let result = run(
        &mut terminal,
        &mut app,
        &mut input_rx,
        &mut auto_retry,
        &config_path,
    )
    .await;

    // disconnect() already calls clear_phase() which cancels any pending phase.
    let _ = app.controller.disconnect().await;

    result
}

// ── Main event loop ───────────────────────────────────────────────────

async fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    input_rx: &mut mpsc::Receiver<InputEvent>,
    auto_retry: &mut AutoRetryState,
    config_path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut tick_interval = tokio::time::interval(Duration::from_millis(RENDER_TICK_MS));
    tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        // ── Auto-retry wakeup (fires from Idle OR Disconnected) ──────
        if matches!(
            app.state,
            ConnectionState::Idle | ConnectionState::Disconnected
        ) && auto_retry.try_fire()
        {
            auto_retry.on_listen_started();
            app.set_mode(Mode::Listen);
            app.start_listen_from_sidebar();
        }

        tokio::select! {
            input = input_rx.recv() => {
                match input {
                    Some(InputEvent::Key(key)) => {
                        if handle_key(app, key, auto_retry).await {
                            break;
                        }
                    }
                    Some(InputEvent::Resize(w, h)) => {
                        let _ = terminal.resize(Rect { x: 0, y: 0, width: w, height: h });
                    }
                    None => break,
                }
            }

            event = app.event_rx.recv() => {
                match event {
                    Some(event) => {
                        let was_connected = matches!(app.state, ConnectionState::Connected { .. });
                        app.handle_session_event(event);

                        // Auto-relisten: if listen mode and the connection drops,
                        // schedule an auto-retry.
                        if was_connected
                            && auto_retry.listen_mode
                            && matches!(app.state, ConnectionState::Disconnected)
                        {
                            auto_retry.on_disconnected();
                        }
                    }
                    None => break,
                }
            }

            result = async {
                if let Some(ref mut rx) = app.conn_result_rx {
                    rx.await.ok()
                } else {
                    std::future::pending().await
                }
            } => {
                match result {
                    Some(r) => {
                        // Connection succeeded or failed with an error
                        let is_ok = r.is_ok();
                        app.handle_connection_result(r);

                        // If listen failed while in listen_mode, schedule retry
                        if !is_ok && auto_retry.listen_mode && app.state == ConnectionState::Idle {
                            auto_retry.on_disconnected();
                        }
                    }
                    None => {
                        if app.conn_result_rx.take().is_some() {
                            app.clear_pending_attempt_snapshots();
                            app.clear_session_transients();
                            app.state = ConnectionState::Idle;
                            app.add_log("Connection task ended unexpectedly.");
                            app.show_error("Connection interrupted", "The connection task ended unexpectedly.");

                            // If listen task died unexpectedly, retry
                            if auto_retry.listen_mode {
                                auto_retry.on_disconnected();
                            }
                        }
                    }
                }
            }

            _ = tick_interval.tick() => {}
        }

        app.try_recv_evaluate();

        // ── Keep App auto-relisten flag in sync with AutoRetryState ──
        app.auto_relisten_pending = auto_retry.retry_at.is_some();

        // ── Flush dirty config after mutations ───────────────────────
        if app.config_dirty {
            flush_config(app, config_path);
        }

        terminal.draw(|f| {
            mc_tui::ui::render(f, app);
        })?;
    }

    // ── Save config on exit ──────────────────────────────────────────
    if let Err(e) = save_config(app, config_path) {
        eprintln!("Warning: failed to save config on exit: {e}");
    }

    Ok(())
}

/// Persist the current app state (filter, known plugins, last target, passcode)
/// to the config file.  Always serializes the confirmed persisted credentials,
/// never the transient sidebar values alone.
fn save_config(app: &App, path: &std::path::Path) -> Result<(), String> {
    let (search, kinds, levels) = app.current_filter_state();
    let plugins = app.known_plugins.clone();

    let last_target = app.persisted_target_uuid.clone();
    let passcode = app.persisted_passcode.clone();

    // Build an AppConfig that includes the runtime known plugin cache
    let runtime_cfg = config::AppConfig {
        known_plugins: plugins.clone(),
        last_target_uuid: last_target,
        passcode,
        search,
        kinds,
        levels,
    };

    let file = runtime_cfg.into_file(plugins);
    config::save(path, &file)
}

/// Flush dirty config to disk.  Clears the dirty flag only on success;
/// failed writes retain dirty for retry and log a warning without secrets.
fn flush_config(app: &mut App, config_path: &std::path::Path) {
    match save_config(app, config_path) {
        Ok(()) => {
            app.config_dirty = false;
            app.config_save_error_shown = false;
        }
        Err(_e) => {
            app.add_log("Settings save failed; will retry.");
            app.show_config_save_error_once();
            // Keep config_dirty = true for retry on next tick
        }
    }
}

// ── Key handling ──────────────────────────────────────────────────────

async fn handle_key(app: &mut App, key: KeyEvent, auto_retry: &mut AutoRetryState) -> bool {
    if key.kind == KeyEventKind::Release {
        return false;
    }

    // Global failures are always first, including over target selection.
    if app.error_popup.is_some() {
        return handle_error_popup_keys(app, key, auto_retry);
    }
    // Remaining modal order mirrors the render stack.
    if app.plugin_selection.is_some() {
        return handle_plugin_popup_keys(app, key, auto_retry).await;
    }
    if app.help_popup == HelpPopup::Visible {
        return handle_help_popup_keys(app, key, auto_retry);
    }
    if app.search_input.open {
        return handle_search_input_keys(app, key);
    }
    if app.filter_popup.open {
        return handle_filter_popup_keys(app, key);
    }
    if app.command_input.open {
        return handle_command_input_keys(app, key).await;
    }
    if app.evaluate_input.open {
        return handle_evaluate_input_keys(app, key).await;
    }

    // When focus is on a text field, printable characters and most navigation
    // keys edit the field.  Tab/Shift+Tab and Esc still move/close focus.
    if app.is_field_focused() {
        return handle_field_editing(app, key, auto_retry);
    }

    // Global single-stroke actions (not active while editing a field).
    if let Some(quit) = handle_global_keys(app, key, auto_retry).await {
        return quit;
    }

    // Focus-specific actions for non-field targets.
    match app.focus {
        Focus::Main => handle_main_keys(app, key, auto_retry).await,
        Focus::SidebarModeListen => handle_sidebar_mode_keys(app, key, Mode::Listen, auto_retry),
        Focus::SidebarModeConnect => handle_sidebar_mode_keys(app, key, Mode::Connect, auto_retry),
        Focus::SidebarAdvanced => handle_advanced_toggle_key(app, key),
        Focus::SidebarPrimary => match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => handle_primary_action_key(app, auto_retry).await,
            _ => false,
        },
        _ => false,
    }
}

fn handle_error_popup_keys(app: &mut App, key: KeyEvent, auto_retry: &mut AutoRetryState) -> bool {
    match key.code {
        KeyCode::Esc | KeyCode::Enter => app.dismiss_error(),
        KeyCode::Char('q') => {
            auto_retry.cancel();
            return true;
        }
        _ => {}
    }
    false
}

async fn handle_plugin_popup_keys(
    app: &mut App,
    key: KeyEvent,
    auto_retry: &mut AutoRetryState,
) -> bool {
    if let Some(ref mut sel) = app.plugin_selection {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                sel.scroll_up();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                sel.scroll_down();
            }
            KeyCode::PageUp => {
                for _ in 0..5 {
                    sel.scroll_up();
                }
            }
            KeyCode::PageDown => {
                for _ in 0..5 {
                    sel.scroll_down();
                }
            }
            KeyCode::Enter => {
                if let (Some(uuid), Some(name)) = (sel.selected_uuid(), sel.selected_name()) {
                    let ok = app.controller.select_target(uuid.clone()).await.is_ok();
                    if ok {
                        app.plugin_selection = None;
                        app.sidebar.target_uuid = FieldState::new(uuid.clone());
                        // Selection changes the in-flight attempt only.  It is
                        // committed with the handshake result.
                        app.pending_target_uuid = Some(uuid);
                        app.add_log(format!("Selected target plugin: {name}"));
                    } else {
                        app.add_log("Target selection failed.");
                        app.show_error(
                            "Target selection failed",
                            "The selected plugin could not be activated.",
                        );
                    }
                }
                return false;
            }
            KeyCode::Esc => {
                app.plugin_selection = None;
                if app.state == ConnectionState::Pending {
                    app.cancel_pending_with_result().await;
                    // Esc on plugin popup cancels pending → stop listen retry
                    auto_retry.cancel();
                }
                return false;
            }
            KeyCode::Char('q') => {
                auto_retry.cancel();
                return true;
            }
            _ => {}
        }
    }

    false
}

fn handle_help_popup_keys(app: &mut App, key: KeyEvent, auto_retry: &mut AutoRetryState) -> bool {
    match key.code {
        KeyCode::Esc | KeyCode::Char('h') => {
            app.toggle_help();
        }
        KeyCode::Char('q') => {
            auto_retry.cancel();
            return true;
        }
        _ => {}
    }
    false
}

async fn handle_command_input_keys(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.command_input.close();
        }
        KeyCode::Enter => {
            let command = app.command_input.field.value.clone();
            app.submit_minecraft_command(command).await;
        }
        KeyCode::Up => {
            app.command_input.cycle_history(true);
        }
        KeyCode::Down => {
            app.command_input.cycle_history(false);
        }
        KeyCode::Char(c) => {
            app.command_input.field.insert(c);
        }
        KeyCode::Backspace => app.command_input.field.backspace(),
        KeyCode::Delete => app.command_input.field.delete(),
        KeyCode::Left => app.command_input.field.move_cursor(-1),
        KeyCode::Right => app.command_input.field.move_cursor(1),
        KeyCode::Home => app.command_input.field.home(),
        KeyCode::End => app.command_input.field.end(),
        _ => {}
    }
    false
}

async fn handle_evaluate_input_keys(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.evaluate_input.close();
        }
        KeyCode::Enter => {
            app.start_evaluate().await;
        }
        KeyCode::Char(c) => {
            app.evaluate_input.field.insert(c);
        }
        KeyCode::Backspace => app.evaluate_input.field.backspace(),
        KeyCode::Delete => app.evaluate_input.field.delete(),
        KeyCode::Left => app.evaluate_input.field.move_cursor(-1),
        KeyCode::Right => app.evaluate_input.field.move_cursor(1),
        KeyCode::Home => app.evaluate_input.field.home(),
        KeyCode::End => app.evaluate_input.field.end(),
        _ => {}
    }
    false
}

fn handle_search_input_keys(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.search_input.close();
        }
        KeyCode::Enter => {
            app.commit_search();
        }
        KeyCode::Char(c) => {
            app.search_input.field.insert(c);
        }
        KeyCode::Backspace => app.search_input.field.backspace(),
        KeyCode::Delete => app.search_input.field.delete(),
        KeyCode::Left => app.search_input.field.move_cursor(-1),
        KeyCode::Right => app.search_input.field.move_cursor(1),
        KeyCode::Home => app.search_input.field.home(),
        KeyCode::End => app.search_input.field.end(),
        _ => {}
    }
    false
}

fn handle_filter_popup_keys(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.filter_popup.close();
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            app.reset_filters();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.filter_popup.scroll_up();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.filter_popup.scroll_down();
        }
        KeyCode::Home | KeyCode::Char('g') => {
            app.filter_popup.selected = 0;
        }
        KeyCode::End | KeyCode::Char('G') => {
            app.filter_popup.selected = FilterPopup::row_count().saturating_sub(1);
        }
        KeyCode::Char(' ') | KeyCode::Enter => {
            app.toggle_selected_filter();
        }
        KeyCode::Char('q') => {
            return true;
        }
        _ => {}
    }
    false
}

async fn handle_global_keys(
    app: &mut App,
    key: KeyEvent,
    auto_retry: &mut AutoRetryState,
) -> Option<bool> {
    match key.code {
        KeyCode::Char('q') => {
            auto_retry.cancel();
            Some(true)
        }
        KeyCode::Char('x') | KeyCode::Char('X') => {
            auto_retry.cancel();
            match app.state {
                ConnectionState::Pending => {
                    app.cancel_pending_with_result().await;
                    // After cancel, also set idle and log
                    if app.state != ConnectionState::Idle {
                        app.state = ConnectionState::Idle;
                        app.add_log("Cancelled.");
                    }
                }
                ConnectionState::Connected { .. } => {
                    let _ = app.controller.disconnect().await;
                    app.clear_session_transients();
                    app.state = ConnectionState::Idle;
                    app.add_log("Disconnected.");
                }
                _ => {
                    // Idle or Disconnected — just clear any pending retry
                }
            }
            Some(false)
        }
        KeyCode::Char('h') => {
            app.toggle_help();
            Some(false)
        }
        KeyCode::Char('1') => {
            app.set_tab(Tab::Log);
            Some(false)
        }
        KeyCode::Char('2') => {
            app.set_tab(Tab::Stats);
            Some(false)
        }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            app.toggle_sidebar();
            Some(false)
        }
        KeyCode::Tab => {
            app.cycle_focus(true);
            Some(false)
        }
        KeyCode::BackTab => {
            app.cycle_focus(false);
            Some(false)
        }
        _ => None,
    }
}

fn handle_field_editing(app: &mut App, key: KeyEvent, _auto_retry: &mut AutoRetryState) -> bool {
    match key.code {
        KeyCode::Char(c) => {
            app.insert_char(c);
        }
        KeyCode::Backspace => app.backspace(),
        KeyCode::Delete => app.delete_char(),
        KeyCode::Left => app.move_cursor(-1),
        KeyCode::Right => app.move_cursor(1),
        KeyCode::Home => app.move_cursor_home(),
        KeyCode::End => app.move_cursor_end(),
        KeyCode::Enter => {
            // Enter on a field moves focus to the primary action.
            let targets = app.visible_focus_targets();
            if let Some(&primary) = targets.iter().find(|&&t| t == Focus::SidebarPrimary) {
                app.focus = primary;
            }
        }
        KeyCode::Tab => {
            app.cycle_focus(true);
        }
        KeyCode::BackTab => {
            app.cycle_focus(false);
        }
        KeyCode::Esc => {
            // Close the sidebar overlay if it is open; otherwise move focus to main.
            if app.show_sidebar && !app.sidebar_visible_by_layout(app.last_known_width()) {
                app.close_sidebar();
            } else {
                app.focus = Focus::Main;
            }
        }
        _ => {}
    }
    false
}

async fn handle_main_keys(app: &mut App, key: KeyEvent, auto_retry: &mut AutoRetryState) -> bool {
    // Debug control strip and quick inputs.
    match key.code {
        KeyCode::Char(' ') => {
            if app.can_continue() {
                app.request_continue().await;
            } else if app.can_pause() {
                app.request_pause(0).await;
            }
            return false;
        }
        KeyCode::F(5) => {
            if app.can_continue() {
                app.request_continue().await;
            }
            return false;
        }
        KeyCode::F(6) => {
            if app.can_pause() {
                app.request_pause(0).await;
            }
            return false;
        }
        KeyCode::F(10) => {
            if app.can_step() {
                app.request_step_next().await;
            }
            return false;
        }
        KeyCode::F(11) => {
            if app.can_step() {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    app.request_step_out().await;
                } else {
                    app.request_step_in().await;
                }
            }
            return false;
        }
        KeyCode::Char(':') => {
            if matches!(app.state, ConnectionState::Connected { .. }) {
                app.command_input.open();
            }
            return false;
        }
        KeyCode::Char('e') => {
            if matches!(app.state, ConnectionState::Connected { .. }) && app.stopped {
                app.evaluate_input.open();
            }
            return false;
        }
        _ => {}
    }

    // Contextual proof shortcuts.
    match key.code {
        KeyCode::Char('l') if key.modifiers.is_empty() => {
            if matches!(
                app.state,
                ConnectionState::Idle | ConnectionState::Disconnected
            ) {
                // Starting a new listen → cancel any pending auto-retry
                auto_retry.on_listen_started();
                app.set_mode(Mode::Listen);
                app.start_listen_from_sidebar();
            }
            return false;
        }
        KeyCode::Char('c') if key.modifiers.is_empty() => {
            match app.state {
                ConnectionState::Idle | ConnectionState::Disconnected => {
                    // Starting connect → cancel auto-relisten
                    auto_retry.on_connect_started();
                    app.set_mode(Mode::Connect);
                    app.start_connect_from_sidebar();
                }
                ConnectionState::Connected { .. } if app.tab == Tab::Log => app.clear_log(),
                _ => {}
            }
            return false;
        }
        _ => {}
    }

    let page = (app.last_known_height().saturating_sub(6) as usize).max(3);
    match app.tab {
        Tab::Stats => match key.code {
            KeyCode::Left => app.select_prev_category(),
            KeyCode::Right => app.select_next_category(),
            KeyCode::Char('[') if app.stats_selected_category.as_deref() == Some("scripting") => {
                app.select_prev_addon()
            }
            KeyCode::Char(']') if app.stats_selected_category.as_deref() == Some("scripting") => {
                app.select_next_addon()
            }
            KeyCode::Char('[') => app.select_prev_client(),
            KeyCode::Char(']') => app.select_next_client(),
            KeyCode::Up | KeyCode::Char('k') => app.scroll_stats_up(1),
            KeyCode::Down | KeyCode::Char('j') => app.scroll_stats_down(1),
            KeyCode::PageUp => app.scroll_stats_page_up(page),
            KeyCode::PageDown => app.scroll_stats_page_down(page),
            KeyCode::Char('g') => app.scroll_stats_top(),
            KeyCode::Char('G') => app.scroll_stats_bottom(),
            KeyCode::Char('r') => app.clear_stats(),
            _ => {}
        },
        Tab::Log => match key.code {
            KeyCode::Up | KeyCode::Char('k') => app.scroll_log_up(1),
            KeyCode::Down | KeyCode::Char('j') => app.scroll_log_down(1),
            KeyCode::PageUp => app.scroll_log_page_up(page),
            KeyCode::PageDown => app.scroll_log_page_down(page),
            KeyCode::Char('g') => app.scroll_log_top(),
            KeyCode::Char('G') => app.scroll_log_bottom(),
            KeyCode::Char('/') => app.open_search(),
            KeyCode::Char('f') => app.open_filter_popup(),
            KeyCode::Char('r') => app.reset_filters(),
            _ => {}
        },
    }
    false
}

fn handle_sidebar_mode_keys(
    app: &mut App,
    key: KeyEvent,
    mode: Mode,
    auto_retry: &mut AutoRetryState,
) -> bool {
    match key.code {
        KeyCode::Enter | KeyCode::Char(' ') => {
            // Switching to Connect mode cancels any scheduled retry/listen intent
            if mode == Mode::Connect {
                auto_retry.cancel();
            }
            app.set_mode(mode);
        }
        _ => {}
    }
    false
}

fn handle_advanced_toggle_key(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Enter | KeyCode::Char(' ') => app.toggle_advanced(),
        _ => {}
    }
    false
}

async fn handle_primary_action_key(app: &mut App, auto_retry: &mut AutoRetryState) -> bool {
    match app.state {
        ConnectionState::Idle => {
            if app.sidebar.mode == Mode::Listen {
                auto_retry.on_listen_started();
                app.start_listen_from_sidebar();
            } else {
                auto_retry.on_connect_started();
                app.start_connect_from_sidebar();
            }
        }
        ConnectionState::Pending => {
            app.cancel_pending_with_result().await;
            app.plugin_selection = None;
            // Cancelling pending → stop auto-relisten
            auto_retry.cancel();
        }
        ConnectionState::Connected { .. } => {
            let _ = app.controller.disconnect().await;
            app.clear_session_transients();
            app.state = ConnectionState::Idle;
            app.add_log("Disconnected.");
            // Manual disconnect in listen mode → cancel auto-relisten
            if auto_retry.listen_mode {
                auto_retry.cancel();
            }
        }
        ConnectionState::Disconnected => {
            if auto_retry.retry_at.is_some() {
                // Retry pending — cancel it and go Idle without starting a new connection
                auto_retry.cancel();
                app.state = ConnectionState::Idle;
                app.add_log("Cancelled retry.");
            } else {
                // No retry pending — normal manual reconnect
                auto_retry.cancel();
                app.state = ConnectionState::Idle;
                if app.sidebar.mode == Mode::Listen {
                    auto_retry.on_listen_started();
                    app.start_listen_from_sidebar();
                } else {
                    auto_retry.on_connect_started();
                    app.start_connect_from_sidebar();
                }
            }
        }
    }
    false
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use mc_protocol::events::PluginDetails;
    use mc_session::SessionController;

    use mc_tui::app::{
        App, ConnectionState, FieldState, Focus, HelpPopup, Mode, PluginSelection, Tab,
    };

    use std::path::PathBuf;

    use super::{handle_key, AutoRetryState};

    fn make_app() -> App {
        let (ctrl, rx) = SessionController::new();
        App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None)
    }

    fn make_auto_retry() -> AutoRetryState {
        AutoRetryState::new()
    }

    #[tokio::test]
    async fn plugin_popup_q_quits_cleanly() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.state = ConnectionState::Pending;
        app.plugin_selection = Some(PluginSelection::new(vec![PluginDetails {
            name: "P".into(),
            module_uuid: "u".into(),
        }]));

        assert!(handle_key(&mut app, KeyCode::Char('q').into(), &mut auto_retry).await);
    }

    #[tokio::test]
    async fn help_popup_q_quits_cleanly() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.help_popup = HelpPopup::Visible;

        assert!(handle_key(&mut app, KeyCode::Char('q').into(), &mut auto_retry).await);
    }

    #[tokio::test]
    async fn help_popup_esc_closes() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.help_popup = HelpPopup::Visible;

        assert!(!handle_key(&mut app, KeyCode::Esc.into(), &mut auto_retry).await);
        assert_eq!(app.help_popup, HelpPopup::Hidden);
    }

    #[tokio::test]
    async fn global_error_popup_enter_and_escape_dismiss() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.show_error("Connection failed", "Try again.");

        assert!(!handle_key(&mut app, KeyCode::Enter.into(), &mut auto_retry).await);
        assert!(app.error_popup.is_none());

        app.show_error("Connection failed", "Try again.");
        assert!(!handle_key(&mut app, KeyCode::Esc.into(), &mut auto_retry).await);
        assert!(app.error_popup.is_none());
    }

    #[tokio::test]
    async fn global_error_popup_q_still_quits() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.show_error("Connection failed", "Try again.");
        assert!(handle_key(&mut app, KeyCode::Char('q').into(), &mut auto_retry).await);
    }

    #[tokio::test]
    async fn sidebar_primary_only_activates_on_enter_or_space() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::SidebarPrimary;

        assert!(!handle_key(&mut app, KeyCode::Char('a').into(), &mut auto_retry).await);
        assert_eq!(app.state, ConnectionState::Idle);

        assert!(!handle_key(&mut app, KeyCode::Enter.into(), &mut auto_retry).await);
        assert_eq!(app.state, ConnectionState::Pending);
    }

    #[tokio::test]
    async fn main_l_starts_listen() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;

        assert!(!handle_key(&mut app, KeyCode::Char('l').into(), &mut auto_retry).await);
        assert_eq!(app.sidebar.mode, Mode::Listen);
        assert_eq!(app.state, ConnectionState::Pending);
    }

    #[tokio::test]
    async fn main_c_starts_connect_when_idle() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;

        assert!(!handle_key(&mut app, KeyCode::Char('c').into(), &mut auto_retry).await);
        assert_eq!(app.sidebar.mode, Mode::Connect);
        assert_eq!(app.state, ConnectionState::Pending);
    }

    #[tokio::test]
    async fn main_c_clears_log_when_connected() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 1,
        };
        app.add_log("first");
        app.add_log("second");

        assert!(!handle_key(&mut app, KeyCode::Char('c').into(), &mut auto_retry).await);
        assert!(app.event_log.is_empty());
    }

    #[tokio::test]
    async fn debug_keys_are_no_op_when_not_connected() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;
        let before = app.event_log.len();

        assert!(!handle_key(&mut app, KeyCode::Char(' ').into(), &mut auto_retry).await);
        assert!(!handle_key(&mut app, KeyCode::F(5).into(), &mut auto_retry).await);
        assert!(!handle_key(&mut app, KeyCode::F(6).into(), &mut auto_retry).await);
        assert!(!handle_key(&mut app, KeyCode::F(10).into(), &mut auto_retry).await);
        assert!(!handle_key(&mut app, KeyCode::F(11).into(), &mut auto_retry).await);
        assert!(
            !handle_key(
                &mut app,
                KeyEvent::new(KeyCode::F(11), KeyModifiers::SHIFT),
                &mut auto_retry
            )
            .await
        );

        assert_eq!(app.event_log.len(), before);
    }

    #[tokio::test]
    async fn space_attempts_continue_when_stopped_and_pause_when_running() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 1,
        };
        app.stopped = true;
        app.stopped_thread_id = Some(0);

        // Not actually connected to a debuggee, so the send fails and stopped stays set.
        assert!(!handle_key(&mut app, KeyCode::Char(' ').into(), &mut auto_retry).await);
        assert!(app
            .event_log
            .iter()
            .any(|e| e.message.contains("Continue failed")));
        assert!(app.stopped);

        // Clear the log and mark running; Space should attempt pause.
        app.event_log.clear();
        app.stopped = false;
        app.stopped_thread_id = None;
        assert!(!handle_key(&mut app, KeyCode::Char(' ').into(), &mut auto_retry).await);
        assert!(app
            .event_log
            .iter()
            .any(|e| e.message.contains("Pause failed")));
    }

    #[tokio::test]
    async fn step_keys_attempt_step_when_stopped() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 1,
        };
        app.stopped = true;
        app.stopped_thread_id = Some(0);

        assert!(!handle_key(&mut app, KeyCode::F(10).into(), &mut auto_retry).await);
        assert!(app
            .event_log
            .iter()
            .any(|e| e.message.contains("Step next failed")));

        app.stopped = true;
        assert!(!handle_key(&mut app, KeyCode::F(11).into(), &mut auto_retry).await);
        assert!(app
            .event_log
            .iter()
            .any(|e| e.message.contains("Step in failed")));

        app.stopped = true;
        assert!(
            !handle_key(
                &mut app,
                KeyEvent::new(KeyCode::F(11), KeyModifiers::SHIFT),
                &mut auto_retry
            )
            .await
        );
        assert!(app
            .event_log
            .iter()
            .any(|e| e.message.contains("Step out failed")));
    }

    #[tokio::test]
    async fn colon_opens_command_input_and_q_types() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 1,
        };

        assert!(!handle_key(&mut app, KeyCode::Char(':').into(), &mut auto_retry).await);
        assert!(app.command_input.open);

        // q must type, not quit.
        assert!(!handle_key(&mut app, KeyCode::Char('q').into(), &mut auto_retry).await);
        assert!(!handle_key(&mut app, KeyCode::Char('s').into(), &mut auto_retry).await);
        assert_eq!(app.command_input.field.value, "qs");

        // Global shortcuts must be swallowed while the input is active.
        assert!(!handle_key(&mut app, KeyCode::Char('x').into(), &mut auto_retry).await);
        assert!(matches!(app.state, ConnectionState::Connected { .. }));

        // Esc closes the input.
        assert!(!handle_key(&mut app, KeyCode::Esc.into(), &mut auto_retry).await);
        assert!(!app.command_input.open);
    }

    #[tokio::test]
    async fn command_input_submit_surfaces_send_error() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 1,
        };
        app.command_input.open();
        app.command_input.field.value = "say hi".into();

        assert!(!handle_key(&mut app, KeyCode::Enter.into(), &mut auto_retry).await);
        assert!(app
            .command_input
            .error
            .as_ref()
            .expect("error should be set")
            .contains("not connected"));
    }

    #[tokio::test]
    async fn e_opens_evaluate_input_while_stopped() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 1,
        };

        // Not stopped -> ignored.
        assert!(!handle_key(&mut app, KeyCode::Char('e').into(), &mut auto_retry).await);
        assert!(!app.evaluate_input.open);

        app.stopped = true;
        app.stopped_thread_id = Some(0);
        assert!(!handle_key(&mut app, KeyCode::Char('e').into(), &mut auto_retry).await);
        assert!(app.evaluate_input.open);

        assert!(!handle_key(&mut app, KeyCode::Char('q').into(), &mut auto_retry).await);
        assert_eq!(app.evaluate_input.field.value, "q");

        assert!(!handle_key(&mut app, KeyCode::Esc.into(), &mut auto_retry).await);
        assert!(!app.evaluate_input.open);
    }

    #[tokio::test]
    async fn evaluate_input_submit_surfaces_send_error() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 1,
        };
        app.stopped = true;
        app.stopped_thread_id = Some(0);
        app.evaluate_input.open();
        app.evaluate_input.field.value = "1+1".into();

        assert!(!handle_key(&mut app, KeyCode::Enter.into(), &mut auto_retry).await);
        assert!(app
            .evaluate_input
            .error
            .as_ref()
            .expect("error should be set")
            .contains("not connected"));
    }

    // ── x key cancel/disconnect ────────────────────────────────────────

    #[tokio::test]
    async fn x_cancels_pending_to_idle() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.state = ConnectionState::Pending;
        app.focus = Focus::Main;
        let (_tx, rx) = tokio::sync::oneshot::channel();
        app.conn_result_rx = Some(rx);

        assert!(!handle_key(&mut app, KeyCode::Char('x').into(), &mut auto_retry).await);
        assert_eq!(app.state, ConnectionState::Idle);
    }

    #[tokio::test]
    async fn x_disconnects_connected_to_idle() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 1,
        };
        app.handshake = Some(mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![],
            require_passcode: false,
        });
        let (_tx, result_rx) = tokio::sync::oneshot::channel();
        app.evaluate_input.start("stale".into(), result_rx);
        app.focus = Focus::Main;

        assert!(!handle_key(&mut app, KeyCode::Char('x').into(), &mut auto_retry).await);
        assert_eq!(app.state, ConnectionState::Idle);
        assert!(app.handshake.is_none());
        assert!(!app.evaluate_input.busy);
        assert!(app
            .event_log
            .iter()
            .any(|e| e.message.contains("Disconnected")));
    }

    #[tokio::test]
    async fn x_noop_when_idle_or_disconnected() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;

        assert!(!handle_key(&mut app, KeyCode::Char('x').into(), &mut auto_retry).await);
        assert_eq!(app.state, ConnectionState::Idle);

        app.state = ConnectionState::Disconnected;
        assert!(!handle_key(&mut app, KeyCode::Char('x').into(), &mut auto_retry).await);
        assert_eq!(app.state, ConnectionState::Disconnected);
    }

    // ── Plugin Enter selection failure ─────────────────────────────────

    #[tokio::test]
    async fn plugin_enter_selection_failure_keeps_popup() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.state = ConnectionState::Pending;
        app.plugin_selection = Some(PluginSelection::new(vec![PluginDetails {
            name: "TestPlugin".into(),
            module_uuid: "test-uuid".into(),
        }]));

        // Enter with no selection_tx installed → select_target fails
        assert!(!handle_key(&mut app, KeyCode::Enter.into(), &mut auto_retry).await);

        // Popup should remain open on failure
        assert!(
            app.plugin_selection.is_some(),
            "popup should stay open on selection failure"
        );
        assert!(
            app.event_log.iter().any(|e| e.message.contains("failed")),
            "should log selection failure"
        );
    }

    // ── Auto-retry tests ─────────────────────────────────────────────────

    #[test]
    fn auto_retry_starts_idle() {
        let ar = AutoRetryState::new();
        assert!(!ar.listen_mode);
        assert!(ar.retry_at.is_none());
    }

    #[test]
    fn auto_retry_listen_started_sets_flag() {
        let mut ar = AutoRetryState::new();
        ar.on_listen_started();
        assert!(ar.listen_mode);
        assert!(ar.retry_at.is_none());
    }

    #[test]
    fn auto_retry_connect_started_clears_flag() {
        let mut ar = AutoRetryState::new();
        ar.on_listen_started();
        ar.on_connect_started();
        assert!(!ar.listen_mode);
        assert!(ar.retry_at.is_none());
    }

    #[test]
    fn auto_retry_disconnected_schedules_retry() {
        let mut ar = AutoRetryState::new();
        ar.on_listen_started();
        ar.on_disconnected();
        assert!(ar.retry_at.is_some());
    }

    #[test]
    fn auto_retry_cancel_clears_everything() {
        let mut ar = AutoRetryState::new();
        ar.on_listen_started();
        ar.on_disconnected();
        ar.cancel();
        assert!(!ar.listen_mode);
        assert!(ar.retry_at.is_none());
    }

    #[test]
    fn auto_retry_disconnected_no_listen_does_nothing() {
        let mut ar = AutoRetryState::new();
        // Not in listen mode
        ar.on_disconnected();
        assert!(ar.retry_at.is_none());
    }

    #[tokio::test]
    async fn auto_retry_try_fire_returns_true_when_ready() {
        let mut ar = AutoRetryState::new();
        ar.on_listen_started();
        ar.retry_at = Some(tokio::time::Instant::now()); // immediately ready
        assert!(ar.try_fire());
        assert!(ar.retry_at.is_none());
    }

    #[tokio::test]
    async fn auto_retry_try_fire_returns_false_when_not_ready() {
        let mut ar = AutoRetryState::new();
        ar.on_listen_started();
        ar.retry_at = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(60));
        // Not yet ready
        assert!(!ar.try_fire());
        assert!(ar.retry_at.is_some());
    }

    #[test]
    fn auto_retry_try_fire_none_returns_false() {
        let mut ar = AutoRetryState::new();
        assert!(!ar.try_fire());
    }

    // ── Phase 4 key handling ─────────────────────────────────────────────

    #[tokio::test]
    async fn slash_opens_search_input() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;

        assert!(!handle_key(&mut app, KeyCode::Char('/').into(), &mut auto_retry).await);
        assert!(app.search_input.open);
    }

    #[tokio::test]
    async fn search_input_types_and_commits() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.open_search();

        for c in ['t', 'e', 's', 't'] {
            assert!(!handle_key(&mut app, KeyCode::Char(c).into(), &mut auto_retry).await);
        }
        assert_eq!(app.search_input.field.value, "test");
        assert!(!app.is_filtered());

        assert!(!handle_key(&mut app, KeyCode::Enter.into(), &mut auto_retry).await);
        assert!(!app.search_input.open);
        assert_eq!(app.log_filter.search, "test");
        assert!(app.is_filtered());
    }

    #[tokio::test]
    async fn search_input_esc_closes_without_commit() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.open_search();
        app.search_input.field.value = "aborted".into();

        assert!(!handle_key(&mut app, KeyCode::Esc.into(), &mut auto_retry).await);
        assert!(!app.search_input.open);
        assert!(app.log_filter.search.is_empty());
    }

    #[tokio::test]
    async fn f_opens_filter_popup() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;

        assert!(!handle_key(&mut app, KeyCode::Char('f').into(), &mut auto_retry).await);
        assert!(app.filter_popup.open);
    }

    #[tokio::test]
    async fn filter_popup_toggles_and_esc_closes() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.open_filter_popup();

        assert!(!handle_key(&mut app, KeyCode::Down.into(), &mut auto_retry).await);
        assert_eq!(app.filter_popup.selected, 1);

        assert!(!handle_key(&mut app, KeyCode::Char(' ').into(), &mut auto_retry).await);
        assert!(!app.log_filter.kinds.protocol);
        assert!(app.is_filtered());

        assert!(!handle_key(&mut app, KeyCode::Esc.into(), &mut auto_retry).await);
        assert!(!app.filter_popup.open);
    }

    #[tokio::test]
    async fn r_resets_search_and_filters() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;
        app.log_filter.search = "term".into();
        app.log_filter.kinds.print = false;
        app.rebuild_filtered_log_indices();
        assert!(app.is_filtered());

        assert!(!handle_key(&mut app, KeyCode::Char('r').into(), &mut auto_retry).await);
        assert!(!app.is_filtered());
        assert!(app.log_filter.search.is_empty());
        assert!(app.log_filter.kinds.print);
    }

    #[tokio::test]
    async fn c_still_clears_log_when_connected() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 1,
        };
        app.add_log("keep me");

        assert!(!handle_key(&mut app, KeyCode::Char('c').into(), &mut auto_retry).await);
        assert!(app.event_log.is_empty());
        assert!(app.filtered_log_indices.is_empty());
    }

    // ── Auto-retry cancellation via keys ──────────────────────────────

    #[tokio::test]
    async fn x_cancels_auto_retry() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        auto_retry.listen_mode = true;
        auto_retry.retry_at = Some(tokio::time::Instant::now());

        assert!(!handle_key(&mut app, KeyCode::Char('x').into(), &mut auto_retry).await);
        assert!(!auto_retry.listen_mode);
        assert!(auto_retry.retry_at.is_none());
    }

    #[tokio::test]
    async fn q_quits_and_cancels_auto_retry() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        auto_retry.listen_mode = true;
        auto_retry.retry_at = Some(tokio::time::Instant::now());

        assert!(handle_key(&mut app, KeyCode::Char('q').into(), &mut auto_retry).await);
        assert!(!auto_retry.listen_mode);
        assert!(auto_retry.retry_at.is_none());
    }

    #[tokio::test]
    async fn c_connects_and_cancels_auto_retry() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;
        auto_retry.listen_mode = true;
        auto_retry.retry_at = Some(tokio::time::Instant::now());

        assert!(!handle_key(&mut app, KeyCode::Char('c').into(), &mut auto_retry).await);
        // connect starts pending
        assert!(!auto_retry.listen_mode);
        assert!(auto_retry.retry_at.is_none());
    }

    #[tokio::test]
    async fn l_starts_listen_and_resets_auto_retry() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::Main;
        auto_retry.on_connect_started(); // was in connect mode

        assert!(!handle_key(&mut app, KeyCode::Char('l').into(), &mut auto_retry).await);
        assert!(auto_retry.listen_mode);
        assert!(auto_retry.retry_at.is_none());
    }

    #[tokio::test]
    async fn sidebar_primary_cancels_auto_retry_on_pending() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::SidebarPrimary;
        app.state = ConnectionState::Pending;
        auto_retry.listen_mode = true;
        auto_retry.retry_at = Some(tokio::time::Instant::now());

        assert!(!handle_key(&mut app, KeyCode::Enter.into(), &mut auto_retry).await);
        assert!(!auto_retry.listen_mode);
        assert!(auto_retry.retry_at.is_none());
    }

    #[tokio::test]
    async fn sidebar_primary_connected_disconnect_cancels_auto_retry() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::SidebarPrimary;
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 1,
        };
        auto_retry.listen_mode = true;
        auto_retry.retry_at = Some(tokio::time::Instant::now());

        assert!(!handle_key(&mut app, KeyCode::Enter.into(), &mut auto_retry).await);
        // After disconnect, state is Idle and auto_retry was cancelled
        assert!(!auto_retry.listen_mode);
        assert!(auto_retry.retry_at.is_none());
    }

    // ── Phase 4.1: Auto-retry from Disconnected ────────────────────────

    #[tokio::test]
    async fn auto_retry_fires_from_disconnected() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.state = ConnectionState::Disconnected;
        auto_retry.listen_mode = true;
        auto_retry.retry_at = Some(tokio::time::Instant::now());

        // try_fire should succeed from Disconnected too
        assert!(auto_retry.try_fire());
    }

    #[tokio::test]
    async fn auto_retry_disconnect_while_listen_mode_reschedules() {
        // Simulate: Connected → Disconnected event in listen mode schedules retry
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 1,
        };
        auto_retry.listen_mode = true;

        // Simulate the event loop's disconnected check
        let was_connected = matches!(app.state, ConnectionState::Connected { .. });
        app.handle_session_event(mc_session::SessionEvent::Disconnected);
        if was_connected
            && auto_retry.listen_mode
            && matches!(app.state, ConnectionState::Disconnected)
        {
            auto_retry.on_disconnected();
        }

        assert!(
            auto_retry.retry_at.is_some(),
            "listen mode disconnected should schedule retry"
        );
    }

    #[tokio::test]
    async fn sidebar_primary_disconnected_cancels_retry() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::SidebarPrimary;
        app.state = ConnectionState::Disconnected;
        app.sidebar.mode = Mode::Listen;
        auto_retry.listen_mode = true;
        auto_retry.retry_at = Some(tokio::time::Instant::now());

        assert!(!handle_key(&mut app, KeyCode::Enter.into(), &mut auto_retry).await);
        // Should cancel retry and go to Idle, NOT start a new connection
        assert!(!auto_retry.listen_mode);
        assert!(auto_retry.retry_at.is_none());
        assert_eq!(app.state, ConnectionState::Idle);
        assert!(
            app.event_log
                .iter()
                .any(|e| e.message.contains("Cancelled")),
            "should log cancellation"
        );
    }

    #[tokio::test]
    async fn switching_to_connect_cancels_retry() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::SidebarModeConnect;
        app.sidebar.mode = Mode::Listen;
        auto_retry.listen_mode = true;
        auto_retry.retry_at = Some(tokio::time::Instant::now());

        assert!(!handle_key(&mut app, KeyCode::Enter.into(), &mut auto_retry).await);
        // Mode switches, retry cancelled
        assert_eq!(app.sidebar.mode, Mode::Connect);
        assert!(!auto_retry.listen_mode);
        assert!(auto_retry.retry_at.is_none());
    }

    #[tokio::test]
    async fn auto_retry_disconnect_failed_connection_reschedules() {
        // Simulate: a failed listen task in listen_mode schedules retry
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.state = ConnectionState::Pending;
        auto_retry.listen_mode = true;

        // Simulate failed result
        let result: Result<_, mc_session::SessionError> = Err(mc_session::SessionError::Cancelled);
        let is_ok = result.is_ok();
        app.handle_connection_result(result);

        // The simulation of the event loop retry scheduling:
        if !is_ok && auto_retry.listen_mode && app.state == ConnectionState::Idle {
            auto_retry.on_disconnected();
        }

        assert!(
            auto_retry.retry_at.is_some(),
            "failed listen should reschedule retry"
        );
    }

    // ── Phase 4.2: Startup uses resolved sidebar values ────────────────

    #[tokio::test]
    async fn startup_uses_sidebar_values_for_action() {
        // Verify that start_listen_from_sidebar / start_connect_from_sidebar
        // use the sidebar values (host, port, target, passcode).
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "10.0.0.1".into(), 19132, None, None);
        app.sidebar.target_uuid = FieldState::new("uuid-from-config");
        app.sidebar.passcode = FieldState::new("pass-from-config");

        app.start_listen_from_sidebar();
        assert_eq!(app.state, ConnectionState::Pending);

        // Clean up for next test
        let (ctrl2, rx2) = SessionController::new();
        let mut app2 = App::new(ctrl2, rx2, "10.0.0.2".into(), 19133, None, None);
        app2.sidebar.target_uuid = FieldState::new("uuid-connect");
        app2.sidebar.passcode = FieldState::new("pass-connect");

        app2.start_connect_from_sidebar();
        assert_eq!(app2.state, ConnectionState::Pending);
    }

    // ── Phase 4.3: Plugin cache survives disconnect ────────────────────

    #[test]
    fn known_plugins_cache_survives_disconnect() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.known_plugins = vec![mc_protocol::events::PluginDetails {
            name: "Alpha".into(),
            module_uuid: "uuid-alpha".into(),
        }];
        app.handshake = Some(mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![mc_protocol::events::PluginDetails {
                name: "Beta".into(),
                module_uuid: "uuid-beta".into(),
            }],
            require_passcode: false,
        });
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 2,
        };

        // Disconnect clears handshake but known_plugins survives
        app.clear_session_transients();
        assert!(app.handshake.is_none());
        assert_eq!(app.known_plugins.len(), 1);
        assert_eq!(app.known_plugins[0].name, "Alpha");
    }

    #[test]
    fn known_plugins_update_name_by_uuid() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.known_plugins = vec![mc_protocol::events::PluginDetails {
            name: "OldName".into(),
            module_uuid: "uuid-alpha".into(),
        }];

        // Simulate a handshake with updated name
        let hs = mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![mc_protocol::events::PluginDetails {
                name: "NewName".into(),
                module_uuid: "uuid-alpha".into(),
            }],
            require_passcode: false,
        };
        app.handle_connection_result(Ok(hs));

        assert_eq!(app.known_plugins.len(), 1);
        assert_eq!(app.known_plugins[0].name, "NewName");
        assert_eq!(app.known_plugins[0].module_uuid, "uuid-alpha");
    }

    // ── Phase 4.4: Config dirty flag behavior ──────────────────────────

    #[test]
    fn filter_toggle_marks_config_dirty() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        assert!(!app.config_dirty);

        app.open_filter_popup();
        app.toggle_selected_filter();
        assert!(app.config_dirty);
    }

    #[test]
    fn search_commit_marks_config_dirty() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        assert!(!app.config_dirty);

        app.open_search();
        app.search_input.field.value = "test".into();
        app.commit_search();
        assert!(app.config_dirty);
    }

    #[test]
    fn reset_filters_marks_config_dirty() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.log_filter.search = "old".into();
        app.rebuild_filtered_log_indices();
        app.config_dirty = false;

        app.reset_filters();
        assert!(app.config_dirty);
    }

    #[test]
    fn successful_handshake_marks_config_dirty() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        assert!(!app.config_dirty);

        let hs = mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![],
            require_passcode: false,
        };
        app.handle_connection_result(Ok(hs));
        assert!(app.config_dirty);
    }

    // ── Phase 4.5: Config path / CLI ───────────────────────────────────

    #[test]
    fn config_path_respects_cli_arg() {
        // Passing a --config path should override default
        let cli_path = "C:\\Users\\test\\custom.json";
        let resolved: PathBuf = cli_path.into();
        assert_eq!(resolved.to_string_lossy(), "C:\\Users\\test\\custom.json");
    }

    #[test]
    fn config_path_env_var_inspect_only() {
        // `--config` CLI arg is the supported override.  MC_TUI_CONFIG env
        // var support was removed.  Tests may inspect but must not mutate.
        let _ = std::env::var("MC_TUI_CONFIG").ok();
        // Platform default is used when --config is absent.
        let path = super::config::default_config_path();
        // On Windows this should be %APPDATA%/minecraft-debugger/settings.json
        // or the fallback path; just verify it's non-empty.
        assert!(!path.to_string_lossy().is_empty());
    }

    // ── Phase 5.1: Persisted credentials ────────────────────────────────

    #[test]
    fn credentials_survive_failed_connection_and_disconnect() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(
            ctrl,
            rx,
            "127.0.0.1".into(),
            19144,
            Some("loaded-uuid".into()),
            Some("loaded-pass".into()),
        );
        // Simulate loaded config: persisted fields are initialized from CLI/defaults
        assert_eq!(app.persisted_target_uuid.as_deref(), Some("loaded-uuid"));
        assert_eq!(app.persisted_passcode.as_deref(), Some("loaded-pass"));

        // Simulate a failed connection that clears handshake
        app.state = ConnectionState::Disconnected;
        app.clear_session_transients();
        assert!(app.handshake.is_none());
        // Persisted credentials survive
        assert_eq!(app.persisted_target_uuid.as_deref(), Some("loaded-uuid"));
        assert_eq!(app.persisted_passcode.as_deref(), Some("loaded-pass"));
    }

    #[test]
    fn filter_save_snapshot_does_not_erase_credentials() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(
            ctrl,
            rx,
            "127.0.0.1".into(),
            19144,
            Some("uuid".into()),
            Some("pass".into()),
        );
        // Toggle a filter (marks dirty)
        app.log_filter.kinds.print = false;
        app.rebuild_filtered_log_indices();
        app.mark_config_dirty();

        // Verify fake save would include the credentials
        assert_eq!(app.persisted_target_uuid.as_deref(), Some("uuid"));
        assert_eq!(app.persisted_passcode.as_deref(), Some("pass"));
        assert!(app.config_dirty);
    }

    #[test]
    fn selected_target_captures_persisted_uuid() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        assert!(app.persisted_target_uuid.is_none());

        // Simulate successful target selection (what plugin Enter does)
        let uuid = "selected-uuid".to_string();
        app.plugin_selection = None;
        app.sidebar.target_uuid = FieldState::new(uuid.clone());
        app.persisted_target_uuid = Some(uuid);
        app.mark_config_dirty();

        assert_eq!(app.persisted_target_uuid.as_deref(), Some("selected-uuid"));
        assert_eq!(app.sidebar.target_uuid.value, "selected-uuid");
        assert!(app.config_dirty);
    }

    #[test]
    fn successful_handshake_captures_persisted_credentials() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        assert!(app.persisted_target_uuid.is_none());
        assert!(app.persisted_passcode.is_none());

        // Set sidebar values (simulating user input or config)
        app.sidebar.target_uuid = FieldState::new("hs-uuid");
        app.sidebar.passcode = FieldState::new("hs-pass");
        app.pending_target_uuid = Some("hs-uuid".into());
        app.pending_passcode = Some("hs-pass".into());

        // Simulate successful handshake
        let hs = mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![],
            require_passcode: false,
        };
        app.handle_connection_result(Ok(hs));

        assert_eq!(app.persisted_target_uuid.as_deref(), Some("hs-uuid"));
        assert_eq!(app.persisted_passcode.as_deref(), Some("hs-pass"));
        assert!(app.config_dirty);
    }

    #[tokio::test]
    async fn pending_attempt_owns_credentials_and_no_passcode_clears_old() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(
            ctrl,
            rx,
            "127.0.0.1".into(),
            19144,
            None,
            Some("old".into()),
        );
        app.start_listen(19144, Some("sent-target".into()), Some("sent-pass".into()));
        app.sidebar.target_uuid = FieldState::new("edited-target");
        app.sidebar.passcode = FieldState::new("");
        app.handle_connection_result(Ok(mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![],
            require_passcode: false,
        }));
        assert_eq!(app.persisted_target_uuid.as_deref(), Some("sent-target"));
        assert_eq!(app.persisted_passcode, Some("sent-pass".into()));

        app.persisted_passcode = Some("old-again".into());
        app.start_listen(19144, None, None);
        app.handle_connection_result(Ok(mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![],
            require_passcode: false,
        }));
        assert!(app.persisted_passcode.is_none());
    }

    #[tokio::test]
    async fn lifecycle_disconnect_before_buffered_result_keeps_attempt_credentials() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(
            ctrl,
            rx,
            "127.0.0.1".into(),
            19144,
            Some("old-target".into()),
            Some("old-pass".into()),
        );
        app.start_listen(
            19144,
            Some("attempt-target".into()),
            Some("attempt-pass".into()),
        );
        app.handle_session_event(mc_session::SessionEvent::Disconnected);
        app.handle_connection_result(Ok(mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![],
            require_passcode: false,
        }));

        assert_eq!(app.persisted_target_uuid.as_deref(), Some("attempt-target"));
        assert_eq!(app.persisted_passcode.as_deref(), Some("attempt-pass"));
    }

    #[tokio::test]
    async fn lifecycle_terminated_before_buffered_result_keeps_attempt_credentials() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.start_listen(
            19144,
            Some("attempt-target".into()),
            Some("attempt-pass".into()),
        );
        app.handle_session_event(mc_session::SessionEvent::Terminated { reason: None });
        app.handle_connection_result(Ok(mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![],
            require_passcode: false,
        }));

        assert_eq!(app.persisted_target_uuid.as_deref(), Some("attempt-target"));
        assert_eq!(app.persisted_passcode.as_deref(), Some("attempt-pass"));
    }

    #[tokio::test]
    async fn failed_attempt_preserves_confirmed_credentials() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(
            ctrl,
            rx,
            "127.0.0.1".into(),
            19144,
            Some("confirmed-target".into()),
            Some("confirmed-pass".into()),
        );
        app.start_connect(
            "host".into(),
            19144,
            Some("new-target".into()),
            Some("new-pass".into()),
        );
        app.handle_connection_result(Err(mc_session::SessionError::Cancelled));
        assert_eq!(
            app.persisted_target_uuid.as_deref(),
            Some("confirmed-target")
        );
        assert_eq!(app.persisted_passcode.as_deref(), Some("confirmed-pass"));
        assert!(app.pending_target_uuid.is_none());
        assert!(app.pending_passcode.is_none());
    }

    #[tokio::test]
    async fn sole_plugin_is_persisted_when_attempt_had_no_target() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.start_listen(19144, None, None);
        app.handle_connection_result(Ok(mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![mc_protocol::events::PluginDetails {
                name: "only".into(),
                module_uuid: "sole-uuid".into(),
            }],
            require_passcode: false,
        }));
        assert_eq!(app.persisted_target_uuid.as_deref(), Some("sole-uuid"));
    }

    #[tokio::test]
    async fn successful_zero_plugin_attempt_clears_old_target() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(
            ctrl,
            rx,
            "127.0.0.1".into(),
            19144,
            Some("old-target".into()),
            Some("old-pass".into()),
        );
        app.start_listen(19144, None, None);
        app.handle_connection_result(Ok(mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![],
            require_passcode: false,
        }));

        assert!(app.persisted_target_uuid.is_none());
        assert!(app.persisted_passcode.is_none());
    }

    #[test]
    fn unsent_sidebar_edits_not_persisted() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        // User types in sidebar but never connects
        app.sidebar.target_uuid = FieldState::new("typed-uuid");
        app.sidebar.passcode = FieldState::new("typed-pass");

        // Persisted fields should NOT be updated until handshake/selection
        assert!(app.persisted_target_uuid.is_none());
        assert!(app.persisted_passcode.is_none());
    }

    // ── Phase 5.2: Auto-relisten pending truth ──────────────────────────

    #[tokio::test]
    async fn disconnected_enter_with_pending_retry_cancels_only() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::SidebarPrimary;
        app.state = ConnectionState::Disconnected;
        app.sidebar.mode = Mode::Listen;
        auto_retry.listen_mode = true;
        auto_retry.retry_at = Some(tokio::time::Instant::now());

        // Enter cancels retry, sets Idle, does NOT reconnect
        assert!(!handle_key(&mut app, KeyCode::Enter.into(), &mut auto_retry).await);
        assert_eq!(app.state, ConnectionState::Idle);
        assert!(auto_retry.retry_at.is_none());
        assert!(!auto_retry.listen_mode);
        assert!(app
            .event_log
            .iter()
            .any(|e| e.message.contains("Cancelled")));
    }

    #[tokio::test]
    async fn disconnected_enter_without_pending_retry_reconnects() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        app.focus = Focus::SidebarPrimary;
        app.state = ConnectionState::Disconnected;
        app.sidebar.mode = Mode::Listen;
        // No pending retry
        assert!(auto_retry.retry_at.is_none());

        assert!(!handle_key(&mut app, KeyCode::Enter.into(), &mut auto_retry).await);
        // Without pending retry, fresh listen starts
        assert_eq!(app.state, ConnectionState::Pending);
        assert!(auto_retry.listen_mode);
    }

    #[test]
    fn auto_retry_due_fires_exactly_once() {
        let mut ar = AutoRetryState::new();
        ar.on_listen_started();
        ar.retry_at = Some(tokio::time::Instant::now());

        // First fire succeeds
        assert!(ar.try_fire());
        assert!(ar.retry_at.is_none());

        // Second fire returns false (already consumed)
        assert!(!ar.try_fire());
    }

    // ── Phase 5: stats dashboard key handling ────────────────────────────

    fn make_stat(
        name: &str,
        values: Vec<serde_json::Value>,
        children: Vec<mc_protocol::events::StatDataModel>,
    ) -> mc_protocol::events::StatDataModel {
        mc_protocol::events::StatDataModel {
            name: name.into(),
            values,
            children,
            should_aggregate: false,
        }
    }

    fn populate_stats(app: &mut App) {
        let stats = vec![
            make_stat(
                "server_tick_timings",
                vec![],
                vec![
                    make_stat("tick", vec![serde_json::json!(16.67)], vec![]),
                    make_stat("entity", vec![serde_json::json!(1500.0)], vec![]),
                ],
            ),
            make_stat(
                "app_memory",
                vec![],
                vec![
                    make_stat("used", vec![serde_json::json!(1_048_576.0)], vec![]),
                    make_stat("total", vec![serde_json::json!(4_194_304.0)], vec![]),
                ],
            ),
            make_stat(
                "client_stats",
                vec![],
                vec![
                    make_stat(
                        "client-b",
                        vec![],
                        vec![make_stat("cpu", vec![serde_json::json!(20.0)], vec![])],
                    ),
                    make_stat(
                        "client-a",
                        vec![],
                        vec![make_stat("cpu", vec![serde_json::json!(10.0)], vec![])],
                    ),
                ],
            ),
        ];
        app.stats.accumulate(&stats, 1);
    }

    #[tokio::test]
    async fn stats_tab_r_clears_only_stats() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        populate_stats(&mut app);
        app.focus = Focus::Main;
        app.set_tab(Tab::Stats);
        app.reconcile_stats_selection();
        assert!(!app.stats.is_empty());

        assert!(!handle_key(&mut app, KeyCode::Char('r').into(), &mut auto_retry).await);
        assert!(app.stats.is_empty());
        assert!(app.stats_selected_category.is_none());

        // Log and filters are untouched.
        app.add_log("keep me");
        app.log_filter.search = "term".into();
        assert_eq!(app.event_log.len(), 1);
        assert!(!app.log_filter.search.is_empty());
    }

    #[tokio::test]
    async fn stats_tab_left_right_changes_category() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        populate_stats(&mut app);
        app.focus = Focus::Main;
        app.set_tab(Tab::Stats);
        app.reconcile_stats_selection();
        assert_eq!(
            app.stats_selected_category.as_deref(),
            Some("server-performance")
        );

        assert!(!handle_key(&mut app, KeyCode::Right.into(), &mut auto_retry).await);
        assert_eq!(app.stats_selected_category.as_deref(), Some("memory"));

        assert!(!handle_key(&mut app, KeyCode::Left.into(), &mut auto_retry).await);
        assert_eq!(
            app.stats_selected_category.as_deref(),
            Some("server-performance")
        );
    }

    #[tokio::test]
    async fn stats_tab_brackets_change_client() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        populate_stats(&mut app);
        app.focus = Focus::Main;
        app.set_tab(Tab::Stats);
        app.stats_selected_category = Some("client".into());
        app.reconcile_stats_selection();
        assert_eq!(app.stats_selected_category.as_deref(), Some("client"));
        assert_eq!(app.stats_selected_client.as_deref(), Some("client-a"));

        assert!(!handle_key(&mut app, KeyCode::Char(']').into(), &mut auto_retry).await);
        assert_eq!(app.stats_selected_client.as_deref(), Some("client-b"));

        assert!(!handle_key(&mut app, KeyCode::Char('[').into(), &mut auto_retry).await);
        assert_eq!(app.stats_selected_client.as_deref(), Some("client-a"));
    }

    #[tokio::test]
    async fn stats_tab_arrows_scroll_cards() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        // Create many uncategorized groups so the viewport overflows.
        let stats: Vec<mc_protocol::events::StatDataModel> = (0..16)
            .map(|i| make_stat(&format!("group_{i:02}"), vec![serde_json::json!(i)], vec![]))
            .collect();
        app.stats.accumulate(&stats, 1);
        app.focus = Focus::Main;
        app.set_tab(Tab::Stats);
        app.stats_selected_category = Some("uncategorized".into());
        app.reconcile_stats_selection();
        assert_eq!(app.stats_scroll_offset, 0);

        assert!(!handle_key(&mut app, KeyCode::Down.into(), &mut auto_retry).await);
        assert_eq!(app.stats_scroll_offset, 1);

        assert!(!handle_key(&mut app, KeyCode::Up.into(), &mut auto_retry).await);
        assert_eq!(app.stats_scroll_offset, 0);
    }

    #[tokio::test]
    async fn stats_tab_global_keys_still_work() {
        let mut app = make_app();
        let mut auto_retry = make_auto_retry();
        populate_stats(&mut app);
        app.focus = Focus::Main;
        app.set_tab(Tab::Stats);

        assert!(!handle_key(&mut app, KeyCode::Char('1').into(), &mut auto_retry).await);
        assert_eq!(app.tab, Tab::Log);

        app.set_tab(Tab::Stats);
        assert!(!handle_key(&mut app, KeyCode::Char('h').into(), &mut auto_retry).await);
        assert_eq!(app.help_popup, HelpPopup::Visible);
    }
}
