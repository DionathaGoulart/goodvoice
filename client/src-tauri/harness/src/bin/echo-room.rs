//! Does a room hear itself on loudspeakers?
//!
//! plan.md §7.6, which tasks 3.4 and 4.7 both left owing. The echo canceller
//! is measured against a synthetic loopback — a reference stream fed straight
//! back in, cancelled by 31.8 dB — and that is the *hard* case rather than the
//! real one. What nobody had done is put a loudspeaker and a microphone in one
//! room, talk into it, and see whether the far end hears itself.
//!
//! ```text
//! cargo run -p goodvoice-harness --bin echo-room
//! cargo run -p goodvoice-harness --bin echo-room -- --seconds 20 --record C:\tmp\heard
//! ```
//!
//! # What is in the room
//!
//! Two clients, both in this one process, exactly as `bin/latency` does it.
//!
//! - **the room** — the real microphone and the real speakers, opened through
//!   the same `hardware::open` the app uses, so the path measured is the app's.
//! - **the far end** — a tone and a tape recorder. It publishes a steady tone
//!   that comes out of the room's loudspeaker, and it keeps every frame the
//!   room sends back.
//!
//! Nothing here is synthetic: the tone makes the trip through the SFU, out of
//! a physical transducer, across the air, into a physical microphone, and back
//! through the SFU. The `echo` column is how much of it survived that.
//!
//! # The control, and why it is the whole point
//!
//! A canceller that is working and a room with no loudspeaker in it produce
//! *the same number*. Every earlier row in §7 that looked like it had passed
//! and had not (§7.5's silent capture device, most recently) failed this way,
//! so the walk starts with the canceller switched **off**:
//!
//! | segment | tone | canceller | suppressor | what it measures |
//! |---|---|---|---|---|
//! | `quiet` | off | off | off | the room's own energy at the tone's frequency |
//! | `suppressed` | off | off | **on** | what the suppressor takes out of that |
//! | `echo off` | **on** | off | off | **the coupling** — can the microphone hear the loudspeaker at all |
//! | `echo on` | **on** | **on** | off | the cancellation |
//!
//! `echo off` against `quiet` is the coupling. Under [`COUPLING_FLOOR_DB`] the
//! run says so and refuses to report a cancellation, because there is nothing
//! in the room to cancel — that is a fact about the room, not about the app,
//! and reporting it as a clean call would be the lie this drill exists to
//! prevent.
//!
//! # The other witness
//!
//! The far end can only say how much came back. Only the near side knows
//! whether WebRTC ever *believed* there was an echo, and it says so through
//! `Microphone::echo_likelihood`. The two agreeing is a measurement; the two
//! disagreeing is worth a Decision Record.
//!
//! # What still wants ears
//!
//! `--record <dir>` writes what the far end heard, one WAV per segment. The
//! numbers say how much came back; whether it *sounds* like an echo, and
//! whether the suppressor is an improvement or a way to lose consonants
//! (task 4.7), is what a person listening to those four files answers.

