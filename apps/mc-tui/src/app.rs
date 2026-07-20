use std::collections::VecDeque;

use mc_protocol::events::{DebuggeeEvent, LogLevel, PluginDetails};
use mc_session::{
    EvaluateResult, HandshakeInfo, SessionCommand, SessionController, SessionError, SessionEvent,
};
use tokio::sync::oneshot::error::TryRecvError;
use tokio::sync::{mpsc, oneshot};

use crate::stats::StatsState;

// ── Constants ─────────────────────────────────────────────────────────

/// Maximum number of log entries kept in the ring buffer.
pub const MAX_LOG_ENTRIES: usize = 500;

/// Interval between forced re-renders (even when no new events arrive).
pub const RENDER_TICK_MS: u64 = 50;

/// Sidebar width in wide layouts.
pub const SIDEBAR_WIDTH: u16 = 30;

/// Threshold for wide layout.
pub const WIDE_WIDTH: u16 = 120;

/// Threshold for compact footer/control labels.
pub const COMPACT_WIDTH: u16 = 70;

/// Minimum height before header/footer abbreviate.
pub const COMPACT_HEIGHT: u16 = 20;

/// Maximum number of Minecraft commands kept in history.
pub const MAX_COMMAND_HISTORY: usize = 8;

/// Maximum number of evaluate results kept in history.
pub const MAX_EVAL_HISTORY: usize = 10;

// ── Connection state ──────────────────────────────────────────────────

/// High-level connection state for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// No connection or pending operation.
    Idle,
    /// A connect/listen is in progress (TCP + possible target selection).
    Pending,
    /// Fully connected with known handshake information.
    Connected { version: u8, plugin_count: usize },
    /// Connection was lost or torn down.
    Disconnected,
}

// ── Main tabs ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Log,
    Stats,
}

// ── Focus targets ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    /// Main pane is active (tabs + log/stats content).
    #[default]
    Main,
    /// Mode selector Listen button.
    SidebarModeListen,
    /// Mode selector Connect button.
    SidebarModeConnect,
    /// Host input (connect mode only).
    SidebarHost,
    /// Port input.
    SidebarPort,
    /// Advanced options toggle.
    SidebarAdvanced,
    /// Target UUID input.
    SidebarTargetUuid,
    /// Passcode input.
    SidebarPasscode,
    /// Primary connect/listen/cancel/disconnect button.
    SidebarPrimary,
}

impl Focus {
    /// Whether this focus target lives inside the sidebar.
    pub fn is_sidebar(self) -> bool {
        !matches!(self, Focus::Main)
    }
}

// ── Connection mode ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Listen,
    Connect,
}

// ── Editable field state ──────────────────────────────────────────────

/// A single editable form field with cursor position.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldState {
    pub value: String,
    pub cursor: usize,
}

impl FieldState {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.len();
        Self { value, cursor }
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// Insert a character at the cursor.
    pub fn insert(&mut self, ch: char) {
        self.value.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let start = self.prev_char_boundary(self.cursor);
            self.value.remove(start);
            self.cursor = start;
        }
    }

    /// Delete the character under the cursor.
    pub fn delete(&mut self) {
        if self.cursor < self.value.len() {
            self.value.remove(self.cursor);
        }
    }

    /// Move the cursor by whole UTF-8 characters.
    pub fn move_cursor(&mut self, delta: isize) {
        if delta > 0 {
            for _ in 0..delta {
                if self.cursor >= self.value.len() {
                    break;
                }
                self.cursor = self.next_char_boundary(self.cursor);
            }
        } else {
            for _ in 0..(-delta) {
                if self.cursor == 0 {
                    break;
                }
                self.cursor = self.prev_char_boundary(self.cursor);
            }
        }
    }

    fn prev_char_boundary(&self, byte_idx: usize) -> usize {
        let mut idx = byte_idx;
        while idx > 0 {
            idx -= 1;
            if self.value.is_char_boundary(idx) {
                return idx;
            }
        }
        0
    }

    fn next_char_boundary(&self, byte_idx: usize) -> usize {
        let mut idx = byte_idx;
        while idx < self.value.len() {
            idx += 1;
            if self.value.is_char_boundary(idx) {
                return idx;
            }
        }
        self.value.len()
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.value.len();
    }
}

// ── Sidebar form ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarForm {
    pub mode: Mode,
    pub host: FieldState,
    pub port: FieldState,
    pub target_uuid: FieldState,
    pub passcode: FieldState,
    pub advanced_open: bool,
}

impl SidebarForm {
    pub fn new(
        default_host: &str,
        default_port: u16,
        default_target_uuid: Option<&str>,
        default_passcode: Option<&str>,
    ) -> Self {
        Self {
            mode: Mode::Listen,
            host: FieldState::new(default_host),
            port: FieldState::new(default_port.to_string()),
            target_uuid: FieldState::new(default_target_uuid.unwrap_or("")),
            passcode: FieldState::new(default_passcode.unwrap_or("")),
            advanced_open: false,
        }
    }

    pub fn port_u16(&self) -> Option<u16> {
        self.port.value.trim().parse().ok()
    }
}

// ── Log entry ─────────────────────────────────────────────────────────

/// Classification for a log row so the UI can render symbols/colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    System,
    Protocol,
    Stopped,
    Thread,
    Print,
    Notification,
    Stat,
    ProfilerCapture,
    Schema,
    Terminated,
    Unknown,
}

impl LogKind {
    pub fn symbol(self) -> char {
        match self {
            LogKind::System => '◆',
            LogKind::Protocol => '◇',
            LogKind::Stopped => '▣',
            LogKind::Thread => '↻',
            LogKind::Print => '▸',
            LogKind::Notification => '◆',
            LogKind::Stat => '▬',
            LogKind::ProfilerCapture => '◐',
            LogKind::Schema => '▤',
            LogKind::Terminated => '✕',
            LogKind::Unknown => '?',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LogKind::System => "INFO",
            LogKind::Protocol => "PROTOCOL",
            LogKind::Stopped => "STOPPED",
            LogKind::Thread => "THREAD",
            LogKind::Print => "PRINT",
            LogKind::Notification => "NOTIFY",
            LogKind::Stat => "STAT",
            LogKind::ProfilerCapture => "PROFILER",
            LogKind::Schema => "SCHEMA",
            LogKind::Terminated => "TERMINATED",
            LogKind::Unknown => "UNKNOWN",
        }
    }

    /// All kinds in the order they appear in the filter popup.
    pub const ALL: &[LogKind] = &[
        LogKind::System,
        LogKind::Protocol,
        LogKind::Stopped,
        LogKind::Thread,
        LogKind::Print,
        LogKind::Notification,
        LogKind::Stat,
        LogKind::ProfilerCapture,
        LogKind::Schema,
        LogKind::Terminated,
        LogKind::Unknown,
    ];
}

// ── Filter state ──────────────────────────────────────────────────────

/// Per-kind toggles. All kinds default to enabled so nothing is hidden
/// until the user explicitly filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogKindFilter {
    pub system: bool,
    pub protocol: bool,
    pub stopped: bool,
    pub thread: bool,
    pub print: bool,
    pub notification: bool,
    pub stat: bool,
    pub profiler_capture: bool,
    pub schema: bool,
    pub terminated: bool,
    pub unknown: bool,
}

impl Default for LogKindFilter {
    fn default() -> Self {
        Self::all_enabled()
    }
}

impl LogKindFilter {
    pub fn all_enabled() -> Self {
        Self {
            system: true,
            protocol: true,
            stopped: true,
            thread: true,
            print: true,
            notification: true,
            stat: true,
            profiler_capture: true,
            schema: true,
            terminated: true,
            unknown: true,
        }
    }

    pub fn get(&self, kind: LogKind) -> bool {
        match kind {
            LogKind::System => self.system,
            LogKind::Protocol => self.protocol,
            LogKind::Stopped => self.stopped,
            LogKind::Thread => self.thread,
            LogKind::Print => self.print,
            LogKind::Notification => self.notification,
            LogKind::Stat => self.stat,
            LogKind::ProfilerCapture => self.profiler_capture,
            LogKind::Schema => self.schema,
            LogKind::Terminated => self.terminated,
            LogKind::Unknown => self.unknown,
        }
    }

    pub fn set(&mut self, kind: LogKind, value: bool) {
        match kind {
            LogKind::System => self.system = value,
            LogKind::Protocol => self.protocol = value,
            LogKind::Stopped => self.stopped = value,
            LogKind::Thread => self.thread = value,
            LogKind::Print => self.print = value,
            LogKind::Notification => self.notification = value,
            LogKind::Stat => self.stat = value,
            LogKind::ProfilerCapture => self.profiler_capture = value,
            LogKind::Schema => self.schema = value,
            LogKind::Terminated => self.terminated = value,
            LogKind::Unknown => self.unknown = value,
        }
    }
}

/// Per-level toggles for protocol events that carry a `LogLevel`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLevelFilter {
    pub log: bool,
    pub warn: bool,
    pub error: bool,
}

impl Default for LogLevelFilter {
    fn default() -> Self {
        Self::all_enabled()
    }
}

impl LogLevelFilter {
    pub fn all_enabled() -> Self {
        Self {
            log: true,
            warn: true,
            error: true,
        }
    }

    pub fn get(&self, level: LogLevel) -> bool {
        match level {
            LogLevel::Verbose | LogLevel::Log => self.log,
            LogLevel::Warn => self.warn,
            LogLevel::Error | LogLevel::Stop => self.error,
        }
    }

    pub fn set(&mut self, level: LogLevel, value: bool) {
        match level {
            LogLevel::Verbose | LogLevel::Log => self.log = value,
            LogLevel::Warn => self.warn = value,
            LogLevel::Error | LogLevel::Stop => self.error = value,
        }
    }
}

/// Serializable filter configuration. Exposed for the next lane to persist
/// or rehydrate without touching raw log storage.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogFilterState {
    pub search: String,
    pub kinds: LogKindFilter,
    pub levels: LogLevelFilter,
}

impl LogFilterState {
    /// A clean filter that shows every entry.
    pub fn show_all() -> Self {
        Self::default()
    }

    /// True when the filter would let the entry through.
    pub fn allows(&self, entry: &LogEntry) -> bool {
        if !self.kinds.get(entry.kind) {
            return false;
        }

        if let Some(level) = entry.log_level {
            if !self.levels.get(level) {
                return false;
            }
        }

        if self.search.is_empty() {
            return true;
        }

        let q = self.search.to_lowercase();
        entry.message.to_lowercase().contains(&q)
            || entry.kind.label().to_lowercase().contains(&q)
            || entry.timestamp.contains(&self.search)
    }

    /// True when no filtering is active.
    pub fn is_identity(&self) -> bool {
        self.search.is_empty()
            && self.kinds == LogKindFilter::all_enabled()
            && self.levels == LogLevelFilter::all_enabled()
    }
}

/// A single entry in the on-screen event log.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub kind: LogKind,
    pub message: String,
    pub timestamp: String,
    /// Protocol log level, if the source event carried one.
    pub log_level: Option<LogLevel>,
}

impl LogEntry {
    pub fn system(message: impl Into<String>) -> Self {
        Self {
            kind: LogKind::System,
            message: message.into(),
            timestamp: format_timestamp(),
            log_level: None,
        }
    }

    pub fn event(kind: LogKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            timestamp: format_timestamp(),
            log_level: None,
        }
    }

    pub fn event_with_level(
        kind: LogKind,
        message: impl Into<String>,
        log_level: Option<LogLevel>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            timestamp: format_timestamp(),
            log_level,
        }
    }
}

fn format_timestamp() -> String {
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let hh = (secs / 3600) % 24;
    let mm = (secs / 60) % 60;
    let ss = secs % 60;
    format!("{hh:02}:{mm:02}:{ss:02}")
}

// ── Plugin selector popup state ───────────────────────────────────────

/// Active plugin-selection popup.
pub struct PluginSelection {
    pub plugins: Vec<PluginDetails>,
    pub selected: usize,
    pub scroll_offset: usize,
}

impl PluginSelection {
    pub fn new(plugins: Vec<PluginDetails>) -> Self {
        Self {
            plugins,
            selected: 0,
            scroll_offset: 0,
        }
    }

    pub fn scroll_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn scroll_down(&mut self) {
        if self.selected + 1 < self.plugins.len() {
            self.selected += 1;
        }
    }

    pub fn selected_uuid(&self) -> Option<String> {
        self.plugins
            .get(self.selected)
            .map(|p| p.module_uuid.clone())
    }

    pub fn selected_name(&self) -> Option<String> {
        self.plugins.get(self.selected).map(|p| p.name.clone())
    }
}

// ── Command input popup state ─────────────────────────────────────────

/// Single-line editor for a Minecraft command plus bounded history.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandInput {
    pub open: bool,
    pub field: FieldState,
    pub history: VecDeque<String>,
    pub history_index: Option<usize>,
    pub error: Option<String>,
}

impl CommandInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self) {
        self.open = true;
        self.error = None;
        self.history_index = None;
        self.field = FieldState::default();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.error = None;
        self.history_index = None;
    }

    /// Clear session-scoped editor state while preserving command history.
    pub fn reset_transient(&mut self) {
        self.close();
        self.field = FieldState::default();
    }

    /// Record a submitted command, newest-first, deduplicated.
    pub fn push_history(&mut self, command: String) {
        if command.is_empty() {
            return;
        }
        self.history.retain(|c| c != &command);
        self.history.push_front(command);
        while self.history.len() > MAX_COMMAND_HISTORY {
            self.history.pop_back();
        }
    }

    /// Cycle through history. `older` moves to older entries; `false` moves newer.
    /// Returns to a blank edit line when moving newer past the newest entry.
    pub fn cycle_history(&mut self, older: bool) {
        let len = self.history.len();
        if len == 0 {
            return;
        }

        if older {
            let next = self.history_index.map(|i| i + 1).unwrap_or(0).min(len - 1);
            self.history_index = Some(next);
            if let Some(cmd) = self.history.get(next) {
                self.field = FieldState::new(cmd.clone());
            }
        } else {
            match self.history_index {
                Some(0) | None => {
                    self.history_index = None;
                    self.field = FieldState::default();
                }
                Some(i) => {
                    let next = i - 1;
                    self.history_index = Some(next);
                    if let Some(cmd) = self.history.get(next) {
                        self.field = FieldState::new(cmd.clone());
                    }
                }
            }
        }
    }
}

