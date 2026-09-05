//! Summing several remote speakers into one signal, and measuring them.
//!
//! One ring per speaker, filled by that speaker's decode task and drained by
//! the render callback. This module is the whole of what happens between those
//! two: per-speaker gain, a saturating sum, and a level per slot so the roster
//! can show who is talking (plan.md task 3.1).
//!
//! It knows nothing about audio devices. [`Mixer::render`] takes a buffer and
//! fills it, which is what makes the mixing rules testable without hardware —
//! `hardware.rs` is only the part that hands cpal's callback that buffer.
//!
//! # Real-time discipline
//!
//! [`Mixer::render`] runs inside a device callback: no allocation, no locking,
//! no logging (styleguide.md). Gains and levels cross the boundary as atomics
//! for that reason, and the producer side ([`Playback`]) never touches the read
//! end of a ring.

use std::sync::{
    atomic::{AtomicBool, AtomicU16, Ordering},
    Arc, Mutex,
};

use ringbuf::{
    traits::{Consumer, Observer, Producer, Split},
    HeapCons, HeapProd, HeapRb,
};

use super::{health, opus::Frame};

/// The largest block the render path handles in one pass. Bigger device
/// buffers are processed in several, so the scratch array stays on the stack.
pub const MAX_BLOCK: usize = 1024;

/// Unity gain in Q8.8: `256` means "leave it alone".
const UNITY: u16 = 256;

/// The loudest a single speaker may be turned up. Four times is enough to
/// rescue someone with a quiet microphone and not enough to make them a
/// weapon.
pub const MAX_GAIN: f32 = 4.0;

/// How fast the *decision* level falls when a speaker stops. One 64th per
/// block: at a typical 10 ms callback that is a couple of seconds to fade from
/// full scale, which is what stops "who is talking" flickering between words.
const LEVEL_DECAY: u16 = 64;

/// How fast the *displayed* level falls. A quarter per block: from full scale
/// down to the speaking threshold in about 14 blocks, which is 140 ms of a
/// remote speaker's 10 ms render callbacks and 280 ms of this client's own
/// 20 ms frames.
///
/// Two decays rather than one because the same meter answers two questions
/// with opposite needs. "Is this person talking" wants to hold on through the
/// gap between two words. "How loud are they right now" is a meter, and a
/// meter that takes seconds to come down is not showing the voice anyone is
/// looking at — it is showing the loudest thing that happened recently.
const DISPLAY_DECAY: u16 = 4;

/// Where a level crosses from "background" into "talking". Roughly -34 dBFS,
/// low enough to catch a quiet talker and high enough to ignore the noise
/// floor of a cheap headset.
pub const SPEAKING_LEVEL: f32 = 0.02;

/// Full scale, as a float divisor. `i16::MAX` rather than `32768` so a
/// full-scale sample reads as exactly 1.0.
const FULL_SCALE: f32 = i16::MAX as f32;

/// How loud one stream has been lately, in a form a device callback can write
/// and any thread can read.
///
/// Attack is immediate on both halves. Release is not: [`Meter::is_speaking`]
/// falls slowly, because missing the first syllable of a sentence is worse
/// than holding a light on for a moment after it, while [`Meter::level`] falls
/// quickly, because it is drawn as a level and a level that lags behind the
/// voice is read as a bug in the app.
#[derive(Debug, Default)]
pub struct Meter {
    /// Slow release. What "is this person talking" is decided on.
    held: AtomicU16,
    /// Fast release. What gets drawn.
    shown: AtomicU16,
}

