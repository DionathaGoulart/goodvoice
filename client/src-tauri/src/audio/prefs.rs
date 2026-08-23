//! The audio settings a user can move while a call is running.
//!
//! Three of the four things in here were compile-time constants until someone
//! sat in a real room with them: whether the noise suppressor runs, whether
//! the echo canceller runs, and where the line between "background" and
//! "talking" sits. The fourth is which of the two detectors draws that line —
//! libfvad, which decides for itself, or a level the user set by looking at a
//! meter.
//!
//! # Why atomics rather than a lock
//!
//! Everything here is read on the frame path, fifty times a second for the
//! whole call, and written when a human moves a slider. A lock on that path
//! would be taken a hundred thousand times per write. [`Generation`] is the
//! one thing that is *acted* on rather than read: WebRTC's module is
//! reconfigured by a call that allocates, so the capture side watches a
//! counter and only does that work when the number moves.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

/// The quietest threshold the manual slider can be set to, as a level in
/// `0.0`–`1.0`.
///
/// Below this a cheap microphone's own hiss holds the gate open for the whole
/// call, which reads to everyone else as a client that ignores its setting.
pub const MIN_THRESHOLD: f32 = 0.002;

/// The loudest. Past this only shouting opens the gate, and a slider whose top
/// end is useless is a slider with a shorter useful range than it looks.
pub const MAX_THRESHOLD: f32 = 0.25;

/// Where the manual threshold starts, before anyone has moved it: the same
/// level the meters have always called talking (`mixer::SPEAKING_LEVEL`).
pub const DEFAULT_THRESHOLD: f32 = 0.02;

/// What the window shows and sends back. The wire form of [`AudioPrefs`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSettings {
    /// Whether the voice detector decides for itself. `false` puts
    /// [`Self::threshold`] in charge.
    pub automatic_sensitivity: bool,
    /// The manual gate level, `0.0`–`1.0`. Ignored while
    /// [`Self::automatic_sensitivity`] is set, but kept, so turning automatic
    /// off returns to the number the user chose rather than to a default.
    pub threshold: f32,
    /// WebRTC's noise suppressor.
    pub noise_suppression: bool,
    /// WebRTC's echo canceller. Off is defensible on a headset and a mistake
    /// on speakers, which is why it is a switch and not a guess.
    pub echo_cancellation: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            automatic_sensitivity: true,
            threshold: DEFAULT_THRESHOLD,
            noise_suppression: true,
            echo_cancellation: true,
        }
    }
}

impl AudioSettings {
    /// The settings with the threshold forced into the range the slider spans.
    ///
    /// The window is not the only thing that can call the command, and a
    /// threshold of zero or of NaN is a call that never opens its gate or one
    /// that never closes it.
    #[must_use]
    pub fn sane(mut self) -> Self {
        if !self.threshold.is_finite() {
            self.threshold = DEFAULT_THRESHOLD;
        }
        self.threshold = self.threshold.clamp(MIN_THRESHOLD, MAX_THRESHOLD);
        self
    }
}

/// How the gate should decide, this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sensitivity {
    /// libfvad's verdict.
    Automatic,
    /// Louder than this level counts as voice.
    Manual(f32),
}

/// The live settings, shared between the window, the capture path and the
/// publish loop.
#[derive(Debug)]
pub struct AudioPrefs {
    automatic_sensitivity: AtomicBool,
    /// The threshold as `f32::to_bits`. There is no `AtomicF32`.
    threshold: AtomicU32,
    noise_suppression: AtomicBool,
    echo_cancellation: AtomicBool,
    /// Bumped by every write. The capture path reconfigures WebRTC when it
    /// changes and does nothing at all when it does not.
    generation: AtomicU32,
}

impl Default for AudioPrefs {
    fn default() -> Self {
        Self::new(AudioSettings::default())
    }
}

impl AudioPrefs {
    /// Prefs holding the given settings, with the threshold made sane.
    #[must_use]
    pub fn new(settings: AudioSettings) -> Self {
        let settings = settings.sane();
        Self {
            automatic_sensitivity: AtomicBool::new(settings.automatic_sensitivity),
            threshold: AtomicU32::new(settings.threshold.to_bits()),
            noise_suppression: AtomicBool::new(settings.noise_suppression),
            echo_cancellation: AtomicBool::new(settings.echo_cancellation),
            generation: AtomicU32::new(0),
        }
    }

    /// What the window should be showing.
    #[must_use]
    pub fn settings(&self) -> AudioSettings {
        AudioSettings {
            automatic_sensitivity: self.automatic_sensitivity.load(Ordering::Relaxed),
            threshold: f32::from_bits(self.threshold.load(Ordering::Relaxed)),
            noise_suppression: self.noise_suppression.load(Ordering::Relaxed),
            echo_cancellation: self.echo_cancellation.load(Ordering::Relaxed),
        }
    }

