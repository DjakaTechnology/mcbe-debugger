use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event as CrosstermEvent, KeyEvent, KeyEventKind};
use tokio::sync::mpsc;

/// Events produced by the blocking input thread and consumed by the main loop.
#[derive(Debug)]
pub enum InputEvent {
    Key(KeyEvent),
    Resize(u16, u16),
}

/// Dropping this handle signals the input thread to stop, then joins it.
///
/// The thread checks the stop flag on every poll cycle (~50 ms), so shutdown
/// is prompt without requiring channel closure.
pub struct InputShutdown {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for InputShutdown {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

/// Spawn a background thread that polls crossterm (with a ~50 ms timeout)
/// and forwards events over a bounded channel.
///
/// Returns a receiver **and** a shutdown handle.  The thread exits promptly
/// when the handle is dropped or the receiver is closed.
pub fn start_input_thread() -> (mpsc::Receiver<InputEvent>, InputShutdown) {
    let (tx, rx) = mpsc::channel(256);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let handle = std::thread::spawn(move || {
        loop {
            // Check the stop flag every cycle so we don't block shutdown.
            if stop_clone.load(Ordering::Relaxed) {
                break;
            }

            // Poll with a short timeout instead of blocking read() so the
            // stop flag is checked regularly.
            if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                match event::read() {
                    Ok(CrosstermEvent::Key(key))
                        if key.kind != KeyEventKind::Release
                            && tx.blocking_send(InputEvent::Key(key)).is_err() =>
                    {
                        break; // receiver dropped → shutdown
                    }
                    Ok(CrosstermEvent::Resize(w, h))
                        if tx.blocking_send(InputEvent::Resize(w, h)).is_err() =>
                    {
                        break;
                    }
                    _ => {} // ignore mouse, focus, etc.
                }
            }
        }
    });

    (
        rx,
        InputShutdown {
            stop,
            join: Some(handle),
        },
    )
}
