//! Open the viewer, close it, open it again — and see whether the second one
//! ever gets a picture.
//!
//! The headless half of plan.md task 5.4. `docs/testing/viewer.ps1` drives the
//! real window and is the thing the task's definition of done is written
//! about; this is the same question asked in twenty seconds without a person,
//! a webview or a UI Automation tree, because "does re-subscribing work" is a
//! transport question and belongs where transport questions can be iterated
//! on.
//!
//! ```text
//! cargo run -p goodvoice-harness --bin rewatch
//! cargo run -p goodvoice-harness --bin rewatch -- --rounds 4 --seconds 8
//! ```

#[cfg(not(windows))]
fn main() {
    eprintln!("this drill shares a Windows screen and has to run on Windows");
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
        env,
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc,
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
    const SHARE_TIMEOUT: Duration = Duration::from_secs(20);

    pub async fn run() -> Result<()> {
        let args = Args::parse();
        let room = fresh_room();

        println!("goodvoice re-watch drill (plan.md task 5.4)\n");
        println!("  server  {}", args.base);
        println!("  room    {room}");
        println!("  rounds  {} of {} s\n", args.rounds, args.seconds);

        let target = wgc::monitors()
            .context("listing monitors")?
            .into_iter()
            .next()
            .context("no monitors to share")?;

        let bruno = join(&args.base, &room, "bruno").await?;
        let ana = join(&args.base, &room, "ana").await?;
        ana.start_share(Arc::new(ShareFactory::new(target, Quality::P720)));
        let started = Instant::now();
        let (width, height, _) = wait_for_share(&ana).await?;
        println!(
            "  ana is sharing {width}×{height} after {:.1} s\n",
            started.elapsed().as_secs_f64()
        );

        println!("  round   units   keyframes   first picture");
        let mut rounds = Vec::new();
        for round in 1..=args.rounds {
            let counted = Arc::new(Counted::new());
            let viewer = bruno.watch_screen(Arc::clone(&counted) as Arc<dyn ScreenSink>);
            tokio::time::sleep(Duration::from_secs(args.seconds)).await;
            bruno.unwatch_screen(viewer);

            let units = counted.units.load(Ordering::Relaxed);
            let keyframes = counted.keyframes.load(Ordering::Relaxed);
            let first = counted.first_at.load(Ordering::Relaxed);
            println!(
                "  {round:>5}   {units:>5}   {keyframes:>9}   {}",
                if first == 0 {
                    "**never**".to_owned()
                } else {
                    #[allow(
                        clippy::cast_precision_loss,
                        reason = "milliseconds, bounded by the round"
                    )]
                    let seconds = first as f64 / 1000.0;
                    format!("{seconds:.2} s")
                }
            );
            rounds.push((round, units, keyframes));

            // The gap a person leaves between closing a window and wanting it
            // back.
            tokio::time::sleep(Duration::from_secs(args.gap)).await;
        }

        ana.stop_share();
        drop(ana);
        bruno.leave().await;

        let blind: Vec<u64> = rounds
            .iter()
            .filter(|(_, units, keyframes)| *units == 0 || *keyframes == 0)
            .map(|(round, _, _)| *round)
            .collect();

        println!();
        if blind.is_empty() {
            println!("every viewer got a picture.");
            return Ok(());
        }
        bail!(
            "{} of {} viewers never got a picture (rounds {})",
            blind.len(),
            args.rounds,
            blind
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    /// What one viewer received while it was open.
    struct Counted {
        units: AtomicU64,
        keyframes: AtomicU64,
        /// Milliseconds from opening to the first frame; 0 means none came.
        first_at: AtomicU64,
        opened: Instant,
    }

    impl Counted {
        fn new() -> Self {
            Self {
                units: AtomicU64::new(0),
                keyframes: AtomicU64::new(0),
                first_at: AtomicU64::new(0),
                opened: Instant::now(),
            }
        }
    }

    impl ScreenSink for Counted {
        fn accept(&self, _unit: &[u8], keyframe: bool) {
            self.units.fetch_add(1, Ordering::Relaxed);
            if keyframe {
                self.keyframes.fetch_add(1, Ordering::Relaxed);
            }
            if self.first_at.load(Ordering::Relaxed) == 0 {
                let since = u64::try_from(self.opened.elapsed().as_millis()).unwrap_or(u64::MAX);
                self.first_at.store(since.max(1), Ordering::Relaxed);
            }
        }

        fn ended(&self) {}
    }

    async fn join(base: &str, room: &str, name: &str) -> Result<Call> {
        Call::join(
            CallOptions {
                base: base.to_owned(),
                room: room.to_owned(),
                name: name.to_owned(),
                mode: TransmitMode::Open,
                prefs: Arc::new(AudioPrefs::default()),
            },
            Box::new(ToneSource::new(0.0)),
            Arc::new(NullSink) as Arc<dyn AudioSink>,
        )
        .await
        .with_context(|| format!("{name} could not join"))
    }

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

    fn fresh_room() -> String {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        format!("rewatch-{}", stamp % 1_000_000_000)
    }

    struct Args {
        base: String,
        rounds: u64,
        seconds: u64,
        gap: u64,
    }

    impl Args {
        fn parse() -> Self {
            let mut args = Self {
                base: env::var("GOODVOICE_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_owned()),
                rounds: 3,
                seconds: 6,
                gap: 3,
            };
            let mut rest = env::args().skip(1);
            while let Some(flag) = rest.next() {
                match flag.as_str() {
                    "--base" => {
                        if let Some(value) = rest.next() {
                            args.base = value;
                        }
                    }
                    "--rounds" => {
                        args.rounds = rest.next().and_then(|v| v.parse().ok()).unwrap_or(3);
                    }
                    "--seconds" => {
                        args.seconds = rest.next().and_then(|v| v.parse().ok()).unwrap_or(6);
                    }
                    "--gap" => {
                        args.gap = rest.next().and_then(|v| v.parse().ok()).unwrap_or(3);
                    }
                    _ => {}
                }
            }
            args
        }
    }
}