    /// Replaces every setting and bumps [`Self::generation`].
    ///
    /// The four stores are not one transaction, and deliberately: a frame that
    /// reads a new threshold beside an old noise-suppression flag is one frame
    /// out of fifty in a second, and the alternative is a lock on the capture
    /// path. Only `generation` is ordered against the rest, because it is what
    /// tells the capture side the others are worth re-reading.
    pub fn set(&self, settings: AudioSettings) -> AudioSettings {
        let settings = settings.sane();
        self.automatic_sensitivity
            .store(settings.automatic_sensitivity, Ordering::Relaxed);
        self.threshold
            .store(settings.threshold.to_bits(), Ordering::Relaxed);
        self.noise_suppression
            .store(settings.noise_suppression, Ordering::Relaxed);
        self.echo_cancellation
            .store(settings.echo_cancellation, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
        settings
    }

    /// How the gate should decide right now.
    #[must_use]
    pub fn sensitivity(&self) -> Sensitivity {
        if self.automatic_sensitivity.load(Ordering::Relaxed) {
            Sensitivity::Automatic
        } else {
            Sensitivity::Manual(f32::from_bits(self.threshold.load(Ordering::Relaxed)))
        }
    }

    /// The counter the capture path watches.
    #[must_use]
    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AudioPrefs, AudioSettings, Sensitivity, DEFAULT_THRESHOLD, MAX_THRESHOLD, MIN_THRESHOLD,
    };

    #[test]
    fn the_defaults_are_the_call_that_needs_no_settings_screen() {
        let settings = AudioSettings::default();
        assert!(settings.automatic_sensitivity);
        assert!(settings.noise_suppression);
        assert!(settings.echo_cancellation, "speakers are the common case");
    }

    /// Exactly, to `f32::EPSILON`. Every expectation here is a constant that
    /// `sane` either clamps to or leaves alone, so there is no arithmetic in
    /// between for the last bits to drift in.
    fn is(left: f32, right: f32) -> bool {
        (left - right).abs() < f32::EPSILON
    }

    #[test]
    fn a_threshold_outside_the_slider_is_pulled_back_onto_it() {
        assert!(
            is(
                AudioSettings {
                    threshold: 0.0,
                    ..AudioSettings::default()
                }
                .sane()
                .threshold,
                MIN_THRESHOLD
            ),
            "zero is a gate that never closes"
        );
        assert!(
            is(
                AudioSettings {
                    threshold: 9.0,
                    ..AudioSettings::default()
                }
                .sane()
                .threshold,
                MAX_THRESHOLD
            ),
            "a gate nothing can open is a client that went silent"
        );
    }

    #[test]
    fn a_threshold_that_is_not_a_number_reads_as_the_default() {
        // `clamp` panics on a NaN bound and propagates a NaN value, and a NaN
        // threshold compares false against everything — a microphone that is
        // never loud enough, for the rest of the call.
        let settings = AudioSettings {
            threshold: f32::NAN,
            ..AudioSettings::default()
        }
        .sane();
        assert!(is(settings.threshold, DEFAULT_THRESHOLD));
    }

    #[test]
    fn automatic_asks_the_detector_and_manual_asks_the_number() {
        let prefs = AudioPrefs::default();
        assert_eq!(prefs.sensitivity(), Sensitivity::Automatic);

        prefs.set(AudioSettings {
            automatic_sensitivity: false,
            threshold: 0.05,
            ..AudioSettings::default()
        });
        assert_eq!(prefs.sensitivity(), Sensitivity::Manual(0.05));
    }

    #[test]
    fn turning_automatic_back_off_returns_to_the_number_the_user_chose() {
        let prefs = AudioPrefs::default();
        prefs.set(AudioSettings {
            automatic_sensitivity: false,
            threshold: 0.11,
            ..AudioSettings::default()
        });
        prefs.set(AudioSettings {
            automatic_sensitivity: true,
            threshold: 0.11,
            ..AudioSettings::default()
        });
        assert_eq!(prefs.sensitivity(), Sensitivity::Automatic);

        prefs.set(prefs.settings().sane());
        let back = AudioSettings {
            automatic_sensitivity: false,
            ..prefs.settings()
        };
        prefs.set(back);
        assert_eq!(
            prefs.sensitivity(),
            Sensitivity::Manual(0.11),
            "the slider forgot where it was"
        );
    }

    #[test]
    fn every_write_moves_the_counter_the_capture_path_watches() {
        let prefs = AudioPrefs::default();
        let before = prefs.generation();
        prefs.set(AudioSettings::default());
        assert_eq!(prefs.generation(), before + 1);
    }
}
