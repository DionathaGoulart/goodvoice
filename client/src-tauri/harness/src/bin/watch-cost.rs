//! What a closed viewer still costs the room.
//!
//! plan.md §7.9. Closing the viewer window stops this client *reading* the
//! screen track — `rtc::session::reconcile_watch` aborts the playback task and
//! that is the whole of it. Nothing is said to Cloudflare, so whether the
//! video is still being sent is a question no instrument above the socket can
//! answer: the sink counts zero access units because nobody is draining the
//! track, which is true whether the packets stopped or are being thrown away.
//! Windows' per-process IO counters cannot see it either — 5.2 / 5.4 / 5.6
//! kB/s across never-opened, open and closed-again is the whole process, and a
//! 720p share disappears into that.
//!
//! So this counts inside webrtc instead, through `Call::wire` (`rtc::wire`):
//! the transport's own datagram counters, which see every packet that arrives
//! before anything decides what it is, and the per-SSRC inbound counters,
//! which see every RTP packet the endpoint can name a track for whether or not
//! a receiver is draining it.
//!
//! Three phases, in the order the question is asked:
//!
//! 1. **never opened** — the sharer is live, no viewer has ever been open.
//!    This is the floor: audio, STUN and RTCP, and no video at all.
//! 2. **open** — a viewer is watching. Audio plus the share.
//! 3. **closed** — the viewer is gone. If this reads like phase 1 the video
//!    stops on its own; if it reads like phase 2 the room is paying for a
//!    picture nobody is looking at, and `tracks/close` is the fix.
//!
//! ```text
//! cargo run -p goodvoice-harness --bin watch-cost
//! cargo run -p goodvoice-harness --bin watch-cost -- --seconds 20
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

    /// How much of phase 2's video has to survive into phase 3 before the
    /// verdict is "still arriving".
    ///
    /// Not a tolerance on a measurement — the two answers are far apart. A
    /// 720p share is tens of kB/s and the floor it would fall to is a couple;
    /// anything in between is a third finding and worth reading rather than
    /// rounding to one of the two.
    const STILL_ARRIVING: f64 = 0.5;
    /// And how little of it has to survive before the verdict is "it stopped".
    const STOPPED: f64 = 0.1;

    pub async fn run() -> Result<()> {
        let args = Args::parse();
        let room = fresh_room();

        println!("goodvoice closed-viewer cost (plan.md §7.9)\n");
        println!("  server  {}", args.base);
        println!("  room    {room}");
        println!("  phases  three of {} s\n", args.seconds);

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
            "  ana is sharing {width}×{height} after {:.1} s",
            started.elapsed().as_secs_f64()
        );

        // A share that has just gone live is still ramping: the first seconds
        // carry the opening keyframes and whatever the encoder makes of a
        // desktop it has never seen. Measuring the floor through that would
        // put video in the phase that is meant to have none.
        tokio::time::sleep(Duration::from_secs(args.settle)).await;
        println!("  settled for {} s\n", args.settle);

        let never = measure(&bruno, args.seconds, "never opened").await?;

        let counted = Arc::new(Counted::default());
        let viewer = bruno.watch_screen(Arc::clone(&counted) as Arc<dyn ScreenSink>);
        let open = measure(&bruno, args.seconds, "open").await?;

        bruno.unwatch_screen(viewer);
        // The close is a local abort, and the last packets already in flight
        // are not the question. Phase 3 starts after they have landed.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let closed = measure(&bruno, args.seconds, "closed").await?;

        let seen = counted.units.load(Ordering::Relaxed);
        ana.stop_share();
        drop(ana);
        bruno.leave().await;

        println!();
        report(&[&never, &open, &closed]);
        println!("\n  the viewer saw {seen} access units while it was open");

        verdict(&never, &open, &closed, seen)
    }

    /// One phase: two snapshots and the seconds between them.
    struct Phase {
        name: &'static str,
        seconds: f64,
        /// Every datagram in, headers included.
        transport_bytes: u64,
        /// Every datagram out. Closing a pull is a renegotiation, and a
        /// renegotiation is what killed the microphone's sender in DR-8: this
        /// column is where that would show, as a phase 3 that stops sending.
        sent_bytes: u64,
        /// The video track's payload, counted whether or not anybody read it.
        video_bytes: u64,
        /// The microphones', for scale — this is what the floor is made of.
        audio_bytes: u64,
    }

    impl Phase {
        fn transport_rate(&self) -> f64 {
            rate(self.transport_bytes, self.seconds)
        }

        fn sent_rate(&self) -> f64 {
            rate(self.sent_bytes, self.seconds)
        }

        fn video_rate(&self) -> f64 {
            rate(self.video_bytes, self.seconds)
        }

        fn audio_rate(&self) -> f64 {
            rate(self.audio_bytes, self.seconds)
        }
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "bytes in one phase of a drill, nowhere near 2^53"
    )]
    fn rate(bytes: u64, seconds: f64) -> f64 {
        if seconds <= 0.0 {
            return 0.0;
        }
        bytes as f64 / 1000.0 / seconds
    }

    /// Watches the wire for `seconds` and reports what moved.
    async fn measure(call: &Call, seconds: u64, name: &'static str) -> Result<Phase> {
        let before = call
            .wire()
            .await
            .with_context(|| format!("no transport to count during \"{name}\""))?;
        let clock = Instant::now();

        tokio::time::sleep(Duration::from_secs(seconds)).await;

        let after = call
            .wire()
            .await
            .with_context(|| format!("the transport went away during \"{name}\""))?;
        let elapsed = clock.elapsed().as_secs_f64();

        // A reconnect mid-phase rebuilds the peer connection and restarts its
        // counters, and `since` saturates to zero rather than wrapping. A
        // phase that measured nothing at all on a live call is that, and it is
        // not a bandwidth.
        let moved = after.transport.since(before.transport);
        if moved.packets_received == 0 {
            bail!("\"{name}\" carried no packets at all — the session was rebuilt under it");
        }

        println!(
            "  {name:<12}  {:>7.1} kB/s in    {:>7.1} kB/s video    {} streams",
            rate(moved.bytes_received, elapsed),
            rate(
                after.video_bytes().saturating_sub(before.video_bytes()),
                elapsed
            ),
            after.inbound.len()
        );

        Ok(Phase {
            name,
            seconds: elapsed,
            transport_bytes: moved.bytes_received,
            sent_bytes: moved.bytes_sent,
            video_bytes: after.video_bytes().saturating_sub(before.video_bytes()),
            audio_bytes: after.audio_bytes().saturating_sub(before.audio_bytes()),
        })
    }

    fn report(phases: &[&Phase]) {
        println!("  phase             in    video    audio      out");
        for phase in phases {
            println!(
                "  {:<12}  {:>7.1}  {:>7.1}  {:>7.1}  {:>7.1}",
                phase.name,
                phase.transport_rate(),
                phase.video_rate(),
                phase.audio_rate(),
                phase.sent_rate(),
            );
        }
        println!("  (kB/s. `in` and `out` are every datagram; video and audio are RTP payload)");
    }

    /// Says which of the two answers the numbers are, or refuses to.
    fn verdict(never: &Phase, open: &Phase, closed: &Phase, seen: u64) -> Result<()> {
        println!();

        // Nothing to conclude about a close if the open never carried a
        // picture: that is DR-33's failure, not this one's question.
        if seen == 0 || open.video_rate() < never.video_rate() + 1.0 {
            bail!(
                "the open viewer received {seen} access units and {:.1} kB/s of video — \
                 there was nothing to stop, so this run says nothing about closing one",
                open.video_rate()
            );
        }

        let surviving = closed.video_rate() / open.video_rate();
        println!(
            "  closing the viewer left {:.0}% of the video still arriving \
             ({:.1} of {:.1} kB/s).",
            surviving * 100.0,
            closed.video_rate(),
            open.video_rate()
        );

        if surviving >= STILL_ARRIVING {
            println!(
                "  Cloudflare is still sending it — nothing asked them to stop. \
                 `close_pull` in rtc::session is what should have, and either it did not \
                 run or the SFU refused it; its failures print above this table."
            );
        } else if surviving <= STOPPED {
            println!(
                "  Nothing is being sent for a viewer that is gone. \
                 prd.md §3 F3's opt-in holds on both sides of a close."
            );
        } else {
            println!(
                "  Neither answer: it fell but did not stop. Read the phases above rather \
                 than this line, and write it up before changing anything."
            );
        }

        // Closing a pull renegotiates, and DR-8 is the session where a
        // renegotiation rebuilt the microphone's sender underneath the publish
        // loop and the room went quiet. A phase 3 that receives nothing would
        // have been caught above; one that *sends* nothing would not.
        if closed.sent_rate() < never.sent_rate() / 2.0 {
            bail!(
                "this client stopped sending after the close: {:.1} kB/s out, against \
                 {:.1} before any viewer was opened. The renegotiation took the \
                 microphone with it (DR-8).",
                closed.sent_rate(),
                never.sent_rate()
            );
        }
        println!(
            "  Its own microphone survived the renegotiation: {:.1} kB/s out after the \
             close, {:.1} before any viewer opened.",
            closed.sent_rate(),
            never.sent_rate()
        );

        Ok(())
    }

    /// What the viewer received, so a run that measured nothing knows it.
    #[derive(Default)]
    struct Counted {
        units: AtomicU64,
    }

    impl ScreenSink for Counted {
        fn accept(&self, _unit: &[u8], _keyframe: bool) {
            self.units.fetch_add(1, Ordering::Relaxed);
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
            // A tone rather than silence: the audio floor is what phase 1 is
            // made of, and a muted room would put the share against nothing.
            Box::new(ToneSource::new(0.2)),
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
        format!("watchcost-{}", stamp % 1_000_000_000)
    }

    struct Args {
        base: String,
        seconds: u64,
        settle: u64,
    }

    impl Args {
        fn parse() -> Self {
            let mut args = Self {
                base: env::var("GOODVOICE_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_owned()),
                seconds: 12,
                settle: 5,
            };
            let mut rest = env::args().skip(1);
            while let Some(flag) = rest.next() {
                match flag.as_str() {
                    "--base" => {
                        if let Some(value) = rest.next() {
                            args.base = value;
                        }
                    }
                    "--seconds" => {
                        args.seconds = rest.next().and_then(|v| v.parse().ok()).unwrap_or(12);
                    }
                    "--settle" => {
                        args.settle = rest.next().and_then(|v| v.parse().ok()).unwrap_or(5);
                    }
                    _ => {}
                }
            }
            args
        }
    }
}
