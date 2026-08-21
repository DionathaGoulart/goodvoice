//! Whether the microphone goes on the wire right now.
//!
//! Mute answers "never"; this module answers "this frame". Three modes, and
//! only one of them has to look at the audio — so the detector lives here with
//! the hangover that keeps it from cutting words in half, and the publish loop
//! in [`crate::rtc::session`] asks one question per frame.
//!
//! # Real-time discipline
//!
//! [`Gate::admits`] runs in the encode task rather than a device callback, but
//! it runs fifty times a second for the whole call: no allocation, and the
//! detector is owned by that loop rather than shared, so nothing here locks.

use serde::{Deserialize, Serialize};
use webrtc_vad::{SampleRate, Vad, VadMode};

use super::opus::{Frame, FRAME_MS};

/// How long transmission stays open after the last frame the detector called
/// voice.
///
/// 300 ms carries the gap between two words and the tail of a sentence, which
/// is the difference between voice activity and a bad phone line. Much longer
/// and the room starts hearing the keyboard.
///
/// The real tail is a little longer: libfvad keeps saying "voice" for about
/// four frames after the sound stops (DR-10), and those renew this before it
/// starts counting down.
pub const HANGOVER_MS: u32 = 300;

/// The hangover in frames, which is the unit the publish loop counts in.
const HANGOVER_FRAMES: u32 = HANGOVER_MS / FRAME_MS;

/// How the user wants transmission gated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransmitMode {
    /// Always on, until the user mutes. The default because it is the only
    /// mode that cannot swallow the start of a sentence, which is the failure
    /// people blame on the app rather than on the setting.
    #[default]
    Open,
    /// Only while the talk key is held.
    PushToTalk,
    /// Only while the detector hears a voice, plus [`HANGOVER_MS`].
    VoiceActivity,
}

impl TransmitMode {
    /// The mode as one byte.
    ///
    /// The publish loop reads the mode once per frame and the user changes it
    /// about once a month, so it crosses that boundary in an atomic rather
    /// than behind a lock the frame path would take fifty times a second.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Open => 0,
            Self::PushToTalk => 1,
            Self::VoiceActivity => 2,
        }
    }

    /// The mode a byte stands for. Anything else reads as
    /// [`TransmitMode::Open`]: a byte nobody can decode must not be the reason
    /// a call goes silent.
    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        match code {
            1 => Self::PushToTalk,
            2 => Self::VoiceActivity,
            _ => Self::Open,
        }
    }
}

/// The per-frame decision, and the detector state behind it.
pub struct Gate {
    detector: Vad,
    /// Frames still owed to the last voice the detector heard.
    hangover: u32,
}

// SAFETY: `Vad` is a `*mut Fvad` and nothing else, which is what makes it
// `!Send`. libfvad keeps all its state in that instance — no globals, no
// thread-locals — and a `Gate` is created inside the publish loop and never
// shared, so the pointer is only ever followed by the one task that owns it.
// The loop awaits, so the task may resume on another worker thread; that is a
// move, not an alias, and is exactly what this promises.
unsafe impl Send for Gate {}

impl Gate {
    /// A gate with the detector configured for goodvoice's frames.
    #[must_use]
    pub fn new() -> Self {
        Self {
            // `Aggressive` rather than `VeryAggressive`: the fussiest setting
            // is tuned for telephony bandwidth and clips quiet talkers' first
            // syllable, which is the one mistake a speaking gate must not make.
            detector: Vad::new_with_rate_and_mode(SampleRate::Rate48kHz, VadMode::Aggressive),
            hangover: 0,
        }
    }

    /// Whether this frame goes on the wire.
    pub fn admits(&mut self, mode: TransmitMode, key_down: bool, frame: &Frame) -> bool {
        match mode {
            TransmitMode::Open => true,
            // No hangover on release. The key coming up is the user saying
            // stop, and the mode people choose in order *not* to be heard has
            // to obey immediately.
            TransmitMode::PushToTalk => key_down,
            TransmitMode::VoiceActivity => self.hears_voice(frame),
        }
    }

    /// How much of the hangover is left, in frames. Test-facing: it is the
    /// difference between a gate that is open because someone is talking and
    /// one that is open because someone just stopped.
    #[must_use]
    pub const fn hangover_frames(&self) -> u32 {
        self.hangover
    }

    fn hears_voice(&mut self, frame: &Frame) -> bool {
        // A frame the detector refuses counts as voice. It can only happen on a
        // frame length libfvad does not support, which a fixed 20 ms frame
        // never is — and if that ever changed, transmitting is a far smaller
        // bug than a client that goes mute for the rest of the call.
        if self.detector.is_voice_segment(frame).unwrap_or(true) {
            self.hangover = HANGOVER_FRAMES;
            return true;
        }
        if self.hangover > 0 {
            self.hangover -= 1;
            return true;
        }
        false
    }
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{Gate, TransmitMode, HANGOVER_FRAMES};
    use crate::audio::opus::{silent_frame, Frame, FRAME_SAMPLES, SAMPLE_RATE_HZ};
    use std::f32::consts::TAU;

