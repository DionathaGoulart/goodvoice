//! The seam between the voice path and whatever produces or consumes audio.
//!
//! `rtc::session` talks to these two traits and never to a device API, so the
//! call works identically against real hardware ([`super::hardware`]) and
//! against the synthetic ends used by tests and the 2.3 spike. Task 2.1 decides
//! what backs them on Windows; nothing above this line has to change when it
//! does — see DR-8 in .harness/plan.md.

use std::{
    f32::consts::TAU,
    sync::{Arc, Mutex},
    time::Duration,
};

use super::opus::{silent_frame, Frame, FRAME_MS};

/// How many remote speakers one client mixes at once: everyone in a full room
/// but yourself. The room's own cap of 8 is enforced server-side (prd.md §3
/// F1); this is the client sizing its playback to match.
pub const MAX_REMOTE_SLOTS: usize = 7;

/// Where the microphone's 20 ms frames come from.
///
/// Implementations pace themselves — a hardware source is woken by its capture
/// callback, a synthetic one by a timer — so the encode loop never has to hold
/// a clock of its own and cannot drift against the device.
#[async_trait::async_trait]
pub trait AudioSource: Send {
    /// The next 20 ms of audio, or `None` once the source is finished.
    async fn next_frame(&mut self) -> Option<Frame>;
}

/// Where decoded remote audio goes.
///
/// One slot per remote speaker, assigned by the session when it subscribes.
/// The sink never learns who a slot belongs to: keeping identity out of the
/// playback path is what lets the render callback stay lock-free.
///
/// `play` is called from a decode task, never from a device callback, so it may
/// take a lock. It must never block for long — a sink that cannot keep up drops
/// the frame rather than stalling the decoder.
pub trait AudioSink: Send + Sync {
    /// Queues one frame for playback in `slot`.
    fn play(&self, slot: usize, frame: &Frame);

    /// Drops whatever is still queued for `slot`, so a peer that leaves does
    /// not keep talking for the length of the buffer.
    fn clear(&self, slot: usize);

    /// How loud `slot` has been playing lately, `0.0`–`1.0`.
    ///
    /// This is what the roster's speaking indicator is built on. A sink that
    /// does not play anything has nothing to meter, so the default is silence
    /// rather than something for every test double to implement.
    fn level(&self, _slot: usize) -> f32 {
        0.0
    }

    /// Turns one speaker up or down. Defaults to doing nothing, so a sink with
    /// no mixer behind it stays as simple as it looks.
    fn set_gain(&self, _slot: usize, _gain: f32) {}
}

// --- synthetic ends --------------------------------------------------------

/// A sine wave, paced by a timer, for tests and the SFU spike.
pub struct ToneSource {
    frequency_hz: f32,
    amplitude: f32,
    phase: f32,
    ticker: tokio::time::Interval,
}

impl ToneSource {
    /// A tone at `frequency_hz`, loud enough to survive the codec.
    #[must_use]
    pub fn new(frequency_hz: f32) -> Self {
        Self {
            frequency_hz,
            // From `tone`, because the harnesses that measure what came back
            // of this build their reference frame from the same constant.
            amplitude: super::tone::AMPLITUDE,
            phase: 0.0,
            ticker: tokio::time::interval(Duration::from_millis(u64::from(FRAME_MS))),
        }
    }
}

#[async_trait::async_trait]
impl AudioSource for ToneSource {
    async fn next_frame(&mut self) -> Option<Frame> {
        self.ticker.tick().await;

        let mut frame = silent_frame();
        #[allow(
            clippy::cast_possible_truncation,
            reason = "amplitude is bounded well inside i16 by construction"
        )]
        for sample in &mut frame {
            *sample = (self.phase.sin() * self.amplitude) as i16;
            #[allow(
                clippy::cast_precision_loss,
                reason = "48000 is exactly representable as f32"
            )]
            {
                self.phase += TAU * self.frequency_hz / super::opus::SAMPLE_RATE_HZ as f32;
            }
        }
        Some(frame)
    }
}

/// What one slot of a [`RecordingSink`] heard.
#[derive(Debug, Clone, Copy)]
pub struct SlotRecord {
    /// Frames delivered to this slot since the last [`AudioSink::clear`].
    pub frames: usize,
    /// The most recent frame, for a caller that wants to inspect the signal.
    pub last: Frame,
}

impl Default for SlotRecord {
    fn default() -> Self {
        Self {
            frames: 0,
            last: silent_frame(),
        }
    }
}

/// A sink that remembers instead of playing. The spike and the integration
/// tests assert against this; nothing ships with it.
#[derive(Default)]
pub struct RecordingSink {
    slots: Mutex<Vec<SlotRecord>>,
}

