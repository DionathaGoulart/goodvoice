//! Mute stops the packets and deafen stops the playback — proved through the
//! real SFU instead of in a unit test.
//!
//! plan.md task 3.2. `cargo test` already pins each half where it is
//! implemented: the publish loop stops encoding when muted, and the decode
//! task stops playing when deafened. `server/test/presence.test.ts` pins the
//! room's half — both flags reach everyone else's roster. What none of them
//! can see is the whole path at once, which is what the task's definition of
//! done actually asks for: a peer who mutes going quiet on somebody else's
//! speakers, and the flag arriving there to explain the silence.
//!
//! Two clients in one process with crossed tones, the same shape as
//! `bin/rtc-spike`. The difference is what is being watched. The spike asks
//! whether audio arrives; this asks whether it *stops* arriving on cue, comes
//! back on cue, and whether stopping it for one ear leaves the other alone.
//!
//! Counting frames at the far sink is the strongest form of the "packets stop,
//! not zeroed" assertion the task asks for. A muted client that sent silence
//! would keep the count climbing; only a client that sends nothing freezes it.
//!
//! ```text
//! cargo run --bin mute-drill
//! cargo run --bin mute-drill -- --base http://localhost:8787
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
        vad::TransmitMode,
    },
    rtc::{
        session::{Call, CallOptions},
        signaling::Participant,
    },
};

/// The deploy from plan.md task 1.5.
const DEFAULT_BASE: &str = "https://goodvoice.goodvoice-server.workers.dev";

/// One tone per direction, far enough apart that a Goertzel bin cannot confuse
/// them. Crossed on purpose: a client hearing only itself would pass a check
/// that did not care which tone arrived.
const ALICE_HZ: f32 = 440.0;
const BOB_HZ: f32 = 1_200.0;

/// How much audio to see before calling a direction alive. Fifty frames is one
/// second, long enough to outlast the codec's lookahead at the head of a fresh
/// stream.
const TARGET_FRAMES: usize = 50;

/// How long to let a flag take effect before judging what it did. Packets
/// already in flight when someone mutes are not a bug; packets still arriving
/// after this would be.
const SETTLE: Duration = Duration::from_millis(1_500);

/// How long a silence has to hold before it counts as one. Longer than any
/// plausible jitter, so a single late frame cannot pass for a live stream and
/// a live stream cannot pass for silence.
const WINDOW: Duration = Duration::from_secs(2);

/// What a live stream owes over [`WINDOW`]. A 20 ms frame path produces fifty
/// a second; half of that survives a hiccup and is still nowhere near the zero
/// it is being told apart from.
const WINDOW_FRAMES: usize = (WINDOW.as_millis() as usize / 20) / 2;

/// How long to wait for a flag to cross the room and land in a roster.
const FLAG_TIMEOUT: Duration = Duration::from_secs(10);

const MEDIA_TIMEOUT: Duration = Duration::from_secs(25);

#[tokio::main]
async fn main() -> Result<()> {
    let Args { base, room } = args();
    println!("room {room} on {base}\n");

    println!("1. both clients join and publish");
    let alice_ears = RecordingSink::new();
    let alice = join(&base, &room, "alice", ALICE_HZ, &alice_ears).await?;
    let bob_ears = RecordingSink::new();
    let bob = join(&base, &room, "bob", BOB_HZ, &bob_ears).await?;
    let (alice_id, bob_id) = (alice.self_id(), bob.self_id());
    println!("  alice is {alice_id}\n  bob   is {bob_id}");

    println!("\n2. baseline — each side hears the other");
    tokio::try_join!(
        wait_for_audio(&alice_ears, "alice"),
        wait_for_audio(&bob_ears, "bob"),
    )?;
    let mut ok = check(
        "alice hears bob  ",
        &alice_ears.slot(0).last,
        BOB_HZ,
        ALICE_HZ,
    );
    ok &= check(
        "bob   hears alice",
        &bob_ears.slot(0).last,
        ALICE_HZ,
        BOB_HZ,
    );

    println!("\n3. alice mutes");
    alice.set_muted(true).await;
    ok &= expect_quiet(&bob_ears, "bob stops hearing alice").await;
    ok &= expect_flag(&bob, &alice_id, "muted", true, |peer| peer.muted).await;

    println!("\n4. alice unmutes");
    alice.set_muted(false).await;
    ok &= expect_audio(&bob_ears, "bob hears alice again").await;
    ok &= expect_flag(&bob, &alice_id, "muted", false, |peer| peer.muted).await;

    println!("\n5. bob deafens");
    bob.set_deafened(true).await;
    ok &= expect_quiet(&bob_ears, "bob's playback stops").await;
    ok &= expect_flag(&alice, &bob_id, "deafened", true, |peer| peer.deafened).await;
    // Deafen is one ear, not a disconnection: bob is still publishing, and
    // alice has no reason to stop hearing him.
    ok &= expect_audio(&alice_ears, "alice still hears bob").await;

    println!("\n6. bob undeafens");
    bob.set_deafened(false).await;
    ok &= expect_audio(&bob_ears, "bob's playback resumes").await;
    ok &= expect_flag(&alice, &bob_id, "deafened", false, |peer| peer.deafened).await;

    alice.leave().await;
    bob.leave().await;

    if !ok {
        bail!("mute or deafen did not do what task 3.2 says it does");
    }
    println!("\nPASS — mute stops the packets, deafen stops the playback, and the room saw both.");
    Ok(())
}