impl Meter {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            held: AtomicU16::new(0),
            shown: AtomicU16::new(0),
        }
    }

    /// Feeds the meter the loudest sample of one block.
    pub fn observe(&self, peak: u16) {
        Self::advance(&self.held, peak, LEVEL_DECAY);
        Self::advance(&self.shown, peak, DISPLAY_DECAY);
    }

    /// One peak into one decaying store: `peak.max` is the attack, the
    /// subtraction is the release.
    ///
    /// At least one step, or the release stalls. `previous / decay` is integer
    /// division, so below `decay` it is zero and the level stops falling —
    /// which left the old meter resting at 63/32767 for the rest of the call.
    /// That was invisible under a threshold and is not under a fading dot.
    /// `saturating_sub` is what makes the floor case safe: at zero there is
    /// nothing left to take.
    fn advance(store: &AtomicU16, peak: u16, decay: u16) {
        let previous = store.load(Ordering::Relaxed);
        let decayed = previous.saturating_sub((previous / decay).max(1));
        store.store(peak.max(decayed), Ordering::Relaxed);
    }

    /// The level to draw, `0.0`–`1.0`.
    #[must_use]
    pub fn level(&self) -> f32 {
        f32::from(self.shown.load(Ordering::Relaxed)) / FULL_SCALE
    }

    /// Whether this stream is loud enough to call talking.
    #[must_use]
    pub fn is_speaking(&self) -> bool {
        f32::from(self.held.load(Ordering::Relaxed)) / FULL_SCALE >= SPEAKING_LEVEL
    }

    /// Back to silence at once, for a stream that stopped rather than faded.
    pub fn reset(&self) {
        self.held.store(0, Ordering::Relaxed);
        self.shown.store(0, Ordering::Relaxed);
    }
}

/// The loudest sample in a block, as a meter wants it.
#[must_use]
pub fn peak(samples: &[i16]) -> u16 {
    samples
        .iter()
        .fold(0, |loudest, &sample| loudest.max(sample.unsigned_abs()))
}

/// What the two ends of the mixer share: one gain, one level and one drain
/// request per slot.
#[derive(Debug)]
struct Meters {
    gains: Vec<AtomicU16>,
    levels: Vec<Meter>,
    /// Set by [`Playback::clear`], acted on by [`Mixer::render`]. Only the
    /// consumer end may throw queued audio away, and the consumer end is the
    /// device callback.
    drain: Vec<AtomicBool>,
}

impl Meters {
    fn new(slots: usize) -> Self {
        Self {
            gains: (0..slots).map(|_| AtomicU16::new(UNITY)).collect(),
            levels: (0..slots).map(|_| Meter::new()).collect(),
            drain: (0..slots).map(|_| AtomicBool::new(false)).collect(),
        }
    }
}

/// Builds a mixer and the handle that feeds it.
///
/// `ring_samples` is the depth of each speaker's buffer; `slots` is how many
/// speakers can be heard at once.
#[must_use]
pub fn open(slots: usize, ring_samples: usize) -> (Mixer, Playback) {
    let meters = Arc::new(Meters::new(slots));
    let mut producers = Vec::with_capacity(slots);
    let mut consumers = Vec::with_capacity(slots);

    for _ in 0..slots {
        let (producer, consumer) = HeapRb::<i16>::new(ring_samples).split();
        producers.push(Mutex::new(producer));
        consumers.push(consumer);
    }

    (
        Mixer {
            slots: consumers,
            taken: [0; MAX_BLOCK],
            meters: Arc::clone(&meters),
        },
        Playback {
            slots: producers,
            meters,
        },
    )
}

/// The consumer end: everything the render callback does.
pub struct Mixer {
    slots: Vec<HeapCons<i16>>,
    /// Scratch for one slot's samples. Fixed size so the callback never
    /// allocates; [`Mixer::render`] refuses a block bigger than it.
    taken: [i16; MAX_BLOCK],
    meters: Arc<Meters>,
}

impl Mixer {
    /// Fills `mono` with every speaker summed together.
    ///
    /// Blocks longer than [`MAX_BLOCK`] are refused rather than truncated: the
    /// caller chunks, and silently dropping the tail would be an audible glitch
    /// nobody could find. A slot with nothing queued contributes silence —
    /// repeating its last samples would be a far more audible artefact than a
    /// gap.
    pub fn render(&mut self, mono: &mut [i16]) {
        if mono.len() > MAX_BLOCK {
            return;
        }
        mono.fill(0);

        let taken = &mut self.taken[..mono.len()];
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if self.meters.drain[index].swap(false, Ordering::AcqRel) {
                slot.clear();
                self.meters.levels[index].reset();
            }

            let read = slot.pop_slice(taken);
            // Half a block is a ring that ran out mid-word, which is audible
            // every time. An empty one is a slot whose owner is not talking,
            // and is the ordinary state of most slots most of the time — see
            // `health::starved`.
            if read > 0 && read < taken.len() {
                health::starved();
            }
            health::deep(slot.occupied_len());

            let gain = self.meters.gains[index].load(Ordering::Relaxed);
            let mut loudest = 0_u16;

            for (out, &sample) in mono[..read].iter_mut().zip(taken[..read].iter()) {
                let scaled = amplify(sample, gain);
                loudest = loudest.max(scaled.unsigned_abs());
                *out = out.saturating_add(scaled);
            }

            self.meters.levels[index].observe(loudest);
        }
    }
}

