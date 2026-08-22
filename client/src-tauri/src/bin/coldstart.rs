//! How long from launching goodvoice to being heard in a room.
//!
//! plan.md task 4.4, and prd.md §4's last budget: **in room and talking in
//! under three seconds**. Everything else in Phase 4 can be checked by looking
//! at it; this one is a number or it is nothing.
//!
//! # What is being timed
//!
//! From the instant this drill starts the app's process, to the instant a
//! client that is *already in the room* decodes the app's first frame of audio.
//! One clock, one process boundary, and the far end is a real second client —
//! so the number includes every part of the path a person would wait through:
//!
//! ```text
//! spawn ─ process + WebView2 start ─ audio devices ─ HTTP join ─ ICE ─ DTLS
//!       ─ publish ─ first Opus packet ─ SFU ─ subscribe ─ decode ─ heard
//! ```
//!
//! It does not include the person: the app joins the room named in
//! `GOODVOICE_AUTOJOIN` rather than waiting for somebody to type it and click.
//! A cold start measured with a human in it would be measuring the human.
//!
//! # Running it
//!
//! ```text
//! cargo run --bin coldstart                 # five runs against the live deploy
//! cargo run --bin coldstart -- --runs 3 --exe target\debug\goodvoice-client.exe
//! ```
//!
//! Every run uses a fresh room, so a run cannot be disturbed by the ghost of
//! the last one — the app is killed rather than asked to leave, and a seat
//! nobody gave back lingers until the room's own sweep clears it (DR-5).

use std::{
    env,
    io::BufRead as _,
    path::PathBuf,
    process::{Child, Command},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context as _, Result};
use goodvoice_client_lib::{
    audio::{
        burst::Spread,
        device::{AudioSink, AudioSource, MAX_REMOTE_SLOTS},
        opus::{silent_frame, Frame, FRAME_MS},
    },
    rtc::session::{Call, CallOptions},
};

const DEFAULT_BASE: &str = "https://goodvoice.goodvoice-server.workers.dev";
const DEFAULT_RUNS: usize = 5;

/// prd.md §4: cold start → in room and talking.
const BUDGET_MS: f64 = 3_000.0;

/// How long one run may take before it is called a failure rather than a slow
/// start. Generous on purpose: a run that is over budget is still a
/// measurement, and only one that never arrives is not.
const RUN_TIMEOUT: Duration = Duration::from_secs(45);

/// How long to let the listener settle in the room before starting the clock.
/// It has to be *in* before the app arrives, or the first thing measured is
/// the listener's own subscribe.
const LISTENER_SETTLE: Duration = Duration::from_millis(500);

#[tokio::main]
async fn main() -> Result<()> {
    let (base, exe, runs) = args();
    if !exe.exists() {
        bail!(
            "no app to launch at {}\n  build it first: cargo build --release --features custom-protocol --bin goodvoice-client",
            exe.display()
        );
    }

    println!("goodvoice cold-start drill (plan.md task 4.4)\n");
    println!("  server {base}");
    println!("  app    {}", exe.display());
    println!("  runs   {runs}\n");

    let mut measured = Vec::with_capacity(runs);
    for run in 1..=runs {
        match one_run(&base, &exe).await {
            Ok(timing) => {
                println!(
                    "  run {run}: {:6.0} ms heard{}",
                    timing.heard.as_secs_f64() * 1_000.0,
                    timing.joined.map_or(String::new(), |joined| format!(
                        ", of which {:.0} ms to join",
                        joined.as_secs_f64() * 1_000.0
                    ))
                );
                measured.push(timing);
            }
            Err(error) => println!("  run {run}: {error}"),
        }
    }

    report(&measured, runs)
}

/// The two halves of a cold start, and what separates them.
///
/// `joined` is the app saying it is in the room and publishing; `heard` is the
/// far end actually carrying it. The gap between them belongs to the *other*
/// client's subscribe, not to this one's start, and keeping them apart is what
/// stops an optimisation being aimed at the wrong half.
#[derive(Debug, Clone, Copy)]
struct Run {
    joined: Option<Duration>,
    heard: Duration,
}

/// One launch, timed from the process starting to the far end hearing it.
async fn one_run(base: &str, exe: &PathBuf) -> Result<Run> {
    let room = fresh_room();
    let heard = Arc::new(Ears::default());

    // The listener joins first and stays quiet. Its own join is not in the
    // measurement: the clock starts after it is in.
    let listener = Call::join(
        CallOptions {
            base: base.to_owned(),
            room: room.clone(),
            name: "coldstart-ears".to_owned(),
            mode: goodvoice_client_lib::audio::vad::TransmitMode::Open,
        },
        Box::new(Silence),
        Arc::clone(&heard) as Arc<dyn AudioSink>,
    )
    .await
    .context("the listener could not join")?;
    tokio::time::sleep(LISTENER_SETTLE).await;

    let started = Instant::now();
    let mut app = spawn_app(exe, &room).context("could not start the app")?;
    let joined = watch_for_join(&mut app, started);
    let mut app = Killed(Some(app));

    let deadline = started + RUN_TIMEOUT;
    while !heard.anything() {
        if Instant::now() >= deadline {
            listener.leave().await;
            bail!("nothing was heard in {} s", RUN_TIMEOUT.as_secs());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let elapsed = started.elapsed();

    app.stop();
    listener.leave().await;
    Ok(Run {
        joined: joined.read(),
        heard: elapsed,
    })
}

/// Reads the app's own account of when it got into the room.
///
/// It prints one line when the autojoin lands. Reading it on a thread rather
/// than polling keeps the timing loop free, and a pipe nobody drains would
/// eventually block the app itself.
fn watch_for_join(app: &mut Child, started: Instant) -> JoinMark {
    let mark = JoinMark::default();
    let Some(stdout) = app.stdout.take() else {
        return mark;
    };

    let at = Arc::clone(&mark.0);
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            // The app stamps its own startup marks against its own clock, and
            // they are most of what makes a slow cold start explicable: where
            // the time went, rather than that there was a lot of it.
            if line.ends_with(" ms") {
                println!("      app: {line}");
            }
            if line.starts_with("autojoined") {
                println!("      app: {line}");
                let millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                at.store(millis, Ordering::Release);
                return;
            }
        }
    });
    mark
}

