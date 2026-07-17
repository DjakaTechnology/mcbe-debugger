use std::io::stdout;

use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

/// Result type for terminal initialisation.
pub type TermResult = Result<
    (Terminal<CrosstermBackend<std::io::Stdout>>, TerminalGuard),
    Box<dyn std::error::Error>,
>;

/// Initialise the terminal: enable raw mode and switch to the alternate screen.
///
/// The returned [`TerminalGuard`] is **armed** immediately after raw mode
/// succeeds — if alternate-screen or [`Terminal`] creation fails the guard
/// will restore the terminal on drop.  On success the guard **must** be kept
/// alive for the lifetime of the [`Terminal`].
pub fn init() -> TermResult {
    enable_raw_mode()?;
    // Arm the guard right after raw mode so intermediate failures still
    // restore the terminal.
    let guard = TerminalGuard::new();

    let terminal = (|| -> Result<_, Box<dyn std::error::Error>> {
        execute!(stdout(), EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout());
        let terminal = Terminal::new(backend)?;
        Ok(terminal)
    })();

    match terminal {
        Ok(t) => Ok((t, guard)),
        Err(e) => {
            // guard drops → restore() → raw mode and alternate screen cleaned up
            Err(e)
        }
    }
}

/// Restore the terminal to normal mode.
pub fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
}

/// A Drop guard that restores the terminal on normal exit **and** panic.
///
/// Install a panic hook **before** creating this guard so that the guard's
/// `Drop` runs **after** the panic hook (which also restores the terminal).
pub struct TerminalGuard;

impl TerminalGuard {
    /// Create a new guard.  The terminal must already be in raw/alternate-screen
    /// mode (i.e. after calling [`init`]).
    ///
    /// When this guard is dropped the terminal is restored.
    pub fn new() -> Self {
        Self
    }
}

impl Default for TerminalGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}

/// Install a panic hook that restores the terminal before the default
/// panic handler runs.  Call **before** creating [`TerminalGuard`].
pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        prev(info);
    }));
}
