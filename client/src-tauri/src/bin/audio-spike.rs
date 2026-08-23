//! What the real devices cost, and whether they can be heard at all.
//!
//! plan.md task 2.1. The seam (`audio::device`) and a working cpal backend
//! behind it (`audio::hardware`) already exist, so this is not "build the audio
//! layer" — it is the measurement the task was always about: does cpal's
//! WASAPI backend fit inside the ≤80 ms budget (prd.md §4), or does the
//! `wasapi` crate's control over shared-mode buffers buy enough to justify a
//! second backend? DR-12 answered half of it from the driver's own numbers;
//! this answers the other half by listening.
//!
//! Everything here goes through the same `hardware::open()` the app uses, so
//! what it measures is the app's path and not a spike's.
//!
//! # Monitor — `cargo run --bin audio-spike`
//!
//! The microphone, played back through the speakers. The point is the first
//! half of the task's definition of done: audible loopback. **Wear
//! headphones** — a microphone that can hear the speakers will howl.
//!
//! # Round trip — `cargo run --bin audio-spike -- --roundtrip`
//!
//! A 5 ms burst leaves the speakers once a second and the microphone listens
//! for it. What comes back is the whole path a call's audio takes through the
//! hardware: our mixer ring, the render buffer, the converters, the air, the
//! capture buffer, and our capture ring. **Hold the earcup against the
//! microphone**, or nothing will be heard and the run will say so.
//!
//! A call's mouth-to-ear contains exactly one capture and one render, which is
//! exactly what this contains — plus the trip through the air, which a call
//! does not make. So the number is an upper bound on what the devices cost a
//! call, and it is the number to hold against the budget DR-14 measured the
//! network half of.

use std::{
    env,
    time::{Duration, Instant},
};

use anyhow::{bail, Context as _, Result};
use goodvoice_client_lib::audio::{
    burst::{burst_frame, Edge, Flight, Spread},
    device::{AudioSink, AudioSource},
    hardware,
    mixer::peak,
    opus::{silent_frame, FRAME_MS, FRAME_SAMPLES, SAMPLE_RATE_HZ},
};

/// How long the monitor runs, and how many bursts the round trip times.
const DEFAULT_SECONDS: u64 = 30;
const DEFAULT_BURSTS: usize = 20;

/// Frames between bursts. Fifty is one second — far longer than any plausible
/// round trip, which is what keeps at most one burst in the air.
const BURST_INTERVAL_FRAMES: usize = 50;

/// How long a burst may be out before it is written off. Shorter than the gap
/// between bursts, so a burst nobody heard is lost rather than paired with the
/// next one.
const LOST_AFTER: Duration = Duration::from_millis(900);

/// How long to listen to the room before deciding what counts as loud.
const CALIBRATE: Duration = Duration::from_secs(1);

/// How far over the noise floor a burst has to be. An acoustic path attenuates
/// it — an earcup against a microphone is not a wire — so the threshold is
/// measured rather than assumed, and this is the margin over what the room is
/// already doing.
const OVER_NOISE: u16 = 4;

/// A threshold below this is measuring dither, not audio.
const FLOOR: u16 = 600;

/// From DR-12: 10 ms of shared-mode engine period each way. DR-23 measured the
/// whole round trip at four times that, so this is the part of the number the
/// device's own cadence explains, and the rest is what the report is for.
const DEVICE_MS: f64 = 20.0;

/// prd.md §4, and what DR-14 measured of it on the wire. What is left is what
/// the devices may spend.
const BUDGET_MS: f64 = 80.0;
const WIRE_MS: f64 = 21.4;

#[tokio::main]
async fn main() -> Result<()> {
    let mode = args();

    println!("goodvoice audio spike (plan.md task 2.1)\n");

    // What cpal negotiated, before anything is opened: a measurement that does
    // not say what it was taken on is not reproducible.
    let (capture, render) = hardware::describe().context("no usable audio endpoint")?;
    println!("  capture  {}", endpoint(&capture));
    println!("  render   {}", endpoint(&render));

    // Timed because opening them is on the critical path of every join, and
    // task 4.4 has three seconds to spend on the whole of one.
    let opening = Instant::now();
    let (mut microphone, speakers) = hardware::open(std::sync::Arc::new(
        goodvoice_client_lib::audio::prefs::AudioPrefs::default(),
    ))
    .context("no usable capture or render endpoint")?;
    println!(
        "\ndevices are open at {SAMPLE_RATE_HZ} Hz, {} ms after asking\n",
        opening.elapsed().as_millis()
    );

    match mode {
        Mode::Monitor { seconds } => monitor(&mut microphone, &speakers, seconds).await,
        Mode::RoundTrip { bursts } => roundtrip(&mut microphone, &speakers, bursts).await,
    }
}

fn endpoint(endpoint: &hardware::Endpoint) -> String {
    format!(
        "{} — {} Hz, {} ch, {}, buffer {}",
        endpoint.name, endpoint.sample_rate, endpoint.channels, endpoint.format, endpoint.buffer
    )
}

