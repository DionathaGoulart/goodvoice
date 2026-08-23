//! Echo cancellation, noise suppression and gain control, between the
//! microphone and the encoder.
//!
//! This is WebRTC's `AudioProcessing` module (plan.md task 3.4, DR-11). Its job
//! here is one thing above all: a headset is optional and speakers are common,
//! and without this every word a roommate says comes back to them a moment
//! later in their own room's voice.
//!
//! Cancelling an echo means knowing what the speakers played, so this stage
//! reads two streams. The near end is the capture frame it is handed; the far
//! end arrives over a lock-free ring that the render callback fills with
//! exactly what it sent to the device ([`super::hardware`]). Nothing here runs
//! in a device callback — [`Processing::run`] is called from the encode task —
//! but it still allocates nothing per frame.
//!
//! # Framing
//!
//! WebRTC processes 10 ms at a time and panics on anything else, while
//! goodvoice speaks in 20 ms frames (`opus::FRAME_MS`). Every frame is
//! therefore two passes, and the far end is drawn 10 ms at a time to match.

use ringbuf::{
    traits::{Consumer, Observer},
    HeapCons,
};
use webrtc_audio_processing::{
    config::{
        AdaptiveDigital, EchoCanceller, FixedDigital, GainController, GainController2,
        HighPassFilter, NoiseSuppression, NoiseSuppressionLevel, Pipeline,
    },
    Config, Processor,
};

use super::{
    opus::{Frame, FRAME_SAMPLES, SAMPLE_RATE_HZ},
    prefs::AudioSettings,
    AudioError,
};

/// What WebRTC processes in one pass: 10 ms, fixed in the library.
const CHUNK: usize = SAMPLE_RATE_HZ as usize / 100;

/// How many of those a goodvoice frame is.
const CHUNKS_PER_FRAME: usize = FRAME_SAMPLES / CHUNK;

/// How much render audio the reference ring holds.
///
/// Deep enough to ride out a scheduling hiccup on either device, and no
/// deeper: whatever is queued here is delay the echo canceller has to estimate
/// before it can subtract anything.
pub const REFERENCE_MS: usize = 200;
pub const REFERENCE_SAMPLES: usize = (SAMPLE_RATE_HZ as usize * REFERENCE_MS) / 1000;

/// Past this much backlog the two streams have drifted far enough that the
/// oldest reference audio is not describing the echo in the capture stream any
/// more, and it is thrown away rather than subtracted from the wrong thing.
const MAX_BACKLOG: usize = REFERENCE_SAMPLES / 2;

/// Full scale, as a float divisor. `i16::MAX` rather than `32768` so a
/// full-scale sample reads as exactly 1.0 — the same convention as the mixer's
/// meters.
const FULL_SCALE: f32 = i16::MAX as f32;

// One frame has to divide into whole 10 ms passes, or the library panics on the
// remainder. Both values are constants, so this is settled at compile time.
const _: () = {
    assert!(FRAME_SAMPLES == CHUNK * CHUNKS_PER_FRAME);
};

/// The configuration goodvoice runs the module with. See DR-11 for why each of
/// these and not its neighbours, and `prefs` for which of them the user is
/// allowed to switch off.
///
/// Two stages are not negotiable. The high-pass filter is what keeps desk
/// knocks out from under a voice and costs nothing, and the gain controller is
/// what stops a quiet talker having to lean into the microphone — neither is a
/// setting anyone would want to find. The other two are: a headset makes the
/// echo canceller pointless, and a good microphone in a quiet room makes the
/// noise suppressor a way to lose consonants.
fn settings(prefs: AudioSettings) -> Config {
    Config {
        pipeline: Pipeline::default(),
        // Nothing before the pipeline: the gain that matters is applied after
        // the echo canceller has had its look, by the controller below.
        capture_amplifier: None,
        // Strongly recommended alongside echo cancellation, and it is what
        // takes desk knocks and breath rumble out from under the voice.
        high_pass_filter: Some(HighPassFilter {
            apply_in_full_band: true,
        }),
        // AEC3, with the delay left to its own estimator. The real figure is
        // whatever WASAPI's render and capture buffers add up to on a machine
        // nobody here can see, and a confident wrong number is worse than none.
        echo_canceller: prefs.echo_cancellation.then_some(EchoCanceller::Full {
            stream_delay_ms: None,
        }),
        noise_suppression: prefs.noise_suppression.then_some(NoiseSuppression {
            // Moderate: the stronger levels buy a quieter fan at the cost of
            // chewing into consonants, and a room full of people asking "what?"
            // is worse than a room with a fan in it.
            level: NoiseSuppressionLevel::Moderate,
            analyze_linear_aec_output: false,
        }),
        gain_controller: Some(GainController::GainController2(GainController2 {
            // Digital gain only. The alternative reaches out and moves the
            // microphone slider in the operating system, which is the user's
            // setting and not ours to change behind their back.
            input_volume_controller_enabled: false,
            adaptive_digital: Some(AdaptiveDigital::default()),
            fixed_digital: FixedDigital::default(),
        })),
    }
}