// ── Evaluate history entry ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EvaluateEntry {
    pub expression: String,
    pub success: bool,
    pub detail: String,
}

// ── Evaluate input popup state ──────────────────────────────────────────

/// Single-line expression editor and bounded result history.
#[derive(Debug, Default)]
pub struct EvaluateInput {
    pub open: bool,
    pub field: FieldState,
    pub busy: bool,
    pub error: Option<String>,
    pub history: VecDeque<EvaluateEntry>,
    pub pending_expression: String,
    pub result_rx: Option<oneshot::Receiver<Result<EvaluateResult, SessionError>>>,
}

impl EvaluateInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self) {
        if self.open {
            return;
        }
        self.open = true;
        self.field = FieldState::default();
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Cancel session-scoped evaluation state while preserving result history.
    pub fn reset_transient(&mut self) {
        self.open = false;
        self.field = FieldState::default();
        self.busy = false;
        self.error = None;
        self.pending_expression.clear();
        self.result_rx.take();
    }

    /// Start an evaluation if there is no in-flight request.
    pub fn start(
        &mut self,
        expression: String,
        rx: oneshot::Receiver<Result<EvaluateResult, SessionError>>,
    ) -> bool {
        if self.busy || expression.is_empty() {
            return false;
        }
        self.busy = true;
        self.pending_expression = expression.clone();
        self.field = FieldState::default();
        self.result_rx = Some(rx);
        true
    }

    /// Finish an in-flight evaluation and record the result.
    pub fn finish(
        &mut self,
        result: Result<EvaluateResult, SessionError>,
    ) -> Option<EvaluateEntry> {
        let expression = std::mem::take(&mut self.pending_expression);
        self.busy = false;
        self.result_rx.take();

        let entry = match result {
            Ok(EvaluateResult {
                success,
                args,
                message,
            }) => {
                let detail = if let Some(args) = args {
                    serde_json::to_string_pretty(&args).unwrap_or_default()
                } else {
                    message.unwrap_or_default()
                };
                EvaluateEntry {
                    expression: expression.clone(),
                    success,
                    detail,
                }
            }
            Err(e) => EvaluateEntry {
                expression: expression.clone(),
                success: false,
                detail: e.to_string(),
            },
        };

        self.push_history(entry.clone());
        Some(entry)
    }

    fn push_history(&mut self, entry: EvaluateEntry) {
        self.history.push_front(entry);
        while self.history.len() > MAX_EVAL_HISTORY {
            self.history.pop_back();
        }
    }
}

// ── Search input popup state ───────────────────────────────────────────

/// Single-line search editor that filters the event log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchInput {
    pub open: bool,
    pub field: FieldState,
}

impl SearchInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self) {
        self.open = true;
        self.field = FieldState::default();
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn reset_transient(&mut self) {
        self.close();
        self.field = FieldState::default();
    }
}

// ── Filter popup state ─────────────────────────────────────────────────

/// Compact popup for toggling event-kind and log-level filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterPopup {
    pub open: bool,
    /// Cursor position over the flattened list of toggles.
    pub selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorPopup {
    pub title: String,
    pub message: String,
}

impl ErrorPopup {
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
        }
    }
}

impl FilterPopup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self) {
        self.open = true;
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Number of selectable toggle rows in the popup.
    pub fn row_count() -> usize {
        LogKind::ALL.len() + 3 // kinds + Log/Warn/Error
    }

    pub fn scroll_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn scroll_down(&mut self) {
        let max = Self::row_count().saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
    }

    /// Toggle the item under the cursor, returning what changed.
    pub fn toggle_selected(&mut self, filter: &mut LogFilterState) -> Option<FilterToggle> {
        let idx = self.selected;
        if idx < LogKind::ALL.len() {
            let kind = LogKind::ALL[idx];
            let current = filter.kinds.get(kind);
            filter.kinds.set(kind, !current);
            Some(FilterToggle::Kind(kind, !current))
        } else {
            let level_idx = idx - LogKind::ALL.len();
            let level = match level_idx {
                0 => LogLevel::Log,
                1 => LogLevel::Warn,
                2 => LogLevel::Error,
                _ => return None,
            };
            let current = filter.levels.get(level);
            filter.levels.set(level, !current);
            Some(FilterToggle::Level(level, !current))
        }
    }
}

/// Description of a filter toggle for tests and callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterToggle {
    Kind(LogKind, bool),
    Level(LogLevel, bool),
}

// ── Log view state ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LogState {
    /// Index of the selected row in `event_log`.
    pub selected: Option<usize>,
    /// First visible row index.
    pub offset: usize,
    /// Whether the log should auto-scroll to the bottom.
    pub follow: bool,
}

impl Default for LogState {
    fn default() -> Self {
        Self {
            selected: None,
            offset: 0,
            follow: true,
        }
    }
}

impl LogState {
    /// Scroll by a signed number of rows.
    pub fn scroll(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.selected = None;
            self.offset = 0;
            self.follow = false;
            return;
        }

        let selected = self
            .selected
            .map(|s| (s as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize)
            .unwrap_or_else(|| {
                (len.saturating_sub(1) as isize + delta).clamp(0, len.saturating_sub(1) as isize)
                    as usize
            });
        self.selected = Some(selected);
        self.follow = selected == len.saturating_sub(1);
    }

    pub fn page_up(&mut self, page: usize, len: usize) {
        self.scroll(-(page as isize), len);
    }

    pub fn page_down(&mut self, page: usize, len: usize) {
        self.scroll(page as isize, len);
    }

    pub fn top(&mut self) {
        self.selected = Some(0);
        self.offset = 0;
        self.follow = false;
    }

    pub fn bottom(&mut self, len: usize) {
        if len == 0 {
            self.selected = None;
            self.offset = 0;
            self.follow = true;
        } else {
            self.selected = Some(len.saturating_sub(1));
            self.offset = 0;
            self.follow = true;
        }
    }

    /// Keep the visible window in sync with selection and follow state.
    pub fn update_offset(&mut self, visible_height: usize, len: usize) {
        if len == 0 {
            self.offset = 0;
            return;
        }

        let max_offset = len.saturating_sub(visible_height);

        if self.follow {
            self.offset = max_offset;
            return;
        }

        if let Some(selected) = self.selected {
            if selected < self.offset {
                self.offset = selected;
            } else if selected >= self.offset + visible_height {
                self.offset = selected.saturating_sub(visible_height).saturating_add(1);
            }
        }

        self.offset = self.offset.min(max_offset);
    }
}

// ── App ───────────────────────────────────────────────────────────────

/// Single-owner application state for the TUI.
pub struct App {
    // ── Session integration ────────────────────────────────────────────
    pub controller: SessionController,
    pub event_rx: mpsc::Receiver<SessionEvent>,

    // ── Spawned-connection result receiver ─────────────────────────────
    /// Set when a connect/listen task is spawned.  The task sends the
    /// `Result` over this channel once the handshake completes (or fails).
    pub conn_result_rx: Option<oneshot::Receiver<Result<HandshakeInfo, SessionError>>>,

    // ── State ──────────────────────────────────────────────────────────
    pub state: ConnectionState,
    pub event_log: VecDeque<LogEntry>,

    // ── Navigation / focus ─────────────────────────────────────────────
    pub tab: Tab,
    pub focus: Focus,
    pub show_sidebar: bool,

    // ── Sidebar form ───────────────────────────────────────────────────
    pub sidebar: SidebarForm,

    // ── Plugin selector ────────────────────────────────────────────────
    pub plugin_selection: Option<PluginSelection>,

    // ── Log view ───────────────────────────────────────────────────────
    pub log_state: LogState,
    /// Cached indices into `event_log` that satisfy the active filter.
    /// `log_state.selected` and `log_state.offset` refer to this filtered
    /// view, while `event_log` stays bounded and unchanged.
    pub filtered_log_indices: Vec<usize>,

    // ── Debug state ───────────────────────────────────────────────────
    pub stopped: bool,
    pub stop_reason: String,
    pub stopped_thread_id: Option<u32>,
    pub busy: bool,

    // ── Input popups ───────────────────────────────────────────────────
    pub command_input: CommandInput,
    pub evaluate_input: EvaluateInput,
    pub search_input: SearchInput,
    pub filter_popup: FilterPopup,
    pub error_popup: Option<ErrorPopup>,
    pub config_save_error_shown: bool,

    // ── Filter state ─────────────────────────────────────────────────────
    pub log_filter: LogFilterState,

    // ── Handshake cache ────────────────────────────────────────────────
    pub handshake: Option<HandshakeInfo>,

    // ── CLI defaults ───────────────────────────────────────────────────
    pub default_host: String,
    pub default_port: u16,
    pub default_target_uuid: Option<String>,
    pub default_passcode: Option<String>,

    // ── Config cache / dirty tracking ──────────────────────────────────
    /// Runtime cache of known plugins, preserved across saves
    /// and merged with new handshake data.
    pub known_plugins: Vec<PluginDetails>,
    /// Set to `true` whenever a filter toggle, search commit, reset,
    /// successful handshake, or target selection occurs.
    /// Cleared after a successful save.
    pub config_dirty: bool,

    /// Persisted target UUID that survives disconnects.
    pub persisted_target_uuid: Option<String>,
    /// Persisted passcode that survives disconnects.
    pub persisted_passcode: Option<String>,
    /// Credentials captured when the current connection attempt started.
    /// These are deliberately independent of the editable sidebar.
    pub pending_target_uuid: Option<String>,
    pub pending_passcode: Option<String>,
    /// Whether an auto-relisten retry is currently pending.
    pub auto_relisten_pending: bool,

    // ── Cached terminal size for focus visibility ──────────────────────
    last_width: u16,
    last_height: u16,

    // ── Stats (ephemeral, never persisted) ───────────────────────────────
    pub stats: StatsState,

    // ── Stats dashboard UI state (ephemeral, never persisted) ────────────
    pub stats_selected_category: Option<String>,
    pub stats_selected_client: Option<String>,
    /// Active scripting addon; `None` means all addons.
    pub stats_selected_addon: Option<String>,
    pub stats_scroll_offset: usize,

    // ── Help overlay ─────────────────────────────────────────────────────
    pub help_popup: HelpPopup,
}

impl App {
    /// Create a new application with the given controller, event receiver,
    /// and CLI defaults.
    pub fn new(
        controller: SessionController,
        event_rx: mpsc::Receiver<SessionEvent>,
        default_host: String,
        default_port: u16,
        default_target_uuid: Option<String>,
        default_passcode: Option<String>,
    ) -> Self {
        Self {
            controller,
            event_rx,
            conn_result_rx: None,
            state: ConnectionState::Idle,
            event_log: VecDeque::with_capacity(MAX_LOG_ENTRIES),
            tab: Tab::Log,
            focus: Focus::Main,
            show_sidebar: false,
            sidebar: SidebarForm::new(
                &default_host,
                default_port,
                default_target_uuid.as_deref(),
                default_passcode.as_deref(),
            ),
            plugin_selection: None,
            log_state: LogState::default(),
            filtered_log_indices: Vec::new(),
            stopped: false,
            stop_reason: String::new(),
            stopped_thread_id: None,
            busy: false,
            command_input: CommandInput::new(),
            evaluate_input: EvaluateInput::new(),
            search_input: SearchInput::new(),
            filter_popup: FilterPopup::new(),
            error_popup: None,
            config_save_error_shown: false,
            log_filter: LogFilterState::default(),
            handshake: None,
            persisted_target_uuid: default_target_uuid.clone(),
            persisted_passcode: default_passcode.clone(),
            pending_target_uuid: None,
            pending_passcode: None,
            auto_relisten_pending: false,
            default_host,
            default_port,
            default_target_uuid,
            default_passcode,
            last_width: 80,
            last_height: 24,
            stats: StatsState::new(),
            stats_selected_category: None,
            stats_selected_client: None,
            stats_selected_addon: None,
            stats_scroll_offset: 0,
            help_popup: HelpPopup::default(),
            known_plugins: Vec::new(),
            config_dirty: false,
        }
    }

    // ── Logging helper ─────────────────────────────────────────────────

    pub fn add_log(&mut self, message: impl Into<String>) {
        let entry = LogEntry::system(message);
        self.push_log(entry);
    }

    pub fn show_error(&mut self, title: impl Into<String>, message: impl Into<String>) {
        self.error_popup = Some(ErrorPopup::new(title, message));
    }

    pub fn dismiss_error(&mut self) {
        self.error_popup = None;
    }

    pub fn set_startup_warning(&mut self, warning: impl Into<String>) {
        self.show_error("Configuration warning", warning);
    }

    pub fn show_config_save_error_once(&mut self) {
        if !self.config_save_error_shown {
            self.config_save_error_shown = true;
            self.show_error(
                "Settings not saved",
                "Could not save settings. Changes will be retried.",
            );
        }
    }

    fn push_log(&mut self, entry: LogEntry) {
        if self.event_log.len() >= MAX_LOG_ENTRIES {
            self.event_log.pop_front();
        }
        self.event_log.push_back(entry);
        self.rebuild_filtered_log_indices();
    }

    /// Recompute `filtered_log_indices` from the full `event_log` and the
    /// current filter. Cheap because the ring buffer is bounded.
    pub fn rebuild_filtered_log_indices(&mut self) {
        self.filtered_log_indices.clear();
        for (idx, entry) in self.event_log.iter().enumerate() {
            if self.log_filter.allows(entry) {
                self.filtered_log_indices.push(idx);
            }
        }
        let filtered_len = self.filtered_log_indices.len();
        if self.log_state.follow {
            self.log_state.bottom(filtered_len);
        } else if let Some(selected) = self.log_state.selected {
            let max = filtered_len.saturating_sub(1);
            self.log_state.selected = Some(selected.min(max));
        }
    }

