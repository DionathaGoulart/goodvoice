//! Two clients holding a conversation through the Cloudflare Realtime SFU,
//! without a microphone in sight.
//!
//! Written for plan.md task 2.3 — can `webrtc-rs` complete ICE/DTLS with
//! Realtime and carry an Opus track? — and kept as task 2.4's automated proof.
//! It now drives the real [`Call`], so what it exercises is the shipping code
//! path rather than a parallel implementation of it: join, publish, roster,
//! subscribe, decode.
//!
//! Each side publishes its own tone and listens for the other's. Two different
//! frequencies is what makes the check meaningful — the same tone both ways
//! would pass even if a client were only hearing itself.
//!
//! ```text
//! cargo run --bin rtc-spike -- --room test
//! cargo run --bin rtc-spike -- --base http://localhost:8787
//! ```

use std::{
    env,
    f32::consts::TAU,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context as _, Result};
use goodvoice_client_lib::{
    audio::{
        device::{AudioSink, RecordingSink, ToneSource},
        opus::{Frame, SAMPLE_RATE_HZ},
    },
    rtc::session::{Call, CallOptions},
};

/// The deploy from plan.md task 1.5.
const DEFAULT_BASE: &str = "https://goodvoice.goodvoice-server.workers.dev";

/// One tone per direction, far enough apart that a Goertzel bin cannot confuse
/// them and both well inside the band Opus keeps at 32 kbps.
const ALICE_HZ: f32 = 440.0;
const BOB_HZ: f32 = 1_200.0;

/// How much audio each side waits to hear before judging it. 100 frames is two
/// seconds — long enough to outlast the codec's lookahead and the jitter at the
/// head of a fresh stream.
const TARGET_FRAMES: usize = 100;

const MEDIA_TIMEOUT: Duration = Duration::from_secs(25);

#[tokio::main]
async fn main() -> Result<()> {
    let Args { base, room } = args();
    println!("room {room} on {base}");

    println!("\n1. alice joins and publishes a {ALICE_HZ} Hz tone");
    let alice_ears = RecordingSink::new();
    let alice = join(&base, &room, "alice", ALICE_HZ, &alice_ears).await?;
    println!("  alice is {}", alice.self_id());

    println!("\n2. bob joins and publishes a {BOB_HZ} Hz tone");
    let bob_ears = RecordingSink::new();
    let bob = join(&base, &room, "bob", BOB_HZ, &bob_ears).await?;
    println!("  bob is {}", bob.self_id());

    println!("\n3. waiting for {TARGET_FRAMES} frames each way");
    let (alice_heard, bob_heard) = tokio::try_join!(
        wait_for_audio(&alice_ears, "alice"),
        wait_for_audio(&bob_ears, "bob"),
    )?;

    println!("\n4. checking who heard what");
    // Crossed on purpose: alice must be hearing *bob's* tone, not her own.
    let alice_ok = check("alice heard bob  ", &alice_heard, BOB_HZ, ALICE_HZ);
    let bob_ok = check("bob   heard alice", &bob_heard, ALICE_HZ, BOB_HZ);

    println!(
        "\n  roster carried {} participants",
        alice.roster().borrow().len()
    );

    alice.leave().await;
    bob.leave().await;

    if !(alice_ok && bob_ok) {
        bail!("the two clients did not hear each other");
    }
    println!("\nPASS — two goodvoice clients held a conversation through the SFU.");
    Ok(())
}

async fn join(
    base: &str,
    room: &str,
    name: &str,
    tone_hz: f32,
    ears: &Arc<RecordingSink>,
) -> Result<Call> {
    Call::join(
        CallOptions {
            base: base.to_owned(),
            room: room.to_owned(),
            name: name.to_owned(),
        },
        Box::new(ToneSource::new(tone_hz)),
        Arc::clone(ears) as Arc<dyn AudioSink>,
    )
    .await
    .with_context(|| format!("{name} could not join"))
}

/// Waits until slot 0 has collected enough audio to judge.
///
/// Slot 0 because there is exactly one other person in the room; task 3.1 is
/// what makes the other six slots interesting.
async fn wait_for_audio(ears: &Arc<RecordingSink>, who: &str) -> Result<Frame> {
    let deadline = tokio::time::Instant::now() + MEDIA_TIMEOUT;
    let mut ticker = tokio::time::interval(Duration::from_millis(100));

    loop {
        ticker.tick().await;
        let record = ears.slot(0);
        if record.frames >= TARGET_FRAMES {
            println!("  {who} received {} frames", record.frames);
            return Ok(record.last);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "{who} received only {} of the {TARGET_FRAMES} frames expected",
                record.frames
            );
        }
    }
}

/// Passes when `frame` carries the tone the *other* side is sending, and not
/// the one this side sends itself.
fn check(label: &str, frame: &Frame, expected_hz: f32, other_hz: f32) -> bool {
    let wanted = bin_energy(frame, expected_hz);
    let unwanted = bin_energy(frame, other_hz);
    let ok = wanted > unwanted * 10.0;

    println!(
        "  {label}: {expected_hz:>6.0} Hz {wanted:>10.3e} vs {other_hz:>6.0} Hz {unwanted:>10.3e}  {}",
        if ok { "ok" } else { "FAILED" }
    );
    ok
}

/// Goertzel: energy in one frequency bin, enough to tell two tones apart
/// without pulling in an FFT.
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
}

fn args() -> Args {
    let mut base = env::var("GOODVOICE_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_owned());
    // A fresh room per run by default: participants only leave on a heartbeat
    // sweep, so reusing one code would walk the room toward its 8-person cap.
    let mut room = format!("spike-{:08x}", seed());

    let mut argv = env::args().skip(1);
    while let Some(flag) = argv.next() {
        match flag.as_str() {
            "--base" => {
                if let Some(value) = argv.next() {
                    base = value;
                }
            }
            "--room" => {
                if let Some(value) = argv.next() {
                    room = value;
                }
            }
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }

    Args {
        base: base.trim_end_matches('/').to_owned(),
        room,
    }
}

/// Nanoseconds since the epoch folded into 32 bits. Enough entropy for a room
/// suffix in a spike; not a substitute for an RNG.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the low bits are the point — this is a nonce, not a clock"
)]
fn seed() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |elapsed| {
            elapsed.subsec_nanos() ^ elapsed.as_secs() as u32
        })
}
