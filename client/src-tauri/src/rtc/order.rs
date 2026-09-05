//! Putting one remote voice back in order, and naming what never arrived.
//!
//! # What was missing
//!
//! [`session::playback_loop`](super::session) used to hand every RTP payload
//! straight to the decoder in the order the network happened to deliver it.
//! Opus carries state across packets, so that is wrong in both directions at
//! once: a packet that overtook another is decoded in the wrong place *and*
//! leaves the decoder describing a moment that has not happened yet, and a
//! packet that never came is not noticed at all — the ring simply receives
//! 20 ms less than it is about to play, and `audio::mixer` renders the gap as
//! the silence it was told to render.
//!
//! That silence is the click. [`VoiceDecoder::conceal`](crate::audio::opus)
//! has existed and been tested since the codec went in, and nothing called it,
//! because nothing here knew a packet was missing.
//!
//! # Not a jitter buffer
//!
//! A jitter buffer holds audio back on a clock, so that a packet running late
//! is still early enough to play in its own slot. This does not: it has no
//! timer, and it never delays a packet that arrived in order. What it does is
//! the half of the job that costs nothing —
//!
//! * a packet in order plays immediately,
//! * a packet that arrived early is **held** until the ones before it come or
//!   [`HOLD`] says they are not coming,
//! * a packet that arrived late is **dropped**, because its slot has already
//!   been played and decoding it now would only corrupt what follows,
//! * a slot nobody filled is **[`Step::Lost`]**, which is the concealment call
//!   this module exists to make possible.
//!
//! So reordering costs latency only in the moment reordering is actually
//! happening, and loss costs the decoder's extrapolation rather than a hole.
//! The depth this does *not* add — the buffer that would absorb jitter before
//! it becomes reordering — is deliberately still absent.

use std::collections::BTreeMap;

use bytes::Bytes;

/// How many packets may be held waiting for one that has not arrived.
///
/// Three is 60 ms of audio, and it is a bound on added latency rather than a
/// guess about the network: a reorder deeper than this is not reordering any
/// more, it is loss that happens to be followed by arrivals. Holding longer
/// would delay every voice in the room behind one straggler.
const HOLD: usize = 3;

/// The largest run of missing packets that is concealed rather than skipped.
///
/// Opus extrapolates a frame or two convincingly and then starts to sound like
/// a synthesiser. Past this the honest thing is silence, and the gap is
/// probably not loss at all — a track that was republished, or a sequence that
/// restarted under a reconnect, arrives as a jump of thousands.
const MAX_CONCEAL: u16 = 5;

/// Half the sequence space: the line between "ahead" and "behind" once
/// wrapping is taken into account.
const HALF: u16 = 0x8000;

/// What the caller should do next with one remote speaker's stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Decode and play this payload. Always in sequence order.
    Play(Bytes),
    /// The packet for this slot is not coming. Conceal it.
    Lost,
}

/// One remote track's sequence, reassembled.
///
/// Cheap to keep per speaker: `HOLD` payloads at most, and `Bytes` is a
/// refcount rather than a copy, so holding one costs nothing the transport was
/// not already paying.
#[derive(Debug, Default)]
pub struct Sequence {
    /// The sequence number that would come next in an unbroken stream, or
    /// `None` before the first packet has said where the stream starts.
    next: Option<u16>,
    /// Arrivals from beyond `next`, in order, waiting for the gap to fill.
    held: BTreeMap<u16, Bytes>,
}

