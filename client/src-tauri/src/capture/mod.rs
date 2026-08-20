//! Screen capture (Windows.Graphics.Capture) and hardware H.264 encode.
//!
//! Filled in by plan.md tasks 5.1–5.3.

use thiserror::Error;

/// Failures on the screen-share path.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// No hardware H.264 encoder (NVENC / AMF / `QuickSync`) was found; the
    /// caller must warn the user before falling back to software.
    #[error("no hardware H.264 encoder available")]
    NoHardwareEncoder,
}
