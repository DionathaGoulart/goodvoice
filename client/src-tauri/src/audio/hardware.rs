//! A real microphone and real speakers behind [`super::device`]'s seam.
//!
//! Backed by `cpal`, which is cross-platform — so the whole voice path can be
//! exercised off Windows. Whether cpal is *also* the Windows backend is task
//! 2.1's measurement to make; if the `wasapi` crate wins on buffer control, it
//! lands beside this file and nothing above the seam moves. See DR-8.
//!
//! # Real-time discipline
//!
//! Neither device callback allocates, locks or logs (styleguide.md). They move
//! samples through lock-free SPSC rings and nothing else. Everything that can
//! block — the codec, the network, the mixer's slot bookkeeping — sits on the
//! other side of those rings.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::Duration,
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, FromSample, SampleFormat, StreamConfig, SupportedStreamConfigRange,
};
use ringbuf::{
    traits::{Consumer, Observer, Producer, Split},
    HeapCons, HeapProd, HeapRb,
};
use tokio::sync::Notify;

use super::{
    device::{AudioSink, AudioSource, MAX_REMOTE_SLOTS},
    mixer::{self, Mixer, Playback, MAX_BLOCK},
    opus::{silent_frame, Frame, FRAME_SAMPLES, SAMPLE_RATE_HZ},
    prefs::AudioPrefs,
    processing::{Processing, REFERENCE_SAMPLES},
    AudioError,
};

/// How much audio each ring holds. 200 ms is far more than the voice path
/// wants to be carrying, but a ring that runs dry clicks and one that is
/// merely deep does not — the depth is a ceiling, not a target.
const RING_MS: usize = 200;
const RING_SAMPLES: usize = (SAMPLE_RATE_HZ as usize * RING_MS) / 1000;

/// The largest device callback the capture path handles in one pass. Bigger
/// buffers are processed in several, so the scratch array stays on the stack.
const SCRATCH_FRAMES: usize = 1024;

// The rings and both scratch buffers have to outlast one codec frame, or the
// voice path underruns on every schedule hiccup. Checked at compile time
// because every value involved is a constant.
const _: () = {
    assert!(RING_SAMPLES > FRAME_SAMPLES * 2);
    assert!(SCRATCH_FRAMES * 2 >= FRAME_SAMPLES);
    assert!(MAX_BLOCK * 2 >= FRAME_SAMPLES);
};

// --- opening ---------------------------------------------------------------

/// Opens the default capture and render endpoints.
///
/// Both handles share one worker thread that owns the `cpal` streams, because
/// a `cpal::Stream` is not `Send` on every platform and the voice path is
/// async. Dropping *both* handles stops the thread and the devices with it.
///
/// # Errors
///
/// Returns [`AudioError::NoDevice`] when the host has no default endpoint, and
/// [`AudioError::UnsupportedFormat`] when one cannot run at 48 kHz — goodvoice
/// does not resample (see `opus::SAMPLE_RATE_HZ`).
pub fn open(prefs: Arc<AudioPrefs>) -> Result<(Microphone, Speakers), AudioError> {
    let capture = HeapRb::<i16>::new(RING_SAMPLES);
    let (capture_tx, capture_rx) = capture.split();
    let (mixer, playback) = mixer::open(MAX_REMOTE_SLOTS, RING_SAMPLES);

    // The echo canceller's far end: exactly what the render callback sends to
    // the device, on its way back to the capture path (task 3.4).
    let reference = HeapRb::<i16>::new(REFERENCE_SAMPLES);
    let (reference_tx, reference_rx) = reference.split();
    // A call with an echo beats no call: if the module will not start, the
    // reason goes to the terminal and the microphone is used raw.
    let processing = match Processing::new(reference_rx, prefs.settings()) {
        Ok(processing) => Some(processing),
        Err(error) => {
            eprintln!("{error}; the call will have no echo cancellation");
            None
        }
    };

    let captured = Arc::new(Notify::new());
    let running = Arc::new(AtomicBool::new(true));
    let started = Arc::new((Mutex::new(None::<Result<(), AudioError>>), Condvar::new()));

    let worker = {
        let captured = Arc::clone(&captured);
        let running = Arc::clone(&running);
        let started = Arc::clone(&started);
        thread::Builder::new()
            .name("goodvoice-audio".to_owned())
            .spawn(move || {
                run_devices(
                    capture_tx,
                    mixer,
                    reference_tx,
                    &captured,
                    &running,
                    &started,
                );
            })
            .map_err(|_| AudioError::NoDevice)?
    };

    // The streams are built on the worker, so the caller learns here whether
    // they came up — an error inside the thread would otherwise surface as
    // silence.
    wait_for_start(&started)?;

    let guard = Arc::new(DeviceGuard {
        running,
        worker: Mutex::new(Some(worker)),
    });

    Ok((
        Microphone {
            samples: capture_rx,
            processing,
            applied: prefs.generation(),
            prefs,
            captured,
            _guard: Arc::clone(&guard),
        },
        Speakers {
            playback,
            _guard: guard,
        },
    ))
}