    /// Clear the stopped debug state without logging.
    pub fn clear_stopped(&mut self) {
        self.stopped = false;
        self.stop_reason.clear();
        self.stopped_thread_id = None;
    }

    /// Clear state that must never leak from one debugger session to another.
    /// Filter configuration is preserved; only transient editors are closed.
    pub fn clear_session_transients(&mut self) {
        self.handshake = None;
        self.plugin_selection = None;
        self.clear_stopped();
        self.busy = false;
        self.command_input.reset_transient();
        self.evaluate_input.reset_transient();
        self.search_input.reset_transient();
        self.filter_popup.close();
    }

    /// Abandon the credentials captured for a pending connection attempt.
    pub fn clear_pending_attempt_snapshots(&mut self) {
        self.pending_target_uuid = None;
        self.pending_passcode = None;
    }

    /// Close input popups and clear any transient error state.
    pub fn close_inputs(&mut self) {
        self.command_input.close();
        self.evaluate_input.close();
        self.search_input.close();
        self.filter_popup.close();
    }

    // ── Connection actions ─────────────────────────────────────────────

    /// Begin connecting to a remote debugger.
    pub fn start_connect(
        &mut self,
        host: String,
        port: u16,
        target_uuid: Option<String>,
        passcode: Option<String>,
    ) {
        self.clear_session_transients();
        self.pending_target_uuid = target_uuid.clone();
        self.pending_passcode = passcode.clone();
        let (tx, rx) = oneshot::channel();
        let ctrl = self.controller.clone();
        let host_clone = host.clone();
        tokio::spawn(async move {
            let result = ctrl.connect(host_clone, port, target_uuid, passcode).await;
            let _ = tx.send(result);
        });
        self.conn_result_rx = Some(rx);
        self.state = ConnectionState::Pending;
        self.add_log(format!("Connecting to {host}:{port}..."));
    }

    /// Begin listening for an incoming debugger connection.
    pub fn start_listen(
        &mut self,
        port: u16,
        target_uuid: Option<String>,
        passcode: Option<String>,
    ) {
        self.clear_session_transients();
        self.pending_target_uuid = target_uuid.clone();
        self.pending_passcode = passcode.clone();
        let (tx, rx) = oneshot::channel();
        let ctrl = self.controller.clone();
        tokio::spawn(async move {
            let result = ctrl.listen(port, target_uuid, passcode).await;
            let _ = tx.send(result);
        });
        self.conn_result_rx = Some(rx);
        self.state = ConnectionState::Pending;
        self.add_log(format!("Listening on port {port}..."));
    }

    /// Start connect using the current sidebar form values.
    pub fn start_connect_from_sidebar(&mut self) {
        let Some(port) = self.sidebar.port_u16() else {
            self.add_log("Invalid port number.");
            self.show_error(
                "Invalid port",
                "Enter a valid port number before connecting.",
            );
            return;
        };
        let host = self.sidebar.host.value.clone();
        let target_uuid = self.sidebar.target_uuid.value.trim();
        let target_uuid = if target_uuid.is_empty() {
            None
        } else {
            Some(target_uuid.to_string())
        };
        let passcode = self.sidebar.passcode.value.trim();
        let passcode = if passcode.is_empty() {
            None
        } else {
            Some(passcode.to_string())
        };
        self.start_connect(host, port, target_uuid, passcode);
    }

    /// Start listen using the current sidebar form values.
    pub fn start_listen_from_sidebar(&mut self) {
        let Some(port) = self.sidebar.port_u16() else {
            self.add_log("Invalid port number.");
            self.show_error(
                "Invalid port",
                "Enter a valid port number before listening.",
            );
            return;
        };
        let target_uuid = self.sidebar.target_uuid.value.trim();
        let target_uuid = if target_uuid.is_empty() {
            None
        } else {
            Some(target_uuid.to_string())
        };
        let passcode = self.sidebar.passcode.value.trim();
        let passcode = if passcode.is_empty() {
            None
        } else {
            Some(passcode.to_string())
        };
        self.start_listen(port, target_uuid, passcode);
    }

    // ── Event handling ─────────────────────────────────────────────────

