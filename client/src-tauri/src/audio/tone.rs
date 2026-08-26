//! A steady tone, and how much of it came back.
//!
//! What [`super::burst`] is to the latency half of the audio path, this is to
//! the echo half. A tone leaves one client, comes out of a loudspeaker at the
//! other end, goes into that end's microphone — and if the canceller is doing
//! its job, does not come back. Measuring that needs two things on both sides
//! of the wire: the frame exactly as it was sent, and the energy at one
//! frequency in a frame that arrived.
//!
//! It lives here rather than in the harness package because two harnesses need
//! it, and because a reference frame that disagrees with what was actually
//! played turns the ratio into a measurement of that disagreement instead. The
//! amplitude is [`AMPLITUDE`] in one place and [`super::device::ToneSource`]
//! reads it from here.

use std::f32::consts::TAU;

use super::opus::{silent_frame, Frame, SAMPLE_RATE_HZ};

/// How loud the tone is played, and therefore how loud the reference against
/// which the return is measured has to be. Loud enough to survive Opus, quiet
/// enough to leave headroom for a room on top of it.
pub const AMPLITUDE: f32 = 8_000.0;

/// A frequency far from anything a voice puts energy into, so what is measured
/// at it is the loudspeaker and not the person in front of it.
pub const DEFAULT_HZ: f32 = 1_200.0;

/// One frame of the tone, as it goes on the wire.
///
/// This is the "as sent" half of every ratio here. It starts at phase zero,
/// which a continuous tone only does on its first frame — but [`bin_energy`]
/// is phase-blind, so a reference frame and a returned frame that disagree
/// about phase still agree about energy.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "8000 and 48000 are exact in f32, and the product is bounded by it"
)]
pub fn frame(frequency_hz: f32) -> Frame {
    let mut frame = silent_frame();
    let mut phase = 0.0_f32;
    for sample in &mut frame {
        *sample = (phase.sin() * AMPLITUDE) as i16;
        phase += TAU * frequency_hz / SAMPLE_RATE_HZ as f32;
    }
    frame
}

/// Goertzel: the energy in one frequency bin, without pulling in an FFT.
///
/// The result is a power, not an amplitude — two of these go into a ratio and
/// the ratio into `10 * log10`, which is what [`db_below`] does.
///
/// Takes a slice rather than a [`Frame`] because a tone is coherent and a room
/// is not: reading several consecutive frames as one window buys signal to
/// noise in proportion to its length, which is how an echo too quiet to see in
/// 20 ms is seen in 200. The window has to be *contiguous* audio for that — a
/// gap in the middle is a phase step, and the tone stops adding to itself
/// across it.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "constants and sample values are exact in f32"
)]
pub fn bin_energy(samples: &[i16], frequency_hz: f32) -> f32 {
    let coefficient = 2.0 * (TAU * frequency_hz / SAMPLE_RATE_HZ as f32).cos();
    let (mut previous, mut previous2) = (0.0_f32, 0.0_f32);
    for &sample in samples {
        let current = f32::from(sample) + coefficient * previous - previous2;
        previous2 = previous;
        previous = current;
    }
    previous.mul_add(previous, previous2 * previous2) - coefficient * previous * previous2
}

/// How far below `reference` the energy in `returned` is, in decibels.
///
/// Higher is quieter, which for an echo column means better. `None` when
/// nothing came back at all: a ratio against zero is an infinity, and printing
/// one as though it were a measurement of a canceller is how a silent
/// microphone comes to look like a perfect one.
#[must_use]
pub fn db_below(reference: f32, returned: f32) -> Option<f32> {
    (returned > 0.0 && reference > 0.0).then(|| 10.0 * (reference / returned).log10())
}

/// How much louder `louder` is than `quieter`, in decibels of power.
///
/// The same arithmetic as [`db_below`] read the other way round, for the
/// question "did this lift the bin at all" rather than "how far down is it".
#[must_use]
pub fn db_over(louder: f32, quieter: f32) -> Option<f32> {
    db_below(louder, quieter)
}

