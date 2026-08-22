//! A short loud burst, and the bookkeeping for timing one.
//!
//! Two harnesses time the same shape of thing. Task 2.5's `bin/latency` sends
//! a burst through the SFU and reads it back on another client; task 2.1's
//! `bin/audio-spike` sends one out of the speakers and reads it back off the
//! microphone. What they have in common lives here: the burst itself, the rule
//! that only its leading edge counts, and the "at most one in the air" pairing
//! that lets a burst be matched to its arrival without carrying any identity in
//! the signal.
//!
//! Nothing here is on the voice path — it is measurement apparatus, and it is
//! in the library rather than in one of the binaries so the logic worth getting
//! right is tested once instead of twice.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

use super::opus::{silent_frame, Frame, SAMPLE_RATE_HZ};

/// How long a burst lasts. Five milliseconds is long enough to survive Opus as
/// something obviously loud, and short enough that its leading edge lands in
/// exactly one 20 ms frame.
pub const BURST_MS: usize = 5;
pub const BURST_SAMPLES: usize = (SAMPLE_RATE_HZ as usize * BURST_MS) / 1000;

/// A tone rather than an impulse: an impulse is mostly high frequency, which is
/// the first thing a codec at 32 kbps throws away.
pub const BURST_HZ: f32 = 1_000.0;

/// How loud the burst is, and how loud a frame has to be before a path with
/// nothing on it but digital silence counts one. An acoustic path has a noise
/// floor and wants a threshold measured against it instead — see
/// [`Edge::set_threshold`].
pub const BURST_AMPLITUDE: i16 = 20_000;
pub const SILENT_PATH_THRESHOLD: u16 = 4_000;

/// One frame, silent except for a burst at sample zero.
///
/// The burst sits at the very start on purpose: the instant the frame is handed
/// over is then the instant the burst begins, and the far end's frame boundary
/// means the same thing. Nothing has to be corrected for where inside a frame
/// the sound fell.
#[must_use]
pub fn burst_frame() -> Frame {
    let mut frame = silent_frame();
    for (index, sample) in frame.iter_mut().take(BURST_SAMPLES).enumerate() {
        #[allow(
            clippy::cast_precision_loss,
            reason = "an index inside one frame is exact in f32"
        )]
        let phase = std::f32::consts::TAU * BURST_HZ * index as f32 / SAMPLE_RATE_HZ as f32;
        #[allow(
            clippy::cast_possible_truncation,
            reason = "bounded by BURST_AMPLITUDE"
        )]
        {
            *sample = (phase.sin() * f32::from(BURST_AMPLITUDE)) as i16;
        }
    }
    frame
}

const _: () = {
    // The burst has to fit inside one frame, or its leading edge is not where
    // a harness timing it thinks it is.
    assert!(BURST_SAMPLES < crate::audio::opus::FRAME_SAMPLES);
};

/// Where inside a run of samples a burst starts, if it starts there.
///
/// A caller timing an arrival at frame granularity is quantised to the length
/// of a frame, which is 20 ms of a number this small. Knowing where in the
/// frame the burst fell is what buys that back.
#[must_use]
pub fn onset(samples: &[i16], threshold: u16) -> Option<usize> {
    samples
        .iter()
        .position(|sample| sample.unsigned_abs() >= threshold)
}

/// Notices a burst arriving, once.
///
/// A 5 ms burst can straddle two frames, and a listener that counted every loud
/// frame would time the same burst twice and then pair the second one against
/// whatever came next. Only the transition from quiet to loud counts.
#[derive(Debug)]
pub struct Edge {
    threshold: AtomicU16,
    inside: AtomicBool,
}

impl Edge {
    #[must_use]
    pub fn new(threshold: u16) -> Self {
        Self {
            threshold: AtomicU16::new(threshold),
            inside: AtomicBool::new(false),
        }
    }

    /// Raises or lowers the bar, for a path whose noise floor is only known
    /// once something has listened to it.
    pub fn set_threshold(&self, threshold: u16) {
        self.threshold.store(threshold, Ordering::Relaxed);
    }