    /// Process one frame of events.
    pub fn handle_session_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Debuggee(event) => {
                // Accumulate stats unconditionally (before logging).
                if let DebuggeeEvent::Stat2 { tick, stats } = &event {
                    self.stats.accumulate(stats, *tick);
                }
                let (kind, message, log_level) = classify_debuggee_event(&event);
                match &event {
                    DebuggeeEvent::Stopped { reason, thread } => {
                        self.stopped = true;
                        self.stop_reason.clone_from(reason);
                        self.stopped_thread_id = Some(*thread);
                    }
                    DebuggeeEvent::Thread { reason, thread }
                        if reason.eq_ignore_ascii_case("exited")
                            && self.stopped_thread_id == Some(*thread) =>
                    {
                        self.clear_stopped();
                    }
                    _ => {}
                }
                self.push_log(LogEntry::event_with_level(kind, message, log_level));
            }
            SessionEvent::TargetSelectionRequired { plugins } => {
                self.add_log(format!(
                    "Target selection required: {} plugin(s) available",
                    plugins.len()
                ));
                self.plugin_selection = Some(PluginSelection::new(plugins));
            }
            SessionEvent::Disconnected => {
                self.state = ConnectionState::Disconnected;
                self.clear_session_transients();
                self.add_log("Connection lost (disconnected).");
            }
            SessionEvent::Terminated { reason } => {
                let msg = match &reason {
                    Some(r) => format!("Debuggee terminated: {r}"),
                    None => "Debuggee terminated.".into(),
                };
                self.state = ConnectionState::Disconnected;
                self.clear_session_transients();
                self.add_log(msg);
            }
        }
    }

    /// Handle the result of a spawned connect/listen task.
    pub fn handle_connection_result(&mut self, result: Result<HandshakeInfo, SessionError>) {
        match result {
            Ok(info) => {
                self.handshake = Some(info.clone());
                self.state = ConnectionState::Connected {
                    version: info.version,
                    plugin_count: info.plugins.len(),
                };
                self.add_log(format!(
                    "Connected — protocol v{}, {} plugin(s) registered.",
                    info.version,
                    info.plugins.len(),
                ));

                // Merge handshake plugins into the known plugin cache (by UUID)
                for p in &info.plugins {
                    if let Some(existing) = self
                        .known_plugins
                        .iter_mut()
                        .find(|kp| kp.module_uuid == p.module_uuid)
                    {
                        existing.name.clone_from(&p.name);
                    } else {
                        self.known_plugins.push(p.clone());
                    }
                }

                // Commit only the attempt snapshot, never mutable sidebar values.
                let target = self.pending_target_uuid.take().or_else(|| {
                    if info.plugins.len() == 1 {
                        Some(info.plugins[0].module_uuid.clone())
                    } else {
                        None
                    }
                });
                self.persisted_target_uuid = target;
                self.persisted_passcode = self.pending_passcode.take();
                self.mark_config_dirty();
            }
            Err(e) => {
                // Only log if we haven't already handled it (e.g. cancelled).
                if !matches!(e, SessionError::Cancelled) {
                    self.add_log("Connection failed.");
                    self.show_error(
                        "Connection failed",
                        "Could not establish the debugger connection. Check the target, port, and passcode.",
                    );
                }
                self.clear_pending_attempt_snapshots();
                self.clear_session_transients();
                self.state = ConnectionState::Idle;
            }
        }
        self.conn_result_rx.take();
    }

    // ── Debug controls ─────────────────────────────────────────────────

    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    pub fn is_debug_busy(&self) -> bool {
        self.busy || self.evaluate_input.busy
    }

    pub fn can_pause(&self) -> bool {
        matches!(self.state, ConnectionState::Connected { .. })
            && !self.stopped
            && !self.is_debug_busy()
    }

    pub fn can_continue(&self) -> bool {
        matches!(self.state, ConnectionState::Connected { .. })
            && self.stopped
            && self.stopped_thread_id.is_some()
            && !self.is_debug_busy()
    }

    pub fn can_step(&self) -> bool {
        self.can_continue()
    }

    /// Request a pause on the given thread. Does not change stopped state.
    pub async fn request_pause(&mut self, thread_id: u32) {
        self.set_busy(true);
        let result = self
            .controller
            .send_command(SessionCommand::Pause { thread_id })
            .await;
        self.set_busy(false);
        match result {
            Ok(()) => self.add_log(format!("Pause requested (thread {thread_id}).")),
            Err(e) => self.add_log(format!("Pause failed: {e}")),
        }
    }

    /// Request a continue on the stopped thread and clear stopped state on success.
    pub async fn request_continue(&mut self) {
        let Some(thread_id) = self.stopped_thread_id else {
            return;
        };
        if !self.can_continue() {
            return;
        }
        self.set_busy(true);
        let result = self
            .controller
            .send_command(SessionCommand::Continue { thread_id })
            .await;
        self.set_busy(false);
        match result {
            Ok(()) => {
                self.clear_stopped();
                self.add_log(format!("Continue requested (thread {thread_id})."));
            }
            Err(e) => self.add_log(format!("Continue failed: {e}")),
        }
    }

    async fn request_step(&mut self, command: fn(u32) -> SessionCommand) {
        let Some(thread_id) = self.stopped_thread_id else {
            return;
        };
        if !self.can_step() {
            return;
        }
        self.set_busy(true);
        let cmd = command(thread_id);
        let label = step_label(&cmd);
        let result = self.controller.send_command(cmd).await;
        self.set_busy(false);
        match result {
            Ok(()) => {
                self.clear_stopped();
                self.add_log(format!("{label} requested (thread {thread_id})."));
            }
            Err(e) => self.add_log(format!("{label} failed: {e}")),
        }
    }

    pub async fn request_step_next(&mut self) {
        self.request_step(|thread_id| SessionCommand::StepNext { thread_id })
            .await;
    }

    pub async fn request_step_in(&mut self) {
        self.request_step(|thread_id| SessionCommand::StepIn { thread_id })
            .await;
    }

    pub async fn request_step_out(&mut self) {
        self.request_step(|thread_id| SessionCommand::StepOut { thread_id })
            .await;
    }

    /// Submit a Minecraft command, record history, and surface send errors.
    pub async fn submit_minecraft_command(&mut self, command: String) {
        if command.is_empty() {
            return;
        }
        self.command_input.push_history(command.clone());
        self.set_busy(true);
        let result = self
            .controller
            .send_command(SessionCommand::SendMinecraftCommand {
                command: command.clone(),
            })
            .await;
        self.set_busy(false);
        match result {
            Ok(()) => {
                self.command_input.close();
                self.add_log(format!("Command: /{command}"));
            }
            Err(e) => {
                self.command_input.error = Some(format!("Send failed: {e}"));
            }
        }
    }

    /// Start an async evaluate if stopped and not already busy.
    pub async fn start_evaluate(&mut self) {
        let expression = self.evaluate_input.field.value.clone();
        if expression.is_empty() {
            return;
        }
        if self.evaluate_input.busy {
            self.add_log("Evaluate already in progress.");
            return;
        }
        if !self.stopped {
            self.add_log("Evaluate is only available while stopped.");
            return;
        }
        let (tx, rx) = oneshot::channel();
        let cmd = SessionCommand::Evaluate {
            expression: expression.clone(),
            response_tx: tx,
        };
        let result = self.controller.send_command(cmd).await;
        match result {
            Ok(()) => {
                self.evaluate_input.start(expression, rx);
            }
            Err(e) => {
                self.evaluate_input.error = Some(format!("Send failed: {e}"));
            }
        }
    }

    /// Poll the in-flight evaluate receiver without blocking.
    pub fn try_recv_evaluate(&mut self) {
        if let Some(mut rx) = self.evaluate_input.result_rx.take() {
            match rx.try_recv() {
                Ok(result) => {
                    if let Some(entry) = self.evaluate_input.finish(result) {
                        let status = if entry.success { "ok" } else { "failed" };
                        self.add_log(format!(
                            "Evaluate {status}: {} = {}",
                            entry.expression,
                            entry.detail.lines().next().unwrap_or(""),
                        ));
                    }
                }
                Err(TryRecvError::Empty) => {
                    self.evaluate_input.result_rx = Some(rx);
                }
                Err(TryRecvError::Closed) => {
                    if let Some(entry) = self
                        .evaluate_input
                        .finish(Err(SessionError::ConnectionTaskDropped))
                    {
                        self.add_log(format!(
                            "Evaluate failed: {} = {}",
                            entry.expression,
                            entry.detail.lines().next().unwrap_or(""),
                        ));
                    }
                }
            }
        }
    }

    /// Reconcile a buffered connection result before discarding the receiver.
    ///
    /// Called when the user cancels during [`ConnectionState::Pending`] from
    /// any of the three cancel paths (x, plugin Esc, sidebar primary).
    ///
    /// 1. Takes `conn_result_rx` and calls `try_recv`:
    ///    - **Ok(Ok/Err)** — reconciled through [`handle_connection_result`];
    ///      if the result was `Connected`, calls [`disconnect`] to tear down;
    ///      final state is `Idle`.
    ///    - **Empty** — calls [`cancel_pending`], sets `Idle`, logs cancellation.
    ///    - **Closed** — the sender was dropped without sending a value
    ///      (spawned task panicked or was cancelled externally); logs an
    ///      unexpected-task error and sets `Idle`.
    /// 2. If no receiver exists (defensive), falls back to `cancel_pending`.
    pub async fn cancel_pending_with_result(&mut self) {
        let rx = self.conn_result_rx.take();
        if let Some(mut rx) = rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.handle_connection_result(result);
                    if matches!(self.state, ConnectionState::Connected { .. }) {
                        let _ = self.controller.disconnect().await;
                        self.clear_session_transients();
                        self.add_log("Disconnected.");
                    }
                    self.state = ConnectionState::Idle;
                }
                Err(TryRecvError::Empty) => {
                    let _ = self.controller.cancel_pending().await;
                    self.clear_pending_attempt_snapshots();
                    self.clear_session_transients();
                    self.state = ConnectionState::Idle;
                    self.add_log("Pending operation cancelled.");
                }
                Err(TryRecvError::Closed) => {
                    let _ = self.controller.disconnect().await;
                    self.clear_pending_attempt_snapshots();
                    self.clear_session_transients();
                    self.state = ConnectionState::Idle;
                    self.add_log("Connection task ended unexpectedly.");
                }
            }
        } else {
            let _ = self.controller.cancel_pending().await;
            self.clear_pending_attempt_snapshots();
            self.clear_session_transients();
            self.state = ConnectionState::Idle;
        }
    }

    // ── Focus / navigation ─────────────────────────────────────────────

    /// Toggle the sidebar overlay (relevant in non-wide layouts).
    pub fn toggle_sidebar(&mut self) {
        self.show_sidebar = !self.show_sidebar;
        if self.show_sidebar {
            self.focus = Focus::SidebarModeListen;
        } else {
            self.focus = Focus::Main;
        }
    }

    /// Close the sidebar overlay and return focus to main.
    pub fn close_sidebar(&mut self) {
        self.show_sidebar = false;
        self.focus = Focus::Main;
    }

    /// Cycle focus forward or backward through currently visible targets.
    pub fn cycle_focus(&mut self, forward: bool) {
        let targets = self.visible_focus_targets();
        if targets.is_empty() {
            return;
        }

        let current = self.focus;
        let pos = targets.iter().position(|&t| t == current).unwrap_or(0);
        let next_pos = if forward {
            (pos + 1) % targets.len()
        } else {
            pos.checked_sub(1).unwrap_or(targets.len() - 1)
        };
        self.focus = targets[next_pos];
    }

    /// Return the ordered list of focus targets that are visible right now.
    pub fn visible_focus_targets(&self) -> Vec<Focus> {
        let mut targets = vec![Focus::Main];

        if self.show_sidebar || self.sidebar_visible_by_layout(self.last_known_width()) {
            targets.push(Focus::SidebarModeListen);
            targets.push(Focus::SidebarModeConnect);
            if self.sidebar.mode == Mode::Connect {
                targets.push(Focus::SidebarHost);
            }
            targets.push(Focus::SidebarPort);
            targets.push(Focus::SidebarAdvanced);
            if self.sidebar.advanced_open {
                targets.push(Focus::SidebarTargetUuid);
                targets.push(Focus::SidebarPasscode);
            }
            targets.push(Focus::SidebarPrimary);
        }

        targets
    }

    /// True when the sidebar would be visible due to layout width.
    pub fn sidebar_visible_by_layout(&self, width: u16) -> bool {
        width >= WIDE_WIDTH
    }

    /// Last known terminal width; used for focus visibility.
    pub fn last_known_width(&self) -> u16 {
        self.last_width
    }

    /// Last known terminal height; used for paging calculations.
    pub fn last_known_height(&self) -> u16 {
        self.last_height
    }

    pub fn set_last_known_size(&mut self, width: u16, height: u16) {
        self.last_width = width;
        self.last_height = height;
    }

    /// True when the terminal is in compact mode.
    pub fn compact(&self) -> bool {
        self.last_width < COMPACT_WIDTH || self.last_height < COMPACT_HEIGHT
    }

    /// True when the sidebar is currently visible (persistent wide or overlay).
    pub fn is_sidebar_visible(&self) -> bool {
        self.show_sidebar || self.sidebar_visible_by_layout(self.last_width)
    }

    /// True when focus is currently on an editable text field.
    pub fn is_field_focused(&self) -> bool {
        matches!(
            self.focus,
            Focus::SidebarHost
                | Focus::SidebarPort
                | Focus::SidebarTargetUuid
                | Focus::SidebarPasscode
        )
    }

    // ── Tab switching ────────────────────────────────────────────────────

    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
    }

    pub fn toggle_help(&mut self) {
        self.help_popup = match self.help_popup {
            HelpPopup::Hidden => HelpPopup::Visible,
            HelpPopup::Visible => HelpPopup::Hidden,
        };
    }

    // ── Log scrolling ────────────────────────────────────────────────────
    // These operate on the filtered view so selection and follow always
    // reflect what the user can actually see.

    pub fn scroll_log_up(&mut self, n: usize) {
        let len = self.filtered_log_indices.len();
        self.log_state.scroll(-(n as isize), len);
    }

    pub fn scroll_log_down(&mut self, n: usize) {
        let len = self.filtered_log_indices.len();
        self.log_state.scroll(n as isize, len);
    }

    pub fn scroll_log_page_up(&mut self, page: usize) {
        let len = self.filtered_log_indices.len();
        self.log_state.page_up(page, len);
    }

    pub fn scroll_log_page_down(&mut self, page: usize) {
        let len = self.filtered_log_indices.len();
        self.log_state.page_down(page, len);
    }

    pub fn scroll_log_top(&mut self) {
        self.log_state.top();
    }

    pub fn scroll_log_bottom(&mut self) {
        self.log_state.bottom(self.filtered_log_indices.len());
    }

    // ── Field editing ──────────────────────────────────────────────────

    /// Return the currently focused field, if any.
    pub fn focused_field(&self) -> Option<&FieldState> {
        match self.focus {
            Focus::SidebarHost => Some(&self.sidebar.host),
            Focus::SidebarPort => Some(&self.sidebar.port),
            Focus::SidebarTargetUuid => Some(&self.sidebar.target_uuid),
            Focus::SidebarPasscode => Some(&self.sidebar.passcode),
            _ => None,
        }
    }

    /// Mutable access to the focused field.
    pub fn focused_field_mut(&mut self) -> Option<&mut FieldState> {
        match self.focus {
            Focus::SidebarHost => Some(&mut self.sidebar.host),
            Focus::SidebarPort => Some(&mut self.sidebar.port),
            Focus::SidebarTargetUuid => Some(&mut self.sidebar.target_uuid),
            Focus::SidebarPasscode => Some(&mut self.sidebar.passcode),
            _ => None,
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        let focus = self.focus;
        if let Some(field) = self.focused_field_mut() {
            match focus {
                Focus::SidebarPort => {
                    if ch.is_ascii_digit() {
                        field.insert(ch);
                    }
                }
                _ => field.insert(ch),
            }
        }
    }

    pub fn backspace(&mut self) {
        if let Some(field) = self.focused_field_mut() {
            field.backspace();
        }
    }

    pub fn delete_char(&mut self) {
        if let Some(field) = self.focused_field_mut() {
            field.delete();
        }
    }

    pub fn move_cursor(&mut self, delta: isize) {
        if let Some(field) = self.focused_field_mut() {
            field.move_cursor(delta);
        }
    }

    pub fn move_cursor_home(&mut self) {
        if let Some(field) = self.focused_field_mut() {
            field.home();
        }
    }

    pub fn move_cursor_end(&mut self) {
        if let Some(field) = self.focused_field_mut() {
            field.end();
        }
    }

    // ── Sidebar actions ────────────────────────────────────────────────

    pub fn set_mode(&mut self, mode: Mode) {
        self.sidebar.mode = mode;
        // Re-evaluate focus in case host field disappears.
        if mode == Mode::Listen && self.focus == Focus::SidebarHost {
            self.focus = Focus::SidebarPort;
        }
    }

    pub fn toggle_advanced(&mut self) {
        self.sidebar.advanced_open = !self.sidebar.advanced_open;
    }

    pub fn clear_log(&mut self) {
        self.event_log.clear();
        self.filtered_log_indices.clear();
        self.log_state = LogState::default();
    }

    /// Clear only accumulated stats (does not touch the event log, filters,
    /// session transients, or any other state).
    pub fn clear_stats(&mut self) {
        self.stats.clear();
        self.stats_selected_category = None;
        self.stats_selected_client = None;
        self.stats_selected_addon = None;
        self.stats_scroll_offset = 0;
    }

    // ── Stats dashboard selection / scrolling ────────────────────────────

    /// Reconcile category/client selection against the current stats data.
    ///
    /// Called automatically before rendering the dashboard and after key
    /// navigation so the selected items stay valid as data arrives or
    /// disappears.
    pub fn reconcile_stats_selection(&mut self) {
        let categories = self.stats.categories();
        if categories.is_empty() {
            self.stats_selected_category = None;
            self.stats_selected_client = None;
            self.stats_scroll_offset = 0;
            return;
        }

        // Ensure the selected category still exists; fall back to the first.
        let category_valid = self
            .stats_selected_category
            .as_deref()
            .map(|key| categories.iter().any(|c| c.key == key))
            .unwrap_or(false);
        if !category_valid {
            self.stats_selected_category = categories.first().map(|c| c.key.to_string());
            self.stats_scroll_offset = 0;
        }

        // Client selector is only relevant for the client category with >1 IDs.
        if self.stats_selected_category.as_deref() == Some("client") {
            let clients = self.stats.client_ids();
            if clients.len() > 1 {
                let client_valid = self
                    .stats_selected_client
                    .as_deref()
                    .map(|id| clients.iter().any(|c| c == id))
                    .unwrap_or(false);
                if !client_valid {
                    self.stats_selected_client = clients.first().cloned();
                }
            } else {
                self.stats_selected_client = None;
            }
        } else {
            self.stats_selected_client = None;
        }

        if self.stats_selected_category.as_deref() == Some("scripting") {
            let addons = self.stats.subscriber_addon_ids();
            if let Some(selected) = self.stats_selected_addon.as_deref() {
                if !addons.iter().any(|addon| addon == selected) {
                    self.stats_selected_addon = None;
                }
            }
        } else {
            self.stats_selected_addon = None;
        }
    }

    /// Move to the next category, wrapping around and resetting scroll.
    pub fn select_next_category(&mut self) {
        let categories = self.stats.categories();
        if categories.is_empty() {
            return;
        }
        let current = self.stats_selected_category.as_deref();
        let idx = categories
            .iter()
            .position(|c| Some(c.key) == current)
            .map(|i| (i + 1) % categories.len())
            .unwrap_or(0);
        self.stats_selected_category = Some(categories[idx].key.to_string());
        self.stats_scroll_offset = 0;
        self.reconcile_stats_selection();
    }

    /// Move to the previous category, wrapping around and resetting scroll.
    pub fn select_prev_category(&mut self) {
        let categories = self.stats.categories();
        if categories.is_empty() {
            return;
        }
        let current = self.stats_selected_category.as_deref();
        let idx = categories
            .iter()
            .position(|c| Some(c.key) == current)
            .map(|i| i.checked_sub(1).unwrap_or(categories.len() - 1))
            .unwrap_or(0);
        self.stats_selected_category = Some(categories[idx].key.to_string());
        self.stats_scroll_offset = 0;
        self.reconcile_stats_selection();
    }

    /// Move to the next client ID when the client selector is visible.
    pub fn select_next_client(&mut self) {
        let clients = self.stats.client_ids();
        if clients.len() <= 1 {
            return;
        }
        let current = self.stats_selected_client.as_deref();
        let idx = clients
            .iter()
            .position(|c| Some(c.as_str()) == current)
            .map(|i| (i + 1) % clients.len())
            .unwrap_or(0);
        self.stats_selected_client = Some(clients[idx].clone());
    }

    /// Move to the previous client ID when the client selector is visible.
    pub fn select_prev_client(&mut self) {
        let clients = self.stats.client_ids();
        if clients.len() <= 1 {
            return;
        }
        let current = self.stats_selected_client.as_deref();
        let idx = clients
            .iter()
            .position(|c| Some(c.as_str()) == current)
            .map(|i| i.checked_sub(1).unwrap_or(clients.len() - 1))
            .unwrap_or(0);
        self.stats_selected_client = Some(clients[idx].clone());
    }

    pub fn select_next_addon(&mut self) {
        self.select_addon(1);
    }

    pub fn select_prev_addon(&mut self) {
        self.select_addon(-1);
    }

    fn select_addon(&mut self, direction: isize) {
        let addons = self.stats.subscriber_addon_ids();
        if addons.is_empty() || self.stats_selected_category.as_deref() != Some("scripting") {
            return;
        }
        // None is the first item: All addons.
        let current = self
            .stats_selected_addon
            .as_deref()
            .and_then(|id| addons.iter().position(|addon| addon == id))
            .map(|i| i as isize + 1)
            .unwrap_or(0);
        let next = (current + direction).rem_euclid(addons.len() as isize + 1) as usize;
        self.stats_selected_addon = next.checked_sub(1).map(|i| addons[i].clone());
    }

    /// Scroll the dashboard card viewport up by a number of rows.
    pub fn scroll_stats_up(&mut self, n: usize) {
        self.stats_scroll_offset = self.stats_scroll_offset.saturating_sub(n);
    }

    /// Scroll the dashboard card viewport down by a number of rows.
    pub fn scroll_stats_down(&mut self, n: usize) {
        self.stats_scroll_offset = self.stats_scroll_offset.saturating_add(n);
    }

    /// Scroll the dashboard up by one page.
    pub fn scroll_stats_page_up(&mut self, page: usize) {
        self.scroll_stats_up(page);
    }

    /// Scroll the dashboard down by one page.
    pub fn scroll_stats_page_down(&mut self, page: usize) {
        self.scroll_stats_down(page);
    }

    /// Jump to the top of the dashboard.
    pub fn scroll_stats_top(&mut self) {
        self.stats_scroll_offset = 0;
    }

    /// Jump to the bottom of the dashboard.
    pub fn scroll_stats_bottom(&mut self) {
        self.stats_scroll_offset = usize::MAX;
    }

    // ── Search / filter / reset ─────────────────────────────────────────

    /// Open the search input, closing any other modal first.
    pub fn open_search(&mut self) {
        self.close_inputs();
        self.search_input.open();
    }

    /// Apply the current search field as the active filter and close the input.
    pub fn commit_search(&mut self) {
        self.log_filter
            .search
            .clone_from(&self.search_input.field.value);
        self.rebuild_filtered_log_indices();
        self.search_input.close();
        self.mark_config_dirty();
    }

    /// Open the compact filter popup, closing other modals first.
    pub fn open_filter_popup(&mut self) {
        self.close_inputs();
        self.filter_popup.open();
    }

    /// Toggle the selected filter row and rebuild the filtered view.
    pub fn toggle_selected_filter(&mut self) -> Option<FilterToggle> {
        let result = self.filter_popup.toggle_selected(&mut self.log_filter);
        self.rebuild_filtered_log_indices();
        self.mark_config_dirty();
        result
    }

    /// Reset search and all filters without touching the underlying event log.
    pub fn reset_filters(&mut self) {
        self.log_filter = LogFilterState::show_all();
        self.search_input.reset_transient();
        self.rebuild_filtered_log_indices();
        self.mark_config_dirty();
    }

    /// True when the log view is currently filtered in any way.
    pub fn is_filtered(&self) -> bool {
        !self.log_filter.is_identity()
    }

    /// Number of entries visible after filtering.
    pub fn filtered_log_len(&self) -> usize {
        self.filtered_log_indices.len()
    }

    // ── Config integration ──────────────────────────────────────────────

    /// Mark the config as dirty (needs persistence).
    /// Called after filter changes, successful handshake, or target selection.
    pub fn mark_config_dirty(&mut self) {
        self.config_dirty = true;
        self.config_save_error_shown = false;
    }

    /// Apply a loaded config to the App, populating sidebar defaults and
    /// filter state.  CLI overrides have already been resolved by this point.
    pub fn apply_config(&mut self, cfg: &crate::config::AppConfig) {
        // Filter state
        self.log_filter.search.clone_from(&cfg.search);
        self.log_filter.kinds = LogKindFilter {
            system: cfg.kinds.system,
            protocol: cfg.kinds.protocol,
            stopped: cfg.kinds.stopped,
            thread: cfg.kinds.thread,
            print: cfg.kinds.print,
            notification: cfg.kinds.notification,
            stat: cfg.kinds.stat,
            profiler_capture: cfg.kinds.profiler_capture,
            schema: cfg.kinds.schema,
            terminated: cfg.kinds.terminated,
            unknown: cfg.kinds.unknown,
        };
        self.log_filter.levels = LogLevelFilter {
            log: cfg.levels.log,
            warn: cfg.levels.warn,
            error: cfg.levels.error,
        };

        // Sidebar defaults — CLI overrides have already been resolved, so
        // the config values only apply when the CLI didn't provide them.
        // CLI values were passed to App::new as default_host/default_port/
        // default_target_uuid/default_passcode and used in SidebarForm::new.
        //
        // For the fields that are *not* CLI-mandated (target_uuid, passcode),
        // the config provides the fallback.  The caller already built the
        // sidebar form with CLI values, so we only patch fields that are
        // still empty or defaulted.
        if self.sidebar.target_uuid.value.is_empty() {
            if let Some(ref uuid) = cfg.last_target_uuid {
                self.sidebar.target_uuid = FieldState::new(uuid.clone());
            }
        }
        if self.sidebar.passcode.value.is_empty() {
            if let Some(ref pass) = cfg.passcode {
                self.sidebar.passcode = FieldState::new(pass.clone());
            }
        }

        // Persisted credentials from loaded config (not overridden by CLI)
        if self.persisted_target_uuid.is_none() {
            self.persisted_target_uuid = cfg.last_target_uuid.clone();
        }
        if self.persisted_passcode.is_none() {
            self.persisted_passcode = cfg.passcode.clone();
        }

        // Load known plugins from config into the runtime cache
        self.known_plugins = cfg.known_plugins.clone();

        self.rebuild_filtered_log_indices();
    }

    /// Capture the current filter state for config persistence.
    pub fn current_filter_state(&self) -> (String, LogKindFilter, LogLevelFilter) {
        (
            self.log_filter.search.clone(),
            LogKindFilter {
                system: self.log_filter.kinds.system,
                protocol: self.log_filter.kinds.protocol,
                stopped: self.log_filter.kinds.stopped,
                thread: self.log_filter.kinds.thread,
                print: self.log_filter.kinds.print,
                notification: self.log_filter.kinds.notification,
                stat: self.log_filter.kinds.stat,
                profiler_capture: self.log_filter.kinds.profiler_capture,
                schema: self.log_filter.kinds.schema,
                terminated: self.log_filter.kinds.terminated,
                unknown: self.log_filter.kinds.unknown,
            },
            LogLevelFilter {
                log: self.log_filter.levels.log,
                warn: self.log_filter.levels.warn,
                error: self.log_filter.levels.error,
            },
        )
    }

    /// Map a filtered view index to the absolute `event_log` index.
    pub fn filtered_to_absolute(&self, filtered_idx: usize) -> Option<usize> {
        self.filtered_log_indices.get(filtered_idx).copied()
    }

    // ── Debuggee event classification ──────────────────────────────────

    pub fn log_kind_for(event: &DebuggeeEvent) -> LogKind {
        match event {
            DebuggeeEvent::Protocol { .. } => LogKind::Protocol,
            DebuggeeEvent::Stopped { .. } => LogKind::Stopped,
            DebuggeeEvent::Thread { .. } => LogKind::Thread,
            DebuggeeEvent::Print { .. } => LogKind::Print,
            DebuggeeEvent::Notification { .. } => LogKind::Notification,
            DebuggeeEvent::Stat2 { .. } => LogKind::Stat,
            DebuggeeEvent::ProfilerCapture { .. } => LogKind::ProfilerCapture,
            DebuggeeEvent::Schema { .. } => LogKind::Schema,
            DebuggeeEvent::Terminated { .. } => LogKind::Terminated,
            _ => LogKind::Unknown,
        }
    }
}

