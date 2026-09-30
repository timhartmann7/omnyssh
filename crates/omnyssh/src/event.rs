//! TUI event stream: crossterm input events plus wrapped domain events.

use crossterm::event::{self, Event, KeyEvent, KeyEventKind, MouseEventKind};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::mpsc;

use omnyssh_core::event::CoreEvent;

/// Central event type consumed by the main loop. Input events are produced by
/// the crossterm event thread; everything domain-side arrives wrapped in
/// [`AppEvent::Core`] via the forwarder task in `App::run`.
#[derive(Debug)]
pub enum AppEvent {
    /// Keyboard or mouse input from the user.
    Key(KeyEvent),
    /// Text pasted into the terminal (bracketed paste).
    Paste(String),
    /// Render tick (~30 FPS).
    Tick,
    /// The terminal window was resized to the given dimensions (cols, rows).
    TerminalResized(u16, u16),
    /// Mouse-wheel scroll in the terminal pane: positive = up, negative = down.
    TermScroll(i16),
    /// A domain event produced by the SSH engine or a background task.
    Core(CoreEvent),
    /// A key unlock finished (the key path, and the error to show if it failed).
    PassphraseUnlocked {
        key_path: String,
        result: Result<(), String>,
    },
}

/// Whether a key event should be forwarded to the app.
///
/// Windows emits both a `Press` and a `Release` event per keystroke, while
/// Unix terminals emit only `Press`. Forwarding `Release` would process every
/// keystroke twice (e.g. "j" → "jj"), so it is dropped here.
fn should_forward_key(kind: KeyEventKind) -> bool {
    !matches!(kind, KeyEventKind::Release)
}

/// Spawns a background thread that reads crossterm events and forwards them
/// to the provided sender as [`AppEvent`] values. Also sends a `Tick` every
/// ~33 ms so the render loop stays at ≥30 FPS even when there is no input.
///
/// # Errors
/// Returns an error if the background thread fails to spawn.
pub fn spawn_event_thread(
    tx: mpsc::Sender<AppEvent>,
    pause_requested: Arc<AtomicBool>,
    paused_ack: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    std::thread::spawn(move || {
        let tick = Duration::from_millis(33);
        loop {
            // External editors need exclusive ownership of the terminal.
            // Once requested, acknowledge the pause and do not call
            // crossterm::event::poll/read until the request is released.
            if pause_requested.load(Ordering::Acquire) {
                paused_ack.store(true, Ordering::Release);
                while pause_requested.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                paused_ack.store(false, Ordering::Release);
                continue;
            }

            if event::poll(tick).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(key)) => {
                        if should_forward_key(key.kind)
                            && tx.blocking_send(AppEvent::Key(key)).is_err()
                        {
                            break;
                        }
                    }
                    Ok(Event::Resize(cols, rows)) => {
                        if tx
                            .blocking_send(AppEvent::TerminalResized(cols, rows))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Event::Mouse(m)) => {
                        let delta: Option<i16> = match m.kind {
                            MouseEventKind::ScrollUp => Some(3),
                            MouseEventKind::ScrollDown => Some(-3),
                            _ => None,
                        };
                        if let Some(d) = delta {
                            if tx.blocking_send(AppEvent::TermScroll(d)).is_err() {
                                break;
                            }
                        }
                    }
                    Ok(Event::Paste(text)) => {
                        if tx.blocking_send(AppEvent::Paste(text)).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            } else if tx.blocking_send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_press_and_repeat_but_not_release() {
        assert!(should_forward_key(KeyEventKind::Press));
        assert!(should_forward_key(KeyEventKind::Repeat));
        assert!(!should_forward_key(KeyEventKind::Release));
    }
}