/// The producer end: what the decode tasks and the UI hold.
pub struct Playback {
    slots: Vec<Mutex<HeapProd<i16>>>,
    meters: Arc<Meters>,
}

impl Playback {
    /// Queues one frame for `slot`.
    ///
    /// A slot that has fallen far enough behind to fill its ring is not going
    /// to catch up by being given more, so the overflow is dropped: the delay
    /// stays bounded rather than growing for the rest of the call.
    pub fn play(&self, slot: usize, frame: &Frame) {
        let Some(producer) = self.slots.get(slot) else {
            return;
        };
        let Ok(mut producer) = producer.lock() else {
            return;
        };
        health::played();
        // What `push_slice` does with a frame that will not fit is write the
        // part that does, which is a discontinuity in the middle of 20 ms
        // rather than at the edge of it. Refusing the whole frame keeps the
        // break on a boundary, and counting it is what tells the difference
        // between a link that is behind and a clock that is fast.
        if producer.vacant_len() < frame.len() {
            health::overflowed();
            return;
        }
        producer.push_slice(frame);
    }

    /// How many samples `slot` has queued but not yet played.
    ///
    /// Measurement apparatus, not voice path. Task 2.1 times a burst from the
    /// moment it is handed over, so anything the ring was already holding is
    /// inside that number; this is what makes it separable from what the
    /// device below cpal costs.
    #[must_use]
    pub fn queued(&self, slot: usize) -> usize {
        self.slots
            .get(slot)
            .and_then(|producer| producer.lock().ok())
            .map_or(0, |producer| producer.occupied_len())
    }

    /// Throws away whatever `slot` still has queued, so a speaker who left
    /// stops mid-word rather than finishing out the buffer.
    pub fn clear(&self, slot: usize) {
        if let Some(flag) = self.meters.drain.get(slot) {
            flag.store(true, Ordering::Release);
        }
    }

    /// How loud `slot` has been playing lately, `0.0`–`1.0`.
    #[must_use]
    pub fn level(&self, slot: usize) -> f32 {
        self.meters.levels.get(slot).map_or(0.0, Meter::level)
    }

    /// Whether `slot` is loud enough to call talking.
    #[must_use]
    pub fn is_speaking(&self, slot: usize) -> bool {
        self.meters.levels.get(slot).is_some_and(Meter::is_speaking)
    }

    /// Turns one speaker up or down, `0.0`–[`MAX_GAIN`]. Out-of-range values
    /// are clamped rather than refused: a slider is not a place to fail.
    pub fn set_gain(&self, slot: usize, gain: f32) {
        let Some(current) = self.meters.gains.get(slot) else {
            return;
        };
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to 0..=MAX_GAIN, which is well inside u16 in Q8.8"
        )]
        let quantised = (gain.clamp(0.0, MAX_GAIN) * f32::from(UNITY)) as u16;
        current.store(quantised, Ordering::Relaxed);
    }

    /// The gain `slot` is playing at.
    #[must_use]
    pub fn gain(&self, slot: usize) -> f32 {
        self.meters.gains.get(slot).map_or(0.0, |gain| {
            f32::from(gain.load(Ordering::Relaxed)) / f32::from(UNITY)
        })
    }

    /// How many speakers this playback can carry at once.
    #[must_use]
    pub fn slots(&self) -> usize {
        self.slots.len()
    }
}