impl RecordingSink {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            slots: Mutex::new(vec![SlotRecord::default(); MAX_REMOTE_SLOTS]),
        })
    }

    /// What `slot` has heard so far.
    ///
    /// # Panics
    ///
    /// Panics if `slot` is outside [`MAX_REMOTE_SLOTS`], or if another thread
    /// panicked while holding the record.
    #[must_use]
    pub fn slot(&self, slot: usize) -> SlotRecord {
        self.slots.lock().expect("recording sink poisoned")[slot]
    }

    /// The busiest slot, and what it heard.
    ///
    /// Which slot a given speaker lands in is an implementation detail of the
    /// session, and it changes across a reconnect — so a caller that only wants
    /// to know "is audio arriving" asks this rather than guessing an index.
    #[must_use]
    pub fn loudest(&self) -> SlotRecord {
        let Ok(slots) = self.slots.lock() else {
            return SlotRecord::default();
        };
        slots
            .iter()
            .max_by_key(|record| record.frames)
            .copied()
            .unwrap_or_default()
    }

    /// Forgets every slot, so a caller can measure a second stretch of a call
    /// without the first one counting towards it.
    pub fn reset(&self) {
        if let Ok(mut slots) = self.slots.lock() {
            for record in slots.iter_mut() {
                *record = SlotRecord::default();
            }
        }
    }
}

impl AudioSink for RecordingSink {
    fn play(&self, slot: usize, frame: &Frame) {
        let Ok(mut slots) = self.slots.lock() else {
            return;
        };
        let Some(record) = slots.get_mut(slot) else {
            return;
        };
        record.frames += 1;
        record.last = *frame;
    }

    fn clear(&self, slot: usize) {
        let Ok(mut slots) = self.slots.lock() else {
            return;
        };
        if let Some(record) = slots.get_mut(slot) {
            *record = SlotRecord::default();
        }
    }
}

/// A sink that throws audio away, for a client that is deafened or headless.
pub struct NullSink;

impl AudioSink for NullSink {
    fn play(&self, _slot: usize, _frame: &Frame) {}
    fn clear(&self, _slot: usize) {}
}

#[cfg(test)]
mod tests {
    use super::{AudioSink, AudioSource, RecordingSink, ToneSource, MAX_REMOTE_SLOTS};
    use crate::audio::opus::{silent_frame, FRAME_SAMPLES};

    #[tokio::test]
    async fn a_tone_source_fills_whole_frames() {
        let mut source = ToneSource::new(440.0);
        let frame = source.next_frame().await.expect("a frame");

        assert_eq!(frame.len(), FRAME_SAMPLES);
        assert!(
            frame.iter().any(|&sample| sample != 0),
            "the tone was silent"
        );
    }

    #[tokio::test]
    async fn a_tone_source_is_continuous_across_frames() {
        let mut source = ToneSource::new(440.0);
        let first = source.next_frame().await.expect("a frame");
        let second = source.next_frame().await.expect("a frame");

        // A restarted phase would repeat the frame exactly.
        assert_ne!(first, second, "the tone restarted instead of continuing");
    }

    #[test]
    fn a_recording_sink_keeps_slots_apart() {
        let sink = RecordingSink::new();
        let mut loud = silent_frame();
        loud[0] = 1_234;

        sink.play(0, &loud);
        sink.play(0, &loud);
        sink.play(1, &silent_frame());

        assert_eq!(sink.slot(0).frames, 2);
        assert_eq!(sink.slot(0).last[0], 1_234);
        assert_eq!(sink.slot(1).frames, 1);
        assert_eq!(sink.slot(2).frames, 0);
    }

    #[test]
    fn clearing_a_slot_forgets_only_that_slot() {
        let sink = RecordingSink::new();
        sink.play(0, &silent_frame());
        sink.play(1, &silent_frame());

        sink.clear(0);

        assert_eq!(sink.slot(0).frames, 0);
        assert_eq!(sink.slot(1).frames, 1);
    }

    #[test]
    fn the_loudest_slot_is_found_without_being_named() {
        let sink = RecordingSink::new();
        let mut loud = silent_frame();
        loud[0] = 4_242;

        sink.play(3, &silent_frame());
        sink.play(5, &loud);
        sink.play(5, &loud);

        let busiest = sink.loudest();
        assert_eq!(busiest.frames, 2);
        assert_eq!(busiest.last[0], 4_242);
    }

    #[test]
    fn resetting_forgets_every_slot_at_once() {
        let sink = RecordingSink::new();
        sink.play(0, &silent_frame());
        sink.play(4, &silent_frame());

        sink.reset();

        assert_eq!(sink.loudest().frames, 0);
    }

    #[test]
    fn an_out_of_range_slot_is_ignored_rather_than_fatal() {
        let sink = RecordingSink::new();
        // A slot past the cap can only come from a bug upstream; dropping the
        // frame is survivable, panicking in a decode task is not.
        sink.play(MAX_REMOTE_SLOTS, &silent_frame());
        sink.clear(MAX_REMOTE_SLOTS);
    }
}
