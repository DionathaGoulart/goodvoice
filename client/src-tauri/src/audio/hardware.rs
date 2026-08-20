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
    opus::{silent_frame, Frame, FRAME_SAMPLES, SAMPLE_RATE_HZ},
    AudioError,
};

/// How much audio each ring holds. 200 ms is far more than the voice path
/// wants to be carrying, but a ring that runs dry clicks and one that is
/// merely deep does not — the depth is a ceiling, not a target.
const RING_MS: usize = 200;
const RING_SAMPLES: usize = (SAMPLE_RATE_HZ as usize * RING_MS) / 1000;

/// The largest device callback the render path handles in one pass. Bigger
/// buffers are processed in several, so the scratch arrays stay on the stack.
const SCRATCH_FRAMES: usize = 1024;

// Both rings and both scratch buffers have to outlast one codec frame, or the
// voice path underruns on every schedule hiccup. Checked at compile time
// because every value involved is a constant.
const _: () = {
    assert!(RING_SAMPLES > FRAME_SAMPLES * 2);
    assert!(SCRATCH_FRAMES * 2 >= FRAME_SAMPLES);
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
pub fn open() -> Result<(Microphone, Speakers), AudioError> {
    let capture = HeapRb::<i16>::new(RING_SAMPLES);
    let (capture_tx, capture_rx) = capture.split();

    let mut playback_tx = Vec::with_capacity(MAX_REMOTE_SLOTS);
    let mut playback_rx = Vec::with_capacity(MAX_REMOTE_SLOTS);
    for _ in 0..MAX_REMOTE_SLOTS {
        let (producer, consumer) = HeapRb::<i16>::new(RING_SAMPLES).split();
        playback_tx.push(Mutex::new(producer));
        playback_rx.push(consumer);
    }

    let drain = Arc::new(
        std::iter::repeat_with(|| AtomicBool::new(false))
            .take(MAX_REMOTE_SLOTS)
            .collect::<Vec<_>>(),
    );

    let captured = Arc::new(Notify::new());
    let running = Arc::new(AtomicBool::new(true));
    let started = Arc::new((Mutex::new(None::<Result<(), AudioError>>), Condvar::new()));

    let worker = {
        let captured = Arc::clone(&captured);
        let running = Arc::clone(&running);
        let started = Arc::clone(&started);
        let drain = Arc::clone(&drain);
        thread::Builder::new()
            .name("goodvoice-audio".to_owned())
            .spawn(move || {
                run_devices(
                    capture_tx,
                    playback_rx,
                    drain,
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
            captured,
            _guard: Arc::clone(&guard),
        },
        Speakers {
            slots: playback_tx,
            drain,
            _guard: guard,
        },
    ))
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
    playback_rx: Vec<HeapCons<i16>>,
    drain: Arc<Vec<AtomicBool>>,
    captured: &Arc<Notify>,
    running: &Arc<AtomicBool>,
    started: &StartSignal,
) {
    let outcome = build_and_play(capture_tx, playback_rx, drain, captured);

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
    playback_rx: Vec<HeapCons<i16>>,
    drain: Arc<Vec<AtomicBool>>,
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
        playback_rx,
        drain,
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
    captured: Arc<Notify>,
    _guard: Arc<DeviceGuard>,
}

#[async_trait::async_trait]
impl AudioSource for Microphone {
    async fn next_frame(&mut self) -> Option<Frame> {
        loop {
            if self.samples.occupied_len() >= FRAME_SAMPLES {
                let mut frame = silent_frame();
                let taken = self.samples.pop_slice(&mut frame);
                debug_assert_eq!(taken, FRAME_SAMPLES);
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
    slots: Vec<HeapCons<i16>>,
    drain: Arc<Vec<AtomicBool>>,
) -> Result<cpal::Stream, AudioError> {
    let channels = config.channels as usize;
    match format {
        SampleFormat::I16 => {
            device.build_output_stream(*config, mixer::<i16>(slots, channels, drain), report, None)
        }
        _ => {
            device.build_output_stream(*config, mixer::<f32>(slots, channels, drain), report, None)
        }
    }
    .map_err(|_| AudioError::NoDevice)
}

/// The render callback: sums every slot into one mono signal and fans it out
/// across the device's channels.
///
/// A saturating sum, nothing more. Task 3.1 replaces this with a real mixer —
/// per-speaker gain, and the level metering the roster needs to show who is
/// talking. What is here is enough for everyone in the room to be audible.
fn mixer<T>(
    mut slots: Vec<HeapCons<i16>>,
    channels: usize,
    drain: Arc<Vec<AtomicBool>>,
) -> impl FnMut(&mut [T], &cpal::OutputCallbackInfo) + Send + 'static
where
    T: cpal::SizedSample + FromSample<i16>,
{
    let mut taken = [0_i16; SCRATCH_FRAMES];
    let mut mixed = [0_i16; SCRATCH_FRAMES];
    let channels = channels.max(1);

    move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        // Only the consumer end can throw queued audio away, and the consumer
        // end lives here. A flag rather than a lock, because this is a device
        // callback (styleguide.md).
        for (slot, flag) in slots.iter_mut().zip(drain.iter()) {
            if flag.swap(false, Ordering::AcqRel) {
                slot.clear();
            }
        }

        for block in data.chunks_mut(SCRATCH_FRAMES * channels) {
            let frames = block.len() / channels;
            mixed[..frames].fill(0);

            for slot in &mut slots {
                let read = slot.pop_slice(&mut taken[..frames]);
                for (out, &sample) in mixed[..read].iter_mut().zip(taken[..read].iter()) {
                    *out = out.saturating_add(sample);
                }
            }

            // Underrun writes silence rather than the previous buffer: a
            // repeated frame is a far more audible artefact than a gap.
            for (index, out) in block.chunks_mut(channels).enumerate() {
                let sample = T::from_sample_(mixed[index]);
                out.fill(sample);
            }
        }
    }
}

/// The speakers, as the decode tasks see them.
pub struct Speakers {
    slots: Vec<Mutex<HeapProd<i16>>>,
    /// Set by [`AudioSink::clear`], acted on by the render callback.
    drain: Arc<Vec<AtomicBool>>,
    _guard: Arc<DeviceGuard>,
}

impl AudioSink for Speakers {
    fn play(&self, slot: usize, frame: &Frame) {
        let Some(producer) = self.slots.get(slot) else {
            return;
        };
        let Ok(mut producer) = producer.lock() else {
            return;
        };
        // A slot that has fallen this far behind is not going to catch up by
        // being given more; dropping the frame keeps the delay bounded.
        producer.push_slice(frame);
    }

    fn clear(&self, slot: usize) {
        // Asking the callback to drain rather than draining here: the ring's
        // read side belongs to the device thread, and a peer who left should
        // stop mid-word rather than finish out the buffer.
        if let Some(flag) = self.drain.get(slot) {
            flag.store(true, Ordering::Release);
        }
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