use std::{
    env,
    fs::{self, File},
    io::{BufWriter, Write as _},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use anyhow::{bail, Context as _, Result};
use goodvoice_client_lib::{
    audio::{
        device::{AudioSink, AudioSource},
        hardware::{self, Microphone},
        mixer::peak,
        opus::{silent_frame, Frame, FRAME_MS, SAMPLE_RATE_HZ},
        prefs::{AudioPrefs, AudioSettings},
        tone::{bin_energy, db_below, db_over, frame as tone_frame, AMPLITUDE, DEFAULT_HZ},
        vad::TransmitMode,
    },
    rtc::session::{Call, CallOptions},
};

const DEFAULT_BASE: &str = "https://goodvoice.goodvoice-server.workers.dev";
const DEFAULT_ROOM: &str = "echo-room";

/// How long each segment is measured for, and how long it is left alone first.
///
/// AEC3 converges on a second or two of real playback, and the gain controller
/// takes about as long to settle after either switch moves — so the settle is
/// thrown away rather than measured.
const DEFAULT_SEGMENT_SECONDS: u64 = 12;
const SETTLE: Duration = Duration::from_secs(3);

/// How far the tone has to stand out of the room before there is an echo path
/// worth talking about.
///
/// Six decibels is four times the power. Anything less and the tone is not
/// distinguishable from what the room was doing anyway, which is what a
/// loudspeaker nobody can hear looks like from here.
const COUPLING_FLOOR_DB: f32 = 6.0;

/// Where the room's own noise is sampled, as multiples of the tone's frequency.
///
/// Measuring the tone's bin against the room's *silent* level is the obvious
/// thing and it does not survive contact with a room: between one segment and
/// the next somebody moves a chair, and the gain controller — which sits after
/// both switches and is not one of them — moves everything else. Two bins on
/// either side of the tone, read out of the same frame, move with all of that
/// and hold none of the tone. The ratio is what the tone is worth over the room
/// *at that instant*, and nothing outside the frame can shift it.
///
/// Far enough out to be clear of the tone: a 20 ms frame resolves 50 Hz, and
/// these are 240 Hz below and 300 Hz above a 1200 Hz tone.
const NEIGHBOURS: [f32; 2] = [0.8, 1.25];

/// How many consecutive frames are read as one window.
///
/// A tone adds to itself across frames and a room does not, so ten of them buy
/// about ten decibels over reading one — which is the difference between
/// seeing a quiet echo and reporting an empty room. The ceiling on this is the
/// two device clocks: they are both nominally 48 kHz and not the same crystal,
/// and once their drift reaches half a cycle of the tone the window stops
/// adding up. At 200 ms and a hundred parts per million that is a fortieth of
/// a cycle, with three orders of magnitude to spare.
const BLOCK_FRAMES: usize = 10;

/// How close to the quiet room's own standout the cancelled tone has to come
/// before this stops claiming a number and starts claiming a floor. Below this
/// the echo is under the room's noise and all that can honestly be said is
/// "at least this much".
const FLOOR_MARGIN_DB: f32 = 1.5;

/// How long to wait for the far end to hear the room at all before giving up.
/// The join is fast; the SFU subscription behind it took nine seconds in the
/// runs §7.2 recorded.
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(45);

fn main() -> Result<()> {
    let options = args();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start a runtime")?
        .block_on(run(options))
}

async fn run(options: Options) -> Result<()> {
    println!("goodvoice echo-room (plan.md §7.6)\n");
    println!("  server  {}", options.base);
    println!("  room    {}", options.room);
    println!("  tone    {:.0} Hz", options.hz);
    println!(
        "  segment {} s, after {} s of settling",
        options.seconds,
        SETTLE.as_secs()
    );

    // Both switches start off: the walk turns them on one at a time and the
    // first segment is the room with nothing done to it.
    let prefs = Arc::new(AudioPrefs::new(AudioSettings {
        noise_suppression: false,
        echo_cancellation: false,
        ..AudioSettings::default()
    }));
    let (capture, render) = hardware::describe().context("no usable audio endpoint")?;
    println!("\n  capture {}", endpoint(&capture));
    println!("  render  {}", endpoint(&render));
    let (microphone, speakers) =
        hardware::open(Arc::clone(&prefs)).context("no usable audio device")?;

    let canceller = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
    let room = Call::join(
        options.call("the room"),
        Box::new(Tap::new(microphone, Arc::clone(&canceller))),
        Arc::new(speakers) as Arc<dyn AudioSink>,
    )
    .await
    .context("the room could not join")?;

    let playing = Arc::new(AtomicBool::new(false));
    let ears = Arc::new(Ears::default());
    let far = Call::join(
        options.call("the far end"),
        Box::new(Loudspeaker::new(options.hz, Arc::clone(&playing))),
        Arc::clone(&ears) as Arc<dyn AudioSink>,
    )
    .await
    .context("the far end could not join")?;

    let reference = bin_energy(&tone_frame(options.hz), options.hz);
    let walk = match subscribe(&ears).await {
        Ok(()) => Ok(measure(&options, &prefs, &playing, &ears, &canceller, reference).await),
        Err(error) => Err(error),
    };

    // Both clients leave whatever happened, so a room the drill gave up on
    // does not hold two of its eight seats until the heartbeat sweeps them.
    room.leave().await;
    far.leave().await;

    let walk = walk?;
    if let Some(directory) = options.record.as_deref() {
        record(directory, &walk)?;
    }
    report(&walk, reference, options.hz)
}

/// Waits until the far end is actually hearing the room.
///
/// Joining a room is not the same as being subscribed to what is in it
/// (DR-14), and a segment measured before the subscription is live is a
/// measurement of that gap instead.
async fn subscribe(ears: &Ears) -> Result<()> {
    print!("\nwaiting for the far end to hear the room");
    std::io::stdout().flush().ok();
    let deadline = tokio::time::Instant::now() + SUBSCRIBE_TIMEOUT;
    while ears.heard() == 0 {
        if tokio::time::Instant::now() > deadline {
            println!();
            bail!(
                "the far end never heard the room, so nothing below would be about the loudspeaker.\n  \
                 The room publishes every frame it captures, so this is the SFU or the network — not the microphone."
            );
        }
        print!(".");
        std::io::stdout().flush().ok();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    println!(" heard.\n");
    Ok(())
}

/// The four segments, in the order that makes each one's control adjacent to
/// it: the two suppressor segments next to each other, and the coupling
/// measured just before the cancellation it qualifies.
const WALK: [Segment; 4] = [
    Segment {
        name: "quiet",
        what: "the room, loudspeaker silent",
        tone: false,
        echo: false,
        noise: false,
    },
    Segment {
        name: "suppressed",
        what: "the room again, suppressor on",
        tone: false,
        echo: false,
        noise: true,
    },
    Segment {
        name: "echo off",
        what: "the loudspeaker playing, canceller off",
        tone: true,
        echo: false,
        noise: false,
    },
    Segment {
        name: "echo on",
        what: "the loudspeaker playing, canceller on",
        tone: true,
        echo: true,
        noise: false,
    },
];

struct Segment {
    name: &'static str,
    what: &'static str,
    tone: bool,
    echo: bool,
    noise: bool,
}

/// What one segment sounded like at the far end.
struct Heard {
    name: &'static str,
    frames: Vec<Frame>,
    /// Per-frame energy at the tone's frequency, sorted, for a median that a
    /// single door slam cannot move.
    energies: Vec<f32>,
    /// Decibels of the tone's bin over [`NEIGHBOURS`], one per window of
    /// [`BLOCK_FRAMES`], sorted. The measurement everything below is decided
    /// on.
    standouts: Vec<f32>,
    levels: Vec<f32>,
    /// What the canceller believed while this segment was being measured.
    likelihood: Option<f64>,
}

impl Heard {
    fn median_energy(&self) -> f32 {
        median(&self.energies)
    }

    fn median_level(&self) -> f32 {
        median(&self.levels)
    }

    /// How far the tone stood out of this room, in decibels, at the median
    /// frame. Zero means it did not.
    fn standout(&self) -> f32 {
        median(&self.standouts)
    }
}

/// How far above its neighbouring frequencies the tone sits in one window.
///
/// Both bins hold whatever the room is doing across the band and neither holds
/// the tone, so their mean is the noise under it. See [`NEIGHBOURS`].
fn standout(window: &[i16], hz: f32) -> f32 {
    let room = mean(
        &NEIGHBOURS
            .iter()
            .map(|multiple| bin_energy(window, hz * multiple))
            .collect::<Vec<_>>(),
    );
    db_over(bin_energy(window, hz), room).unwrap_or(0.0)
}

/// The frames as contiguous windows of [`BLOCK_FRAMES`], and how far the tone
/// stood out of each.
///
/// A partial window at the end is dropped rather than measured short: a
/// shorter window has less of the tone in it and would drag the median down
/// for a reason that has nothing to do with the room.
fn standouts(frames: &[Frame], hz: f32) -> Vec<f32> {
    frames
        .chunks_exact(BLOCK_FRAMES)
        .map(|block| {
            let window: Vec<i16> = block.iter().flatten().copied().collect();
            standout(&window, hz)
        })
        .collect()
}

fn median(sorted: &[f32]) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[sorted.len() / 2]
}

fn median_of(mut values: Vec<f32>) -> f32 {
    values.sort_by(f32::total_cmp);
    median(&values)
}

/// Runs the walk and returns what each segment sounded like.
async fn measure(
    options: &Options,
    prefs: &AudioPrefs,
    playing: &AtomicBool,
    ears: &Ears,
    canceller: &AtomicU64,
    reference: f32,
) -> Vec<Heard> {
    let mut walk = Vec::with_capacity(WALK.len());
    for segment in &WALK {
        walk.push(run_segment(options, prefs, playing, ears, canceller, reference, segment).await);
    }
    walk
}

/// One segment: set the switches, throw away the settle, then measure.
async fn run_segment(
    options: &Options,
    prefs: &AudioPrefs,
    playing: &AtomicBool,
    ears: &Ears,
    canceller: &AtomicU64,
    reference: f32,
    segment: &'static Segment,
) -> Heard {
    prefs.set(AudioSettings {
        noise_suppression: segment.noise,
        echo_cancellation: segment.echo,
        ..AudioSettings::default()
    });
    playing.store(segment.tone, Ordering::Relaxed);

    println!("--- {} — {} ---", segment.name, segment.what);
    tokio::time::sleep(SETTLE).await;
    ears.drain();

    let mut frames = Vec::new();
    let mut likelihoods = Vec::new();
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    ticker.tick().await;
    for second in 1..=options.seconds {
        ticker.tick().await;
        let batch = ears.drain();
        let energy = mean(
            &batch
                .iter()
                .map(|f| bin_energy(f, options.hz))
                .collect::<Vec<_>>(),
        );
        let believed = f64::from_bits(canceller.load(Ordering::Relaxed));
        if believed.is_finite() {
            likelihoods.push(believed);
        }
        let stands = median_of(standouts(&batch, options.hz));
        println!(
            "  {second:>3}s   {:>4} frames   peak {:>5}   stands out {:>5}   {:>22}   {}",
            batch.len(),
            batch.iter().map(|f| peak(f)).max().unwrap_or(0),
            format!("{stands:.1}"),
            column(reference, energy, batch.len()),
            believed_column(believed),
        );
        frames.extend(batch);
    }
    println!();

    let mut energies: Vec<f32> = frames.iter().map(|f| bin_energy(f, options.hz)).collect();
    let mut standouts: Vec<f32> = standouts(&frames, options.hz);
    let mut levels: Vec<f32> = frames.iter().map(rms).collect();
    energies.sort_by(f32::total_cmp);
    standouts.sort_by(f32::total_cmp);
    levels.sort_by(f32::total_cmp);
    Heard {
        name: segment.name,
        frames,
        energies,
        standouts,
        levels,
        likelihood: (!likelihoods.is_empty()).then(|| mean64(&likelihoods)),
    }
}

fn column(reference: f32, energy: f32, frames: usize) -> String {
    if frames == 0 {
        return "— nothing arriving".to_owned();
    }
    db_below(reference, energy).map_or_else(
        || "— nothing came back".to_owned(),
        |db| format!("{db:.1} dB below sent"),
    )
}

fn believed_column(likelihood: f64) -> String {
    if likelihood.is_finite() {
        format!("canceller {likelihood:.2}")
    } else {
        "canceller —".to_owned()
    }
}

fn endpoint(endpoint: &hardware::Endpoint) -> String {
    format!(
        "{} — {} Hz, {} ch, {}, buffer {}",
        endpoint.name, endpoint.sample_rate, endpoint.channels, endpoint.format, endpoint.buffer
    )
}

// --- the report ------------------------------------------------------------

fn report(walk: &[Heard], reference: f32, hz: f32) -> Result<()> {
    let quiet = &walk[0];
    let suppressed = &walk[1];
    let echo_off = &walk[2];
    let echo_on = &walk[3];

    println!("--- what {hz:.0} Hz did in this room ---\n");
    println!("  segment       frames   stands out   at the tone         room level   canceller");
    for segment in walk {
        println!(
            "  {:<12}  {:>6}   {:>7.1} dB   {:>17}   {:>10.1}   {}",
            segment.name,
            segment.frames.len(),
            segment.standout(),
            column(reference, segment.median_energy(), segment.frames.len()),
            segment.median_level(),
            segment
                .likelihood
                .map_or_else(|| "—".to_owned(), |value| format!("{value:.2}"))
        );
    }
    println!();

    // The suppressor first, because it is the one number here that is not a
    // verdict. The gain controller runs after both switches and is not one of
    // them: it answers a quieter frame by turning it up, so a suppressor that
    // works can leave the level where it was or above it. What this line is
    // for is the WAV beside it — task 4.7's question is whether it *sounds*
    // better, and no level answers that.
    println!(
        "NOISE_SUPPRESSED={} (median room level {:.1} → {:.1}; the gain controller sits after it)",
        db_over(quiet.median_level(), suppressed.median_level())
            .map_or_else(|| "—".to_owned(), |db| format!("{db:.1} dB")),
        quiet.median_level(),
        suppressed.median_level()
    );

    if echo_off.frames.is_empty() || quiet.frames.is_empty() {
        bail!("nothing arrived from the room, so there is nothing to say about a loudspeaker");
    }

    let coupling = echo_off.standout() - quiet.standout();
    println!(
        "COUPLING={coupling:.1} dB (the tone stood {:.1} dB out of the room, against {:.1} dB with it silent)",
        echo_off.standout(),
        quiet.standout()
    );

    if coupling < COUPLING_FLOOR_DB {
        println!("VERDICT=inconclusive");
        bail!(
            "the microphone cannot hear the loudspeaker: the tone stood only {coupling:.1} dB out \
             of the room, under the {COUPLING_FLOOR_DB:.0} dB this needs.\n  \
             That is a fact about the room and not about the canceller — nothing here says \
             whether it works.\n  \
             Put a loudspeaker in front of the microphone (an earcup laid against it counts, a \
             headset on a head does not), turn it up, and run this again."
        );
    }

    // What the canceller took off the tone, measured the same way in the same
    // room seconds apart. `quiet` is the floor: an echo pushed under the room's
    // own noise cannot be measured further down, and claiming the number this
    // subtraction gives would be claiming to have heard past it.
    let cancelled = echo_off.standout() - echo_on.standout();
    let floored = echo_on.standout() - quiet.standout() < FLOOR_MARGIN_DB;
    println!(
        "ECHO_CANCELLED={}{cancelled:.1} dB ({:.1} dB of standout → {:.1} dB)",
        if floored { "at least " } else { "" },
        echo_off.standout(),
        echo_on.standout()
    );
    println!(
        "RESIDUAL={:.1} dB of standout, against {:.1} dB in the silent room",
        echo_on.standout(),
        quiet.standout()
    );
    println!(
        "CANCELLER_BELIEVED={} with the canceller off, {} with it on",
        believed(echo_off),
        believed(echo_on)
    );
    println!("VERDICT=measured");
    Ok(())
}

fn believed(segment: &Heard) -> String {
    segment
        .likelihood
        .map_or_else(|| "—".to_owned(), |value| format!("{value:.2}"))
}

// --- what the far end heard ------------------------------------------------

/// A sink that keeps every frame, so a segment can be measured after the fact
/// and listened to afterwards.
///
/// Frames are kept regardless of which slot they arrive in: this room holds
/// exactly two clients, so there is only ever one speaker in it, and its slot
/// changes across a reconnect (`RecordingSink::loudest` exists for the same
/// reason).
#[derive(Default)]
struct Ears {
    frames: Mutex<Vec<Frame>>,
    heard: AtomicU64,
}

impl Ears {
    /// Everything since the last drain, and starts a new stretch.
    fn drain(&self) -> Vec<Frame> {
        self.frames
            .lock()
            .map(|mut frames| std::mem::take(&mut *frames))
            .unwrap_or_default()
    }

    /// How many frames have arrived since the call started, ever.
    fn heard(&self) -> u64 {
        self.heard.load(Ordering::Relaxed)
    }
}

impl AudioSink for Ears {
    fn play(&self, _slot: usize, frame: &Frame) {
        self.heard.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut frames) = self.frames.lock() {
            frames.push(*frame);
        }
    }

    fn clear(&self, _slot: usize) {}
}