// --- monitor ---------------------------------------------------------------

/// The microphone, straight back out of the speakers.
async fn monitor(
    microphone: &mut impl AudioSource,
    speakers: &impl AudioSink,
    seconds: u64,
) -> Result<()> {
    println!("monitor: the microphone is playing back through the speakers.");
    println!("wear headphones — a microphone that can hear the speakers will howl.");
    println!("say something; the meter is the level going out.\n");

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut window = Instant::now();
    let mut loudest = 0;
    let mut frames = 0_usize;

    while Instant::now() < deadline {
        let Some(frame) = microphone.next_frame().await else {
            bail!("the microphone stopped producing frames");
        };
        frames += 1;
        loudest = loudest.max(peak(&frame));
        speakers.play(0, &frame);

        if window.elapsed() >= Duration::from_secs(1) {
            println!("  {}", meter(loudest));
            loudest = 0;
            window = Instant::now();
        }
    }

    println!("\n{frames} frames captured and played, {seconds} s.");
    if frames == 0 {
        bail!("no audio was captured at all");
    }
    println!("Whether it was audible is the half only a person can sign off.");
    Ok(())
}

/// A level as a bar, so a run in a log still shows whether the microphone was
/// alive.
fn meter(loudest: u16) -> String {
    const WIDTH: usize = 40;
    let filled = (usize::from(loudest) * WIDTH) / usize::from(u16::MAX);
    format!(
        "[{}{}] {loudest:5}",
        "#".repeat(filled),
        " ".repeat(WIDTH - filled)
    )
}

// --- round trip ------------------------------------------------------------

async fn roundtrip(
    microphone: &mut hardware::Microphone,
    speakers: &hardware::Speakers,
    bursts: usize,
) -> Result<()> {
    println!("round trip: hold the earcup against the microphone.");
    println!("a burst leaves the speakers once a second; the microphone times it.\n");

    let noise = calibrate(microphone).await?;
    let threshold = (noise.saturating_mul(OVER_NOISE)).max(FLOOR);
    println!("  room noise floor {noise}, counting anything over {threshold} as a burst\n");

    let flight = Flight::default();
    let edge = Edge::new(threshold);
    let mut times = Vec::with_capacity(bursts);
    let mut backlog = Vec::with_capacity(bursts);
    let mut waiting = Vec::with_capacity(bursts);
    let mut produced = 0_usize;

    // Twice as long as the bursts should take, plus a little: a run that is
    // hearing nothing should say so rather than hang.
    let deadline = Instant::now() + Duration::from_secs(bursts as u64 * 2 + 10);

    while times.len() < bursts && Instant::now() < deadline {
        let Some(frame) = microphone.next_frame().await else {
            bail!("the microphone stopped producing frames");
        };

        if let Some(index) = edge.crossed(&frame) {
            if let Some(elapsed) = flight.arrive() {
                times.push(elapsed.saturating_sub(tail_of(index)));
                // Read after the frame was taken, so it is what was queued
                // *behind* this one: the burst is that much older than the
                // moment it was noticed.
                waiting.push(microphone.queued());
            }
        }
        flight.expire(LOST_AFTER);

        // Silence between bursts rather than nothing at all: a render ring
        // that runs dry is a device underrun, and it would be measuring a
        // path no call is ever in.
        produced += 1;
        if produced % BURST_INTERVAL_FRAMES == 0 {
            // Handed over and timed in that order, so the clock starts as
            // close to the speakers as this side can get. Close is not the
            // same as at: whatever the ring is already holding is time the
            // burst waits before the device sees it, and it is inside every
            // number below. Read it first, or it cannot be taken back out.
            backlog.push(speakers.queued(0));
            speakers.play(0, &burst_frame());
            flight.depart();
        } else {
            speakers.play(0, &silent_frame());
        }
    }

    report(&times, &flight, threshold, &backlog, &waiting)
}

/// What the room is doing when nothing is being played into it.
async fn calibrate(microphone: &mut impl AudioSource) -> Result<u16> {
    let until = Instant::now() + CALIBRATE;
    let mut noise = 0;
    while Instant::now() < until {
        let Some(frame) = microphone.next_frame().await else {
            bail!("the microphone stopped producing frames while calibrating");
        };
        noise = noise.max(peak(&frame));
    }
    Ok(noise)
}

/// How much of this frame came *after* the burst started.
///
/// A frame is 20 ms of audio handed over at its end, so timing the arrival at
/// the moment the frame appears is 20 ms late by however far into the frame
/// the burst fell. This buys that back. It assumes the capture ring is being
/// drained as fast as it fills, which is the only state this loop can be in:
/// it does nothing else.
fn tail_of(onset: usize) -> Duration {
    let samples = FRAME_SAMPLES.saturating_sub(onset) as u64;
    Duration::from_nanos((samples * 1_000_000_000) / u64::from(SAMPLE_RATE_HZ))
}