/// The processing stage: two streams in, one cleaned-up frame out.
pub struct Processing {
    module: Processor,
    /// What the speakers played, filled by the render callback.
    reference: HeapCons<i16>,
    /// Scratch for one 10 ms pass of each stream. Fixed size so the frame path
    /// never allocates.
    near: [f32; CHUNK],
    far: [f32; CHUNK],
    taken: [i16; CHUNK],
}

impl Processing {
    /// Starts the module against a reference stream.
    ///
    /// # Errors
    ///
    /// [`AudioError::Processing`] if WebRTC refuses the sample rate or the
    /// configuration. The caller is expected to carry on without it: a call
    /// with an echo is worse than one without, and far better than none.
    pub fn new(reference: HeapCons<i16>, prefs: AudioSettings) -> Result<Self, AudioError> {
        let module = Processor::new(SAMPLE_RATE_HZ)
            .map_err(|error| AudioError::Processing(format!("{error:?}")))?;
        module.set_config(settings(prefs));

        Ok(Self {
            module,
            reference,
            near: [0.0; CHUNK],
            far: [0.0; CHUNK],
            taken: [0; CHUNK],
        })
    }

    /// Switches stages on or off mid-call.
    ///
    /// `set_config` builds WebRTC's own configuration objects, so this is the
    /// one thing in this module that allocates — which is why the capture path
    /// calls it on a generation change rather than per frame. AEC3 loses its
    /// delay estimate when it is switched back on and spends a second or two
    /// finding it again; that is the cost of the switch existing, and it is
    /// paid by the person who flipped it.
    pub fn reconfigure(&mut self, prefs: AudioSettings) {
        self.module.set_config(settings(prefs));
    }

    /// Cleans one captured frame in place.
    ///
    /// A pass the module refuses leaves that 10 ms exactly as it arrived. The
    /// only errors it can raise here are about framing, which is settled at
    /// compile time — and if one ever appeared, an un-cancelled frame is a
    /// worse call, not a dropped one.
    pub fn run(&mut self, frame: &mut Frame) {
        for pass in 0..CHUNKS_PER_FRAME {
            let span = pass * CHUNK..(pass + 1) * CHUNK;

            // The far end first: the module has to be told what was played
            // before it is asked to find that sound inside what was heard.
            self.take_reference();
            let _ = self.module.analyze_render_frame([&self.far[..]]);

            for (out, &sample) in self.near.iter_mut().zip(frame[span.clone()].iter()) {
                *out = f32::from(sample) / FULL_SCALE;
            }
            if self
                .module
                .process_capture_frame([&mut self.near[..]])
                .is_ok()
            {
                for (out, &sample) in frame[span].iter_mut().zip(self.near.iter()) {
                    *out = to_i16(sample);
                }
            }
        }
    }

    /// Whether the module currently believes it is looking at an echo.
    ///
    /// Only meaningful once the canceller has converged, which takes a second
    /// or two of real playback; test-facing rather than something the UI should
    /// draw.
    #[must_use]
    pub fn echo_likelihood(&self) -> Option<f64> {
        self.module.get_stats().residual_echo_likelihood
    }

