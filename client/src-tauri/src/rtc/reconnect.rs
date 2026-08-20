//! What a client does when a call it was already in falls over.
//!
//! Two things live here: the schedule a client retries on, and the state the UI
//! shows while it does. Neither knows how a call is built — [`super::session`]
//! owns that — so the policy can be reasoned about, and tested, without a
//! network.
//!
//! # Why a dropped call is rebuilt rather than repaired
//!
//! A goodvoice room holds no storage (DR-5): a Worker redeploy, an idle
//! eviction or ten seconds of dead network all leave the client holding a seat
//! that no longer exists. There is nothing to resume, so reconnecting means
//! joining again from the top — new participant id, new Realtime session, mic
//! republished, every roommate pulled afresh. The room code is the only thing
//! that survives, and it is the only thing the user ever typed.

use std::time::Duration;

use serde::Serialize;

/// How long to wait before the first retry. Short enough that a blip is
/// invisible, long enough that a client whose network just died is not asking
/// again before the interface is back.
const FIRST_DELAY: Duration = Duration::from_millis(500);

/// Ceiling on one wait. Past this the user is watching a "reconnecting" label
/// and deserves to see it try.
const MAX_DELAY: Duration = Duration::from_secs(15);

/// How many times a drop is retried before the call is called dead. With the
/// schedule below that is roughly 90 seconds of trying — long enough to ride
/// out a Worker redeploy or a router reboot, short enough that a client left
/// running overnight on a dead link stops rather than spinning until morning.
const MAX_ATTEMPTS: u32 = 10;

/// Doublings before [`MAX_DELAY`] takes over. `FIRST_DELAY << 5` is 16 s, so
/// the cap bites exactly once the schedule would overshoot it.
const DOUBLINGS: u32 = 5;

/// The retry schedule for one drop: 0.5 s, 1, 2, 4, 8, then 15 s until the
/// attempts run out.
///
/// No jitter, deliberately. Jitter exists to decorrelate a herd, and a
/// goodvoice room holds eight clients whose reconnects are already spread by
/// whenever each of them noticed — spending an RNG on it would buy nothing.
#[derive(Debug, Default)]
pub struct Backoff {
    attempt: u32,
}

impl Backoff {
    #[must_use]
    pub const fn new() -> Self {
        Self { attempt: 0 }
    }

    /// How many attempts have been handed out since the last [`Self::reset`].
    #[must_use]
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Forgets the run of failures. Called when a session connects, so the next
    /// drop starts from the short delay rather than wherever the last one
    /// finished.
    pub const fn reset(&mut self) {
        self.attempt = 0;
    }

    /// The next wait, or `None` once [`MAX_ATTEMPTS`] have been used.
    pub fn next_delay(&mut self) -> Option<Duration> {
        if self.attempt >= MAX_ATTEMPTS {
            return None;
        }
        let delay = FIRST_DELAY
            .saturating_mul(2_u32.saturating_pow(self.attempt.min(DOUBLINGS)))
            .min(MAX_DELAY);
        self.attempt += 1;
        Some(delay)
    }
}

/// Why a call stopped for good.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "lowercase")]
pub enum EndReason {
    /// The user left. The only ending that is not a failure.
    Left,
    /// The room said no in a way that saying it again would not change — full,
    /// bad code, a client too old to parse the answer.
    Refused { detail: String },
    /// The retry schedule ran out with the room still unreachable.
    Unreachable { detail: String },
}

/// What the UI shows about the call it is in.
///
/// A dropped call is never silent: the client is either live, visibly trying,
/// or finished with a reason (prd.md §5 flow E).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum CallState {
    /// Media is flowing.
    Live,
    /// The seat is gone and the client is taking another one. `attempt` counts
    /// from 1 so it reads as "attempt 3 of 10" rather than from zero.
    Reconnecting { attempt: u32 },
    /// Nothing more will happen without the user.
    Ended(EndReason),
}

impl CallState {
    /// Whether the call is over. Used to stop the tasks that push state at a
    /// UI which has nothing left to hear.
    #[must_use]
    pub const fn is_ended(&self) -> bool {
        matches!(self, Self::Ended(_))
    }
}

#[cfg(test)]
mod tests {
    use super::{Backoff, CallState, EndReason, MAX_ATTEMPTS, MAX_DELAY};
    use std::time::Duration;

    #[test]
    fn the_schedule_doubles_and_then_flattens() {
        let mut backoff = Backoff::new();
        let delays: Vec<Duration> = std::iter::from_fn(|| backoff.next_delay()).collect();

        assert_eq!(
            delays[..5],
            [
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
            ]
        );
        // Everything past the doublings sits on the ceiling rather than growing
        // into waits nobody would sit through.
        assert!(delays[5..].iter().all(|&delay| delay == MAX_DELAY));
    }

    #[test]
    fn the_schedule_gives_up_rather_than_retrying_forever() {
        let mut backoff = Backoff::new();
        for _ in 0..MAX_ATTEMPTS {
            assert!(backoff.next_delay().is_some());
        }
        assert!(backoff.next_delay().is_none());
        assert_eq!(backoff.attempt(), MAX_ATTEMPTS);
    }

    #[test]
    fn the_whole_schedule_is_about_ninety_seconds() {
        // The number the "give up" decision is really made on: long enough for
        // a Worker redeploy (DR-5) or a router reboot, short enough that a dead
        // link stops rather than spinning overnight.
        let mut backoff = Backoff::new();
        let total: Duration = std::iter::from_fn(|| backoff.next_delay()).sum();

        assert!(
            (Duration::from_secs(85)..Duration::from_secs(95)).contains(&total),
            "the retry schedule now spans {total:?}"
        );
    }

    #[test]
    fn connecting_forgets_the_failures_that_came_before() {
        let mut backoff = Backoff::new();
        backoff.next_delay();
        backoff.next_delay();
        backoff.reset();

        assert_eq!(backoff.attempt(), 0);
        assert_eq!(backoff.next_delay(), Some(Duration::from_millis(500)));
    }

    #[test]
    fn call_state_serialises_flat_for_the_ui() {
        let encode = |state: &CallState| serde_json::to_string(state).expect("json");

        assert_eq!(encode(&CallState::Live), r#"{"state":"live"}"#);
        assert_eq!(
            encode(&CallState::Reconnecting { attempt: 3 }),
            r#"{"state":"reconnecting","attempt":3}"#
        );
        assert_eq!(
            encode(&CallState::Ended(EndReason::Left)),
            r#"{"state":"ended","reason":"left"}"#
        );
        assert_eq!(
            encode(&CallState::Ended(EndReason::Refused {
                detail: "room is full".to_owned()
            })),
            r#"{"state":"ended","reason":"refused","detail":"room is full"}"#
        );
    }

    #[test]
    fn only_an_ending_is_an_ending() {
        assert!(CallState::Ended(EndReason::Left).is_ended());
        assert!(!CallState::Live.is_ended());
        assert!(!CallState::Reconnecting { attempt: 1 }.is_ended());
    }
}