/// A count of 48 kHz samples as the time they take to play.
fn played_in(samples: usize) -> Duration {
    let samples = u64::try_from(samples).unwrap_or(0);
    Duration::from_nanos((samples * 1_000_000_000) / u64::from(SAMPLE_RATE_HZ))
}

fn report(
    times: &[Duration],
    flight: &Flight,
    threshold: u16,
    backlog: &[usize],
    waiting: &[usize],
) -> Result<()> {
    let heard = times.len();
    let sent = heard + flight.lost();

    // A run that heard a handful of its bursts heard the room, not the
    // speakers, and a plausible-looking millisecond count out of that is worse
    // than no number at all: it is the one that would end up in a Decision
    // Record. Half is a low bar deliberately — coupling this crude drops some.
    if heard * 2 < sent {
        bail!(
            "only {heard} of {sent} bursts came back, so what did arrive is room noise \
             rather than the burst.\n  Hold the earcup against the microphone, and check \
             the burst is going to the device it is held against.\n  (anything over \
             {threshold} counted as a burst)"
        );
    }

    let Some(spread) = Spread::of(times) else {
        bail!(
            "no burst ever came back — {sent} went out. Is the earcup against the \
             microphone, and is the output device the one it is against? \
             (threshold was {threshold})"
        );
    };

    println!(
        "\n--- {} round trips, {} bursts nobody heard ---\n",
        spread.count,
        flight.lost()
    );
    println!("  speakers → air → microphone");
    println!("    min     {:6.1} ms", spread.min);
    println!("    median  {:6.1} ms", spread.median);
    println!("    p95     {:6.1} ms", spread.p95);
    println!("    max     {:6.1} ms", spread.max);

    // How much of that was this process rather than the platform. The burst
    // is timed from the moment it is queued, so a ring holding 40 ms of audio
    // puts 40 ms into the round trip that no device charged for.
    let queued: Vec<Duration> = backlog.iter().copied().map(played_in).collect();
    let behind: Vec<Duration> = waiting.iter().copied().map(played_in).collect();
    let ours = match (Spread::of(&queued), Spread::of(&behind)) {
        (Some(render), Some(capture)) => {
            println!("\n  of which our own rings were holding");
            println!(
                "    render  median {:6.1} ms, max {:6.1} ms",
                render.median, render.max
            );
            println!(
                "    capture median {:6.1} ms, max {:6.1} ms",
                capture.median, capture.max
            );
            render.median + capture.median
        }
        _ => 0.0,
    };
    println!(
        "\n  so below cpal the platform costs about {:.1} ms.",
        spread.median - ours
    );

    let devices = BUDGET_MS - WIRE_MS;
    println!("\n  of which DR-12's shared-mode engine periods explain {DEVICE_MS:.1} ms;");
    println!("  the rest is the driver stack, the converters and the air.");
    println!(
        "\n  a call spends {WIRE_MS:.1} ms on the wire (DR-14), so the devices\n  \
         may spend {devices:.1} ms of the {BUDGET_MS:.0} ms budget."
    );

    if spread.median <= devices {
        println!(
            "\nWITHIN BUDGET: cpal's WASAPI path costs {:.1} ms of the {devices:.1} ms it has.",
            spread.median
        );
    } else {
        println!(
            "\nOVER BUDGET by {:.1} ms — and the rings above say it is not this process.\n\
             That is not yet a case for the `wasapi` crate: what it would buy is a\n\
             shorter engine period, and DR-12's device reports minimum = default here.\n\
             What separates the stack from the hardware is the same run on an endpoint\n\
             that is not USB; until that exists this number belongs to these devices.",
            spread.median - devices
        );
    }
    println!("\nIncluded here and not in a call: the trip through the air.");
    Ok(())
}

// --- arguments -------------------------------------------------------------

enum Mode {
    Monitor { seconds: u64 },
    RoundTrip { bursts: usize },
}

fn args() -> Mode {
    let mut roundtrip = false;
    let mut seconds = DEFAULT_SECONDS;
    let mut bursts = DEFAULT_BURSTS;

    let mut argv = env::args().skip(1);
    while let Some(flag) = argv.next() {
        match flag.as_str() {
            "--roundtrip" => roundtrip = true,
            "--seconds" => {
                seconds = argv
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(DEFAULT_SECONDS);
            }
            "--bursts" => {
                bursts = argv
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(DEFAULT_BURSTS);
            }
            _ => {}
        }
    }

    if roundtrip {
        Mode::RoundTrip {
            bursts: bursts.max(1),
        }
    } else {
        Mode::Monitor {
            seconds: seconds.max(1),
        }
    }
}

const _: () = {
    // A burst every `BURST_INTERVAL_FRAMES` frames has to be rarer than the
    // patience for one, or every burst is written off before it can arrive.
    assert!(BURST_INTERVAL_FRAMES as u128 * FRAME_MS as u128 > LOST_AFTER.as_millis());
};