    /// Draws the next 10 ms of what the speakers played.
    fn take_reference(&mut self) {
        // A backlog means playback has run ahead of capture — the usual cause
        // is a stall on this side. Dropping the oldest keeps the far end
        // roughly abreast of the echo it explains; letting it grow would leave
        // the two describing different moments for the rest of the call.
        let backlog = self.reference.occupied_len();
        if backlog > MAX_BACKLOG {
            self.reference.skip(backlog - CHUNK);
        }

        let read = self.reference.pop_slice(&mut self.taken);
        for (out, &sample) in self.far.iter_mut().zip(self.taken[..read].iter()) {
            *out = f32::from(sample) / FULL_SCALE;
        }
        // Whatever the ring could not fill is silence, not the last block
        // again: nothing was playing, and repeating it would set the canceller
        // hunting an echo that was never made.
        self.far[read..].fill(0.0);
    }
}

/// Back to the integer samples the encoder wants, clamped rather than wrapped.
///
/// The gain controller and the limiter both work in floats and can hand back
/// something a shade outside full scale; wrapping that would be the loudest
/// click of the call.
fn to_i16(sample: f32) -> i16 {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "clamped into i16's range on the line above"
    )]
    {
        (sample * FULL_SCALE).clamp(-FULL_SCALE, FULL_SCALE) as i16
    }
}

#[cfg(test)]
mod tests {
    use super::{Processing, CHUNK, REFERENCE_SAMPLES};
    use crate::audio::{
        opus::{silent_frame, Frame, FRAME_SAMPLES},
        prefs::AudioSettings,
    };
    use ringbuf::{
        traits::{Observer, Producer, Split},
        HeapProd, HeapRb,
    };

    /// A processing stage and the end that pretends to be the speakers.
    fn rigged() -> (Processing, HeapProd<i16>) {
        let (producer, consumer) = HeapRb::<i16>::new(REFERENCE_SAMPLES).split();
        (
            Processing::new(consumer, AudioSettings::default()).expect("the module should start"),
            producer,
        )
    }

    /// Deterministic noise. An echo canceller converges on something
    /// broadband; it has very little to learn from a pure tone, and neither
    /// would a test built on one.
    struct Noise(u32);

