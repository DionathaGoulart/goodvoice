//! One screen, captured, encoded, and handed over as H.264 packets.
//!
//! plan.md task 5.3. This is the seam between the two Windows-only halves of
//! [`crate::capture`] and the rest of the client: [`rtc`](crate::rtc) never
//! sees a texture, a COM interface or a device — it sees a
//! [`Receiver`](std::sync::mpsc::Receiver) of [`Packet`]s and a handle that
//! stops it.
//!
//! # Why a thread and not a task
//!
//! Neither [`Capturer`] nor [`H264Encoder`] is `Send`: both hold COM
//! interfaces and share one D3D11 immediate context, which is not free-threaded
//! even when the frame pool is (see [`crate::capture::wgc`]). They have to live
//! and die on one thread. So they get one — a plain OS thread, owning both,
//! sending its output down a channel. The async side of the client never
//! blocks on it and never touches what it holds.
//!
//! # What it does about frame rate
//!
//! WGC produces a frame when the content changes and nothing when it does not
//! (DR-31), so on a busy screen it will offer frames faster than
//! [`SHARE_FPS`]. This drops those rather than encoding them: the bitrate is
//! budgeted for a rate, and a viewer cannot see a frame that is replaced two
//! milliseconds later. On a still screen it encodes nothing at all, which is
//! the same reason.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use tokio::sync::mpsc as async_mpsc;

use super::{
    encoder::{EncodeConfig, H264Encoder, Packet, Quality},
    wgc::{Capturer, Cursor, Target},
    CaptureError,
};
use crate::rtc::screen::{ScreenSource, ScreenSourceFactory, VideoFrame};

/// What a share publishes at.
///
/// 30 rather than 60: prd.md §4 budgets one sharer, the FPS benchmark (task
/// 5.5) is written against 1080p30, and doubling the frame rate doubles the
/// egress for a difference nobody watching a screen share notices.
pub const SHARE_FPS: u32 = 30;

/// How many encoded packets may queue before the capture thread drops them.
///
/// A share that has got ahead of its sender is a share whose oldest frames are
/// worthless — dropping is what the viewer wants, and blocking the capture
/// thread would stall the frame pool it is draining.
const PACKET_QUEUE: usize = 8;

/// How long a still screen may go without saying anything before the last
/// keyframe is sent again.
///
/// WGC produces nothing while nothing changes (DR-31), so a share of a
/// document, an IDE or a paused game is a track that stops mid-GOP — and a
/// viewer opening onto one has nothing to start a decoder with. It would wait,
/// showing "nobody is sharing", until whoever is sharing happened to move
/// something (DR-34).
///
/// Re-sending the keyframe that is already in hand fixes that without encoding
/// anything: the picture has not changed, so the last IDR *is* the current
/// picture. What it costs is one keyframe every two seconds while the screen
/// is still, and nothing at all while it is not.
///
/// # It is load-bearing for more than the latency
///
/// DR-34 wrote this down as the thing that saves a viewer from waiting, and
/// §7.10 then proposed replacing it with a keyframe sent on request. Removing
/// it was tried and measured, and it does not merely make viewers wait: it
/// makes the share **unsubscribable**. Cloudflare refuses a `tracks/new` for a
/// track that has never carried a packet, in those words — *the publisher
/// never started sending: Track not found on remote peer* — so a share of a
/// still document that said nothing at all was one that four viewers out of
/// four could not open (DR-44).
///
/// So this is not only a repeat for latecomers. It is the heartbeat that keeps
/// the track a thing the SFU will let anybody subscribe to, and
/// `docs\testing\keyframe.ps1` is what measures both halves of that.
const STILL_KEYFRAME: Duration = Duration::from_secs(2);

/// How long the capture thread waits on a frame before checking whether it has
/// been told to stop.
///
/// It has nothing else to wake for, and a still screen produces nothing, so
/// this is also the worst case for how long [`ScreenShare::stop`] takes.
const POLL: Duration = Duration::from_millis(100);

/// A running screen share. Dropping it stops the capture.
pub struct ScreenShare {
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    started: Started,
}

/// What the capture thread found once it had opened everything.
///
/// Sent back before the first frame so [`ScreenShare::start`] can fail
/// properly rather than returning a handle to a thread that is about to die.
#[derive(Debug, Clone)]
struct Started {
    config: EncodeConfig,
    encoder: String,
    hardware: bool,
    zero_copy: bool,
}

impl ScreenShare {
    /// Start capturing `target` and encoding it at `quality`.
    ///
    /// Returns the handle and the packets. The channel closes when the share
    /// stops, whether that was asked for or was the window going away.
    ///
    /// # Errors
    ///
    /// Whatever [`Capturer::start`] or [`H264Encoder::open`] refused with, on
    /// the thread that tried it.
    pub fn start(
        target: &Target,
        quality: Quality,
    ) -> Result<(Self, async_mpsc::Receiver<Packet>), CaptureError> {
        // A tokio channel rather than a std one: the far end is an async task
        // publishing to the SFU, and the near end is a plain thread that only
        // ever `try_send`s. Neither blocks on the other.
        let (packets_tx, packets) = async_mpsc::channel(PACKET_QUEUE);
        let (ready_tx, ready) = mpsc::channel::<Result<Started, CaptureError>>();
        let stop = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(false));