// --- the three assertions --------------------------------------------------

/// Passes when nothing more arrives at `ears` once the network has settled.
async fn expect_quiet(ears: &Arc<RecordingSink>, label: &str) -> bool {
    tokio::time::sleep(SETTLE).await;
    let before = ears.slot(0).frames;
    tokio::time::sleep(WINDOW).await;
    let arrived = ears.slot(0).frames.saturating_sub(before);

    let ok = arrived == 0;
    println!(
        "  {label}: {arrived} frames in {:.0} s  {}",
        WINDOW.as_secs_f64(),
        verdict(ok)
    );
    ok
}

/// Passes when audio is flowing again — the mirror of [`expect_quiet`], and the
/// half that catches a mute nobody can undo.
async fn expect_audio(ears: &Arc<RecordingSink>, label: &str) -> bool {
    tokio::time::sleep(SETTLE).await;
    let before = ears.slot(0).frames;
    tokio::time::sleep(WINDOW).await;
    let arrived = ears.slot(0).frames.saturating_sub(before);

    let wanted = WINDOW_FRAMES;
    let ok = arrived >= wanted;
    println!(
        "  {label}: {arrived} frames in {:.0} s (wanted {wanted}+)  {}",
        WINDOW.as_secs_f64(),
        verdict(ok)
    );
    ok
}

/// Passes when `watcher`'s roster says what it should about `who`.
///
/// The point of the task's "state visible in roster for everyone" is that the
/// flag is read from somebody *else's* copy of the room, so this always asks
/// the other client.
async fn expect_flag(
    watcher: &Call,
    who: &str,
    name: &str,
    want: bool,
    read: fn(&Participant) -> bool,
) -> bool {
    let deadline = tokio::time::Instant::now() + FLAG_TIMEOUT;
    let mut ticker = tokio::time::interval(Duration::from_millis(100));

    loop {
        ticker.tick().await;
        let seen = watcher
            .roster()
            .borrow()
            .iter()
            .find(|participant| participant.id == who)
            .map(read);

        if seen == Some(want) {
            println!("  roster says {name}={want}  {}", verdict(true));
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            println!(
                "  roster says {name}={seen:?}, wanted {want}  {}",
                verdict(false)
            );
            return false;
        }
    }
}

const fn verdict(ok: bool) -> &'static str {
    if ok {
        "ok"
    } else {
        "FAILED"
    }
}

// --- setup, shared with bin/rtc-spike --------------------------------------

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
            // Open: the source is a pure tone, and a voice detector has every
            // right to decide that is not a voice. This drill is about mute,
            // and a gate closing underneath it would look exactly like one.
            mode: TransmitMode::Open,
        },
        Box::new(ToneSource::new(tone_hz)),
        Arc::clone(ears) as Arc<dyn AudioSink>,
    )
    .await
    .with_context(|| format!("{name} could not join"))
}

async fn wait_for_audio(ears: &Arc<RecordingSink>, who: &str) -> Result<()> {
    let deadline = tokio::time::Instant::now() + MEDIA_TIMEOUT;
    let mut ticker = tokio::time::interval(Duration::from_millis(100));

    loop {
        ticker.tick().await;
        let record = ears.slot(0);
        if record.frames >= TARGET_FRAMES {
            println!("  {who} received {} frames", record.frames);
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "{who} received only {} of the {TARGET_FRAMES} frames expected",
                record.frames
            );
        }
    }
}

fn check(label: &str, frame: &Frame, expected_hz: f32, other_hz: f32) -> bool {
    let wanted = bin_energy(frame, expected_hz);
    let unwanted = bin_energy(frame, other_hz);
    let ok = wanted > unwanted * 10.0;
    println!(
        "  {label}: {expected_hz:>6.0} Hz {wanted:>10.3e} vs {other_hz:>6.0} Hz {unwanted:>10.3e}  {}",
        verdict(ok)
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
    let mut room = format!("mute-{:08x}", seed());

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
