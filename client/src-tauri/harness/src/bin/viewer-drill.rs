//! The other end of task 5.4's viewer: somebody sharing, and somebody
//! listening to whether the voice survives it.
//!
//! Task 5.4's definition of done is *open and close the viewer repeatedly
//! during a live share, and hear no glitch in the voice*. Two of those three
//! things are not the app under test:
//!
//! - **the share.** A viewer window needs a screen to show, from a client that
//!   is not the one being driven. This joins the room and publishes one.
//! - **the ear.** "Voice never glitches" measured at the far end is what DR-26
//!   established: a 20 ms frame path delivers fifty frames a second, so the
//!   claim stops being a judgement — it is 50 a second across every open and
//!   every close, or it is not.
//!
//! The third thing, the opening and closing, belongs to something that can
//! click: `docs/testing/viewer.ps1` drives the shipping client through UI
//! Automation while this runs.
//!
//! ```text
//! cargo run -p goodvoice-harness --bin viewer-drill -- --room view42 --seconds 90
//! cargo run -p goodvoice-harness --bin viewer-drill -- --room view42 --1080
//! ```
//!
//! # What it asserts
//!
//! From the second the app is first heard until the second it leaves the
//! room, every one-second window must carry at least `--floor` frames. One
//! that does not is printed with a `<-- DIP` and fails the run. Silence before
//! the app arrives is not a dip: there is nobody to hear yet.