#[cfg(test)]
mod tests {
    use super::{bin_energy, db_below, db_over, frame, AMPLITUDE, DEFAULT_HZ, TAU};
    use crate::audio::opus::{silent_frame, FRAME_SAMPLES};

    /// The sample rate as the arithmetic below wants it. 48 000 is exact in
    /// `f32`, but a cast inside the loop is a lint every time.
    const SAMPLE_RATE: f32 = 48_000.0;

    #[test]
    fn the_tone_is_loudest_in_its_own_bin() {
        let tone = frame(DEFAULT_HZ);
        let mine = bin_energy(&tone, DEFAULT_HZ);
        for other in [300.0, 700.0, 2_400.0, 4_000.0] {
            assert!(
                bin_energy(&tone, other) < mine / 100.0,
                "{other} Hz should hold almost none of a {DEFAULT_HZ} Hz tone"
            );
        }
    }

    #[test]
    fn silence_holds_no_energy_anywhere() {
        assert!(bin_energy(&silent_frame(), DEFAULT_HZ) < 1.0);
    }

    /// The reference is what a returned frame is measured against, so a tone
    /// that came back untouched has to read as zero decibels of loss rather
    /// than as some offset that would then sit inside every echo number.
    #[test]
    fn a_tone_that_came_back_whole_is_zero_db_down() {
        let sent = bin_energy(&frame(DEFAULT_HZ), DEFAULT_HZ);
        let back = bin_energy(&frame(DEFAULT_HZ), DEFAULT_HZ);
        let db = db_below(sent, back).expect("both frames hold the tone");
        assert!(db.abs() < 0.001, "{db} dB");
    }

    #[test]
    fn a_tone_at_half_the_amplitude_is_six_db_down() {
        let sent = frame(DEFAULT_HZ);
        let mut halved = sent;
        for sample in &mut halved {
            *sample /= 2;
        }
        let db = db_below(
            bin_energy(&sent, DEFAULT_HZ),
            bin_energy(&halved, DEFAULT_HZ),
        )
        .expect("both frames hold the tone");
        assert!((db - 6.02).abs() < 0.1, "{db} dB");
    }

    /// The reason [`bin_energy`] takes a slice: a tone adds to itself across
    /// consecutive frames and noise does not, so the same tone under the same
    /// noise stands further out of a longer window.
    #[test]
    fn a_longer_window_pulls_the_tone_further_out_of_the_noise() {
        let stands_out = |frames: usize| {
            let mut window = Vec::new();
            let mut phase = 0.0_f32;
            // A generator with all its state here, so both windows are the
            // same room heard for different lengths of time.
            let mut noise = 12_345_u32;
            for _ in 0..frames * FRAME_SAMPLES {
                noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_possible_wrap,
                    reason = "deliberately taking the low bits of the generator as a sample"
                )]
                let hiss = ((noise >> 16) as i16) / 8;
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "the amplitude is bounded by construction"
                )]
                let tone = (phase.sin() * 400.0) as i16;
                phase += TAU * DEFAULT_HZ / SAMPLE_RATE;
                window.push(tone.saturating_add(hiss));
            }
            let room = (bin_energy(&window, DEFAULT_HZ * 0.8)
                + bin_energy(&window, DEFAULT_HZ * 1.25))
                / 2.0;
            db_over(bin_energy(&window, DEFAULT_HZ), room).expect("a window holds energy")
        };
        let (short, long) = (stands_out(1), stands_out(10));
        assert!(
            long > short + 5.0,
            "ten frames should beat one by about ten decibels, got {short:.1} then {long:.1}"
        );
    }

    #[test]
    fn nothing_coming_back_is_not_a_number() {
        let sent = bin_energy(&frame(DEFAULT_HZ), DEFAULT_HZ);
        assert!(db_below(sent, 0.0).is_none());
    }

    #[test]
    fn the_amplitude_is_the_one_the_source_plays() {
        let peak = frame(DEFAULT_HZ)
            .iter()
            .copied()
            .map(i16::abs)
            .max()
            .expect("a frame holds samples");
        assert!(
            (f32::from(peak) - AMPLITUDE).abs() < AMPLITUDE / 20.0,
            "peaked at {peak}, not near {AMPLITUDE}"
        );
    }
}