/// What one endpoint turned out to be, for a harness that has to say what it
/// measured on (task 2.1).
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub name: String,
    pub channels: u16,
    pub sample_rate: u32,
    pub format: String,
    /// What cpal will admit about the buffer. On WASAPI's software stack this
    /// is the device default: shared mode takes the engine's own period, and
    /// cpal never asks `IAudioClient3` for a smaller one (DR-12, DR-15).
    pub buffer: String,
}

/// The two default endpoints and the configuration [`open`] would pick for
/// them, without opening anything.
///
/// # Errors
///
/// The same two as [`open`]: no endpoint, or none that runs at 48 kHz.
pub fn describe() -> Result<(Endpoint, Endpoint), AudioError> {
    let host = cpal::default_host();
    let input = host.default_input_device().ok_or(AudioError::NoDevice)?;
    let output = host.default_output_device().ok_or(AudioError::NoDevice)?;

    let (input_config, input_format) = pick_config(
        input
            .supported_input_configs()
            .map_err(|_| AudioError::NoDevice)?,
    )?;
    let (output_config, output_format) = pick_config(
        output
            .supported_output_configs()
            .map_err(|_| AudioError::NoDevice)?,
    )?;

    Ok((
        describe_one(&input, &input_config, input_format),
        describe_one(&output, &output_config, output_format),
    ))
}

fn describe_one(device: &Device, config: &StreamConfig, format: SampleFormat) -> Endpoint {
    Endpoint {
        // `Display` rather than a `name()` — cpal 0.18 requires it of every
        // device and that is where the endpoint's name comes out.
        name: device.to_string(),
        channels: config.channels,
        sample_rate: config.sample_rate,
        format: format.to_string(),
        buffer: match config.buffer_size {
            cpal::BufferSize::Default => "device default".to_owned(),
            cpal::BufferSize::Fixed(frames) => format!("{frames} frames"),
        },
    }
}

/// How `open` learns whether the worker thread got its streams up.
type StartSignal = Arc<(Mutex<Option<Result<(), AudioError>>>, Condvar)>;

fn wait_for_start(started: &StartSignal) -> Result<(), AudioError> {
    let (lock, condvar) = &**started;
    let mut outcome = lock.lock().map_err(|_| AudioError::NoDevice)?;
    while outcome.is_none() {
        let (guard, timeout) = condvar
            .wait_timeout(outcome, Duration::from_secs(5))
            .map_err(|_| AudioError::NoDevice)?;
        outcome = guard;
        if timeout.timed_out() && outcome.is_none() {
            return Err(AudioError::NoDevice);
        }
    }
    outcome.take().unwrap_or(Err(AudioError::NoDevice))
}

/// Stops the worker thread once every handle to it is gone.
struct DeviceGuard {
    running: Arc<AtomicBool>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl Drop for DeviceGuard {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(handle) = worker.take() {
                let _ = handle.join();
            }
        }
    }
}

// --- the worker thread -----------------------------------------------------