impl Sequence {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes one arrival and appends what it makes playable to `out`.
    ///
    /// `out` is the caller's buffer rather than a return value so that a
    /// steady stream — one `Step` per packet, forever — allocates nothing
    /// after the first few frames.
    pub fn accept(&mut self, seq: u16, payload: &Bytes, out: &mut Vec<Step>) {
        let Some(next) = self.next else {
            // The first packet decides where the stream starts. There is
            // nothing before it to have lost.
            self.next = Some(seq.wrapping_add(1));
            push(out, payload);
            return;
        };

        let ahead = seq.wrapping_sub(next);
        if ahead >= HALF {
            // Behind `next`: its slot has been played or concealed already.
            // Decoding it now would put the wrong 20 ms on the wire and leave
            // the decoder in the past for the packet after it.
            return;
        }

        if ahead == 0 {
            push(out, payload);
            self.next = Some(next.wrapping_add(1));
            self.drain(out);
            return;
        }

        // Early. Hold it, unless holding one more would cost more latency than
        // the packets it is waiting for are worth.
        self.held.insert(seq, payload.clone());
        if self.held.len() > HOLD {
            self.give_up(out);
        }
    }

    /// Plays out whatever the arrival just unblocked.
    fn drain(&mut self, out: &mut Vec<Step>) {
        while let Some(next) = self.next {
            let Some(payload) = self.held.remove(&next) else {
                return;
            };
            push(out, &payload);
            self.next = Some(next.wrapping_add(1));
        }
    }

    /// Declares the packets in front of the oldest held one lost, and carries
    /// on from there.
    ///
    /// Called when the hold is full, which is the only thing that ever ends a
    /// wait: there is no timer here, so a gap is closed by the packets that
    /// arrive after it rather than by time passing. A speaker who stops
    /// talking mid-gap leaves those payloads held until they speak again,
    /// which is the right answer — nobody is waiting to hear silence.
    fn give_up(&mut self, out: &mut Vec<Step>) {
        let Some((&oldest, _)) = self.held.iter().next() else {
            return;
        };
        let Some(next) = self.next else {
            return;
        };
        let missing = oldest.wrapping_sub(next);
        if missing <= MAX_CONCEAL {
            for _ in 0..missing {
                out.push(Step::Lost);
            }
        }
        // Concealed or skipped, the stream resumes at the packet in hand: a
        // jump too large to conceal is a stream that restarted, and following
        // it is better than concealing a thousand frames or stalling forever.
        self.next = Some(oldest);
        self.drain(out);
    }
}