    /// Where the burst starts in this frame, if this frame is the start of
    /// one. `None` for silence, and `None` again for the tail of a burst
    /// already counted.
    ///
    /// `swap` rather than load-then-store: two decode tasks must not both
    /// decide they saw the same edge.
    pub fn crossed(&self, frame: &[i16]) -> Option<usize> {
        let Some(index) = onset(frame, self.threshold.load(Ordering::Relaxed)) else {
            self.inside.store(false, Ordering::Relaxed);
            return None;
        };
        if self.inside.swap(true, Ordering::Relaxed) {
            return None;
        }
        Some(index)
    }
}

/// The one burst that may be in the air, and the tally of the ones that were
/// not heard.
///
/// Only one burst is ever in flight, which is what lets an arrival be paired
/// with a departure without either side carrying a sequence number. A burst
/// still out when the next one leaves, or one that has been out too long, is
/// written off rather than paired with something it is not.
#[derive(Debug, Default)]
pub struct Flight {
    sent: Mutex<Option<Instant>>,
    lost: AtomicUsize,
}

impl Flight {
    /// Records a burst leaving.
    pub fn depart(&self) {
        let Ok(mut sent) = self.sent.lock() else {
            return;
        };
        if sent.is_some() {
            self.lost.fetch_add(1, Ordering::Relaxed);
        }
        *sent = Some(Instant::now());
    }

    /// Claims the burst in flight, if there is one, and returns how long it
    /// took. `None` means audio arrived that no burst explains.
    #[must_use]
    pub fn arrive(&self) -> Option<Duration> {
        let mut sent = self.sent.lock().ok()?;
        sent.take().map(|departed| departed.elapsed())
    }

