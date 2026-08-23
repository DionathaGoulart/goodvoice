//! How long a word takes to cross the wire.
//!
//! plan.md task 2.5. The PRD's headline budget is ≤80 ms mouth to ear
//! (prd.md §4) and nothing in this repository had ever measured any part of
//! it. This does the part the code controls.
//!
//! # What is being timed, exactly
//!
//! From the moment one client hands a frame to its encoder, to the moment the
//! other client hands the decoded frame to its speakers. That covers Opus,
//! the network, Cloudflare's SFU, and the pull side's decode. It does **not**
//! cover the audio devices: DR-12 measured those separately at 10 ms of
//! shared-mode period each way. Add 20 ms to anything printed here to compare
//! against the PRD's number.
//!
//! Both clients run in this one process, so both timestamps come off the same
//! clock and there is no synchronisation to get wrong. The audio still makes
//! the whole real trip through Cloudflare.
//!
//! # Method
//!
//! One side is silent except for a short loud burst once a second. It notes
//! the instant it produced the burst; the other side notes the instant the
//! burst reached its speakers. At most one burst is in flight at a time, so
//! pairing them needs no identity in the signal — a burst that never arrives
//! is counted as lost rather than paired with the next one.
//!
//! ```text
//! cargo run --bin latency
//! cargo run --bin latency -- --base http://localhost:8787 --room test --pings 30
//! ```

use std::{
    env,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context as _, Result};
use goodvoice_client_lib::{
    audio::{
        burst::{self, burst_frame, Edge, Spread, SILENT_PATH_THRESHOLD},
        device::{AudioSink, AudioSource, NullSink},
        opus::{silent_frame, Frame, FRAME_MS},
        prefs::AudioPrefs,
        vad::TransmitMode,
    },
    rtc::session::{Call, CallOptions},
};

const DEFAULT_BASE: &str = "https://goodvoice.goodvoice-server.workers.dev";

/// How many bursts to time before reporting. Thirty seconds of call.
const DEFAULT_PINGS: usize = 30;

/// Frames between bursts. Fifty is one second — far longer than any plausible
/// latency, which is what keeps at most one burst in flight.
const PING_INTERVAL_FRAMES: usize = 50;

/// Frames to let the call settle before the first burst counts. ICE finishes
/// during the join, but the first packets after it are not representative.
const WARMUP_FRAMES: usize = 50;

/// A burst still unheard after this is lost, not slow.
const LOST_AFTER: Duration = Duration::from_secs(2);

/// The PRD's budget, and what the devices take out of it before a packet moves.
///
/// This used to be DR-12's 20 ms of shared-mode engine period, which is the
/// callback cadence and not the latency. DR-23 measured the two legs acoustically
/// on the same machine — speakers to air to microphone, with the rings shown to
/// be holding nothing — and got 84.7 ms. An engine period is one term of that
/// sum, and on this hardware a small one.
const BUDGET_MS: f64 = 80.0;
const DEVICE_MS: f64 = 84.7;

#[tokio::main]
async fn main() -> Result<()> {
    let (base, room, pings) = args();
    println!("goodvoice latency harness");
    println!("  server {base}");
    println!("  room   {room}");
    println!("  pings  {pings}\n");

    let flight = Arc::new(Flight::default());

    // The talker is silent apart from its bursts; the listener has no
    // microphone worth the name, so it sends silence and only listens.
    let heard = Arc::new(Listener::new(Arc::clone(&flight)));
    let talker = Call::join(
        options(&base, &room, "talker"),
        Box::new(Pinger::new(Arc::clone(&flight))),
        Arc::new(NullSink) as Arc<dyn AudioSink>,
    )
    .await
    .context("the talker could not join")?;
    let listener = Call::join(
        options(&base, &room, "listener"),
        Box::new(Silence),
        Arc::clone(&heard) as Arc<dyn AudioSink>,
    )
    .await
    .context("the listener could not join")?;

    flight.ready();
    println!("both sides are in; timing {pings} bursts (about {pings} seconds)\n");

    let deadline = Instant::now() + Duration::from_secs(pings as u64 * 2 + 30);
    while heard.count() < pings && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
        flight.expire();
    }

    talker.leave().await;
    listener.leave().await;

    report(&heard, &flight, pings)
}