/// Applies a Q8.8 gain, saturating instead of wrapping.
///
/// Integer rather than float because this runs per sample inside a device
/// callback, and because a gain the user set as `1.0` should be exactly
/// transparent rather than nearly so.
fn amplify(sample: i16, gain: u16) -> i16 {
    if gain == UNITY {
        return sample;
    }
    let scaled = (i32::from(sample) * i32::from(gain)) >> 8;
    #[allow(
        clippy::cast_possible_truncation,
        reason = "clamped into i16's range on the line above"
    )]
    {
        scaled.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
    }
}

#[cfg(test)]
mod tests {
    use super::{open, MAX_BLOCK, MAX_GAIN, SPEAKING_LEVEL};
    use crate::audio::opus::{silent_frame, FRAME_SAMPLES};

    /// Deep enough for several frames per slot in a test.
    const RING: usize = FRAME_SAMPLES * 4;

    /// A frame at a constant amplitude, so a test can reason about the number
    /// that comes out the other end.
    fn tone(amplitude: i16) -> [i16; FRAME_SAMPLES] {
        let mut frame = silent_frame();
        for (index, sample) in frame.iter_mut().enumerate() {
            *sample = if index % 2 == 0 {
                amplitude
            } else {
                -amplitude
            };
        }
        frame
    }

    #[test]
    fn an_empty_mixer_renders_silence() {
        let (mut mixer, _playback) = open(2, RING);
        let mut out = [1_234_i16; 64];

        mixer.render(&mut out);

        // Not the previous contents of the buffer: an underrun is a gap, and a
        // repeated buffer is the more audible artefact.
        assert!(out.iter().all(|&sample| sample == 0));
    }

    #[test]
    fn two_speakers_are_summed() {
        let (mut mixer, playback) = open(2, RING);
        playback.play(0, &tone(1_000));
        playback.play(1, &tone(1_500));

        let mut out = [0_i16; 64];
        mixer.render(&mut out);

        assert_eq!(out[0], 2_500);
        assert_eq!(out[1], -2_500);
    }

    #[test]
    fn a_loud_room_saturates_instead_of_wrapping() {
        let (mut mixer, playback) = open(4, RING);
        for slot in 0..4 {
            playback.play(slot, &tone(30_000));
        }

        let mut out = [0_i16; 32];
        mixer.render(&mut out);

        // Wrapping would put a loud *negative* sample here, which is the click
        // this saturation exists to prevent.
        assert_eq!(out[0], i16::MAX);
        assert_eq!(out[1], i16::MIN);
    }