    /// Gives up on a burst that has been out longer than `patience`, so the
    /// next one is not paired against it.
    pub fn expire(&self, patience: Duration) {
        let Ok(mut sent) = self.sent.lock() else {
            return;
        };
        if sent.is_some_and(|departed| departed.elapsed() > patience) {
            *sent = None;
            self.lost.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[must_use]
    pub fn lost(&self) -> usize {
        self.lost.load(Ordering::Relaxed)
    }
}

/// min / median / p95 / max of what a harness timed, in milliseconds.
///
/// Both harnesses report the same five numbers, and getting a percentile
/// slightly wrong in two places independently is exactly the kind of thing
/// nobody notices in a table of plausible milliseconds.
#[derive(Debug, Clone, Copy)]
pub struct Spread {
    pub min: f64,
    pub median: f64,
    pub p95: f64,
    pub max: f64,
    pub count: usize,
}

impl Spread {
    /// `None` when nothing was heard: a report with no measurement in it is a
    /// failure to measure, not a measurement of zero.
    #[must_use]
    pub fn of(times: &[Duration]) -> Option<Self> {
        if times.is_empty() {
            return None;
        }
        let mut sorted = times.to_vec();
        sorted.sort_unstable();

        let ms = |duration: Duration| duration.as_secs_f64() * 1_000.0;
        let at = |fraction: f64| {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "an index into a vector of at most a few hundred"
            )]
            let index =
                (f64::from(u32::try_from(sorted.len() - 1).unwrap_or(0)) * fraction) as usize;
            ms(sorted[index])
        };

        Some(Self {
            min: ms(sorted[0]),
            median: at(0.5),
            p95: at(0.95),
            max: ms(sorted[sorted.len() - 1]),
            count: sorted.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{burst_frame, onset, Edge, Flight, Spread, BURST_SAMPLES, SILENT_PATH_THRESHOLD};
    use crate::audio::{mixer::peak, opus::silent_frame};
    use std::{thread::sleep, time::Duration};

    #[test]
    fn a_burst_is_loud_at_the_start_and_silent_after_it() {
        let frame = burst_frame();
        assert!(
            peak(&frame[..BURST_SAMPLES]) > SILENT_PATH_THRESHOLD,
            "the burst is not loud enough to be heard over a threshold set for silence"
        );
        assert_eq!(
            peak(&frame[BURST_SAMPLES..]),
            0,
            "the burst leaked past its own length, so its trailing edge is not where it looks"
        );
    }

    #[test]
    fn only_the_leading_edge_of_a_burst_counts() {
        let edge = Edge::new(SILENT_PATH_THRESHOLD);
        let loud = burst_frame();

        // A sine starts at zero, so the first sample over the threshold is a
        // sample or two in. Close enough that the sub-frame correction the
        // onset feeds is exact to a fortieth of a millisecond.
        let index = edge
            .crossed(&loud)
            .expect("the first loud frame is the edge");
        assert!(index < 8, "the burst's onset landed at sample {index}");
        assert!(
            edge.crossed(&loud).is_none(),
            "a burst spread over two frames was timed twice"
        );
        assert!(
            edge.crossed(&silent_frame()).is_none(),
            "silence is not an edge"
        );
        assert!(
            edge.crossed(&loud).is_some(),
            "the next burst after a quiet frame is a new edge"
        );
    }

    #[test]
    fn a_threshold_the_signal_cannot_reach_hears_nothing() {
        let edge = Edge::new(u16::MAX);
        assert!(edge.crossed(&burst_frame()).is_none());
        edge.set_threshold(SILENT_PATH_THRESHOLD);
        assert!(
            edge.crossed(&burst_frame()).is_some(),
            "the calibrated threshold was not picked up"
        );
    }

    #[test]
    fn an_arrival_is_timed_from_the_departure_it_belongs_to() {
        let flight = Flight::default();
        flight.depart();
        sleep(Duration::from_millis(5));

        let elapsed = flight.arrive().expect("a burst was in the air");
        assert!(elapsed >= Duration::from_millis(5));
        assert_eq!(flight.lost(), 0);
        assert!(
            flight.arrive().is_none(),
            "the same burst was claimed twice, so the next arrival would be timed from nothing"
        );
    }

    #[test]
    fn a_burst_still_out_when_the_next_one_leaves_is_lost() {
        let flight = Flight::default();
        flight.depart();
        flight.depart();
        assert_eq!(flight.lost(), 1);

        // The arrival belongs to the second one, not the first.
        assert!(flight.arrive().is_some());
        assert_eq!(flight.lost(), 1);
    }

    #[test]
    fn patience_runs_out_on_a_burst_nobody_heard() {
        let flight = Flight::default();
        flight.depart();

        flight.expire(Duration::from_secs(60));
        assert_eq!(flight.lost(), 0, "gave up on a burst that still had time");

        sleep(Duration::from_millis(5));
        flight.expire(Duration::from_millis(1));
        assert_eq!(flight.lost(), 1);
        assert!(
            flight.arrive().is_none(),
            "an expired burst was still claimable, so the next arrival would be timed from it"
        );
    }

    /// The sub-frame correction is only worth making if the onset is where
    /// the burst is, not where the frame is.
    #[test]
    fn the_onset_is_the_first_loud_sample_not_the_loudest() {
        let mut frame = silent_frame();
        frame[100] = 6_000;
        frame[200] = 20_000;
        assert_eq!(onset(&frame, SILENT_PATH_THRESHOLD), Some(100));
        assert_eq!(onset(&silent_frame(), SILENT_PATH_THRESHOLD), None);
    }

    #[test]
    fn a_spread_of_nothing_is_not_a_measurement() {
        assert!(Spread::of(&[]).is_none());
    }

    /// The percentiles are nearest-rank on a floored index, which is what a
    /// few dozen samples deserve: no interpolation between two real
    /// measurements to invent a third.
    #[test]
    fn the_spread_reads_off_the_sorted_times() {
        let times: Vec<Duration> = [40, 10, 30, 20]
            .iter()
            .map(|&ms| Duration::from_millis(ms))
            .collect();
        let spread = Spread::of(&times).expect("four times are a measurement");

        assert!(
            (spread.min - 10.0).abs() < f64::EPSILON,
            "min {}",
            spread.min
        );
        assert!(
            (spread.max - 40.0).abs() < f64::EPSILON,
            "max {}",
            spread.max
        );
        assert!(
            (spread.median - 20.0).abs() < f64::EPSILON,
            "median {}",
            spread.median
        );
        assert!(
            (spread.p95 - 30.0).abs() < f64::EPSILON,
            "p95 {}",
            spread.p95
        );
        assert_eq!(spread.count, 4);
    }
}