/// When the app said it was in the room, in milliseconds since it was started.
/// Zero means it never said so.
#[derive(Default)]
struct JoinMark(Arc<AtomicU64>);

impl JoinMark {
    fn read(&self) -> Option<Duration> {
        match self.0.load(Ordering::Acquire) {
            0 => None,
            millis => Some(Duration::from_millis(millis)),
        }
    }
}

fn spawn_app(exe: &PathBuf, room: &str) -> std::io::Result<Child> {
    Command::new(exe)
        .env("GOODVOICE_AUTOJOIN", room)
        // Ask the join to say where its time went: three server round trips
        // and an ICE gathering, and which of them is slow is not guessable.
        .env("GOODVOICE_TRACE_JOIN", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
}

/// The app, killed rather than asked to leave.
///
/// A person closing goodvoice now hides it (task 4.1), so there is no polite
/// way to end it from outside — and what is being measured is the start, not
/// the stop. The room's sweep clears the seat.
struct Killed(Option<Child>);

impl Killed {
    fn stop(&mut self) {
        if let Some(mut app) = self.0.take() {
            let _ = app.kill();
            let _ = app.wait();
        }
    }
}

impl Drop for Killed {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A listener that only reports whether anything has arrived.
///
/// Not how loud it was: a microphone in a quiet room produces very little, and
/// the question is whether the far end is *carrying* this client at all. One
/// decoded frame is the answer.
#[derive(Default)]
struct Ears {
    heard: std::sync::atomic::AtomicBool,
}

impl Ears {
    fn anything(&self) -> bool {
        self.heard.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl AudioSink for Ears {
    fn play(&self, slot: usize, _frame: &Frame) {
        debug_assert!(slot < MAX_REMOTE_SLOTS);
        self.heard.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    fn clear(&self, _slot: usize) {}
}

/// The listener's microphone. A client that never sends is not a client this
/// path supports, so it sends silence.
struct Silence;

#[async_trait::async_trait]
impl AudioSource for Silence {
    async fn next_frame(&mut self) -> Option<Frame> {
        tokio::time::sleep(Duration::from_millis(u64::from(FRAME_MS))).await;
        Some(silent_frame())
    }
}

fn report(runs: &[Run], wanted: usize) -> Result<()> {
    let heard: Vec<Duration> = runs.iter().map(|run| run.heard).collect();
    let Some(spread) = Spread::of(&heard) else {
        bail!("no run produced a measurement");
    };

    println!("\n--- {} of {wanted} runs ---\n", spread.count);
    println!("  launch → heard in the room");
    println!("    min     {:6.0} ms", spread.min);
    println!("    median  {:6.0} ms", spread.median);
    println!("    max     {:6.0} ms", spread.max);

    let joined: Vec<Duration> = runs.iter().filter_map(|run| run.joined).collect();
    if let Some(join) = Spread::of(&joined) {
        println!("\n  of which launch → publishing, this client's own half");
        println!("    min     {:6.0} ms", join.min);
        println!("    median  {:6.0} ms", join.median);
        println!("    max     {:6.0} ms", join.max);
        println!(
            "\n  and {:6.0} ms waiting for the other client to subscribe",
            spread.median - join.median
        );
    }
    println!("\n  against a {BUDGET_MS:.0} ms budget (prd.md §4)");

    if spread.median <= BUDGET_MS {
        println!(
            "\nWITHIN BUDGET, with {:.0} ms to spare.",
            BUDGET_MS - spread.median
        );
    } else {
        println!(
            "\nOVER BUDGET by {:.0} ms. Suspects, in the order worth checking:\n\
             \x20 - the pull side: a subscription is not live the moment a\n\
             \x20   publisher appears, and this waits for one (DR-14).\n\
             \x20 - ICE: gathering is bounded by a quiet window, not by an\n\
             \x20   answer (DR-14), so an unreachable server costs it.\n\
             \x20 - the shell: WebView2 starts before anything here does, and\n\
             \x20   a debug build starts slower than a release one.",
            spread.median - BUDGET_MS
        );
    }
    Ok(())
}

// --- arguments -------------------------------------------------------------

fn args() -> (String, PathBuf, usize) {
    let mut base = env::var("GOODVOICE_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_owned());
    let mut exe = default_exe();
    let mut runs = DEFAULT_RUNS;

    let mut argv = env::args().skip(1);
    while let Some(flag) = argv.next() {
        let Some(value) = argv.next() else {
            break;
        };
        match flag.as_str() {
            "--base" => base = value,
            "--exe" => exe = PathBuf::from(value),
            "--runs" => runs = value.parse().unwrap_or(DEFAULT_RUNS),
            _ => {}
        }
    }
    (base, exe, runs.max(1))
}

/// The app next to this drill: both are targets of the same crate, so the one
/// that was built with it is the one in the same directory.
fn default_exe() -> PathBuf {
    let name = if cfg!(windows) {
        "goodvoice-client.exe"
    } else {
        "goodvoice-client"
    };
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// A room nobody else is in, and a different one every run.
fn fresh_room() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("cold-{}", stamp % 1_000_000_000)
}
