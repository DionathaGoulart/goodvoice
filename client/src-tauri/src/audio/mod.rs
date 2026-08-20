//! Capture, processing, encode and playback of voice.
//!
//! Real-time discipline applies to everything under this module: no allocation,
//! locking or logging on the callback path (styleguide.md, Rust conventions).
//!
//! Capture and playback are filled in by plan.md tasks 2.1 and 3.1–3.4.

pub mod opus;

use thiserror::Error;

/// Failures on the voice path. Callbacks return these instead of panicking —
/// a panic in a stream callback is a dropped call.
#[derive(Debug, Error)]
pub enum AudioError {
    /// No usable capture or render endpoint was found.
    #[error("no audio device available")]
    NoDevice,
    /// The Opus encoder or decoder refused a call.
    // `::opus` is the crate; `opus` alone would resolve to the module above.
    #[error("opus: {0}")]
    Codec(#[from] ::opus::Error),
    /// A bitrate outside the 20–40 kbps the PRD allows per speaker.
    #[error("bitrate {0} bps is outside the supported 20000–40000 range")]
    Bitrate(i32),
    /// The packet buffer handed to the encoder cannot hold a worst-case packet.
    #[error("packet buffer of {0} bytes is smaller than the maximum Opus packet")]
    PacketBuffer(usize),
    /// A zero-length packet reached the decoder; loss is concealed, not decoded.
    #[error("cannot decode an empty packet")]
    EmptyPacket,
}