fn run_devices(
    capture_tx: HeapProd<i16>,
    mixer: Mixer,
    reference_tx: HeapProd<i16>,
    captured: &Arc<Notify>,
    running: &Arc<AtomicBool>,
    started: &StartSignal,
) {
    let outcome = build_and_play(capture_tx, mixer, reference_tx, captured);

    let streams = match outcome {
        Ok(streams) => {
            signal_start(started, Ok(()));
            streams
        }
        Err(error) => {
            signal_start(started, Err(error));
            return;
        }
    };

    // `streams` stays alive — and therefore so do the devices — until the last
    // handle is dropped. Polling beats a condvar here only because there is
    // nothing else for this thread to do.
    while running.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(50));
    }
    drop(streams);
}

fn signal_start(started: &StartSignal, outcome: Result<(), AudioError>) {
    let (lock, condvar) = &**started;
    if let Ok(mut slot) = lock.lock() {
        *slot = Some(outcome);
    }
    condvar.notify_all();
}

/// The two `cpal` streams, kept together only so one value owns both.
struct Streams {
    _input: cpal::Stream,
    _output: cpal::Stream,
}

fn build_and_play(
    capture_tx: HeapProd<i16>,
    mixer: Mixer,
    reference_tx: HeapProd<i16>,
    captured: &Arc<Notify>,
) -> Result<Streams, AudioError> {
    let host = cpal::default_host();
    let input_device = host.default_input_device().ok_or(AudioError::NoDevice)?;
    let output_device = host.default_output_device().ok_or(AudioError::NoDevice)?;

    let (input_config, input_format) = pick_config(
        input_device
            .supported_input_configs()
            .map_err(|_| AudioError::NoDevice)?,
    )?;
    let (output_config, output_format) = pick_config(
        output_device
            .supported_output_configs()
            .map_err(|_| AudioError::NoDevice)?,
    )?;

    let input = build_input(
        &input_device,
        &input_config,
        input_format,
        capture_tx,
        Arc::clone(captured),
    )?;
    let output = build_output(
        &output_device,
        &output_config,
        output_format,
        mixer,
        reference_tx,
    )?;

    input.play().map_err(|_| AudioError::NoDevice)?;
    output.play().map_err(|_| AudioError::NoDevice)?;

    Ok(Streams {
        _input: input,
        _output: output,
    })
}

/// Picks a 48 kHz configuration, preferring `i16` because that is the shape the
/// codec already wants.
///
/// goodvoice does not resample: 48 kHz is Opus' native rate and a converter on
/// this path would cost latency for nothing (`opus::SAMPLE_RATE_HZ`). A device
/// that cannot do 48 kHz is an error rather than a silent downgrade.
fn pick_config(
    supported: impl Iterator<Item = SupportedStreamConfigRange>,
) -> Result<(StreamConfig, SampleFormat), AudioError> {
    let mut best: Option<SupportedStreamConfigRange> = None;

    for range in supported {
        if range.min_sample_rate() > SAMPLE_RATE_HZ || range.max_sample_rate() < SAMPLE_RATE_HZ {
            continue;
        }
        if !matches!(range.sample_format(), SampleFormat::I16 | SampleFormat::F32) {
            continue;
        }
        let better = best.as_ref().is_none_or(|current| {
            // i16 first, then the narrowest channel count — a mono capture is
            // one downmix we do not have to do.
            let rank = |candidate: &SupportedStreamConfigRange| {
                (
                    u8::from(candidate.sample_format() != SampleFormat::I16),
                    candidate.channels(),
                )
            };
            rank(&range) < rank(current)
        });
        if better {
            best = Some(range);
        }
    }

    let chosen = best.ok_or(AudioError::UnsupportedFormat)?;
    let format = chosen.sample_format();
    Ok((chosen.with_sample_rate(SAMPLE_RATE_HZ).into(), format))
}

// --- capture ---------------------------------------------------------------