// ── Help popup state ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HelpPopup {
    #[default]
    Hidden,
    Visible,
}

// ── Event formatting ──────────────────────────────────────────────────

fn step_label(cmd: &SessionCommand) -> &'static str {
    match cmd {
        SessionCommand::StepNext { .. } => "Step next",
        SessionCommand::StepIn { .. } => "Step in",
        SessionCommand::StepOut { .. } => "Step out",
        _ => "Step",
    }
}

/// Format a `DebuggeeEvent` into a grounded one-line log string.
pub fn format_debuggee_event(event: &DebuggeeEvent) -> String {
    match event {
        DebuggeeEvent::Protocol {
            version,
            plugins,
            require_passcode,
        } => {
            let pc = if *require_passcode {
                " (passcode required)"
            } else {
                ""
            };
            format!("Protocol v{version}, {} plugin(s){pc}", plugins.len())
        }
        DebuggeeEvent::Stopped { reason, thread } => {
            format!("Stopped: {reason} (thread {thread})")
        }
        DebuggeeEvent::Thread { reason, thread } => {
            format!("Thread: {reason} (thread {thread})")
        }
        DebuggeeEvent::Print { message, log_level } => {
            let level = match log_level {
                LogLevel::Verbose => "VERBOSE",
                LogLevel::Log => "LOG",
                LogLevel::Warn => "WARN",
                LogLevel::Error => "ERROR",
                LogLevel::Stop => "STOP",
            };
            format!("[{level}] {message}")
        }
        DebuggeeEvent::Notification { message, log_level } => {
            let level = match log_level {
                LogLevel::Verbose => "VERBOSE",
                LogLevel::Log => "LOG",
                LogLevel::Warn => "WARN",
                LogLevel::Error => "ERROR",
                LogLevel::Stop => "STOP",
            };
            format!("[{level}] {message}")
        }
        DebuggeeEvent::Stat2 { tick, stats } => {
            let count = count_stats(stats);
            format!("Stats tick={tick} ({count} value(s))")
        }
        DebuggeeEvent::ProfilerCapture {
            capture_base_path, ..
        } => {
            format!("Profiler capture: {capture_base_path}")
        }
        DebuggeeEvent::DebuggeeResponse {
            request_seq,
            success,
            ..
        } => {
            let ok = success.unwrap_or(true);
            format!("DebuggeeResponse seq={request_seq} success={ok}")
        }
        DebuggeeEvent::Response {
            request_seq,
            command,
            success,
            error,
            ..
        } => {
            let cmd = command.as_deref().unwrap_or("?");
            let status = match (success, error) {
                (_, Some(e)) => format!("error: {e}"),
                (Some(true), _) | (None, _) => "ok".into(),
                (Some(false), _) => "fail".into(),
            };
            format!("Response seq={request_seq} [{cmd}] {status}")
        }
        DebuggeeEvent::Schema { descriptors } => {
            format!("Schema: {} descriptor(s)", descriptors.len())
        }
        DebuggeeEvent::Terminated { reason } => match reason {
            Some(r) => format!("Terminated: {r}"),
            None => "Terminated".into(),
        },
        DebuggeeEvent::Unknown { type_name, .. } => {
            format!("Unknown event: {type_name}")
        }
    }
}

fn classify_debuggee_event(event: &DebuggeeEvent) -> (LogKind, String, Option<LogLevel>) {
    let kind = App::log_kind_for(event);
    let level = debuggee_event_log_level(event);
    (kind, format_debuggee_event(event), level)
}

fn debuggee_event_log_level(event: &DebuggeeEvent) -> Option<LogLevel> {
    match event {
        DebuggeeEvent::Print { log_level, .. } => Some(*log_level),
        DebuggeeEvent::Notification { log_level, .. } => Some(*log_level),
        _ => None,
    }
}

