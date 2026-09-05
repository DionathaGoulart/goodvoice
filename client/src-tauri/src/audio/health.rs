//! How well the voice path is running, counted where it runs.
//!
//! # The question this exists to answer
//!
//! When somebody says the voices are breaking up, there are three different
//! bugs behind that sentence and they want opposite fixes:
//!
//! * **Loss.** Packets never arrive. [`rtc::order`](crate::rtc::order) names
//!   them and the decoder conceals them, so this is the one that is already
//!   handled — but how *much* of it there is decides whether the handling is
//!   enough.
//! * **The ring running dry.** Audio is arriving, and not soon enough. That is
//!   jitter, and the fix is depth: hold audio back so a late packet is still
//!   early.
//! * **The ring filling up.** Audio is arriving faster than the speakers play
//!   it, which after a few minutes is the sender's clock running fast against
//!   this machine's. The fix is the opposite one — shed audio, or resample.
//!
//! Depth and drift pull in opposite directions, so building for the wrong one
//! makes the call worse. These counters are what says which is happening
//! before anybody writes either.
//!
//! # Why the counters are global
//!
//! The same reason [`crate::report`]'s are: this is diagnostics, and threading
//! a handle from the render callback through the mixer, the device, the call
//! and the session would put the plumbing for it in five files that have
//! nothing else to do with it. There is one output device and one call, so
//! there is one of these.
//!
//! # Real-time discipline
//!
//! [`starved`], [`deep`] and [`played`] are called from inside the device
//! callback. Every one of them is a single relaxed atomic and nothing else —
//! no allocation, no locking, no logging (styleguide.md). `Relaxed` is right
//! because nothing here orders anything: these are counts read by a task that
//! wakes on a timer, and a count that lands one window late is a count that
//! lands.

use std::sync::atomic::{AtomicU32, Ordering};

/// Blocks the render callback could only fill part of.
static STARVED: AtomicU32 = AtomicU32::new(0);
/// Frames dropped because the ring they were meant for was full.
static OVERFLOWED: AtomicU32 = AtomicU32::new(0);
/// Frames the decoder extrapolated because the packet never came.
static CONCEALED: AtomicU32 = AtomicU32::new(0);
/// Frames handed to a ring, concealed or decoded. The denominator.
static PLAYED: AtomicU32 = AtomicU32::new(0);
/// The most any one ring was holding, in samples.
static DEEPEST: AtomicU32 = AtomicU32::new(0);

/// A block the mixer could not fill from the ring it was reading.
///
/// Counted on a *partial* read rather than an empty one, and the difference is
/// the whole reliability of this number: a ring with nothing in it is the
/// ordinary state of a slot whose owner is not talking, while a ring that ran
/// out halfway through a block was mid-word when it did. One is silence and
/// the other is the click.
pub fn starved() {
    STARVED.fetch_add(1, Ordering::Relaxed);
}

/// How much one ring was holding, in samples, after a block was taken from it.
pub fn deep(samples: usize) {
    DEEPEST.fetch_max(
        u32::try_from(samples).unwrap_or(u32::MAX),
        Ordering::Relaxed,
    );
}

/// A frame that would not fit in the ring it was meant for.
pub fn overflowed() {
    OVERFLOWED.fetch_add(1, Ordering::Relaxed);
}

/// A frame the decoder invented because the packet carrying it never arrived.
pub fn concealed() {
    CONCEALED.fetch_add(1, Ordering::Relaxed);
}

/// A frame queued for playback, however it was produced.
pub fn played() {
    PLAYED.fetch_add(1, Ordering::Relaxed);
}

/// What happened over one window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Health {
    pub starved: u32,
    pub overflowed: u32,
    pub concealed: u32,
    pub played: u32,
    pub deepest: u32,
}

