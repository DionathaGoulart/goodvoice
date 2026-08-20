//! Capture, processing, encode and playback of voice.
//!
//! Real-time discipline applies to everything under this module: no allocation,
//! locking or logging on the callback path (styleguide.md, Rust conventions).
//!
//! Filled in by plan.md tasks 2.1–2.2 and 3.1–3.4.

use thiserror::Error;

/// Failures on the voice path. Callbacks return these instead of panicking —
/// a panic in a stream callback is a dropped call.
#[derive(Debug, Error)]
pub enum AudioError {
    /// No usable capture or render endpoint was found.
    #[error("no audio device available")]
    NoDevice,
}