#[cfg(not(windows))]
fn main() {
    eprintln!("the 5.4 drill shares a Windows screen and has to run on Windows");
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
        sync::Arc,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    use anyhow::{bail, Context as _, Result};
    use goodvoice_client_lib::{
        audio::{
            device::{AudioSink, RecordingSink, ToneSource},
            mixer::peak,
            prefs::AudioPrefs,
            vad::TransmitMode,
        },
        capture::{encoder::Quality, share::ShareFactory, wgc},
        rtc::{
            screen::ShareState,
            session::{Call, CallOptions},
        },
    };

    const DEFAULT_BASE: &str = "https://goodvoice.goodvoice-server.workers.dev";
    const DEFAULT_SECONDS: u64 = 90;

    /// A 20 ms frame path delivers fifty a second. Five below that is tick
    /// jitter — this drill's clock and the sender's are not the same clock —
    /// and anything below *this* is the voice path actually stopping.
    const DEFAULT_FLOOR: usize = 45;

    /// How long to wait for a share to go live before calling it a failure.
    /// Opening a capture, an encoder and a renegotiation, generously.
    const SHARE_TIMEOUT: Duration = Duration::from_secs(20);

    pub async fn run() -> Result<()> {
        let args = Args::parse();
        let room = args.room.clone().unwrap_or_else(fresh_room);

        println!("goodvoice viewer drill (plan.md task 5.4)\n");
        println!("  server  {}", args.base);
        println!("  room    {room}");
        println!("  quality {}p", args.quality.height());
        println!("  floor   {} frames a second", args.floor);
        println!();

        let target = wgc::monitors()
            .context("listing monitors")?
            .into_iter()
            .next()
            .context("no monitors to share")?;

        // Silence out, everything in: this client is the room's screen and the
        // room's ear, and a tone from here would only give the app's echo
        // canceller something to do.
        let ears = RecordingSink::new();
        let call = Call::join(
            CallOptions {
                base: args.base.clone(),
                room: room.clone(),
                name: "sharer".to_owned(),
                mode: TransmitMode::Open,
                prefs: Arc::new(AudioPrefs::default()),
            },
            Box::new(ToneSource::new(0.0)),
            Arc::clone(&ears) as Arc<dyn AudioSink>,
        )
        .await
        .context("the sharer could not join")?;

        println!("  sharing {}", target.name);
        let started = Instant::now();
        call.start_share(Arc::new(ShareFactory::new(target, args.quality)));
        let (width, height, hardware) = wait_for_share(&call).await?;
        println!(
            "  live at {width}×{height} on {} after {:.1} s\n",
            if hardware { "hardware" } else { "**software**" },
            started.elapsed().as_secs_f64()
        );
        println!("  join it with:  set GOODVOICE_AUTOJOIN={room}\n");

        let heard = measure(&call, &ears, args.seconds, args.floor).await;
        call.stop_share();
        call.leave().await;

        report(&heard, args.floor)
    }

    /// One second at a time: what arrived, from whom, and whether it held.
    async fn measure(call: &Call, ears: &RecordingSink, seconds: u64, floor: usize) -> Heard {
        println!("  time   frames/s   peak   room");

        let roster = call.roster();
        let mut watch = Watch::new(floor);
        let mut previous = 0_usize;
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        let clock = Instant::now();

        while clock.elapsed() < Duration::from_secs(seconds) {
            ticker.tick().await;
            let record = ears.loudest();
            let arrived = record.frames.saturating_sub(previous);
            previous = record.frames;

            // Who else is in the room, by name, so a run that measured nothing
            // says whether the app was ever there to be measured.
            let others: Vec<String> = roster
                .borrow()
                .iter()
                .filter(|peer| peer.name != "sharer")
                .map(|peer| peer.name.clone())
                .collect();

            let verdict = watch.tick(arrived, !others.is_empty());
            println!(
                "  {:>4}s   {arrived:>8}   {:>4}   {}{verdict}",
                clock.elapsed().as_secs(),
                peak(&record.last),
                if others.is_empty() {
                    "nobody else".to_owned()
                } else {
                    others.join(", ")
                },
            );
        }

        watch.finish()
    }

    /// The summary, and the pass or fail the task turns on.
    fn report(heard: &Heard, floor: usize) -> Result<()> {
        println!();
        println!("### what the far end heard");
        println!();
        match heard.first {
            Some(second) => println!("- the app was first heard at {second} s"),
            None => println!("- **the app was never heard at all**"),
        }
        println!(
            "- {} seconds measured, {} of them below {floor} frames",
            heard.measured, heard.dips
        );
        if !heard.dip_seconds.is_empty() {
            println!(
                "- dips at {}",
                heard
                    .dip_seconds
                    .iter()
                    .map(|second| format!("{second} s"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        println!("- lowest second: {} frames", heard.lowest);

        if heard.first.is_none() {
            bail!("no voice reached the far end — was the app in the room?");
        }
        if heard.dips > 0 {
            bail!("the voice path dipped while the viewer was being opened and closed");
        }
        println!("\nthe voice held throughout.");
        Ok(())
    }

    // --- the assertion -----------------------------------------------------

    /// The one-second windows, and which of them are allowed to be quiet.
    ///
    /// Quiet before the app is heard is the room waiting for it. Quiet after
    /// the app has left the roster is the app being closed by the script that
    /// opened it. Everything in between is the claim under test.
    struct Watch {
        floor: usize,
        second: u64,
        first_heard: Option<u64>,
        present: bool,
        gone: bool,
        measured: u64,
        dips: u64,
        dip_seconds: Vec<u64>,
        lowest: usize,
    }

    impl Watch {
        fn new(floor: usize) -> Self {
            Self {
                floor,
                second: 0,
                first_heard: None,
                present: false,
                gone: false,
                measured: 0,
                dips: 0,
                dip_seconds: Vec::new(),
                lowest: usize::MAX,
            }
        }

        /// Records one second, and returns what to print beside it.
        fn tick(&mut self, frames: usize, anyone_else: bool) -> &'static str {
            self.second += 1;
            if anyone_else {
                self.present = true;
            } else if self.present {
                // Seen and then not seen: the driver closed the app, and the
                // measurement window is over.
                self.gone = true;
            }

            if self.first_heard.is_none() {
                if frames >= self.floor {
                    self.first_heard = Some(self.second);
                    self.measured += 1;
                    self.lowest = frames;
                }
                return "";
            }
            if self.gone {
                return "   (the app has left)";
            }

            self.measured += 1;
            self.lowest = self.lowest.min(frames);
            if frames < self.floor {
                self.dips += 1;
                self.dip_seconds.push(self.second);
                return "   <-- DIP";
            }
            ""
        }

        fn finish(self) -> Heard {
            Heard {
                first: self.first_heard,
                measured: self.measured,
                dips: self.dips,
                dip_seconds: self.dip_seconds,
                lowest: if self.lowest == usize::MAX {
                    0
                } else {
                    self.lowest
                },
            }
        }
    }

    struct Heard {
        first: Option<u64>,
        measured: u64,
        dips: u64,
        dip_seconds: Vec<u64>,
        lowest: usize,
    }

    // --- the room ----------------------------------------------------------

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

    /// A room nobody else is in, and a different one every run (DR-5).
    fn fresh_room() -> String {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        format!("view-{}", stamp % 1_000_000_000)
    }

    // --- arguments ---------------------------------------------------------

    struct Args {
        base: String,
        /// The room the app will be pointed at with `GOODVOICE_AUTOJOIN`.
        room: Option<String>,
        seconds: u64,
        quality: Quality,
        floor: usize,
    }

    impl Args {
        fn parse() -> Self {
            let mut args = Self {
                base: env::var("GOODVOICE_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_owned()),
                room: None,
                seconds: DEFAULT_SECONDS,
                quality: Quality::P720,
                floor: DEFAULT_FLOOR,
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
                    "--floor" => {
                        args.floor = rest
                            .next()
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(DEFAULT_FLOOR);
                    }
                    _ => {}
                }
            }
            args.seconds = args.seconds.max(1);
            args
        }
    }
}
