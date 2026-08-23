//! One client shares a screen; a second client is shown to be receiving it.
//!
//! plan.md task 5.3's definition of done — *share visible to a second client*
//! — measured rather than watched. Three clients join a real room through the
//! live deploy:
//!
//! - **ana** captures a monitor, encodes it, and publishes an H.264 track.
//! - **bruno** opens a viewer and writes down every access unit that arrives.
//! - **carla** tries to share as well, and is expected to be refused. That is
//!   prd.md §8's one-sharer rule, enforced by the Durable Object and checked
//!   here from the outside (`server/test/tracks.test.ts` checks it from the
//!   inside).
//!
//! What bruno writes is a playable H.264 elementary stream. Feeding it to a
//! decoder that has never heard of goodvoice — VLC — turns "packets arrived"
//! into "ana's screen, at bruno's end", which is the claim the task actually
//! makes. `docs/perf/screenshare-encode.md` has the command.
//!
//! ```text
//! cargo run -p goodvoice-harness --bin share-drill
//! cargo run -p goodvoice-harness --bin share-drill -- --seconds 20 --1080
//! cargo run -p goodvoice-harness --bin share-drill -- --base http://localhost:8787
//! cargo run -p goodvoice-harness --bin share-drill -- --room viewtest --seconds 90
//! ```

#[cfg(not(windows))]
fn main() {
    eprintln!("the 5.3 drill shares a Windows screen and has to run on Windows");
    std::process::exit(2);
}

#[cfg(windows)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    windows_drill::run().await
}

#[cfg(windows)]
mod windows_drill {
    use std::{
        env, fs,
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            Arc, Mutex,
        },
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    use anyhow::{bail, Context as _, Result};
    use goodvoice_client_lib::{
        audio::{
            device::{AudioSink, NullSink, ToneSource},
            prefs::AudioPrefs,
            vad::TransmitMode,
        },
        capture::{encoder::Quality, share::ShareFactory, wgc},
        rtc::{
            screen::{ScreenSink, ShareState},
            session::{Call, CallOptions},
        },
    };

    const DEFAULT_BASE: &str = "https://goodvoice.goodvoice-server.workers.dev";
    const DEFAULT_SECONDS: u64 = 12;

    /// How long to wait for a share to go live before calling it a failure.
    /// Opening a capture, an encoder and a renegotiation, generously.
    const SHARE_TIMEOUT: Duration = Duration::from_secs(20);

    pub async fn run() -> Result<()> {
        let args = Args::parse();
        let room = args.room.clone().unwrap_or_else(fresh_room);

        println!("goodvoice screen-share drill (plan.md task 5.3)\n");
        println!("  server  {}", args.base);
        println!("  room    {room}");
        println!("  quality {}p", args.quality.height());
        println!();

        let target = wgc::monitors()
            .context("listing monitors")?
            .into_iter()
            .next()
            .context("no monitors to share")?;
        println!("  sharing {}\n", target.name);

        // Bruno first, and in the room before ana starts: a viewer that opens
        // after the share has to wait for a keyframe, and this drill is not
        // measuring that.
        let bruno = join(&args.base, &room, "bruno").await?;
        let watched = Arc::new(Watched::new(args.out.join("received.h264")));
        bruno.watch_screen(Arc::clone(&watched) as Arc<dyn ScreenSink>);

        let ana = join(&args.base, &room, "ana").await?;
        ana.start_share(Arc::new(ShareFactory::new(target, args.quality)));

        let started = Instant::now();
        let live = wait_for_share(&ana).await?;
        println!(
            "  ana is sharing {}×{} on {} after {:.1} s",
            live.0,
            live.1,
            if live.2 { "hardware" } else { "**software**" },
            started.elapsed().as_secs_f64()
        );

        // The second sharer, while the first one is live. The room is what
        // refuses this, not the client.
        let carla = join(&args.base, &room, "carla").await?;
        carla.start_share(Arc::new(ShareFactory::new(
            wgc::monitors()
                .context("listing monitors")?
                .into_iter()
                .next()
                .context("no monitors")?,
            Quality::P720,
        )));
        let refused = wait_for_refusal(&carla).await;

        println!("\n  watching for {} s", args.seconds);
        tokio::time::sleep(Duration::from_secs(args.seconds)).await;

        ana.stop_share();
        bruno.unwatch_screen();
        let report = watched.report()?;

        drop(carla);
        drop(ana);
        drop(bruno);

        println!();
        println!("### what reached bruno");
        println!();
        println!("- {} access units, {} bytes", report.units, report.bytes);
        println!("- {} of them keyframes", report.keyframes);
        match report.first {
            Some(first) => println!(
                "- first picture {:.2} s after the share went live",
                first.saturating_duration_since(started).as_secs_f64()
            ),
            None => println!("- **nothing arrived**"),
        }
        println!("- wrote {}", report.path.display());
        println!();
        println!("### the one-sharer rule");
        println!();
        match &refused {
            Some(detail) => println!("- carla was refused: {detail}"),
            None => println!("- **carla was not refused** — the room let a second screen through"),
        }

        if report.units == 0 {
            bail!("no video reached the second client");
        }
        if report.keyframes == 0 {
            bail!("video reached the second client with no keyframe in it");
        }
        if refused.is_none() {
            bail!("the room allowed two sharers at once");
        }
        println!("\nboth halves hold.");
        Ok(())
    }

    // --- the viewer --------------------------------------------------------

    /// A [`ScreenSink`] that keeps what it is given.
    ///
    /// Writing the units to a file is the point: it makes the claim checkable
    /// by something that is not this program.
    struct Watched {
        path: PathBuf,
        units: AtomicU64,
        keyframes: AtomicU64,
        bytes: AtomicU64,
        first: Mutex<Option<Instant>>,
        stream: Mutex<Vec<u8>>,
        ended: AtomicBool,
    }

