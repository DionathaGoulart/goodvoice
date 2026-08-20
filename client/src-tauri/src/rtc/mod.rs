//! Signaling client and WebRTC transport to the Cloudflare Realtime SFU.
//!
//! Filled in by plan.md tasks 2.3–2.4, 3.1 and 3.5.

use thiserror::Error;

/// Failures on the transport path.
#[derive(Debug, Error)]
pub enum RtcError {
    /// The signaling Worker rejected the join (room full, bad code, …).
    #[error("join rejected: {0}")]
    JoinRejected(String),
    /// ICE/DTLS never reached a connected state.
    #[error("transport did not connect")]
    NotConnected,
}