        let target = target.clone();
        let thread = {
            let stop = Arc::clone(&stop);
            let keyframe = Arc::clone(&keyframe);
            thread::Builder::new()
                .name("goodvoice-share".to_owned())
                .spawn(move || run(&target, quality, &stop, &keyframe, &ready_tx, &packets_tx))
                .map_err(|error| CaptureError::Start(format!("no thread for the share: {error}")))?
        };

        // The thread reports what it opened, or why it could not. A closed
        // channel means it died before it managed either, which the join
        // below turns back into an error rather than a hang.
        match ready.recv() {
            Ok(Ok(started)) => Ok((
                Self {
                    stop,
                    keyframe,
                    thread: Some(thread),
                    started,
                },
                packets,
            )),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(CaptureError::Start(
                    "the share thread stopped before it started".into(),
                ))
            }
        }
    }

    /// What is actually being encoded, after scaling.
    #[must_use]
    pub fn config(&self) -> EncodeConfig {
        self.started.config
    }

    /// The encoder that was opened.
    #[must_use]
    pub fn encoder(&self) -> &str {
        &self.started.encoder
    }

    /// Whether that encoder is in silicon.
    ///
    /// `false` is a share the user has to be told about (prd.md §3 F3: a
    /// software fallback is allowed and must warn).
    #[must_use]
    pub fn is_hardware(&self) -> bool {
        self.started.hardware
    }

    /// Whether frames reach the encoder without leaving the GPU.
    #[must_use]
    pub fn is_zero_copy(&self) -> bool {
        self.started.zero_copy
    }

    /// Ask for a keyframe as soon as possible.
    ///
    /// What a viewer opening mid-share needs. Cheap to call and safe to call
    /// often: it sets a flag the capture thread reads once per frame.
    pub fn request_keyframe(&self) {
        self.keyframe.store(true, Ordering::Relaxed);
    }

    /// Stop capturing and wait for the thread to let go of the device.
    pub fn stop(mut self) {
        self.shut_down();
    }

    fn shut_down(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ScreenShare {
    fn drop(&mut self) {
        // Not a formality: the thread holds a capture session, and a session
        // nobody closed keeps Windows drawing a capture border around the
        // target.
        self.shut_down();
    }
}

/// The capture thread's whole life.
fn run(
    target: &Target,
    quality: Quality,
    stop: &AtomicBool,
    keyframe: &AtomicBool,
    ready: &mpsc::Sender<Result<Started, CaptureError>>,
    packets: &async_mpsc::Sender<Packet>,
) {
    let mut pipeline = match Pipeline::open(target, quality) {
        Ok(pipeline) => pipeline,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if ready.send(Ok(pipeline.started())).is_err() {
        return;
    }

    let interval = Duration::from_nanos(1_000_000_000 / u64::from(SHARE_FPS.max(1)));
    let mut last: Option<Instant> = None;
    let mut encoded = Vec::new();
    // The last picture that a decoder could start on, and when anything was
    // last put on the wire. See [`STILL_KEYFRAME`].
    let mut recent_key: Option<Packet> = None;
    let mut sent = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        let frame = match pipeline.capturer.next_frame(POLL) {
            Ok(Some(frame)) => frame,
            // Nothing changed on screen. If nothing has for a while, say the
            // last thing again so that a viewer opening now has a picture.
            Ok(None) => {
                if let Some(key) = recent_key.as_ref() {
                    if sent.elapsed() >= STILL_KEYFRAME {
                        if packets.try_send(key.clone()).is_err() {
                            // Closed, or a queue nobody is draining: the first
                            // ends the share, and the second means the wire is
                            // busy enough that a repeat is not what it needs.
                            if packets.is_closed() {
                                return;
                            }
                        }
                        sent = Instant::now();
                    }
                }
                continue;
            }
            // The window closed or the display went away. Ending the channel
            // is how the rest of the client finds out.
            Err(_) => break,
        };

        // Faster than the rate we publish at: drop it. Dropping is a frame the
        // viewer would not have seen anyway, and encoding it would spend
        // bitrate on it.
        let now = Instant::now();
        if last.is_some_and(|last| now.duration_since(last) < interval) {
            continue;
        }
        last = Some(now);

        // A window that was resized is arriving at a different size, and the
        // encoder was built around the old one. Rebuilding is the whole
        // handling: the new SPS travels in the bitstream and a decoder follows
        // it.
        let (width, height) = frame.size();
        if width != pipeline.config().source_width || height != pipeline.config().source_height {
            drop(frame);
            if pipeline.resize(quality).is_err() {
                break;
            }
            // A keyframe from before the resize describes a picture of a
            // different shape, and sending it again would hand a decoder a
            // sequence that contradicts the one it is following.
            recent_key = None;
            continue;
        }

        if keyframe.swap(false, Ordering::Relaxed) {
            pipeline.encoder.request_keyframe();
        }

        encoded.clear();
        if pipeline
            .encoder
            .encode(frame.texture(), frame.time(), &mut encoded)
            .is_err()
        {
            break;
        }
        // Held only as long as the encode: a frame not returned is one of the
        // pool's two buffers held out of circulation.
        drop(frame);

        for packet in encoded.drain(..) {
            if packet.keyframe {
                recent_key = Some(packet.clone());
            }
            sent = Instant::now();
            // A full queue means nobody is draining fast enough, and the
            // frames waiting in it are the least worth keeping — so the new
            // one is dropped rather than blocking the capture. A disconnected
            // one means there is no share any more.
            if matches!(
                packets.try_send(packet),
                Err(async_mpsc::error::TrySendError::Closed(_))
            ) {
                return;
            }
        }
    }

    // Whatever the encoder is still holding, on the way out. It is at most a
    // frame or two and it costs one call.
    encoded.clear();
    if pipeline.encoder.drain(&mut encoded).is_ok() {
        for packet in encoded.drain(..) {
            if packets.try_send(packet).is_err() {
                break;
            }
        }
    }
}

/// The capturer and the encoder, which are rebuilt together or not at all.
struct Pipeline {
    capturer: Capturer,
    encoder: H264Encoder,
}

impl Pipeline {
    fn open(target: &Target, quality: Quality) -> Result<Self, CaptureError> {
        let capturer = Capturer::start(target, Cursor::Shown)?;
        let encoder = Self::encoder_for(&capturer, quality)?;
        Ok(Self { capturer, encoder })
    }

    fn encoder_for(capturer: &Capturer, quality: Quality) -> Result<H264Encoder, CaptureError> {
        let source = capturer.size()?;
        H264Encoder::open(
            capturer.device(),
            capturer.context(),
            quality.plan(source, SHARE_FPS),
        )
    }

    fn config(&self) -> EncodeConfig {
        self.encoder.config()
    }

    /// Follow the target to its new size.
    fn resize(&mut self, quality: Quality) -> Result<(), CaptureError> {
        self.capturer.resize()?;
        self.encoder = Self::encoder_for(&self.capturer, quality)?;
        Ok(())
    }

    fn started(&self) -> Started {
        Started {
            config: self.encoder.config(),
            encoder: self.encoder.name().to_owned(),
            hardware: self.encoder.is_hardware(),
            zero_copy: self.encoder.is_zero_copy(),
        }
    }
}

// --- what the transport sees -------------------------------------------------

/// A [`ScreenShare`] behind [`ScreenSource`].
///
/// The only thing this adds is the shape: packets become
/// [`VideoFrame`]s and the D3D11 half of the client stops here.
pub struct ShareSource {
    share: ScreenShare,
    packets: async_mpsc::Receiver<Packet>,
    /// The frame period the encoder was configured for, which is what an
    /// unknown gap between two packets is worth calling.
    period: Duration,
}

#[async_trait::async_trait]
impl ScreenSource for ShareSource {
    async fn next_frame(&mut self) -> Option<VideoFrame> {
        let packet = self.packets.recv().await?;
        Some(VideoFrame {
            bytes: packet.bytes,
            duration: self.period,
            keyframe: packet.keyframe,
        })
    }

    fn size(&self) -> (u32, u32) {
        let config = self.share.config();
        (config.width, config.height)
    }

    fn request_keyframe(&self) {
        self.share.request_keyframe();
    }

    fn is_hardware(&self) -> bool {
        self.share.is_hardware()
    }
}

/// Opens a [`ShareSource`] for one target at one quality.
///
/// Held by the call for as long as the user is sharing, and asked again after
/// every reconnect — which is why it stores what was picked rather than what
/// was opened.
#[derive(Debug, Clone)]
pub struct ShareFactory {
    target: Target,
    quality: Quality,
}

impl ShareFactory {
    /// Share `target` at `quality`.
    #[must_use]
    pub const fn new(target: Target, quality: Quality) -> Self {
        Self { target, quality }
    }
}

impl ScreenSourceFactory for ShareFactory {
    fn open(&self) -> Result<Box<dyn ScreenSource>, String> {
        let (share, packets) =
            ScreenShare::start(&self.target, self.quality).map_err(|error| error.to_string())?;
        Ok(Box::new(ShareSource {
            share,
            packets,
            period: Duration::from_nanos(1_000_000_000 / u64::from(SHARE_FPS.max(1))),
        }))
    }

    fn describe(&self) -> String {
        self.target.name.clone()
    }
}