    /// Something the detector will call voice: a 220 Hz tone with a little
    /// noise on it, which is closer to a vowel than a pure sine.
    fn speech() -> Frame {
        let mut frame = silent_frame();
        let mut noise = 0x2545_f491_u32;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_precision_loss,
            reason = "amplitudes are bounded well inside i16 by construction"
        )]
        for (index, sample) in frame.iter_mut().enumerate() {
            // xorshift rather than a crate: a test needs the same "noise"
            // every run more than it needs a good generator.
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let phase = TAU * 220.0 * index as f32 / SAMPLE_RATE_HZ as f32;
            let jitter = (f32::from(((noise >> 20) & 0x0fff) as u16) - 2_048.0) / 16.0;
            *sample = (phase.sin() * 9_000.0 + jitter) as i16;
        }
        frame
    }

    #[test]
    fn an_open_mode_transmits_whatever_the_microphone_heard() {
        let mut gate = Gate::new();

        // Silence, key up: still on the wire. Open means open.
        assert!(gate.admits(TransmitMode::Open, false, &silent_frame()));
    }

    #[test]
    fn push_to_talk_follows_the_key_and_nothing_else() {
        let mut gate = Gate::new();

        assert!(gate.admits(TransmitMode::PushToTalk, true, &silent_frame()));
        assert!(!gate.admits(TransmitMode::PushToTalk, false, &speech()));
    }

    #[test]
    fn push_to_talk_closes_on_the_same_frame_the_key_comes_up() {
        let mut gate = Gate::new();
        // Talking, so voice activity would still be holding the gate open.
        assert!(gate.admits(TransmitMode::PushToTalk, true, &speech()));

        // No tail: a mode chosen to not be overheard cannot have one.
        assert!(!gate.admits(TransmitMode::PushToTalk, false, &speech()));
    }

    #[test]
    fn voice_activity_opens_on_speech() {
        let mut gate = Gate::new();

        assert!(gate.admits(TransmitMode::VoiceActivity, false, &speech()));
        assert_eq!(gate.hangover_frames(), HANGOVER_FRAMES);
    }

    #[test]
    fn voice_activity_closes_on_silence_but_not_at_once() {
        let mut gate = Gate::new();
        assert!(gate.admits(TransmitMode::VoiceActivity, false, &speech()));

        // The hangover is what stops the gap between two words sounding like a
        // dropped connection, so the gate owes at least that many frames.
        let quiet = silent_frame();
        for frame in 0..HANGOVER_FRAMES {
            assert!(
                gate.admits(TransmitMode::VoiceActivity, false, &quiet),
                "closed {frame} frames into a {HANGOVER_FRAMES}-frame hangover"
            );
        }

        // And it does close. libfvad holds its own verdict on for a few frames
        // after the sound stops (measured at four — DR-10), and those renew the
        // hangover before it starts counting down, so the tail is the two added
        // together rather than either alone.
        let mut overhang = 0;
        while gate.admits(TransmitMode::VoiceActivity, false, &quiet) {
            overhang += 1;
            assert!(
                overhang <= HANGOVER_FRAMES,
                "the gate never closed on a silent microphone"
            );
        }
        assert_eq!(gate.hangover_frames(), 0);
    }

    #[test]
    fn every_word_renews_the_hangover() {
        let mut gate = Gate::new();
        let quiet = silent_frame();

        gate.admits(TransmitMode::VoiceActivity, false, &speech());
        for _ in 0..HANGOVER_FRAMES / 2 {
            gate.admits(TransmitMode::VoiceActivity, false, &quiet);
        }
        assert!(gate.hangover_frames() < HANGOVER_FRAMES);

        gate.admits(TransmitMode::VoiceActivity, false, &speech());
        assert_eq!(
            gate.hangover_frames(),
            HANGOVER_FRAMES,
            "a pause in the middle of a sentence ate into the next one"
        );
    }

    #[test]
    fn a_silent_room_never_opens_the_gate() {
        let mut gate = Gate::new();
        let quiet = silent_frame();

        // Nothing has spoken yet, so there is no hangover to spend either.
        for _ in 0..100 {
            assert!(!gate.admits(TransmitMode::VoiceActivity, false, &quiet));
        }
    }

    #[test]
    fn a_mode_survives_the_byte_it_crosses_into_the_publish_loop_as() {
        for mode in [
            TransmitMode::Open,
            TransmitMode::PushToTalk,
            TransmitMode::VoiceActivity,
        ] {
            assert_eq!(TransmitMode::from_code(mode.code()), mode);
        }
    }

    #[test]
    fn an_unrecognised_byte_reads_as_open_rather_than_silent() {
        // Only reachable from a bug, and the safe direction is audible.
        assert_eq!(TransmitMode::from_code(9), TransmitMode::Open);
    }

    #[test]
    fn the_detector_takes_the_frames_the_encoder_produces() {
        // libfvad accepts 10, 20 or 30 ms only. If `opus::FRAME_MS` ever moves,
        // this is the test that says so rather than a call that never opens.
        assert_eq!(FRAME_SAMPLES, 960);
        let mut gate = Gate::new();
        assert!(gate.admits(TransmitMode::VoiceActivity, false, &speech()));
    }
}
