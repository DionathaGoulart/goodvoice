//! System tray icon, tray menu and the global push-to-talk hotkey.
//!
//! Filled in by plan.md tasks 4.1–4.3.

use thiserror::Error;

/// Failures on the tray / hotkey path.
#[derive(Debug, Error)]
pub enum TrayError {
    /// The low-level keyboard hook could not be installed.
    #[error("global hotkey registration failed")]
    HotkeyUnavailable,
}