/// Count leaf stat values recursively.
fn count_stats(stats: &[mc_protocol::events::StatDataModel]) -> usize {
    let mut count = 0;
    for stat in stats {
        count += stat.values.len();
        count += count_stats(&stat.children);
    }
    count
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mc_protocol::events::DebuggeeEvent;
    use mc_session::{SessionController, SessionEvent};

    // ── Key handling tests ─────────────────────────────────────────────

    #[test]
    fn add_log_respects_max_entries() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        for i in 0..MAX_LOG_ENTRIES + 10 {
            app.add_log(format!("entry {i}"));
        }

        assert_eq!(app.event_log.len(), MAX_LOG_ENTRIES);
        assert!(app
            .event_log
            .back()
            .unwrap()
            .message
            .contains(&format!("entry {}", MAX_LOG_ENTRIES + 9)));
    }

    #[test]
    fn initial_state_is_idle() {
        let (ctrl, rx) = SessionController::new();
        let app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        assert_eq!(app.state, ConnectionState::Idle);
        assert!(app.plugin_selection.is_none());
        assert_eq!(app.tab, Tab::Log);
        assert_eq!(app.focus, Focus::Main);
        assert!(!app.show_sidebar);
    }

    #[test]
    fn default_form_values_hydrated_from_cli() {
        let (ctrl, rx) = SessionController::new();
        let app = App::new(
            ctrl,
            rx,
            "10.0.0.1".into(),
            19132,
            Some("my-uuid".into()),
            Some("secret".into()),
        );
        assert_eq!(app.sidebar.host.value, "10.0.0.1");
        assert_eq!(app.sidebar.port.value, "19132");
        assert_eq!(app.sidebar.target_uuid.value, "my-uuid");
        assert_eq!(app.sidebar.passcode.value, "secret");
    }

    // ── Focus / navigation ─────────────────────────────────────────────

    #[test]
    fn focus_cycles_through_visible_targets() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.set_last_known_size(80, 24);
        app.show_sidebar = true;

        let targets = app.visible_focus_targets();
        assert!(targets.contains(&Focus::Main));
        assert!(targets.contains(&Focus::SidebarModeListen));
        assert!(targets.contains(&Focus::SidebarPort));

        app.focus = Focus::Main;
        app.cycle_focus(true);
        assert_eq!(app.focus, Focus::SidebarModeListen);
        app.cycle_focus(false);
        assert_eq!(app.focus, Focus::Main);
    }

    #[test]
    fn host_field_hidden_in_listen_mode() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.set_last_known_size(120, 30);
        app.show_sidebar = false; // wide mode makes it visible anyway
        app.set_mode(Mode::Listen);

        let targets = app.visible_focus_targets();
        assert!(!targets.contains(&Focus::SidebarHost));

        app.set_mode(Mode::Connect);
        let targets = app.visible_focus_targets();
        assert!(targets.contains(&Focus::SidebarHost));
    }

    #[test]
    fn advanced_fields_hidden_when_closed() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.set_last_known_size(120, 30);
        app.sidebar.advanced_open = false;

        let targets = app.visible_focus_targets();
        assert!(!targets.contains(&Focus::SidebarTargetUuid));
        assert!(!targets.contains(&Focus::SidebarPasscode));

        app.toggle_advanced();
        let targets = app.visible_focus_targets();
        assert!(targets.contains(&Focus::SidebarTargetUuid));
        assert!(targets.contains(&Focus::SidebarPasscode));
    }

    #[test]
    fn toggle_sidebar_opens_and_focuses() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        assert!(!app.show_sidebar);
        assert_eq!(app.focus, Focus::Main);

        app.toggle_sidebar();
        assert!(app.show_sidebar);
        assert_eq!(app.focus, Focus::SidebarModeListen);

        app.close_sidebar();
        assert!(!app.show_sidebar);
        assert_eq!(app.focus, Focus::Main);
    }

    // ── Field editing ──────────────────────────────────────────────────

    #[test]
    fn insert_and_backspace() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.focus = Focus::SidebarHost;
        app.sidebar.host = FieldState::new("");

        app.insert_char('a');
        app.insert_char('b');
        app.insert_char('c');
        assert_eq!(app.sidebar.host.value, "abc");
        assert_eq!(app.sidebar.host.cursor, 3);

        app.backspace();
        assert_eq!(app.sidebar.host.value, "ab");
        assert_eq!(app.sidebar.host.cursor, 2);
    }

    #[test]
    fn cursor_left_right_home_end() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.focus = Focus::SidebarHost;
        app.sidebar.host = FieldState::new("abc");

        app.move_cursor_home();
        app.insert_char('X');
        assert_eq!(app.sidebar.host.value, "Xabc");

        app.move_cursor_end();
        app.insert_char('Y');
        assert_eq!(app.sidebar.host.value, "XabcY");

        app.move_cursor(-2);
        app.insert_char('Z');
        assert_eq!(app.sidebar.host.value, "XabZcY");
    }

    #[test]
    fn port_field_accepts_only_digits() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.focus = Focus::SidebarPort;
        app.sidebar.port = FieldState::new("");

        app.insert_char('1');
        app.insert_char('a');
        app.insert_char('2');
        assert_eq!(app.sidebar.port.value, "12");
    }

    #[test]
    fn field_editing_is_unicode_safe() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.focus = Focus::SidebarPasscode;
        app.sidebar.passcode = FieldState::new("");

        // Emoji are multi-byte; cursor must advance by whole characters.
        app.insert_char('🔑');
        app.insert_char('é');
        app.insert_char('X');
        assert_eq!(app.sidebar.passcode.value, "🔑éX");
        assert_eq!(app.sidebar.passcode.cursor, 7);

        // Left moves one whole character (back over X).
        app.move_cursor(-1);
        assert_eq!(app.sidebar.passcode.cursor, 6);

        // Backspace removes the preceding whole character (é), not a byte.
        app.backspace();
        assert_eq!(app.sidebar.passcode.value, "🔑X");
        assert_eq!(app.sidebar.passcode.cursor, 4);

        // Delete removes the character under the cursor (X).
        app.delete_char();
        assert_eq!(app.sidebar.passcode.value, "🔑");
        assert_eq!(app.sidebar.passcode.cursor, 4);
    }

    #[test]
    fn invalid_port_logs_error_and_stays_idle() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.sidebar.port.value = "not-a-port".into();

        assert_eq!(app.sidebar.port_u16(), None);

        app.start_connect_from_sidebar();
        assert_eq!(app.state, ConnectionState::Idle);
        assert!(
            app.event_log
                .iter()
                .any(|e| e.message.contains("Invalid port")),
            "should log invalid port error"
        );

        app.start_listen_from_sidebar();
        assert_eq!(app.state, ConnectionState::Idle);
    }

    // ── Event formatting ───────────────────────────────────────────────

    #[test]
    fn format_protocol_event() {
        let event = DebuggeeEvent::Protocol {
            version: 9,
            plugins: vec![PluginDetails {
                name: "Test".into(),
                module_uuid: "uuid".into(),
            }],
            require_passcode: true,
        };
        let s = format_debuggee_event(&event);
        assert_eq!(s, "Protocol v9, 1 plugin(s) (passcode required)");
    }

    #[test]
    fn format_stopped_event() {
        let event = DebuggeeEvent::Stopped {
            reason: "breakpoint".into(),
            thread: 0,
        };
        let s = format_debuggee_event(&event);
        assert_eq!(s, "Stopped: breakpoint (thread 0)");
    }

    #[test]
    fn format_print_event() {
        let event = DebuggeeEvent::Print {
            message: "hello".into(),
            log_level: LogLevel::Warn,
        };
        let s = format_debuggee_event(&event);
        assert_eq!(s, "[WARN] hello");
    }

    #[test]
    fn format_notification_event() {
        let event = DebuggeeEvent::Notification {
            message: "notify!".into(),
            log_level: LogLevel::Error,
        };
        let s = format_debuggee_event(&event);
        assert_eq!(s, "[ERROR] notify!");
    }

    #[test]
    fn format_stat2_event() {
        use mc_protocol::events::StatDataModel;
        let event = DebuggeeEvent::Stat2 {
            tick: 42,
            stats: vec![StatDataModel {
                name: "cpu".into(),
                children: vec![],
                values: vec![serde_json::json!(1.0)],
                should_aggregate: false,
            }],
        };
        let s = format_debuggee_event(&event);
        assert_eq!(s, "Stats tick=42 (1 value(s))");
    }

    #[test]
    fn format_terminated_event() {
        let event = DebuggeeEvent::Terminated {
            reason: Some("game over".into()),
        };
        let s = format_debuggee_event(&event);
        assert_eq!(s, "Terminated: game over");
    }

    #[test]
    fn format_terminated_no_reason() {
        let event = DebuggeeEvent::Terminated { reason: None };
        let s = format_debuggee_event(&event);
        assert_eq!(s, "Terminated");
    }

    #[test]
    fn format_debuggee_response() {
        let event = DebuggeeEvent::DebuggeeResponse {
            request_seq: 5,
            args: None,
            success: Some(true),
            response_message: None,
        };
        let s = format_debuggee_event(&event);
        assert_eq!(s, "DebuggeeResponse seq=5 success=true");
    }

    #[test]
    fn format_response() {
        let event = DebuggeeEvent::Response {
            request_seq: 3,
            command: Some("next".into()),
            success: Some(true),
            body: None,
            error: None,
        };
        let s = format_debuggee_event(&event);
        assert_eq!(s, "Response seq=3 [next] ok");
    }

    #[test]
    fn format_response_with_error() {
        let event = DebuggeeEvent::Response {
            request_seq: 4,
            command: None,
            success: Some(false),
            body: None,
            error: Some("not found".into()),
        };
        let s = format_debuggee_event(&event);
        assert_eq!(s, "Response seq=4 [?] error: not found");
    }

    #[test]
    fn format_schema_event() {
        use mc_protocol::events::DiagnosticsTabDescriptor;
        let event = DebuggeeEvent::Schema {
            descriptors: vec![DiagnosticsTabDescriptor {
                name: "Timing".into(),
                stat_group_id: "timing".into(),
                data_source: mc_protocol::events::DiagnosticsDataSource::Server,
                display_type: mc_protocol::events::DiagnosticsDisplayType::LineChart,
                title: None,
                y_label: None,
                tick_range: None,
                value_scalar: None,
                target_value: None,
                key_label: None,
                value_labels: None,
                statistic_id: None,
                statistic_ids: None,
            }],
        };
        let s = format_debuggee_event(&event);
        assert_eq!(s, "Schema: 1 descriptor(s)");
    }

    #[test]
    fn format_profiler_capture() {
        let event = DebuggeeEvent::ProfilerCapture {
            capture_base_path: "/tmp/cap".into(),
            capture_data: "binary".into(),
        };
        let s = format_debuggee_event(&event);
        assert_eq!(s, "Profiler capture: /tmp/cap");
    }

    #[test]
    fn format_unknown() {
        let event = DebuggeeEvent::Unknown {
            type_name: "FooBar".into(),
            data: serde_json::json!({}),
        };
        let s = format_debuggee_event(&event);
        assert_eq!(s, "Unknown event: FooBar");
    }

    // ── Log navigation ─────────────────────────────────────────────────

    #[test]
    fn log_follow_auto_scrolls_on_new_events() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        for i in 0..5 {
            app.add_log(format!("entry {i}"));
        }

        assert_eq!(app.log_state.selected, Some(4));
        assert!(app.log_state.follow);

        app.log_state.scroll(-2, app.event_log.len());
        assert_eq!(app.log_state.selected, Some(2));
        assert!(!app.log_state.follow);

        // While not following, new events do not steal the viewport.
        app.add_log("new entry");
        assert_eq!(app.log_state.selected, Some(2));
        assert!(!app.log_state.follow);

        // Re-enabling follow jumps to the newest entry.
        app.log_state.bottom(app.event_log.len());
        assert_eq!(app.log_state.selected, Some(5));
        assert!(app.log_state.follow);
    }

    #[test]
    fn log_top_bottom_keys() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        for i in 0..10 {
            app.add_log(format!("entry {i}"));
        }

        app.log_state.top();
        assert_eq!(app.log_state.selected, Some(0));
        assert!(!app.log_state.follow);

        app.log_state.bottom(app.event_log.len());
        assert_eq!(app.log_state.selected, Some(9));
        assert!(app.log_state.follow);
    }

    #[test]
    fn log_offset_keeps_selection_visible() {
        let mut state = LogState::default();
        state.bottom(100);
        state.update_offset(10, 100);
        assert_eq!(state.offset, 90);

        state.scroll(-40, 100);
        state.update_offset(10, 100);
        assert_eq!(state.selected, Some(59));
        assert_eq!(state.offset, 59);
    }

    // ── Session event handling ─────────────────────────────────────────

    #[test]
    fn target_selection_opens_popup() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        app.handle_session_event(SessionEvent::TargetSelectionRequired {
            plugins: vec![
                PluginDetails {
                    name: "A".into(),
                    module_uuid: "a".into(),
                },
                PluginDetails {
                    name: "B".into(),
                    module_uuid: "b".into(),
                },
            ],
        });

        assert!(app.plugin_selection.is_some());
        assert_eq!(app.plugin_selection.as_ref().unwrap().plugins.len(), 2);
        assert_eq!(app.plugin_selection.as_ref().unwrap().selected, 0);
    }

    #[test]
    fn disconnected_event_updates_state() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        app.handle_session_event(SessionEvent::Disconnected);
        assert_eq!(app.state, ConnectionState::Disconnected);
    }

    #[test]
    fn terminated_event_updates_state() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        app.handle_session_event(SessionEvent::Terminated {
            reason: Some("done".into()),
        });
        assert_eq!(app.state, ConnectionState::Disconnected);
    }

    #[test]
    fn debuggee_event_adds_log_entry() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Print {
            message: "test".into(),
            log_level: LogLevel::Log,
        }));

        assert_eq!(app.event_log.len(), 1);
        assert!(app.event_log[0].message.contains("test"));
        assert_eq!(app.event_log[0].kind, LogKind::Print);
    }

    #[test]
    fn stopped_event_sets_stopped_state() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stopped {
            reason: "breakpoint".into(),
            thread: 0,
        }));

        assert!(app.stopped);
        assert_eq!(app.stop_reason, "breakpoint");
        assert_eq!(app.stopped_thread_id, Some(0));
    }

    #[test]
    fn thread_exited_matching_thread_clears_stopped() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stopped {
            reason: "breakpoint".into(),
            thread: 7,
        }));
        assert!(app.stopped);

        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Thread {
            reason: "exited".into(),
            thread: 7,
        }));

        assert!(!app.stopped);
        assert!(app.stopped_thread_id.is_none());
    }

    #[test]
    fn thread_exited_other_thread_keeps_stopped() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stopped {
            reason: "breakpoint".into(),
            thread: 7,
        }));

        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Thread {
            reason: "exited".into(),
            thread: 3,
        }));

        assert!(app.stopped);
        assert_eq!(app.stopped_thread_id, Some(7));
    }

    #[tokio::test]
    async fn new_connection_clears_stopped() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stopped {
            reason: "breakpoint".into(),
            thread: 0,
        }));
        app.start_connect("127.0.0.1".into(), 19144, None, None);
        assert!(!app.stopped);
        assert!(app.stopped_thread_id.is_none());
    }

    #[test]
    fn control_enablement_reflects_connection_and_stopped_state() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        assert!(!app.can_pause());
        assert!(!app.can_continue());
        assert!(!app.can_step());

        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 1,
        };
        assert!(app.can_pause());
        assert!(!app.can_continue());
        assert!(!app.can_step());

        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stopped {
            reason: "breakpoint".into(),
            thread: 0,
        }));
        assert!(!app.can_pause());
        assert!(app.can_continue());
        assert!(app.can_step());

        app.busy = true;
        assert!(!app.can_continue());
        assert!(!app.can_step());

        app.busy = false;
        app.evaluate_input.busy = true;
        assert!(app.is_debug_busy());
        assert!(!app.can_pause());
        assert!(!app.can_continue());
        assert!(!app.can_step());
    }

    #[test]
    fn command_history_dedup_and_bounds() {
        let mut input = CommandInput::new();
        input.push_history("say hi".into());
        input.push_history("kill @e".into());
        input.push_history("say hi".into());
        input.push_history("time set day".into());

        assert_eq!(input.history.len(), 3);
        assert_eq!(input.history[0], "time set day");
        assert_eq!(input.history[1], "say hi");
        assert_eq!(input.history[2], "kill @e");

        for i in 0..12 {
            input.push_history(format!("cmd{i}"));
        }
        assert_eq!(input.history.len(), MAX_COMMAND_HISTORY);
    }

    #[test]
    fn command_history_navigation() {
        let mut input = CommandInput::new();
        input.push_history("oldest".into());
        input.push_history("newest".into());

        input.cycle_history(true);
        assert_eq!(input.field.value, "newest");
        input.cycle_history(true);
        assert_eq!(input.field.value, "oldest");
        input.cycle_history(false);
        assert_eq!(input.field.value, "newest");
        input.cycle_history(false);
        assert!(input.field.value.is_empty());
    }

    #[test]
    fn evaluate_history_bounds() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        for i in 0..15 {
            app.evaluate_input.pending_expression = format!("expr{i}");
            let result = Ok(mc_session::EvaluateResult {
                success: true,
                args: Some(serde_json::json!(i)),
                message: None,
            });
            app.evaluate_input.finish(result);
        }

        assert_eq!(app.evaluate_input.history.len(), MAX_EVAL_HISTORY);
        // Most recent submissions are at the front.
        assert_eq!(app.evaluate_input.history[0].expression, "expr14");
    }

    #[test]
    fn session_reset_drops_pending_evaluate_and_preserves_history() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.evaluate_input.pending_expression = "completed".into();
        app.evaluate_input.finish(Ok(mc_session::EvaluateResult {
            success: true,
            args: None,
            message: Some("ok".into()),
        }));
        let history_len = app.evaluate_input.history.len();

        let (_tx, result_rx) = tokio::sync::oneshot::channel();
        app.evaluate_input.start("stale".into(), result_rx);
        app.handshake = Some(mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![],
            require_passcode: false,
        });
        app.stopped = true;
        app.stopped_thread_id = Some(7);

        app.clear_session_transients();

        assert!(app.handshake.is_none());
        assert!(!app.stopped);
        assert!(app.stopped_thread_id.is_none());
        assert!(!app.evaluate_input.busy);
        assert!(app.evaluate_input.result_rx.is_none());
        assert!(app.evaluate_input.pending_expression.is_empty());
        assert_eq!(app.evaluate_input.history.len(), history_len);
    }

    #[test]
    fn closed_evaluate_receiver_records_and_logs_failure() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        let (tx, result_rx) = tokio::sync::oneshot::channel();
        app.evaluate_input.start("1 + 1".into(), result_rx);
        drop(tx);

        app.try_recv_evaluate();

        assert!(!app.evaluate_input.busy);
        assert_eq!(app.evaluate_input.history.len(), 1);
        assert!(!app.evaluate_input.history[0].success);
        assert!(app
            .event_log
            .iter()
            .any(|entry| entry.message.contains("Evaluate failed: 1 + 1")));
    }

    // ── Lifecycle: manual vs. event-driven disconnect ──────────────────

    #[tokio::test]
    async fn manual_disconnect_from_connected_goes_to_idle() {
        // Manual disconnect ('x') transitions Connected → Idle so the user
        // can immediately reconnect with l/c.
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 2,
        };

        // Simulate the synchronous portion of handle_key for 'x'
        app.state = ConnectionState::Idle;
        app.add_log("Disconnected.");

        assert_eq!(app.state, ConnectionState::Idle);
        // User can immediately start a new connection after manual
        // disconnect because state is Idle.
        app.start_connect("10.0.0.1".into(), 19144, None, None);
        assert_eq!(app.state, ConnectionState::Pending);
    }

    #[test]
    fn session_event_disconnect_stays_disconnected() {
        // An unexpected SessionEvent::Disconnected leaves the state as
        // Disconnected (not Idle), distinguishing a session drop from a
        // user-initiated disconnect.
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 2,
        };
        app.handshake = Some(mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![],
            require_passcode: false,
        });

        app.handle_session_event(SessionEvent::Disconnected);
        assert_eq!(app.state, ConnectionState::Disconnected);
        assert!(app.handshake.is_none());

        // Terminated also transitions to Disconnected
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 2,
        };
        app.handshake = Some(mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![],
            require_passcode: false,
        });
        app.handle_session_event(SessionEvent::Terminated { reason: None });
        assert_eq!(app.state, ConnectionState::Disconnected);
        assert!(app.handshake.is_none());
    }

    #[tokio::test]
    async fn manual_disconnect_allows_reconnect() {
        // After manual disconnect (Connected → Idle) the user can
        // immediately start a connect or listen.
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 2,
        };

        // Manual disconnect → Idle
        app.state = ConnectionState::Idle;
        assert_eq!(app.state, ConnectionState::Idle);

        // Can start connect
        app.start_connect("127.0.0.1".into(), 19144, None, None);
        assert_eq!(app.state, ConnectionState::Pending);

        // Can also start listen (separate app for clean state)
        let (ctrl2, rx2) = SessionController::new();
        let mut app2 = App::new(ctrl2, rx2, "127.0.0.1".into(), 19144, None, None);
        app2.state = ConnectionState::Connected {
            version: 9,
            plugin_count: 1,
        };
        app2.state = ConnectionState::Idle; // manual disconnect
        app2.start_listen(19144, None, None);
        assert_eq!(app2.state, ConnectionState::Pending);
    }

    // ── Navigation / edit helpers ────────────────────────────────────────

    #[test]
    fn tab_switching_changes_active_tab() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        app.set_tab(Tab::Stats);
        assert_eq!(app.tab, Tab::Stats);

        app.set_tab(Tab::Log);
        assert_eq!(app.tab, Tab::Log);
    }

    #[test]
    fn help_popup_toggles() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        assert_eq!(app.help_popup, HelpPopup::Hidden);
        app.toggle_help();
        assert_eq!(app.help_popup, HelpPopup::Visible);
        app.toggle_help();
        assert_eq!(app.help_popup, HelpPopup::Hidden);
    }

    #[test]
    fn compact_mode_detected_by_size() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        app.set_last_known_size(120, 30);
        assert!(!app.compact());

        app.set_last_known_size(60, 24);
        assert!(app.compact());

        app.set_last_known_size(80, 18);
        assert!(app.compact());
    }

    #[test]
    fn sidebar_visible_in_wide_or_overlay() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        app.set_last_known_size(80, 24);
        assert!(!app.is_sidebar_visible());

        app.toggle_sidebar();
        assert!(app.is_sidebar_visible());
        app.close_sidebar();
        assert!(!app.is_sidebar_visible());

        app.set_last_known_size(120, 30);
        assert!(app.is_sidebar_visible());
    }

    #[test]
    fn scroll_log_methods_update_state() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        for i in 0..20 {
            app.add_log(format!("entry {i}"));
        }

        app.scroll_log_top();
        assert_eq!(app.log_state.selected, Some(0));
        assert!(!app.log_state.follow);

        app.scroll_log_bottom();
        assert_eq!(app.log_state.selected, Some(19));
        assert!(app.log_state.follow);

        app.scroll_log_up(5);
        assert_eq!(app.log_state.selected, Some(14));

        app.scroll_log_down(3);
        assert_eq!(app.log_state.selected, Some(17));

        app.scroll_log_page_up(10);
        assert_eq!(app.log_state.selected, Some(7));

        app.scroll_log_page_down(10);
        assert_eq!(app.log_state.selected, Some(17));
    }

    #[test]
    fn advanced_toggle_shows_hidden_fields() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.set_last_known_size(120, 30);

        assert!(!app.sidebar.advanced_open);
        let targets_before = app.visible_focus_targets();
        assert!(!targets_before.contains(&Focus::SidebarTargetUuid));

        app.toggle_advanced();
        assert!(app.sidebar.advanced_open);
        let targets_after = app.visible_focus_targets();
        assert!(targets_after.contains(&Focus::SidebarTargetUuid));
        assert!(targets_after.contains(&Focus::SidebarPasscode));
    }

    // ── Cancel-pending reconciliation ─────────────────────────────────

    #[tokio::test]
    async fn cancel_pending_reconciles_buffered_ok() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl.clone(), rx, "127.0.0.1".into(), 19144, None, None);

        let (tx, result_rx) = tokio::sync::oneshot::channel();
        tx.send(Ok(mc_session::HandshakeInfo {
            version: 9,
            plugins: vec![],
            require_passcode: false,
        }))
        .unwrap();
        app.conn_result_rx = Some(result_rx);
        app.state = ConnectionState::Pending;

        app.cancel_pending_with_result().await;

        assert_eq!(app.state, ConnectionState::Idle);
        // Should have logged connection and then disconnection
        assert!(
            app.event_log
                .iter()
                .any(|e| e.message.contains("Connected")),
            "should log connection from buffered result"
        );
        assert!(
            app.event_log
                .iter()
                .any(|e| e.message.contains("Disconnected")),
            "should log disconnection after cancel with buffered ok"
        );
    }

    #[tokio::test]
    async fn cancel_pending_empty_receiver() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(
            ctrl,
            rx,
            "127.0.0.1".into(),
            19144,
            Some("old-target".into()),
            Some("old-pass".into()),
        );

        let (_tx, result_rx) = tokio::sync::oneshot::channel::<
            Result<mc_session::HandshakeInfo, mc_session::SessionError>,
        >();
        app.conn_result_rx = Some(result_rx);
        app.pending_target_uuid = Some("attempt-target".into());
        app.pending_passcode = Some("attempt-pass".into());
        app.state = ConnectionState::Pending;

        app.cancel_pending_with_result().await;

        assert_eq!(app.state, ConnectionState::Idle);
        assert!(app.pending_target_uuid.is_none());
        assert!(app.pending_passcode.is_none());
        assert_eq!(app.persisted_target_uuid.as_deref(), Some("old-target"));
        assert_eq!(app.persisted_passcode.as_deref(), Some("old-pass"));
        assert!(
            app.event_log
                .iter()
                .any(|e| e.message.contains("cancelled")),
            "should log cancellation"
        );
    }

    #[tokio::test]
    async fn cancel_pending_closed_receiver() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        // Dropping the sender closes the channel without sending
        let (tx, result_rx) = tokio::sync::oneshot::channel::<
            Result<mc_session::HandshakeInfo, mc_session::SessionError>,
        >();
        drop(tx);
        app.pending_target_uuid = Some("attempt-target".into());
        app.pending_passcode = Some("attempt-pass".into());
        app.conn_result_rx = Some(result_rx);
        app.state = ConnectionState::Pending;

        app.cancel_pending_with_result().await;

        assert_eq!(app.state, ConnectionState::Idle);
        assert!(app.pending_target_uuid.is_none());
        assert!(app.pending_passcode.is_none());
        assert!(
            app.event_log
                .iter()
                .any(|e| e.message.contains("unexpected")),
            "should log unexpected-task error"
        );
    }

    #[tokio::test]
    async fn cancel_pending_without_receiver_clears_snapshots() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(
            ctrl,
            rx,
            "127.0.0.1".into(),
            19144,
            Some("old-target".into()),
            Some("old-pass".into()),
        );
        app.pending_target_uuid = Some("attempt-target".into());
        app.pending_passcode = Some("attempt-pass".into());
        app.state = ConnectionState::Pending;

        app.cancel_pending_with_result().await;

        assert!(app.pending_target_uuid.is_none());
        assert!(app.pending_passcode.is_none());
        assert_eq!(app.persisted_target_uuid.as_deref(), Some("old-target"));
        assert_eq!(app.persisted_passcode.as_deref(), Some("old-pass"));
    }

    // ── Phase 4: search / filter / reset ───────────────────────────────

    #[test]
    fn log_filter_allows_by_kind() {
        let mut filter = LogFilterState::show_all();
        let entry = LogEntry::event(LogKind::Print, "hello");
        assert!(filter.allows(&entry));

        filter.kinds.print = false;
        assert!(!filter.allows(&entry));
    }

    #[test]
    fn log_filter_allows_by_level() {
        let mut filter = LogFilterState::show_all();
        let entry = LogEntry::event_with_level(LogKind::Print, "hello", Some(LogLevel::Warn));
        assert!(filter.allows(&entry));

        filter.levels.warn = false;
        assert!(!filter.allows(&entry));
    }

    #[test]
    fn log_filter_allows_by_search() {
        let mut filter = LogFilterState::show_all();
        let entry = LogEntry::event(LogKind::Stopped, "breakpoint hit");
        assert!(filter.allows(&entry));

        filter.search = "breakpoint".into();
        assert!(filter.allows(&entry));

        filter.search = "missing".into();
        assert!(!filter.allows(&entry));
    }

    #[test]
    fn log_filter_search_matches_kind_label() {
        let mut filter = LogFilterState::show_all();
        filter.search = "stopped".into();
        let entry = LogEntry::event(LogKind::Stopped, " unrelated ");
        assert!(filter.allows(&entry));
    }

    #[test]
    fn filter_identity_when_all_enabled_and_empty_search() {
        let filter = LogFilterState::show_all();
        assert!(filter.is_identity());

        let mut filter = LogFilterState::show_all();
        filter.search = "x".into();
        assert!(!filter.is_identity());
    }

    #[test]
    fn filtered_indices_follow_kind_filter() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.add_log("system one");
        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Print {
            message: "print one".into(),
            log_level: LogLevel::Log,
        }));
        app.add_log("system two");

        assert_eq!(app.filtered_log_indices.len(), 3);

        app.log_filter.kinds.print = false;
        app.rebuild_filtered_log_indices();

        assert_eq!(app.filtered_log_indices.len(), 2);
        assert_eq!(
            app.event_log[app.filtered_log_indices[0]].message,
            "system one"
        );
        assert_eq!(
            app.event_log[app.filtered_log_indices[1]].message,
            "system two"
        );
        // Raw log is untouched.
        assert_eq!(app.event_log.len(), 3);
    }

    #[test]
    fn filtered_indices_follow_level_filter() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Print {
            message: "log".into(),
            log_level: LogLevel::Log,
        }));
        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Print {
            message: "warn".into(),
            log_level: LogLevel::Warn,
        }));
        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Print {
            message: "error".into(),
            log_level: LogLevel::Error,
        }));

        app.log_filter.levels.log = false;
        app.log_filter.levels.warn = false;
        app.rebuild_filtered_log_indices();

        assert_eq!(app.filtered_log_indices.len(), 1);
        assert!(app.event_log[app.filtered_log_indices[0]]
            .message
            .contains("error"));
        assert_eq!(app.event_log.len(), 3);
    }

    #[test]
    fn filtered_indices_follow_search() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.add_log("alpha");
        app.add_log("beta");
        app.add_log("gamma");

        app.log_filter.search = "beta".into();
        app.rebuild_filtered_log_indices();

        assert_eq!(app.filtered_log_indices.len(), 1);
        assert_eq!(app.event_log[app.filtered_log_indices[0]].message, "beta");
    }

    #[test]
    fn reset_filters_restores_full_view() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.add_log("alpha");
        app.add_log("beta");

        app.log_filter.search = "beta".into();
        app.log_filter.kinds.system = false;
        app.rebuild_filtered_log_indices();
        assert_eq!(app.filtered_log_indices.len(), 0);

        app.reset_filters();
        assert!(app.log_filter.is_identity());
        assert_eq!(app.filtered_log_indices.len(), 2);
        assert_eq!(app.event_log.len(), 2);
    }

    #[test]
    fn follow_jumps_to_new_filtered_event() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.add_log("first");
        app.add_log("second");
        app.log_state.scroll(-1, app.filtered_log_indices.len());
        assert!(!app.log_state.follow);
        assert_eq!(app.log_state.selected, Some(0));

        app.add_log("third");
        assert!(!app.log_state.follow);
        assert_eq!(app.log_state.selected, Some(0));

        app.log_state.bottom(app.filtered_log_indices.len());
        app.add_log("fourth");
        assert!(app.log_state.follow);
        assert_eq!(app.log_state.selected, Some(3));
    }

    #[test]
    fn ring_buffer_pop_keeps_filtered_indices_consistent() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        for i in 0..MAX_LOG_ENTRIES {
            app.add_log(format!("entry {i}"));
        }
        let first_filtered = app.filtered_log_indices[0];
        assert_eq!(app.event_log[first_filtered].message, "entry 0");

        app.add_log("newest");
        // The front entry was dropped, so the filtered index that used to
        // point to entry 0 now points to entry 1.
        let first_filtered = app.filtered_log_indices[0];
        assert_eq!(app.event_log[first_filtered].message, "entry 1");
    }

    #[test]
    fn filter_popup_toggle_changes_state() {
        let mut popup = FilterPopup::new();
        let mut filter = LogFilterState::show_all();
        popup.open();
        assert_eq!(popup.selected, 0);

        popup.scroll_down();
        assert_eq!(popup.selected, 1);

        let toggled = popup.toggle_selected(&mut filter);
        assert_eq!(
            toggled,
            Some(crate::app::FilterToggle::Kind(LogKind::Protocol, false))
        );
        assert!(!filter.kinds.protocol);
    }

    #[test]
    fn filter_popup_can_toggle_levels() {
        let mut popup = FilterPopup::new();
        let mut filter = LogFilterState::show_all();
        popup.open();
        popup.selected = LogKind::ALL.len() + 1; // Warn

        popup.toggle_selected(&mut filter);
        assert!(!filter.levels.warn);
    }

    #[test]
    fn search_input_open_close() {
        let mut input = SearchInput::new();
        assert!(!input.open);
        input.open();
        assert!(input.open);
        input.field.insert('x');
        input.close();
        assert!(!input.open);
        input.reset_transient();
        assert!(input.field.value.is_empty());
    }

    // ── Config integration ──────────────────────────────────────────────

    #[test]
    fn apply_config_sets_filter_state() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        let cfg = crate::config::AppConfig {
            known_plugins: vec![],
            last_target_uuid: None,
            passcode: None,
            search: "test-filter".into(),
            kinds: LogKindFilter {
                print: false,
                ..LogKindFilter::all_enabled()
            },
            levels: LogLevelFilter {
                warn: false,
                ..LogLevelFilter::all_enabled()
            },
        };

        app.apply_config(&cfg);

        assert_eq!(app.log_filter.search, "test-filter");
        assert!(!app.log_filter.kinds.print);
        assert!(app.log_filter.kinds.system);
        assert!(!app.log_filter.levels.warn);
        assert!(app.log_filter.levels.log);
    }

    #[test]
    fn apply_config_updates_sidebar_target_and_passcode() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        // Should be empty initially (no CLI overrides)
        assert!(app.sidebar.target_uuid.value.is_empty());
        assert!(app.sidebar.passcode.value.is_empty());

        let cfg = crate::config::AppConfig {
            known_plugins: vec![],
            last_target_uuid: Some("uuid-from-config".into()),
            passcode: Some("pass-from-config".into()),
            search: String::new(),
            kinds: LogKindFilter::all_enabled(),
            levels: LogLevelFilter::all_enabled(),
        };

        app.apply_config(&cfg);

        assert_eq!(app.sidebar.target_uuid.value, "uuid-from-config");
        assert_eq!(app.sidebar.passcode.value, "pass-from-config");
    }

    #[test]
    fn apply_config_does_not_override_cli_values() {
        let (ctrl, rx) = SessionController::new();
        // CLI provides target_uuid and passcode
        let mut app = App::new(
            ctrl,
            rx,
            "127.0.0.1".into(),
            19144,
            Some("cli-uuid".into()),
            Some("cli-pass".into()),
        );

        assert_eq!(app.sidebar.target_uuid.value, "cli-uuid");
        assert_eq!(app.sidebar.passcode.value, "cli-pass");

        let cfg = crate::config::AppConfig {
            known_plugins: vec![],
            last_target_uuid: Some("uuid-from-config".into()),
            passcode: Some("pass-from-config".into()),
            search: String::new(),
            kinds: LogKindFilter::all_enabled(),
            levels: LogLevelFilter::all_enabled(),
        };

        app.apply_config(&cfg);

        // CLI values should persist (they were passed as defaults to SidebarForm::new)
        assert_eq!(
            app.sidebar.target_uuid.value, "cli-uuid",
            "CLI target uuid should not be overridden by config"
        );
        assert_eq!(
            app.sidebar.passcode.value, "cli-pass",
            "CLI passcode should not be overridden by config"
        );
    }

    #[test]
    fn current_filter_state_matches_log_filter() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        app.log_filter.search = "capture".into();
        app.log_filter.kinds.protocol = false;
        app.log_filter.levels.error = false;

        let (search, kinds, levels) = app.current_filter_state();
        assert_eq!(search, "capture");
        assert!(!kinds.protocol);
        assert!(kinds.system);
        assert!(!levels.error);
        assert!(levels.log);
    }

    #[test]
    fn apply_config_rebuilds_filtered_indices() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        app.add_log("alpha");
        app.add_log("beta");

        assert_eq!(app.filtered_log_indices.len(), 2);

        let cfg = crate::config::AppConfig {
            known_plugins: vec![],
            last_target_uuid: None,
            passcode: None,
            search: "beta".into(),
            kinds: LogKindFilter::all_enabled(),
            levels: LogLevelFilter::all_enabled(),
        };

        app.apply_config(&cfg);

        assert_eq!(app.filtered_log_indices.len(), 1);
        assert_eq!(app.event_log[app.filtered_log_indices[0]].message, "beta");
    }

    // ── Phase 5: Stats integration ───────────────────────────────────────

    #[test]
    fn stat2_both_logs_and_accumulates() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        use mc_protocol::events::StatDataModel;
        let stats_data = vec![StatDataModel {
            name: "cpu".into(),
            children: vec![],
            values: vec![serde_json::json!(42.0)],
            should_aggregate: false,
        }];

        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stat2 {
            tick: 1,
            stats: stats_data,
        }));

        // Log entry is created
        assert_eq!(app.event_log.len(), 1);
        assert_eq!(app.event_log[0].kind, LogKind::Stat);
        assert!(app.event_log[0].message.contains("Stats tick=1"));

        // Stats are accumulated
        assert!(!app.stats.is_empty());
        assert_eq!(app.stats.series_count(), 1);
        assert_eq!(app.stats.collection().get("cpu").unwrap().values[0], 42.0);
    }

    #[test]
    fn stat2_accumulates_unconditionally_even_when_stat_log_filtered() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        use mc_protocol::events::StatDataModel;

        // Filter out Stat log entries
        app.log_filter.kinds.stat = false;
        app.rebuild_filtered_log_indices();

        let stats_data = vec![StatDataModel {
            name: "mem".into(),
            children: vec![],
            values: vec![serde_json::json!(100.0)],
            should_aggregate: false,
        }];

        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stat2 {
            tick: 1,
            stats: stats_data,
        }));

        // Log entry exists in raw log but is hidden from filtered view
        assert_eq!(app.event_log.len(), 1);
        assert_eq!(app.filtered_log_indices.len(), 0);

        // Stats are accumulated regardless of log filtering
        assert!(!app.stats.is_empty());
        assert_eq!(app.stats.series_count(), 1);
        assert_eq!(app.stats.collection().get("mem").unwrap().values[0], 100.0);
    }

    #[test]
    fn clear_stats_clears_only_stats() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        use mc_protocol::events::StatDataModel;

        // Add a log entry manually
        app.add_log("before");

        // Send a Stat2 event to populate stats
        let stats_data = vec![StatDataModel {
            name: "cpu".into(),
            children: vec![],
            values: vec![serde_json::json!(50.0)],
            should_aggregate: false,
        }];
        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stat2 {
            tick: 1,
            stats: stats_data,
        }));

        assert!(!app.stats.is_empty());
        assert_eq!(app.event_log.len(), 2);

        // Clear only stats
        app.clear_stats();

        assert!(app.stats.is_empty());
        // Event log is untouched
        assert_eq!(app.event_log.len(), 2);
        assert_eq!(app.event_log[0].message, "before");
        // Filter state, session transients, etc. are untouched
        assert!(app.log_filter.is_identity());
    }

    #[test]
    fn stat2_repeated_ticks_accumulate_multiple_points() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        use mc_protocol::events::StatDataModel;
        let stats_data = || -> Vec<StatDataModel> {
            vec![StatDataModel {
                name: "cpu".into(),
                children: vec![],
                values: vec![serde_json::json!(42.0)],
                should_aggregate: false,
            }]
        };

        for tick in 1..=5 {
            app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stat2 {
                tick,
                stats: stats_data(),
            }));
        }

        assert_eq!(app.stats.series_count(), 1);
        let series = &app.stats.collection()["cpu"];
        assert_eq!(series.ticks.len(), 5);
        assert_eq!(series.ticks, vec![1, 2, 3, 4, 5]);
        assert_eq!(series.values, vec![42.0, 42.0, 42.0, 42.0, 42.0]);
    }

    #[test]
    fn reset_filters_does_not_clear_stats() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        use mc_protocol::events::StatDataModel;
        let stats_data = vec![StatDataModel {
            name: "cpu".into(),
            children: vec![],
            values: vec![serde_json::json!(42.0)],
            should_aggregate: false,
        }];
        app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stat2 {
            tick: 1,
            stats: stats_data,
        }));

        assert!(!app.stats.is_empty());

        // reset_filters must not touch stats
        app.reset_filters();
        assert!(!app.stats.is_empty());
        assert_eq!(app.stats.series_count(), 1);

        // clear_session_transients must not touch stats
        app.clear_session_transients();
        assert!(!app.stats.is_empty());
        assert_eq!(app.stats.series_count(), 1);
    }

    // ── Phase 5: stats dashboard selection / scrolling ───────────────────

    fn make_stat_model(
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

    #[test]
    fn stats_selection_reconciles_to_first_category() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);
        assert!(app.stats_selected_category.is_none());

        let stats = vec![make_stat_model(
            "server_tick_timings",
            vec![],
            vec![make_stat_model(
                "tick",
                vec![serde_json::json!(16.0)],
                vec![],
            )],
        )];
        app.stats.accumulate(&stats, 1);

        app.reconcile_stats_selection();
        assert_eq!(
            app.stats_selected_category.as_deref(),
            Some("server-performance")
        );
    }

    #[test]
    fn stats_category_navigation_wraps() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        let stats = vec![
            make_stat_model(
                "server_tick_timings",
                vec![],
                vec![make_stat_model(
                    "tick",
                    vec![serde_json::json!(16.0)],
                    vec![],
                )],
            ),
            make_stat_model(
                "app_memory",
                vec![],
                vec![make_stat_model(
                    "used",
                    vec![serde_json::json!(1_048_576.0)],
                    vec![],
                )],
            ),
        ];
        app.stats.accumulate(&stats, 1);
        app.reconcile_stats_selection();
        assert_eq!(
            app.stats_selected_category.as_deref(),
            Some("server-performance")
        );

        app.select_next_category();
        assert_eq!(app.stats_selected_category.as_deref(), Some("memory"));

        app.select_next_category();
        // Only server-performance and memory have groups, so it wraps around.
        assert_eq!(
            app.stats_selected_category.as_deref(),
            Some("server-performance")
        );

        app.select_prev_category();
        assert_eq!(app.stats_selected_category.as_deref(), Some("memory"));
    }

    #[test]
    fn stats_client_navigation_wraps_and_filters() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        let stats = vec![make_stat_model(
            "client_stats",
            vec![],
            vec![
                make_stat_model(
                    "client-b",
                    vec![],
                    vec![make_stat_model(
                        "cpu",
                        vec![serde_json::json!(20.0)],
                        vec![],
                    )],
                ),
                make_stat_model(
                    "client-a",
                    vec![],
                    vec![make_stat_model(
                        "cpu",
                        vec![serde_json::json!(10.0)],
                        vec![],
                    )],
                ),
            ],
        )];
        app.stats.accumulate(&stats, 1);
        app.stats_selected_category = Some("client".into());
        app.reconcile_stats_selection();

        assert_eq!(app.stats_selected_category.as_deref(), Some("client"));
        // First sorted client is selected.
        assert_eq!(app.stats_selected_client.as_deref(), Some("client-a"));

        app.select_next_client();
        assert_eq!(app.stats_selected_client.as_deref(), Some("client-b"));

        app.select_next_client();
        assert_eq!(app.stats_selected_client.as_deref(), Some("client-a"));

        app.select_prev_client();
        assert_eq!(app.stats_selected_client.as_deref(), Some("client-b"));
    }

    #[test]
    fn stats_client_selector_hidden_with_one_client() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        let stats = vec![make_stat_model(
            "client_stats",
            vec![],
            vec![make_stat_model(
                "only",
                vec![],
                vec![make_stat_model(
                    "cpu",
                    vec![serde_json::json!(10.0)],
                    vec![],
                )],
            )],
        )];
        app.stats.accumulate(&stats, 1);
        app.stats_selected_category = Some("client".into());
        app.stats_selected_client = Some("stale".into());
        app.reconcile_stats_selection();

        assert_eq!(app.stats_selected_category.as_deref(), Some("client"));
        assert!(app.stats_selected_client.is_none());
    }

    #[test]
    fn stats_clear_resets_selection_and_scroll() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        let stats = vec![make_stat_model(
            "server_tick_timings",
            vec![],
            vec![make_stat_model(
                "tick",
                vec![serde_json::json!(16.0)],
                vec![],
            )],
        )];
        app.stats.accumulate(&stats, 1);
        app.reconcile_stats_selection();
        app.scroll_stats_down(10);
        app.select_next_client(); // no-op, but sets no state

        app.clear_stats();
        assert!(app.stats.is_empty());
        assert!(app.stats_selected_category.is_none());
        assert!(app.stats_selected_client.is_none());
        assert_eq!(app.stats_scroll_offset, 0);
    }

    #[test]
    fn stats_scroll_clamps_to_zero_and_max() {
        let (ctrl, rx) = SessionController::new();
        let mut app = App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None);

        app.scroll_stats_down(100);
        // No data yet; render will clamp to 0.
        assert_eq!(app.stats_scroll_offset, 100);

        app.scroll_stats_top();
        assert_eq!(app.stats_scroll_offset, 0);

        app.scroll_stats_bottom();
        assert_eq!(app.stats_scroll_offset, usize::MAX);
    }
}
