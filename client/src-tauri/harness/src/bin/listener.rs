//! A second person in the room, made of numbers.
//!
//! Several things in plan.md are marked done-but-unverified for the same
//! reason: the last step is "and somebody hears you". Task 3.3's push to talk,
//! task 3.2's mute, and task 3.4's speaker echo all end that way, and all three
//! are one person short rather than one feature short.
//!
//! This is that person. It joins a room, publishes either silence or a tone,
//! and once a second says what it is receiving: how many frames arrived, how
//! loud they were, and — the part task 3.4 needs — how much of its *own* tone
//! came back.
//!
//! ```text
//! cargo run -p goodvoice-harness --bin listener -- --room squad
//! cargo run -p goodvoice-harness --bin listener -- --room squad --tone 1200
//! ```
//!
//! # Reading the echo column
//!
//! With `--tone`, the room hears a steady tone from here. On the other end it
//! comes out of a loudspeaker, goes into a microphone, and — if the echo
//! canceller does its job — does not come back. The `echo` column is the
//! energy at that exact frequency in what does come back, against the energy
//! at that frequency in the tone as sent. That ratio in decibels is what task
//! 3.4's definition of done is asking about, measured instead of judged.
//!
//! # One slot
//!
//! Everything is read from slot 0, which is the first remote speaker the call
//! subscribed to. That is exactly right with two people in the room and wrong
//! with three, so keep the room to two.

use std::{
    env,
    f32::consts::TAU,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use goodvoice_client_lib::{
    audio::{
        device::{AudioSink, RecordingSink, ToneSource},
        mixer::peak,
        opus::{Frame, FRAME_SAMPLES, SAMPLE_RATE_HZ},
        prefs::AudioPrefs,
        vad::TransmitMode,
    },
    rtc::session::{Call, CallOptions},
};

const DEFAULT_BASE: &str = "https://goodvoice.goodvoice-server.workers.dev";
const DEFAULT_ROOM: &str = "goodvoice";

/// A frequency far from anything a voice puts energy into, so the echo column
/// measures the loudspeaker and not the person in front of it.
const DEFAULT_TONE_HZ: f32 = 1_200.0;

/// What [`ToneSource`] puts out, copied here because the reference frame has to
/// be the same shape as what the room is actually being sent.
const TONE_AMPLITUDE: f32 = 8_000.0;

#[tokio::main]
async fn main() -> Result<()> {
    let Args {
        base,
        room,
        name,
        tone,
        seconds,
    } = args();

    println!("joining {room} on {base} as {name}");
    match tone {
        Some(hz) => println!("publishing a {hz:.0} Hz tone — the room will hear it steadily"),
        None => println!("publishing silence — nothing here will be heard"),
    }

    let ears = RecordingSink::new();
    let call = Call::join(
        CallOptions {
            base,
            room,
            name,
            // Open, always: a gate here would make the tone come and go and
            // the far end's echo column meaningless.
            mode: TransmitMode::Open,
            prefs: std::sync::Arc::new(AudioPrefs::default()),
        },
        // 0 Hz is a sine of zero: a source that is genuinely silent rather
        // than one that is merely quiet.
        Box::new(ToneSource::new(tone.unwrap_or(0.0))),
        Arc::clone(&ears) as Arc<dyn AudioSink>,
    )
    .await
    .context("could not join the room")?;

    println!("connected — {}\n", call.self_id());
    println!("  time   frames/s   peak   {}", header(tone));

    let reference = tone.map(|hz| bin_energy(&tone_frame(hz), hz));
    let started = Instant::now();
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    let mut previous = 0_usize;

    while started.elapsed() < Duration::from_secs(seconds) {
        ticker.tick().await;
        let record = ears.slot(0);
        let arrived = record.frames.saturating_sub(previous);
        previous = record.frames;

        println!(
            "  {:>4}s   {arrived:>8}   {:>4}   {}",
            started.elapsed().as_secs(),
            peak(&record.last),
            column(&record.last, tone, reference, arrived),
        );
    }

    call.leave().await;
    println!("\nleft the room.");
    Ok(())
}

fn header(tone: Option<f32>) -> &'static str {
    if tone.is_some() {
        "echo (dB below what was sent — higher is better)"
    } else {
        ""
    }
}

/// What came back of our own tone, as decibels below what went out.
fn column(frame: &Frame, tone: Option<f32>, reference: Option<f32>, arrived: usize) -> String {
    let (Some(hz), Some(reference)) = (tone, reference) else {
        return String::new();
    };
    if arrived == 0 {
        // Nothing arrived, so there is nothing to say about it. Reporting a
        // clean cancellation here would be reporting on silence.
        return "— nothing arriving".to_owned();
    }

    let returned = bin_energy(frame, hz);
    if returned <= 0.0 {
        return ">60 (nothing came back)".to_owned();
    }
    format!("{:.1}", 10.0 * (reference / returned).log10())
}

/// One frame of the tone as [`ToneSource`] produces it, for the "as sent" half
/// of the ratio. Its amplitude has to match, or the echo column is offset by
/// the difference and says nothing useful.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "8000 and 48000 are exact in f32, and the product is bounded by it"
)]
fn tone_frame(frequency_hz: f32) -> Frame {
    let mut frame = [0_i16; FRAME_SAMPLES];
    let mut phase = 0.0_f32;
    for sample in &mut frame {
        *sample = (phase.sin() * TONE_AMPLITUDE) as i16;
        phase += TAU * frequency_hz / SAMPLE_RATE_HZ as f32;
    }
    frame
}

/// Goertzel: energy in one frequency bin, without pulling in an FFT.
#[allow(
    clippy::cast_precision_loss,
    reason = "constants and sample values are exact in f32"
)]
fn bin_energy(frame: &Frame, frequency_hz: f32) -> f32 {
    let coefficient = 2.0 * (TAU * frequency_hz / SAMPLE_RATE_HZ as f32).cos();
    let (mut previous, mut previous2) = (0.0_f32, 0.0_f32);
    for &sample in frame {
        let current = f32::from(sample) + coefficient * previous - previous2;
        previous2 = previous;
        previous = current;
    }
    previous.mul_add(previous, previous2 * previous2) - coefficient * previous * previous2
}

struct Args {
    base: String,
    room: String,
    name: String,
    tone: Option<f32>,
    seconds: u64,
}

fn args() -> Args {
    let mut base = env::var("GOODVOICE_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_owned());
    let mut room = DEFAULT_ROOM.to_owned();
    let mut name = "listener".to_owned();
    let mut tone = None;
    let mut seconds = 120;

    let mut argv = env::args().skip(1);
    while let Some(flag) = argv.next() {
        match (flag.as_str(), argv.next()) {
            ("--base", Some(value)) => base = value,
            ("--room", Some(value)) => room = value,
            ("--name", Some(value)) => name = value,
            ("--tone", Some(value)) => tone = value.parse().ok().or(Some(DEFAULT_TONE_HZ)),
            ("--seconds", Some(value)) => seconds = value.parse().unwrap_or(seconds),
            (other, _) => eprintln!("ignoring unknown argument {other}"),
        }
    }

    Args {
        base: base.trim_end_matches('/').to_owned(),
        room,
        name,
        tone,
        seconds,
    }
}