    #[test]
    fn gain_turns_one_speaker_down_without_touching_the_others() {
        let (mut mixer, playback) = open(2, RING);
        playback.set_gain(0, 0.5);
        playback.play(0, &tone(1_000));
        playback.play(1, &tone(1_000));

        let mut out = [0_i16; 32];
        mixer.render(&mut out);

        assert_eq!(out[0], 1_500, "slot 0 should be halved, slot 1 untouched");
        assert!((playback.gain(0) - 0.5).abs() < f32::EPSILON);
        assert!((playback.gain(1) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn gain_zero_silences_one_speaker_entirely() {
        let (mut mixer, playback) = open(2, RING);
        playback.set_gain(0, 0.0);
        playback.play(0, &tone(8_000));

        let mut out = [0_i16; 32];
        mixer.render(&mut out);

        assert!(out.iter().all(|&sample| sample == 0));
        // Silenced is silent on the meter too — a speaker turned all the way
        // down must not show up as talking.
        assert!(!playback.is_speaking(0));
    }

    #[test]
    fn gain_is_clamped_rather_than_refused() {
        let (_mixer, playback) = open(1, RING);

        playback.set_gain(0, -3.0);
        assert!((playback.gain(0) - 0.0).abs() < f32::EPSILON);

        playback.set_gain(0, 99.0);
        assert!((playback.gain(0) - MAX_GAIN).abs() < f32::EPSILON);
    }

    #[test]
    fn a_speaker_registers_on_the_meter_and_fades_when_they_stop() {
        let (mut mixer, playback) = open(1, RING);
        playback.play(0, &tone(20_000));

        // One whole frame in, one whole frame out, so the ring is empty and
        // every render after this one is genuine silence.
        let mut out = [0_i16; FRAME_SAMPLES];
        mixer.render(&mut out);

        let talking = playback.level(0);
        assert!(
            talking > SPEAKING_LEVEL,
            "a speaker at -4 dBFS read {talking}"
        );
        assert!(playback.is_speaking(0));

        // Silence from here on. The drawn level has to come down on its own
        // and not all at once — a hard reset would be a dot that snaps to grey
        // mid-word — but it comes down quickly, because it is a meter.
        mixer.render(&mut out);
        let straight_after = playback.level(0);
        assert!(straight_after < talking, "the meter did not move at all");
        assert!(
            straight_after > talking * 0.5,
            "the drawn level halved in one block; that is a jump, not a release"
        );

        // The *decision* behind it is the slow one, and still holding: this is
        // the gap between two words, and nobody has stopped talking.
        assert!(
            playback.is_speaking(0),
            "one silent block put the speaking indicator out"
        );

        for _ in 0..600 {
            mixer.render(&mut out);
        }
        assert!(!playback.is_speaking(0), "a silent speaker never stopped");
        assert!(
            playback.level(0) < f32::EPSILON,
            "the drawn level never reached zero"
        );
    }

    #[test]
    fn a_quiet_slot_is_not_speaking() {
        let (mut mixer, playback) = open(1, RING);
        // Below the threshold: the noise floor of a cheap headset must not
        // light up the roster.
        playback.play(0, &tone(200));

        let mut out = [0_i16; 128];
        mixer.render(&mut out);

        assert!(playback.level(0) > 0.0, "it is audible, just quiet");
        assert!(!playback.is_speaking(0));
    }

    #[test]
    fn clearing_a_slot_drops_its_audio_and_its_level() {
        let (mut mixer, playback) = open(2, RING);
        playback.play(0, &tone(20_000));

        let mut out = [0_i16; 128];
        mixer.render(&mut out);
        assert!(playback.is_speaking(0));

        // The speaker left mid-word.
        playback.clear(0);
        mixer.render(&mut out);

        assert!(out.iter().all(|&sample| sample == 0));
        assert!(playback.level(0) < f32::EPSILON);
    }

    #[test]
    fn a_slot_that_does_not_exist_is_ignored_rather_than_fatal() {
        let (_mixer, playback) = open(1, RING);

        // Only reachable from a bug upstream; dropping the frame is
        // survivable, panicking in a decode task is not.
        playback.play(9, &tone(1_000));
        playback.clear(9);
        playback.set_gain(9, 2.0);
        assert!(playback.level(9) < f32::EPSILON);
    }

    #[test]
    fn a_block_bigger_than_the_scratch_is_refused_rather_than_half_filled() {
        let (mut mixer, playback) = open(1, RING);
        playback.play(0, &tone(1_000));

        let mut out = vec![7_i16; MAX_BLOCK + 1];
        mixer.render(&mut out);

        // Untouched: the caller chunks, and a half-filled buffer would be a
        // glitch nobody could trace back to here.
        assert!(out.iter().all(|&sample| sample == 7));
    }

    #[test]
    fn a_short_read_leaves_the_rest_of_the_block_silent() {
        let (mut mixer, playback) = open(1, RING);
        playback.play(0, &tone(5_000));

        // Ask for more than one frame's worth: the tail has nothing behind it.
        let mut out = [0_i16; MAX_BLOCK];
        mixer.render(&mut out);

        assert_ne!(out[0], 0);
        assert!(out[FRAME_SAMPLES..].iter().all(|&sample| sample == 0));
    }

    #[test]
    fn a_frame_that_will_not_fit_is_refused_whole_rather_than_torn() {
        // Room for one frame and half of another.
        let (_mixer, playback) = open(1, FRAME_SAMPLES + FRAME_SAMPLES / 2);
        playback.play(0, &tone(1_000));

        // Writing the half that fits would put the break in the middle of
        // 20 ms rather than at the edge of it, which is the more audible of
        // the two and the harder one to count.
        playback.play(0, &tone(2_000));

        assert_eq!(
            playback.queued(0),
            FRAME_SAMPLES,
            "half a frame was written into the ring"
        );
    }
}