fn options(base: &str, room: &str, name: &str) -> CallOptions {
    CallOptions {
        base: base.to_owned(),
        room: room.to_owned(),
        name: name.to_owned(),
        // The gate would have opinions about a burst of silence-then-tone, and
        // this is measuring the path, not the gate.
        mode: TransmitMode::Open,
        prefs: Arc::new(AudioPrefs::default()),
    }
}

// --- the burst in flight ---------------------------------------------------

/// The burst in flight, plus what this harness has to know that the shared
/// bookkeeping does not.
#[derive(Default)]
struct Flight {
    inner: burst::Flight,
    /// When both clients were in the room, and what the first burst that was
    /// actually heard cost to get there.
    ///
    /// Joining a room is not the same as being subscribed to what is in it:
    /// the pull side has to see the talker on the roster, ask the SFU for the
    /// track and renegotiate for it, and every burst that goes out meanwhile
    /// is heard by nobody. Those are not lost packets and reporting them as
    /// such would be a lie about the network, so they are counted separately
    /// (DR-14).
    ready: Mutex<Option<Instant>>,
    warmup: Mutex<Option<Warmup>>,
}

/// What it took before the far end could hear anything at all.
struct Warmup {
    after: Duration,
    lost: usize,
}

impl Flight {
    fn depart(&self) {
        self.inner.depart();
    }

    fn expire(&self) {
        self.inner.expire(LOST_AFTER);
    }

    /// Notes that both clients are in the room, which is where the wait for
    /// the first audible burst is timed from.
    fn ready(&self) {
        if let Ok(mut ready) = self.ready.lock() {
            *ready = Some(Instant::now());
        }
    }

    /// Claims the burst in flight, if there is one, and returns how long it
    /// took. `None` means audio arrived that no burst explains.
    fn arrive(&self) -> Option<Duration> {
        let elapsed = self.inner.arrive()?;
        self.note_warmup();
        Some(elapsed)
    }

    /// The first burst through is the one that says the subscription is live.
    fn note_warmup(&self) {
        let Ok(mut warmup) = self.warmup.lock() else {
            return;
        };
        if warmup.is_some() {
            return;
        }
        let since_ready = self
            .ready
            .lock()
            .ok()
            .and_then(|ready| *ready)
            .map_or(Duration::ZERO, |ready| ready.elapsed());
        *warmup = Some(Warmup {
            after: since_ready,
            lost: self.inner.lost(),
        });
    }

    /// Bursts written off once the far end was demonstrably listening. These
    /// are the ones that mean something: audio the network swallowed.
    fn lost(&self) -> usize {
        let total = self.inner.lost();
        let warmed = self
            .warmup
            .lock()
            .ok()
            .and_then(|warmup| warmup.as_ref().map(|warmup| warmup.lost))
            .unwrap_or(total);
        total.saturating_sub(warmed)
    }

    /// How long the subscription took to carry sound, and how many bursts went
    /// out into nothing while it did.
    fn warmup(&self) -> Option<(Duration, usize)> {
        let warmup = self.warmup.lock().ok()?;
        warmup.as_ref().map(|warmup| (warmup.after, warmup.lost))
    }
}

// --- the two ends ----------------------------------------------------------

/// Silence, with a burst at the start of every `PING_INTERVAL_FRAMES`th frame.
///
/// The burst sits at sample zero on purpose: the instant this frame is handed
/// over is then the instant the burst begins, and the far end's frame boundary
/// means the same thing. Nothing has to be corrected for where in a frame the
/// sound fell.
struct Pinger {
    ticker: tokio::time::Interval,
    produced: usize,
    flight: Arc<Flight>,
}

impl Pinger {
    fn new(flight: Arc<Flight>) -> Self {
        Self {
            ticker: tokio::time::interval(Duration::from_millis(u64::from(FRAME_MS))),
            produced: 0,
            flight,
        }
    }
}

#[async_trait::async_trait]
impl AudioSource for Pinger {
    async fn next_frame(&mut self) -> Option<Frame> {
        self.ticker.tick().await;
        self.produced += 1;

        if self.produced > WARMUP_FRAMES && self.produced % PING_INTERVAL_FRAMES == 0 {
            let frame = burst_frame();
            // Last thing before handing the frame over, so the clock starts as
            // close to the encoder as this side can get.
            self.flight.depart();
            return Some(frame);
        }
        Some(silent_frame())
    }
}

/// A microphone with nothing to say. The listener still has to publish one:
/// a client that never sends is not a client this path supports.
struct Silence;

