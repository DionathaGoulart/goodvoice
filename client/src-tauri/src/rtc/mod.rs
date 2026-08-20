//! Signaling client and WebRTC transport to the Cloudflare Realtime SFU.
//!
//! [`signaling`] speaks to the Worker; [`session`] turns that plus an audio
//! source and sink into a call. Neither knows what a microphone is — the voice
//! path meets hardware only at [`crate::audio::device`]'s seam.
//!
//! [`reconnect`] holds what a call does when the session under it dies: the
//! retry schedule and the state the UI shows while it runs.

pub mod reconnect;
pub mod session;
pub mod signaling;

use thiserror::Error;

/// Failures on the transport path.
#[derive(Debug, Error)]
pub enum RtcError {
    /// The signaling Worker rejected the join (room full, bad code, …).
    ///
    /// `code` is the room's own machine-readable reason (`room_full`,
    /// `bad_request`, …) when it sent one; `None` when the failure came from
    /// somewhere that does not speak the room's error shape, such as a proxy
    /// in front of the Worker.
    #[error("join rejected: {detail}")]
    JoinRejected {
        code: Option<String>,
        detail: String,
    },
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

    /// Whether this is a room that has no space left.
    ///
    /// Worth waiting out when reconnecting and not when joining for the first
    /// time: a client whose call just dropped is often being refused by the
    /// room on account of its own phantom seat, which the heartbeat sweep
    /// clears within 30 s (DR-5, DR-8).
    #[must_use]
    pub fn is_room_full(&self) -> bool {
        matches!(self, Self::JoinRejected { code, .. } if code.as_deref() == Some("room_full"))
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
        assert!(!full_room().is_worth_retrying());
        assert!(!RtcError::Sfu("unknown track".to_owned()).is_worth_retrying());
        assert!(!RtcError::Protocol("bad json".to_owned()).is_worth_retrying());
    }

    #[test]
    fn a_full_room_is_recognised_by_its_code_not_its_prose() {
        // The message is for a human and may be reworded; the code is the
        // contract (`ROOM_ERROR_CODES` in server/src/protocol.ts).
        assert!(full_room().is_room_full());
        assert!(!RtcError::JoinRejected {
            code: Some("bad_request".to_owned()),
            detail: "room is full".to_owned(),
        }
        .is_room_full());
        assert!(!RtcError::NotConnected.is_room_full());
    }

    fn full_room() -> RtcError {
        RtcError::JoinRejected {
            code: Some("room_full".to_owned()),
            detail: "room is full (8 participants)".to_owned(),
        }
    }
}