    impl Noise {
        fn next(&mut self) -> i16 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap,
                reason = "the low bits are the point"
            )]
            {
                ((self.0 >> 16) as u16 as i16) / 4
            }
        }

        fn frame(&mut self) -> Frame {
            let mut frame = silent_frame();
            for sample in &mut frame {
                *sample = self.next();
            }
            frame
        }
    }

    /// Loudness, as the only thing worth asserting about a cancelled signal.
    fn rms(frame: &Frame) -> f32 {
        let sum: f64 = frame
            .iter()
            .map(|&sample| f64::from(sample) * f64::from(sample))
            .sum();
        #[allow(
            clippy::cast_possible_truncation,
            reason = "an RMS of i16 samples fits an f32 with room to spare"
        )]
        {
            (sum / f64::from(u32::try_from(FRAME_SAMPLES).unwrap_or(1))).sqrt() as f32
        }
    }

    #[test]
    fn silence_in_silence_out() {
        let (mut processing, _speakers) = rigged();
        let mut frame = silent_frame();

        processing.run(&mut frame);

        assert!(
            frame.iter().all(|&sample| sample == 0),
            "the module invented a signal out of silence"
        );
    }

    #[test]
    fn a_frame_with_nothing_playing_comes_back_the_same_length() {
        let (mut processing, _speakers) = rigged();
        let mut noise = Noise(0x1234_5678);
        let mut frame = noise.frame();
        let before = rms(&frame);

        processing.run(&mut frame);

        assert_eq!(frame.len(), FRAME_SAMPLES);
        // Not silence: with nothing on the speakers there is no echo to remove,
        // and a stage that ate the microphone would be the worst bug here.
        assert!(
            rms(&frame) > before / 8.0,
            "a voice with nothing playing came back at {} from {before}",
            rms(&frame)
        );
    }

    /// plan.md task 3.4's whole point, without a room: the speakers play
    /// something, the microphone hears exactly that, and what reaches the
    /// encoder should not.
    #[test]
    fn what_the_speakers_played_does_not_come_back() {
        let (mut processing, mut speakers) = rigged();
        let mut noise = Noise(0x9e37_79b9);

        // Long enough for AEC3 to find the delay and converge on the path.
        // Two seconds of 20 ms frames.
        let mut loudest_late = 0.0_f32;
        let mut loudest_input = 0.0_f32;
        for index in 0..100 {
            let played = noise.frame();
            speakers.push_slice(&played);

            // The microphone hears the speakers and nothing else.
            let mut heard = played;
            processing.run(&mut heard);

            // The first second is the canceller learning; only what survives
            // after that is a failure.
            if index >= 50 {
                loudest_late = loudest_late.max(rms(&heard));
                loudest_input = loudest_input.max(rms(&played));
            }
        }

        // Reported rather than only asserted: the number is the point of the
        // task, it differs between machines and AEC3 builds, and a test that
        // keeps it to itself makes the next person re-derive it.
        // `cargo test --lib processing -- --nocapture` prints it.
        println!(
            "echo cancelled by {:.1} dB (residual {loudest_late:.0} of {loudest_input:.0} played)",
            20.0 * (loudest_input / loudest_late.max(f32::MIN_POSITIVE)).log10()
        );

        // Twenty decibels. The measured figure is nearer thirty (DR-11); the
        // gap is there so a slower machine or a differently built AEC3 fails
        // this test only when the echo is genuinely audible again.
        assert!(
            loudest_late < loudest_input / 10.0,
            "echo survived at {loudest_late} against {loudest_input} played"
        );
    }

    /// The switch is the whole point of the setting: with the canceller off,
    /// what the speakers played comes back. Without this, "echo cancellation:
    /// off" could be a checkbox wired to nothing and every other test would
    /// still pass.
    #[test]
    fn switching_the_canceller_off_lets_the_echo_through() {
        let (mut processing, mut speakers) = rigged();
        processing.reconfigure(AudioSettings {
            echo_cancellation: false,
            // Off as well: the suppressor would attack the same broadband
            // noise and this test would not know which stage removed it.
            noise_suppression: false,
            ..AudioSettings::default()
        });
        let mut noise = Noise(0x9e37_79b9);

        let mut loudest_late = 0.0_f32;
        let mut loudest_input = 0.0_f32;
        for index in 0..100 {
            let played = noise.frame();
            speakers.push_slice(&played);

            let mut heard = played;
            processing.run(&mut heard);

            if index >= 50 {
                loudest_late = loudest_late.max(rms(&heard));
                loudest_input = loudest_input.max(rms(&played));
            }
        }

        // Half of what was played, against the tenth the canceller gets it
        // under. The gap is wide enough that this fails on a stage that is
        // merely weaker and passes on one that is genuinely not running.
        assert!(
            loudest_late > loudest_input / 2.0,
            "the echo was cancelled at {loudest_late} against {loudest_input} \
             played, with the canceller switched off"
        );
    }

    #[test]
    fn a_reference_that_ran_away_is_trimmed_rather_than_followed() {
        let (mut processing, mut speakers) = rigged();
        let mut noise = Noise(0x0bad_c0de);

        // Playback ran on while this side stalled: the ring is full of audio
        // that describes moments already gone.
        for _ in 0..(REFERENCE_SAMPLES / FRAME_SAMPLES) {
            let played = noise.frame();
            speakers.push_slice(&played);
        }

        let mut frame = silent_frame();
        processing.run(&mut frame);

        // One frame took two passes, so a stage that only drained what it used
        // would still be carrying nearly everything.
        assert!(
            processing.reference.occupied_len() < REFERENCE_SAMPLES / 2,
            "the backlog was followed instead of trimmed"
        );
    }

    #[test]
    fn a_full_scale_float_does_not_wrap_on_the_way_back() {
        // The limiter can hand back a shade over full scale; wrapping that
        // would be the loudest click of the call.
        assert_eq!(super::to_i16(1.5), i16::MAX);
        assert_eq!(super::to_i16(-1.5), i16::MIN + 1);
        assert_eq!(super::to_i16(0.0), 0);
    }

    #[test]
    fn the_module_takes_the_frames_the_capture_path_produces() {
        // WebRTC panics on anything that is not 10 ms. If `opus::FRAME_MS`
        // ever moves, this says so rather than a call that dies in a callback.
        assert_eq!(CHUNK, 480);
        assert_eq!(FRAME_SAMPLES % CHUNK, 0);
    }
}