/// The tone the room's loudspeaker plays, with a switch on it.
///
/// `ToneSource` is fixed at construction, and the control segment needs the
/// same source to fall silent without leaving the room — a client that comes
/// and goes would take its SFU subscription with it and the two segments would
/// not be comparable.
struct Loudspeaker {
    frequency_hz: f32,
    phase: f32,
    playing: Arc<AtomicBool>,
    ticker: tokio::time::Interval,
}

impl Loudspeaker {
    fn new(frequency_hz: f32, playing: Arc<AtomicBool>) -> Self {
        Self {
            frequency_hz,
            phase: 0.0,
            playing,
            ticker: tokio::time::interval(Duration::from_millis(u64::from(FRAME_MS))),
        }
    }
}

#[async_trait::async_trait]
impl AudioSource for Loudspeaker {
    async fn next_frame(&mut self) -> Option<Frame> {
        self.ticker.tick().await;
        let mut frame = silent_frame();
        if !self.playing.load(Ordering::Relaxed) {
            // Genuinely silent rather than merely quiet, and the phase goes
            // back to where the reference frame starts.
            self.phase = 0.0;
            return Some(frame);
        }
        #[allow(
            clippy::cast_possible_truncation,
            reason = "the amplitude is bounded well inside i16 by construction"
        )]
        for sample in &mut frame {
            *sample = (self.phase.sin() * AMPLITUDE) as i16;
            #[allow(
                clippy::cast_precision_loss,
                reason = "48000 is exactly representable as f32"
            )]
            {
                self.phase += std::f32::consts::TAU * self.frequency_hz / SAMPLE_RATE_HZ as f32;
            }
        }
        Some(frame)
    }
}