    impl Watched {
        fn new(path: PathBuf) -> Self {
            Self {
                path,
                units: AtomicU64::new(0),
                keyframes: AtomicU64::new(0),
                bytes: AtomicU64::new(0),
                first: Mutex::new(None),
                stream: Mutex::new(Vec::new()),
                ended: AtomicBool::new(false),
            }
        }

        fn report(&self) -> Result<Report> {
            if let Some(parent) = self.path.parent() {
                fs::create_dir_all(parent).context("creating the output directory")?;
            }
            let stream = self
                .stream
                .lock()
                .map_err(|_| anyhow::anyhow!("poisoned"))?;
            fs::write(&self.path, stream.as_slice()).context("writing the received stream")?;
            Ok(Report {
                units: self.units.load(Ordering::Relaxed),
                keyframes: self.keyframes.load(Ordering::Relaxed),
                bytes: self.bytes.load(Ordering::Relaxed),
                first: *self.first.lock().map_err(|_| anyhow::anyhow!("poisoned"))?,
                path: self.path.clone(),
            })
        }
    }

    impl ScreenSink for Watched {
        fn accept(&self, unit: &[u8], keyframe: bool) {
            self.units.fetch_add(1, Ordering::Relaxed);
            self.bytes.fetch_add(unit.len() as u64, Ordering::Relaxed);
            if keyframe {
                self.keyframes.fetch_add(1, Ordering::Relaxed);
            }
            if let Ok(mut first) = self.first.lock() {
                if first.is_none() && keyframe {
                    *first = Some(Instant::now());
                }
            }
            // Everything before the first keyframe is undecodable on its own,
            // so it would only make the file harder to play.
            if self.first.lock().is_ok_and(|first| first.is_some()) {
                if let Ok(mut stream) = self.stream.lock() {
                    stream.extend_from_slice(unit);
                }
            }
        }

        fn ended(&self) {
            self.ended.store(true, Ordering::Relaxed);
        }
    }

    struct Report {
        units: u64,
        keyframes: u64,
        bytes: u64,
        first: Option<Instant>,
        path: PathBuf,
    }

    // --- the room ----------------------------------------------------------

    async fn join(base: &str, room: &str, name: &str) -> Result<Call> {
        Call::join(
            CallOptions {
                base: base.to_owned(),
                room: room.to_owned(),
                name: name.to_owned(),
                mode: TransmitMode::Open,
                prefs: Arc::new(AudioPrefs::default()),
            },
            // Silence, on both counts: this drill is about video, and a room
            // full of tones would only make the audio path's meters lie.
            Box::new(ToneSource::new(0.0)),
            Arc::new(NullSink) as Arc<dyn AudioSink>,
        )
        .await
        .with_context(|| format!("{name} could not join"))
    }

    /// Waits for a share to go live, and reports what it went live as.
    async fn wait_for_share(call: &Call) -> Result<(u32, u32, bool)> {
        let mut share = call.share();
        let deadline = Instant::now() + SHARE_TIMEOUT;
        loop {
            match &*share.borrow_and_update() {
                ShareState::Sharing {
                    width,
                    height,
                    hardware,
                    ..
                } => return Ok((*width, *height, *hardware)),
                ShareState::Failed { detail } => bail!("the share failed: {detail}"),
                ShareState::Idle => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("the share never went live");
            }
            if tokio::time::timeout(left, share.changed()).await.is_err() {
                bail!("the share never went live");
            }
        }
    }

    /// Waits for a second sharer to be turned away, and reports why.
    async fn wait_for_refusal(call: &Call) -> Option<String> {
        let mut share = call.share();
        let deadline = Instant::now() + SHARE_TIMEOUT;
        loop {
            if let ShareState::Failed { detail } = &*share.borrow_and_update() {
                return Some(detail.clone());
            }
            if call.is_sharing() {
                return None;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            if tokio::time::timeout(left, share.changed()).await.is_err() {
                return None;
            }
        }
    }

    /// A room nobody else is in, and a different one every run (DR-5).
    fn fresh_room() -> String {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        format!("share-{}", stamp % 1_000_000_000)
    }

    // --- arguments ---------------------------------------------------------

    struct Args {
        base: String,
        /// A room to join instead of a fresh one.
        ///
        /// For the one case a fresh room cannot serve: task 5.4's viewer, where
        /// a person has to be able to point the app at the same room this
        /// drill is sharing into.
        room: Option<String>,
        seconds: u64,
        quality: Quality,
        out: PathBuf,
    }

    impl Args {
        fn parse() -> Self {
            let mut args = Self {
                base: env::var("GOODVOICE_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_owned()),
                room: None,
                seconds: DEFAULT_SECONDS,
                quality: Quality::P720,
                out: env::temp_dir().join("goodvoice-share"),
            };
            let mut rest = env::args().skip(1);
            while let Some(flag) = rest.next() {
                match flag.as_str() {
                    "--720" => args.quality = Quality::P720,
                    "--1080" => args.quality = Quality::P1080,
                    "--base" => {
                        if let Some(value) = rest.next() {
                            args.base = value;
                        }
                    }
                    "--room" => args.room = rest.next(),
                    "--seconds" => {
                        args.seconds = rest
                            .next()
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(DEFAULT_SECONDS);
                    }
                    "--out" => {
                        if let Some(value) = rest.next() {
                            args.out = PathBuf::from(value);
                        }
                    }
                    _ => {}
                }
            }
            args.seconds = args.seconds.max(1);
            args
        }
    }
}