#[async_trait::async_trait]
impl AudioSource for Silence {
    async fn next_frame(&mut self) -> Option<Frame> {
        tokio::time::sleep(Duration::from_millis(u64::from(FRAME_MS))).await;
        Some(silent_frame())
    }
}

/// Ears that stop the clock on the burst's leading edge.
struct Listener {
    flight: Arc<Flight>,
    edge: Edge,
    times: Mutex<Vec<Duration>>,
}

impl Listener {
    fn new(flight: Arc<Flight>) -> Self {
        Self {
            flight,
            // Nothing but digital silence shares this path, so the threshold
            // does not have to be measured against a noise floor.
            edge: Edge::new(SILENT_PATH_THRESHOLD),
            times: Mutex::new(Vec::new()),
        }
    }

    fn count(&self) -> usize {
        self.times.lock().map_or(0, |times| times.len())
    }

    fn times(&self) -> Vec<Duration> {
        self.times
            .lock()
            .map(|times| times.clone())
            .unwrap_or_default()
    }
}

impl AudioSink for Listener {
    fn play(&self, _slot: usize, frame: &Frame) {
        if self.edge.crossed(frame).is_none() {
            return;
        }
        if let Some(elapsed) = self.flight.arrive() {
            if let Ok(mut times) = self.times.lock() {
                times.push(elapsed);
            }
        }
    }

    fn clear(&self, _slot: usize) {}
}

// --- the report ------------------------------------------------------------

fn report(heard: &Listener, flight: &Flight, wanted: usize) -> Result<()> {
    let Some(spread) = Spread::of(&heard.times()) else {
        bail!(
            "no burst was ever heard — {} went out and none came back",
            flight.lost()
        );
    };
    let total = spread.median + DEVICE_MS;

    println!(
        "--- {} bursts heard, {} lost ---\n",
        spread.count,
        flight.lost()
    );
    if let Some((after, before_it)) = flight.warmup() {
        println!(
            "  first burst heard {:.1} s after both clients were in the room\n\
             \x20 ({before_it} went out before the pull was carrying anything)\n",
            after.as_secs_f64()
        );
    }
    println!("  wire path (encode → SFU → decode)");
    println!("    min     {:6.1} ms", spread.min);
    println!("    median  {:6.1} ms", spread.median);
    println!("    p95     {:6.1} ms", spread.p95);
    println!("    max     {:6.1} ms", spread.max);
    println!("\n  devices, measured in DR-23 (one capture and one render)");
    println!("    fixed   {DEVICE_MS:6.1} ms");
    println!("    note    that figure is this machine's, not a constant");
    println!("\n  mouth to ear, median");
    println!("    total   {total:6.1} ms  against a {BUDGET_MS:.0} ms budget");

    if spread.count < wanted {
        println!(
            "\nnote: only {} of {wanted} bursts were heard.",
            spread.count
        );
    }
    if total > BUDGET_MS {
        println!(
            "\nOVER BUDGET by {:.1} ms. Suspects, in the order worth checking:\n\
             \x20 - no jitter buffer exists yet, so this is the raw network;\n\
             \x20   a real one will *add* to the number, not subtract.\n\
             \x20 - the SFU's own relay hop, which nothing here can shorten.\n\
             \x20 - Opus at 20 ms frames: 10 ms frames would halve part of it\n\
             \x20   at the cost of bandwidth.",
            total - BUDGET_MS
        );
    } else {
        println!(
            "\nWITHIN BUDGET, with {:.1} ms to spare.",
            BUDGET_MS - total
        );
    }
    println!(
        "\nNot included: the analog path either side of the converters, and any\n\
         jitter buffer this client does not yet have."
    );
    Ok(())
}

// --- arguments -------------------------------------------------------------

fn args() -> (String, String, usize) {
    let mut base = env::var("GOODVOICE_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_owned());
    let mut room = default_room();
    let mut pings = DEFAULT_PINGS;

    let mut argv = env::args().skip(1);
    while let Some(flag) = argv.next() {
        let Some(value) = argv.next() else {
            break;
        };
        match flag.as_str() {
            "--base" => base = value,
            "--room" => room = value,
            "--pings" => pings = value.parse().unwrap_or(DEFAULT_PINGS),
            _ => {}
        }
    }
    (base, room, pings.max(1))
}

/// A room nobody else is in, so a run cannot be disturbed by one next door.
fn default_room() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    format!("latency-{stamp}")
}