/// The real microphone, with the canceller's own opinion read off it on the
/// way past.
struct Tap {
    microphone: Microphone,
    likelihood: Arc<AtomicU64>,
}

impl Tap {
    fn new(microphone: Microphone, likelihood: Arc<AtomicU64>) -> Self {
        Self {
            microphone,
            likelihood,
        }
    }
}

#[async_trait::async_trait]
impl AudioSource for Tap {
    async fn next_frame(&mut self) -> Option<Frame> {
        let frame = self.microphone.next_frame().await;
        self.likelihood.store(
            self.microphone
                .echo_likelihood()
                .unwrap_or(f64::NAN)
                .to_bits(),
            Ordering::Relaxed,
        );
        frame
    }
}

// --- writing it down -------------------------------------------------------

fn record(directory: &Path, walk: &[Heard]) -> Result<()> {
    fs::create_dir_all(directory)
        .with_context(|| format!("could not make {}", directory.display()))?;
    println!();
    for segment in walk {
        let path = directory.join(format!("{}.wav", segment.name.replace(' ', "-")));
        write_wav(&path, &segment.frames)
            .with_context(|| format!("could not write {}", path.display()))?;
        println!("  wrote {}", path.display());
    }
    println!();
    Ok(())
}

/// One segment as a mono 16-bit WAV at 48 kHz, which is what it already is.
fn write_wav(path: &Path, frames: &[Frame]) -> Result<()> {
    let samples = frames.len() * frames.first().map_or(0, |frame| frame.len());
    let data = u32::try_from(samples * 2).unwrap_or(u32::MAX);
    let mut out = BufWriter::new(File::create(path)?);
    out.write_all(b"RIFF")?;
    out.write_all(&(36 + data).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16_u32.to_le_bytes())?;
    out.write_all(&1_u16.to_le_bytes())?; // PCM
    out.write_all(&1_u16.to_le_bytes())?; // mono
    out.write_all(&SAMPLE_RATE_HZ.to_le_bytes())?;
    out.write_all(&(SAMPLE_RATE_HZ * 2).to_le_bytes())?; // bytes a second
    out.write_all(&2_u16.to_le_bytes())?; // bytes a sample
    out.write_all(&16_u16.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&data.to_le_bytes())?;
    for frame in frames {
        for sample in frame {
            out.write_all(&sample.to_le_bytes())?;
        }
    }
    out.flush()?;
    Ok(())
}