/// Queues a payload for playback, unless there is nothing in it to play.
///
/// The sequence number still counted — see [`Sequence::accept`] — but a
/// zero-length packet is padding, and `VoiceDecoder::decode` refuses it.
fn push(out: &mut Vec<Step>, payload: &Bytes) {
    if !payload.is_empty() {
        out.push(Step::Play(payload.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::{Sequence, Step, HOLD, MAX_CONCEAL};
    use bytes::Bytes;

    /// A payload a test can tell from the others.
    fn payload(tag: u8) -> Bytes {
        Bytes::from(vec![tag])
    }

    /// The tags that came out, in the order they came out, with `None` for a
    /// concealment.
    fn tags(steps: &[Step]) -> Vec<Option<u8>> {
        steps
            .iter()
            .map(|step| match step {
                Step::Play(bytes) => Some(bytes[0]),
                Step::Lost => None,
            })
            .collect()
    }

    #[test]
    fn a_stream_in_order_is_played_in_order_and_held_nowhere() {
        let mut sequence = Sequence::new();
        let mut out = Vec::new();

        for tag in 0..5_u8 {
            sequence.accept(100 + u16::from(tag), &payload(tag), &mut out);
        }

        assert_eq!(tags(&out), [Some(0), Some(1), Some(2), Some(3), Some(4)]);
        assert!(
            sequence.held.is_empty(),
            "nothing should wait when nothing is missing"
        );
    }

    #[test]
    fn the_first_packet_is_never_treated_as_loss() {
        // The sequence number a track starts on is whatever the sender chose.
        // Concealing up to it would be tens of thousands of frames.
        let mut sequence = Sequence::new();
        let mut out = Vec::new();

        sequence.accept(40_000, &payload(1), &mut out);

        assert_eq!(tags(&out), [Some(1)]);
    }

    #[test]
    fn a_packet_that_overtook_another_waits_for_it() {
        let mut sequence = Sequence::new();
        let mut out = Vec::new();

        sequence.accept(10, &payload(0), &mut out);
        sequence.accept(12, &payload(2), &mut out);
        assert_eq!(tags(&out), [Some(0)], "12 must not play before 11");

        sequence.accept(11, &payload(1), &mut out);
        assert_eq!(tags(&out), [Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn a_packet_that_arrived_after_its_slot_was_played_is_dropped() {
        let mut sequence = Sequence::new();
        let mut out = Vec::new();

        sequence.accept(10, &payload(0), &mut out);
        sequence.accept(11, &payload(1), &mut out);
        out.clear();

        // Its 20 ms is already on the wire. Decoding it now would play the
        // wrong moment and leave the decoder behind for everything after.
        sequence.accept(10, &payload(0), &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn a_gap_the_hold_gives_up_on_is_concealed_once_per_missing_packet() {
        let mut sequence = Sequence::new();
        let mut out = Vec::new();

        sequence.accept(10, &payload(0), &mut out);
        out.clear();

        // 11 never arrives. The packets behind it fill the hold and force it.
        for tag in 1..=u8::try_from(HOLD).expect("HOLD is small") + 1 {
            sequence.accept(11 + u16::from(tag), &payload(tag), &mut out);
        }

        assert_eq!(
            tags(&out).first(),
            Some(&None),
            "the missing packet should be concealed, not skipped in silence"
        );
        assert_eq!(
            tags(&out).iter().filter(|tag| tag.is_none()).count(),
            1,
            "one packet was missing, so one frame is concealed"
        );
    }

    #[test]
    fn a_gap_too_large_to_conceal_resyncs_in_silence() {
        // A republished track or a reconnect restarts the numbering. Extra-
        // polating thousands of frames would be a noise, not a voice.
        let mut sequence = Sequence::new();
        let mut out = Vec::new();

        sequence.accept(10, &payload(0), &mut out);
        out.clear();

        let far = 11 + MAX_CONCEAL + 1;
        for tag in 0..=u8::try_from(HOLD).expect("HOLD is small") {
            sequence.accept(far + u16::from(tag), &payload(tag), &mut out);
        }

        assert!(
            !out.contains(&Step::Lost),
            "a jump this large is a new stream, not loss to conceal"
        );
        assert_eq!(tags(&out).first(), Some(&Some(0)));
    }

    #[test]
    fn the_sequence_number_wrapping_is_not_read_as_a_jump() {
        let mut sequence = Sequence::new();
        let mut out = Vec::new();

        sequence.accept(u16::MAX - 1, &payload(0), &mut out);
        sequence.accept(u16::MAX, &payload(1), &mut out);
        sequence.accept(0, &payload(2), &mut out);
        sequence.accept(1, &payload(3), &mut out);

        assert_eq!(tags(&out), [Some(0), Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn padding_advances_the_sequence_without_being_played() {
        // Cloudflare sends payload-less packets. They are not audio, but they
        // do occupy a sequence number — reading one as loss would conceal a
        // frame nobody spoke.
        let mut sequence = Sequence::new();
        let mut out = Vec::new();

        sequence.accept(10, &payload(0), &mut out);
        sequence.accept(11, &Bytes::new(), &mut out);
        sequence.accept(12, &payload(2), &mut out);

        assert_eq!(tags(&out), [Some(0), Some(2)]);
    }

    #[test]
    fn a_hold_that_never_fills_does_not_stall_the_next_arrival() {
        // The gap closes on packets, not on time. What must not happen is the
        // held packets being lost outright when it finally does close.
        let mut sequence = Sequence::new();
        let mut out = Vec::new();

        sequence.accept(10, &payload(0), &mut out);
        sequence.accept(13, &payload(3), &mut out);
        sequence.accept(12, &payload(2), &mut out);
        assert_eq!(tags(&out), [Some(0)]);

        sequence.accept(11, &payload(1), &mut out);
        assert_eq!(tags(&out), [Some(0), Some(1), Some(2), Some(3)]);
    }
}
