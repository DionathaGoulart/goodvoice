//! A call that drops mid-conversation and comes back on its own.
//!
//! plan.md task 3.5's automated proof. Two clients hold a conversation through
//! the live SFU, one of them loses its seat, and the drill checks the three
//! things a reconnect has to get right:
//!
//! 1. the client that dropped takes a **new** seat by itself,
//! 2. it can be heard again on it — the microphone was republished,
//! 3. the roommate who did nothing hears it again — their subscription followed
//!    the new session id rather than staying pointed at the dead one (DR-8).
//!
//! ```text
//! cargo run --bin reconnect-drill
//! cargo run --bin reconnect-drill -- --base http://localhost:8787 --room test
//! ```
//!
//! The drop here is [`Call::drop_session`], not a real network failure: taking
//! the network away needs a privileged tool and cannot run everywhere. See
//! docs/testing/reconnect.md for the run that does it for real — this drill is
//! what makes the *code path* checkable on any host, on demand.

use std::{
    env,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context as _, Result};
use goodvoice_client_lib::{
    audio::{
        device::{AudioSink, RecordingSink, ToneSource},
        vad::TransmitMode,
    },
    rtc::{
        reconnect::CallState,
        session::{Call, CallOptions},
    },
};

const DEFAULT_BASE: &str = "https://goodvoice.goodvoice-server.workers.dev";

/// One tone per direction, far enough apart to be told apart by ear if a human
/// ever listens in on the run.
const ALICE_HZ: f32 = 440.0;
const BOB_HZ: f32 = 1_200.0;

/// Frames each side waits for before calling a stretch of the call good. 50
/// frames is one second of audio.
const TARGET_FRAMES: usize = 50;

/// How long one stretch of conversation gets to happen.
const MEDIA_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the reconnect itself gets. The schedule's first wait is 500 ms and
/// a join takes a couple of seconds, so this is generous by design — the drill
/// is checking that it happens, not how fast.
const RECONNECT_TIMEOUT: Duration = Duration::from_secs(45);

#[tokio::main]
async fn main() -> Result<()> {
    let Args { base, room } = args();
    println!("room {room} on {base}");

    println!("\n1. two clients join and start talking");
    let alice_ears = RecordingSink::new();
    let alice = join(&base, &room, "alice", ALICE_HZ, &alice_ears).await?;
    let bob_ears = RecordingSink::new();
    let bob = join(&base, &room, "bob", BOB_HZ, &bob_ears).await?;

    let before = alice.self_id();
    println!("  alice is {before}");
    println!("  bob   is {}", bob.self_id());

    both_hear(&alice_ears, &bob_ears).await?;

    println!("\n2. alice loses her seat");
    alice_ears.reset();
    bob_ears.reset();
    alice.drop_session();

    println!("\n3. waiting for alice to take a new one");
    let saw_reconnecting = wait_for_recovery(&alice).await?;
    let after = alice.self_id();
    println!("  alice is now {after}");

    if !saw_reconnecting {
        // Not fatal — a fast rejoin can be over before the watcher looks — but
        // the UI's "reconnecting…" state is what this task promised the user,
        // so a run that never showed it is worth saying out loud.
        println!("  note: the reconnecting state came and went too fast to observe");
    }
    if after == before {
        bail!("alice kept her old participant id; she never actually rejoined");
    }

    println!("\n4. checking both sides can hear each other again");
    // The interesting half is bob: he did nothing at all, so his subscription
    // has to have followed alice to her new session on its own.
    let heard = both_hear(&alice_ears, &bob_ears).await;
    if heard.is_err() {
        println!("\n  what each side could see when it gave up:");
        describe_roster("alice", &alice);
        describe_roster("bob", &bob);
    }

    alice.leave().await;
    bob.leave().await;
    heard?;

    println!("\nPASS — a dropped call rejoined and both sides were audible again.");
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
            // Open: the source here is a pure tone, and a voice detector has
            // every right to decide that is not a voice.
            mode: TransmitMode::Open,
        },
        Box::new(ToneSource::new(tone_hz)),
        Arc::clone(ears) as Arc<dyn AudioSink>,
    )
    .await
    .with_context(|| format!("{name} could not join"))
}

/// Waits until a client is live again, reporting whether the reconnecting state
/// was ever visible on the way.
async fn wait_for_recovery(call: &Call) -> Result<bool> {
    let mut state = call.state();
    let deadline = tokio::time::Instant::now() + RECONNECT_TIMEOUT;
    let mut saw_reconnecting = false;
    // The call was live when the drop was requested, so the first change to
    // watch for is away from `Live`.
    let mut was_live = true;

    loop {
        if tokio::time::timeout_at(deadline, state.changed())
            .await
            .is_err()
        {
            bail!("alice never came back within {RECONNECT_TIMEOUT:?}");
        }

        let current = state.borrow_and_update().clone();
        match current {
            CallState::Reconnecting { attempt } => {
                println!("  reconnecting (attempt {attempt})");
                saw_reconnecting = true;
                was_live = false;
            }
            CallState::Live if !was_live || saw_reconnecting => return Ok(saw_reconnecting),
            CallState::Live => was_live = true,
            CallState::Ended(reason) => bail!("the call ended instead of reconnecting: {reason:?}"),
        }
    }
}

/// Waits for both sides to hear enough of each other, and reports what each of
/// them got either way.
///
/// Both are waited out even when one has already failed: which side went silent
/// is the whole diagnosis, and cancelling the other the moment the first gives
/// up throws that away.
async fn both_hear(alice: &Arc<RecordingSink>, bob: &Arc<RecordingSink>) -> Result<()> {
    let (alice_frames, bob_frames) =
        tokio::join!(wait_for_audio(alice, "alice"), wait_for_audio(bob, "bob"));

    if alice_frames >= TARGET_FRAMES && bob_frames >= TARGET_FRAMES {
        return Ok(());
    }
    bail!(
        "expected {TARGET_FRAMES} frames each way; alice heard {alice_frames}, bob heard \
         {bob_frames}"
    );
}

/// Waits until enough audio has arrived, and answers with how much did.
async fn wait_for_audio(ears: &Arc<RecordingSink>, who: &str) -> usize {
    let deadline = tokio::time::Instant::now() + MEDIA_TIMEOUT;
    let mut ticker = tokio::time::interval(Duration::from_millis(100));

    loop {
        ticker.tick().await;
        let heard = ears.loudest().frames;
        if heard >= TARGET_FRAMES || tokio::time::Instant::now() >= deadline {
            println!("  {who} received {heard} frames");
            return heard;
        }
    }
}

/// Prints what one client believes the room looks like.
fn describe_roster(who: &str, call: &Call) {
    let roster = call.roster();
    let participants = roster.borrow();
    println!("  {who} sees {} participants:", participants.len());
    for peer in participants.iter() {
        println!(
            "    {} session={} tracks=[{}]{}",
            peer.name,
            peer.session_id.as_deref().unwrap_or("none"),
            peer.tracks
                .iter()
                .map(|track| track.name.as_str())
                .collect::<Vec<_>>()
                .join(","),
            if peer.id == call.self_id() {
                " (self)"
            } else {
                ""
            },
        );
    }
}

struct Args {
    base: String,
    room: String,
}

fn args() -> Args {
    let mut base = env::var("GOODVOICE_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_owned());
    // A fresh room per run: a drill leaves an abandoned seat behind for as long
    // as the sweep takes, and reusing one code would walk the room toward its
    // 8-person cap.
    let mut room = format!("drill-{:08x}", seed());

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
/// suffix in a drill; not a substitute for an RNG.
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