impl Health {
    /// Reads the counters and resets them, so the next call describes the next
    /// window rather than the whole call.
    ///
    /// Not atomic across all five, deliberately: a frame counted in the window
    /// after the one it happened in is not a wrong answer to any question this
    /// is asked, and the alternative is a lock on the render callback's path.
    #[must_use]
    pub fn take() -> Self {
        Self {
            starved: STARVED.swap(0, Ordering::Relaxed),
            overflowed: OVERFLOWED.swap(0, Ordering::Relaxed),
            concealed: CONCEALED.swap(0, Ordering::Relaxed),
            played: PLAYED.swap(0, Ordering::Relaxed),
            deepest: DEEPEST.swap(0, Ordering::Relaxed),
        }
    }

    /// Whether anything happened at all. A room where nobody spoke produces a
    /// window of zeroes, and there is nothing to say about it.
    #[must_use]
    pub const fn is_quiet(&self) -> bool {
        self.played == 0
    }

    /// What share of the audio played was invented rather than received.
    ///
    /// The number that says whether loss is the problem. Opus hides a percent
    /// or two; past that it is audible however good the concealment is.
    #[must_use]
    pub fn loss(&self) -> f32 {
        if self.played == 0 {
            return 0.0;
        }
        #[allow(
            clippy::cast_precision_loss,
            reason = "a window holds thousands of frames, not sixteen million"
        )]
        {
            self.concealed as f32 / self.played as f32
        }
    }

    /// Whether this window is worth an issue rather than a breadcrumb.
    ///
    /// Starvation and overflow are counted at all only because both are
    /// audible every time they happen, so one is enough. Loss is not: it is
    /// what concealment is for, and a call with none of it is a call on a
    /// network that does not exist.
    #[must_use]
    pub fn is_bad(&self) -> bool {
        self.starved > 0 || self.overflowed > 0 || self.loss() > AUDIBLE_LOSS
    }

    /// Which of the three bugs in the module docs this window looks like.
    ///
    /// A tag rather than part of the message, so that [`crate::report`]'s
    /// fingerprint stays stable — see `rtc::session::health_loop`. It is also
    /// the first thing worth reading in the issue list.
    #[must_use]
    pub fn shape(&self) -> &'static str {
        if self.overflowed > 0 {
            // The ring filled and stayed full. Nothing but a clock difference
            // fills a 200 ms buffer on a link that is otherwise keeping up.
            "drift"
        } else if self.starved > 0 {
            "jitter"
        } else {
            "loss"
        }
    }
}

/// Where concealment stops covering for the network.
const AUDIBLE_LOSS: f32 = 0.05;

#[cfg(test)]
mod tests {
    use super::{Health, AUDIBLE_LOSS};

    #[test]
    fn a_window_where_nobody_spoke_says_nothing() {
        let health = Health::default();
        assert!(health.is_quiet());
        assert!(!health.is_bad());
    }

    #[test]
    fn a_clean_window_is_not_worth_an_issue() {
        let health = Health {
            played: 1_500,
            concealed: 3,
            ..Health::default()
        };
        assert!(!health.is_quiet());
        assert!(!health.is_bad(), "0.2% loss is a network, not a bug");
    }

    #[test]
    fn one_starved_block_is_enough_to_report() {
        // It is audible every time it happens, which is what makes a single
        // one worth an event.
        let health = Health {
            played: 1_500,
            starved: 1,
            ..Health::default()
        };
        assert!(health.is_bad());
        assert_eq!(health.shape(), "jitter");
    }

    #[test]
    fn a_full_ring_is_read_as_drift_even_when_it_also_starved() {
        // Both happen at once while a ring thrashes between full and empty.
        // Drift is the one that explains the pair.
        let health = Health {
            played: 1_500,
            starved: 4,
            overflowed: 9,
            ..Health::default()
        };
        assert_eq!(health.shape(), "drift");
    }

    #[test]
    fn loss_past_what_concealment_hides_is_reported_on_its_own() {
        let health = Health {
            played: 1_000,
            concealed: 100,
            ..Health::default()
        };
        assert!(health.loss() > AUDIBLE_LOSS);
        assert!(health.is_bad());
        assert_eq!(health.shape(), "loss");
    }

    #[test]
    fn loss_is_zero_rather_than_undefined_in_a_silent_window() {
        assert!((Health::default().loss() - 0.0).abs() < f32::EPSILON);
    }
}
