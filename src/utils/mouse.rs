//! Minimal mouse-capture commands for the TUI.
//!
//! `crossterm::event::EnableMouseCapture` enables five DEC private modes at
//! once: ?1000h (basic), ?1002h (button-event motion), ?1003h (any-event
//! motion), ?1015h (urxvt) and ?1006h (SGR). The motion-tracking modes make
//! host terminal emulators refuse to honor their modifier-bypass for native
//! drag selection (Option/Shift+Drag on iTerm2/Terminal.app, Shift+Drag on
//! most Linux terminals), because even no-button mouse motion is reported to
//! the application as a captured event.
//!
//! These commands enable only ?1000h (button press/release, which includes
//! scroll-wheel events as buttons 4/5) and ?1006h (SGR coordinate format).
//! Scroll events still reach the app, but drag motion is not captured, so the
//! terminal emulator can perform native text selection when the user holds
//! the bypass modifier.
//!
//! On Windows the full crossterm commands are re-exported, since the
//! any-motion concern does not apply in the same way to Windows Terminal /
//! Conhost.

#[cfg(not(windows))]
use std::fmt;

#[cfg(not(windows))]
pub struct EnableMinimalMouseCapture;

#[cfg(not(windows))]
impl crossterm::Command for EnableMinimalMouseCapture {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        // ?1000h: normal mouse tracking (button press/release, incl. wheel).
        // ?1006h: SGR extended coordinate format.
        write!(f, "\x1b[?1000h\x1b[?1006h")
    }
}

#[cfg(not(windows))]
pub struct DisableMinimalMouseCapture;

#[cfg(not(windows))]
impl crossterm::Command for DisableMinimalMouseCapture {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        // Disable in reverse order of enable.
        write!(f, "\x1b[?1006l\x1b[?1000l")
    }
}

#[cfg(windows)]
pub use crossterm::event::DisableMouseCapture as DisableMinimalMouseCapture;
#[cfg(windows)]
pub use crossterm::event::EnableMouseCapture as EnableMinimalMouseCapture;