// --- arithmetic ------------------------------------------------------------

#[allow(
    clippy::cast_precision_loss,
    reason = "a segment holds hundreds of frames, not billions"
)]
fn mean(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f32>() / values.len() as f32
}

#[allow(
    clippy::cast_precision_loss,
    reason = "a segment holds hundreds of frames, not billions"
)]
fn mean64(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

/// The frame's level, on the same `0`–`32767` scale the peak column uses.
#[allow(
    clippy::cast_precision_loss,
    reason = "sample values and the frame length are exact in f32"
)]
fn rms(frame: &Frame) -> f32 {
    let sum: f32 = frame.iter().map(|&s| f32::from(s) * f32::from(s)).sum();
    (sum / frame.len() as f32).sqrt()
}

// --- arguments -------------------------------------------------------------

struct Options {
    base: String,
    room: String,
    hz: f32,
    seconds: u64,
    record: Option<PathBuf>,
}

impl Options {
    fn call(&self, name: &str) -> CallOptions {
        CallOptions {
            base: self.base.clone(),
            room: self.room.clone(),
            name: name.to_owned(),
            // A gate would make the tone come and go, and the quiet segments
            // would stop arriving altogether — which is exactly the silence
            // this drill has to be able to tell from a cancelled echo.
            mode: TransmitMode::Open,
            prefs: Arc::new(AudioPrefs::default()),
        }
    }
}

fn args() -> Options {
    let mut options = Options {
        base: env::var("GOODVOICE_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_owned()),
        room: DEFAULT_ROOM.to_owned(),
        hz: DEFAULT_HZ,
        seconds: DEFAULT_SEGMENT_SECONDS,
        record: None,
    };
    let mut argv = env::args().skip(1);
    while let Some(flag) = argv.next() {
        let Some(value) = argv.next() else {
            eprintln!("{flag} needs a value");
            break;
        };
        match flag.as_str() {
            "--base" => options.base = value,
            "--room" => options.room = value,
            "--tone" => match value.parse() {
                Ok(hz) => options.hz = hz,
                Err(_) => eprintln!("--tone wants a frequency in Hz, got {value}"),
            },
            "--seconds" => match value.parse() {
                Ok(seconds) => options.seconds = seconds,
                Err(_) => eprintln!("--seconds wants a number, got {value}"),
            },
            "--record" => options.record = Some(PathBuf::from(value)),
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    options
        .base
        .truncate(options.base.trim_end_matches('/').len());
    options
}
