//! Signaling client and WebRTC transport to the Cloudflare Realtime SFU.
//!
//! [`signaling`] speaks to the Worker; [`session`] turns that plus an audio
//! source and sink into a call. Neither knows what a microphone is — the voice
//! path meets hardware only at [`crate::audio::device`]'s seam.
//!
//! Reconnection is filled in by plan.md task 3.5.

pub mod session;
pub mod signaling;

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
    /// The Worker could not be reached at all.
    #[error("signaling unreachable: {0}")]
    Http(String),
    /// The Worker or Cloudflare refused a track negotiation.
    #[error("sfu refused: {0}")]
    Sfu(String),
    /// A message crossed the wire in a shape this client does not understand.
    #[error("unreadable message: {0}")]
    Protocol(String),
    /// `webrtc-rs` refused an operation on the peer connection.
    #[error("webrtc: {0}")]
    Transport(String),
    /// The voice path could not be set up on this host.
    #[error("audio: {0}")]
    Audio(#[from] crate::audio::AudioError),
}

impl RtcError {
    /// Whether trying the same thing again could plausibly work.
    ///
    /// Transport failures are worth another go: the handshake with Realtime is
    /// not reliable on the first attempt (DR-8). A room that is full, a code
    /// that is invalid, or a message this client cannot parse will fail exactly
    /// the same way every time, and retrying only delays telling the user.
    #[must_use]
    pub const fn is_worth_retrying(&self) -> bool {
        matches!(
            self,
            Self::NotConnected | Self::Transport(_) | Self::Http(_)
        )
    }
}

impl From<rtc::shared::error::Error> for RtcError {
    fn from(error: rtc::shared::error::Error) -> Self {
        Self::Transport(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::RtcError;

    #[test]
    fn transport_failures_are_retried() {
        assert!(RtcError::NotConnected.is_worth_retrying());
        assert!(RtcError::Transport("ice died".to_owned()).is_worth_retrying());
        assert!(RtcError::Http("connection reset".to_owned()).is_worth_retrying());
    }

    #[test]
    fn a_refusal_is_not_retried() {
        // Asking a full room eight more times does not make room.
        assert!(!RtcError::JoinRejected("room is full".to_owned()).is_worth_retrying());
        assert!(!RtcError::Sfu("unknown track".to_owned()).is_worth_retrying());
        assert!(!RtcError::Protocol("bad json".to_owned()).is_worth_retrying());
    }
}