fn build_input(
    device: &Device,
    config: &StreamConfig,
    format: SampleFormat,
    producer: HeapProd<i16>,
    captured: Arc<Notify>,
) -> Result<cpal::Stream, AudioError> {
    let channels = config.channels as usize;
    match format {
        SampleFormat::I16 => device.build_input_stream(
            *config,
            downmixer::<i16>(producer, channels, captured),
            report,
            None,
        ),
        _ => device.build_input_stream(
            *config,
            downmixer::<f32>(producer, channels, captured),
            report,
            None,
        ),
    }
    .map_err(|_| AudioError::NoDevice)
}

/// The capture callback: interleaved device samples in, mono `i16` out.
///
/// Allocates nothing and locks nothing. Overrun drops the newest samples
/// instead of blocking the device — a stalled callback is a glitch on every
/// stream the host owns, not just ours.
fn downmixer<T>(
    mut producer: HeapProd<i16>,
    channels: usize,
    captured: Arc<Notify>,
) -> impl FnMut(&[T], &cpal::InputCallbackInfo) + Send + 'static
where
    T: cpal::SizedSample,
    i16: FromSample<T>,
{
    let mut scratch = [0_i16; SCRATCH_FRAMES];

    move |data: &[T], _: &cpal::InputCallbackInfo| {
        let mut written = 0;
        for frame in data.chunks(channels.max(1)) {
            // Averaging beats taking channel 0: a headset that puts the mic on
            // the right channel would otherwise record silence.
            let sum: i32 = frame
                .iter()
                .map(|&sample| i32::from(i16::from_sample_(sample)))
                .sum();
            #[allow(
                clippy::cast_possible_truncation,
                reason = "the average of i16s is an i16"
            )]
            let mono = (sum / i32::try_from(frame.len().max(1)).unwrap_or(1)) as i16;

            scratch[written] = mono;
            written += 1;
            if written == SCRATCH_FRAMES {
                producer.push_slice(&scratch[..written]);
                written = 0;
            }
        }
        if written > 0 {
            producer.push_slice(&scratch[..written]);
        }
        captured.notify_one();
    }
}

/// The microphone, as the encode loop sees it.
pub struct Microphone {
    samples: HeapCons<i16>,
    /// Echo cancellation, noise suppression and gain, applied on the way out.
    /// `None` only when the module refused to start.
    processing: Option<Processing>,
    /// What the user has set. Read once per frame — an atomic load — and acted
    /// on only when [`Self::applied`] disagrees with it.
    prefs: Arc<AudioPrefs>,
    /// The generation the module is currently configured for.
    applied: u32,
    captured: Arc<Notify>,
    _guard: Arc<DeviceGuard>,
}

impl Microphone {
    /// How many samples the device has captured that no frame has taken yet.
    ///
    /// The mirror of [`Speakers::queued`], and there for the same reason: a
    /// harness timing an arrival at the moment a frame appears is late by
    /// whatever was already waiting behind it.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.samples.occupied_len()
    }

    /// What the canceller itself thinks it is looking at, `0.0`–`1.0`.
    ///
    /// The far end can measure how much of its own tone came back; only this
    /// side can say whether WebRTC ever believed there was an echo to remove.
    /// The two disagreeing is the interesting case — see §7.6. `None` when the
    /// module refused to start, and meaningless until it has converged on a
    /// second or two of real playback.
    #[must_use]
    pub fn echo_likelihood(&self) -> Option<f64> {
        self.processing.as_ref()?.echo_likelihood()
    }
}

#[async_trait::async_trait]
impl AudioSource for Microphone {
    async fn next_frame(&mut self) -> Option<Frame> {
        loop {
            if self.samples.occupied_len() >= FRAME_SAMPLES {
                let mut frame = silent_frame();
                let taken = self.samples.pop_slice(&mut frame);
                debug_assert_eq!(taken, FRAME_SAMPLES);
                // Here rather than further up the call: every frame has to go
                // through, including the ones mute or the gate will throw away,
                // or the echo canceller loses its place in the stream. It is
                // also why the voice detector downstream sees clean audio.
                if let Some(processing) = self.processing.as_mut() {
                    // A generation behind means somebody moved a switch since
                    // the last frame. Reconfiguring allocates, so it happens
                    // here — once per change — rather than per frame.
                    let wanted = self.prefs.generation();
                    if wanted != self.applied {
                        processing.reconfigure(self.prefs.settings());
                        self.applied = wanted;
                    }
                    processing.run(&mut frame);
                }
                return Some(frame);
            }
            // Woken by the capture callback, so the encode loop runs on the
            // device's clock and cannot drift against it.
            self.captured.notified().await;
        }
    }
}

