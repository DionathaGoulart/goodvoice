//! The seam between a screen and the room.
//!
//! plan.md task 5.3. [`session`](super::session) publishes video the way it
//! publishes audio — through a trait, never through a device. On the far side
//! of [`ScreenSource`] is [`crate::capture::share`] on Windows and a fake in
//! the tests; on this side is an H.264 track and nothing that knows what a
//! monitor is.
//!
//! # Why a factory
//!
//! A call outlives the session it is running on: a dropped transport is
//! rebuilt underneath it (`super::reconnect`). A screen share cannot be
//! carried across that — the encoder's stream ends with the track it was
//! feeding, and a viewer joining the new session needs a keyframe from a
//! sequence that starts. So what a caller hands over is a
//! [`ScreenSourceFactory`], and each session opens its own capture from it.
//! Restarting the capture is also what makes the reconnect correct rather than
//! merely survivable.

use std::time::Duration;

/// One encoded frame, ready for the wire.
///
/// Annex B, as the encoder produced it: the payloader wants start codes, so
/// this is not stripped or reframed anywhere between the encoder and the RTP
/// packets.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    /// The H.264 access unit.
    pub bytes: Vec<u8>,
    /// How long it is on screen for. One frame period, unless the source knows
    /// better.
    pub duration: Duration,
    /// Whether a decoder could start here.
    pub keyframe: bool,
}

/// A running capture, as the transport sees it.
#[async_trait::async_trait]
pub trait ScreenSource: Send {
    /// The next encoded frame, or `None` once the capture has ended.
    ///
    /// A still screen produces nothing for as long as it stays still (DR-31),
    /// so this pending for seconds is ordinary rather than a stall.
    async fn next_frame(&mut self) -> Option<VideoFrame>;

    /// What is being encoded, after any scaling.
    fn size(&self) -> (u32, u32);

    /// Ask the encoder for a keyframe at the first opportunity.
    ///
    /// Called when a viewer subscribes: without it they wait out the encoder's
    /// own keyframe interval before the first picture appears.
    fn request_keyframe(&self);

    /// Whether the encode is happening in silicon.
    ///
    /// prd.md §3 F3 allows a software fallback and requires that the user be
    /// told, so this has to survive all the way up to the window.
    fn is_hardware(&self) -> bool;
}

/// Opens a capture. See the module docs for why this is not just a source.
pub trait ScreenSourceFactory: Send + Sync {
    /// Start capturing. Called once per session, including after a reconnect.
    ///
    /// # Errors
    ///
    /// Whatever the capture refused with, as prose for the user: a window that
    /// closed between the picker and the share is the ordinary case.
    fn open(&self) -> Result<Box<dyn ScreenSource>, String>;

    /// What the user picked, for the window to show.
    fn describe(&self) -> String;
}

/// Whether an Annex B access unit carries somewhere a decoder could start.
///
/// Asked on both sides of the wire and answered the same way, because nothing
/// else carries it: Media Foundation does not set the sample attribute on
/// every encoder, and RTP has no keyframe bit at all. So the bitstream is read
/// — an IDR slice, or a parameter set, means a decoder can begin here.
#[must_use]
pub fn starts_with_idr(bytes: &[u8]) -> bool {
    let mut index = 0;
    while index + 4 <= bytes.len() {
        // Annex B start codes are 00 00 01 or 00 00 00 01.
        let (step, header) = match bytes[index..] {
            [0, 0, 1, header, ..] => (4, header),
            [0, 0, 0, 1, header, ..] => (5, header),
            _ => {
                index += 1;
                continue;
            }
        };
        // 5 = IDR slice, 7 = SPS, 8 = PPS. Any of the three means a decoder
        // can start here.
        if matches!(header & 0x1f, 5 | 7 | 8) {
            return true;
        }
        index += step;
    }
    false
}

/// Where a remote screen's frames arrive.
///
/// The mirror of [`ScreenSource`], and the reason nothing in
/// [`session`](super::session) knows what a decoder is. Implemented by the
/// viewer on Windows and by a counter in the tests.
///
/// **Opt-in, by construction.** A `Call` has no sink until somebody opens a
/// viewer, and with no sink it never subscribes — which is prd.md §3 F3's
/// "viewers opt in" enforced by there being nothing to send frames to rather
/// than by a flag somebody could forget to check.
pub trait ScreenSink: Send + Sync {
    /// One complete H.264 access unit, Annex B, as the sharer's encoder
    /// produced it.
    ///
    /// Called from the receive path, so it must not block: a decoder belongs
    /// on the far side of a queue.
    fn accept(&self, unit: &[u8], keyframe: bool);

    /// The share ended — the sharer stopped, left, or the session dropped.
    fn ended(&self);
}

/// What a share is doing, as the UI draws it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ShareState {
    /// Nothing is being shared by this client.
    Idle,
    /// Publishing. `hardware` is false on the software fallback, which the
    /// window has to say out loud.
    Sharing {
        /// What is being shared, in the words the picker used.
        target: String,
        /// Encoded width.
        width: u32,
        /// Encoded height.
        height: u32,
        /// Whether the encoder is in silicon.
        hardware: bool,
    },
    /// The last attempt failed and this is why.
    Failed {
        /// What to show the user.
        detail: String,
    },
}

impl ShareState {
    /// Whether this client is publishing a screen right now.
    #[must_use]
    pub const fn is_sharing(&self) -> bool {
        matches!(self, Self::Sharing { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_idr_is_found_behind_either_start_code() {
        // Four-byte start code, NAL header 0x65: IDR slice.
        assert!(starts_with_idr(&[0, 0, 0, 1, 0x65, 0xff]));
        // Three-byte start code, NAL header 0x67: SPS.
        assert!(starts_with_idr(&[0, 0, 1, 0x67, 0x42]));
        // 0x41 is a non-IDR slice: a P-frame, and not a place to start.
        assert!(!starts_with_idr(&[0, 0, 0, 1, 0x41, 0x9a, 0x00]));
        assert!(!starts_with_idr(&[]));
    }

    #[test]
    fn an_sps_after_a_p_slice_still_counts() {
        // What every access unit from the encoder looks like at a keyframe:
        // a delimiter, then the parameter sets, then the picture.
        let bytes = [0, 0, 0, 1, 0x41, 0x9a, 0, 0, 0, 1, 0x67, 0x42];
        assert!(starts_with_idr(&bytes));
    }

    #[test]
    fn only_sharing_counts_as_sharing() {
        assert!(!ShareState::Idle.is_sharing());
        assert!(!ShareState::Failed {
            detail: "the window closed".to_owned()
        }
        .is_sharing());
        assert!(ShareState::Sharing {
            target: "\\\\.\\DISPLAY1".to_owned(),
            width: 1920,
            height: 1080,
            hardware: true,
        }
        .is_sharing());
    }

    #[test]
    fn a_share_serialises_with_its_state_named() {
        let json = serde_json::to_string(&ShareState::Sharing {
            target: "Notepad".to_owned(),
            width: 1280,
            height: 720,
            hardware: false,
        })
        .expect("serialise");
        // The UI switches on `state`, never on the presence of a field.
        assert!(json.contains(r#""state":"sharing""#), "{json}");
        assert!(json.contains(r#""hardware":false"#), "{json}");
        assert_eq!(
            serde_json::to_string(&ShareState::Idle).expect("serialise"),
            r#"{"state":"idle"}"#
        );
    }
}