// --- playback --------------------------------------------------------------

fn build_output(
    device: &Device,
    config: &StreamConfig,
    format: SampleFormat,
    mixer: Mixer,
    reference: HeapProd<i16>,
) -> Result<cpal::Stream, AudioError> {
    let channels = config.channels as usize;
    match format {
        SampleFormat::I16 => device.build_output_stream(
            *config,
            render::<i16>(mixer, channels, reference),
            report,
            None,
        ),
        _ => device.build_output_stream(
            *config,
            render::<f32>(mixer, channels, reference),
            report,
            None,
        ),
    }
    .map_err(|_| AudioError::NoDevice)
}

/// The render callback: asks the mixer for one mono block and fans it out
/// across the device's channels.
///
/// The mixing itself lives in [`super::mixer`], which is where the rules that
/// are worth testing are. What is left here is the part that only a real device
/// can exercise: chunking the buffer the host handed us, and converting the
/// sample type it asked for.
fn render<T>(
    mut mixer: Mixer,
    channels: usize,
    mut reference: HeapProd<i16>,
) -> impl FnMut(&mut [T], &cpal::OutputCallbackInfo) + Send + 'static
where
    T: cpal::SizedSample + FromSample<i16>,
{
    let mut block_out = [0_i16; MAX_BLOCK];
    let channels = channels.max(1);

    move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        for block in data.chunks_mut(MAX_BLOCK * channels) {
            let frames = block.len() / channels;
            mixer.render(&mut block_out[..frames]);

            // Copied to the echo canceller before it is fanned out: mono, and
            // exactly what the device is about to play. A ring push, so this
            // stays a callback that does not allocate or lock.
            reference.push_slice(&block_out[..frames]);

            for (index, out) in block.chunks_mut(channels).enumerate() {
                let sample = T::from_sample_(block_out[index]);
                out.fill(sample);
            }
        }
    }
}

/// The speakers, as the decode tasks see them.
pub struct Speakers {
    playback: Playback,
    _guard: Arc<DeviceGuard>,
}

impl Speakers {
    /// How many samples `slot` has handed over but the device has not played.
    ///
    /// See [`Playback::queued`]: this exists so task 2.1's round trip can say
    /// how much of what it measured was our own buffering.
    #[must_use]
    pub fn queued(&self, slot: usize) -> usize {
        self.playback.queued(slot)
    }
}

impl AudioSink for Speakers {
    fn play(&self, slot: usize, frame: &Frame) {
        self.playback.play(slot, frame);
    }

    fn clear(&self, slot: usize) {
        self.playback.clear(slot);
    }

    fn level(&self, slot: usize) -> f32 {
        self.playback.level(slot)
    }

    fn set_gain(&self, slot: usize, gain: f32) {
        self.playback.set_gain(slot, gain);
    }
}

/// Device errors reach the caller as silence either way, so this exists to
/// keep the reason visible in a terminal rather than to recover.
#[allow(
    clippy::needless_pass_by_value,
    reason = "cpal's error callback is FnMut(Error), by value"
)]
fn report(error: cpal::Error) {
    eprintln!("audio device error: {error}");
}

#[cfg(test)]
mod tests {
    use super::pick_config;
    use crate::audio::AudioError;

    #[test]
    fn a_device_without_48khz_is_refused() {
        // A host offering nothing usable is the same shape of failure as one
        // whose ranges all sit at the wrong rate, and it needs no real
        // hardware to check. Silence would be the wrong answer here: a client
        // that cannot capture has to say so at join time.
        assert!(matches!(
            pick_config(std::iter::empty()),
            Err(AudioError::UnsupportedFormat)
        ));
    }
}
