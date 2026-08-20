//! Opus encode/decode for the voice path.
//!
//! One shape only: 48 kHz mono, 20 ms frames. Every buffer on this path is
//! caller-owned and fixed-size, so a steady-state call encodes and decodes
//! without allocating (see `frame_path_never_allocates` below).

use opus::{Application, Bitrate, Channels, Decoder, Encoder};

use super::AudioError;

/// The only sample rate goodvoice runs at. Opus is native at 48 kHz and
/// resampling anywhere on this path would cost latency for nothing.
pub const SAMPLE_RATE_HZ: u32 = 48_000;

/// Frame length in milliseconds (prd.md §3 F1).
pub const FRAME_MS: u32 = 20;

/// Samples in one frame, one channel: 48 kHz × 20 ms.
pub const FRAME_SAMPLES: usize = (SAMPLE_RATE_HZ as usize * FRAME_MS as usize) / 1000;

/// Opus' own hard ceiling for a single packet. Sizing every packet buffer to
/// this means the frame path never has to ask how big the next one will be.
pub const MAX_PACKET_BYTES: usize = 1275;

/// Starting bitrate (prd.md §10 open question 5 leans fixed for v1).
pub const DEFAULT_BITRATE_BPS: i32 = 32_000;

/// Range the PRD allows per speaker.
pub const MIN_BITRATE_BPS: i32 = 20_000;
pub const MAX_BITRATE_BPS: i32 = 40_000;

/// One frame of mono PCM, sized at compile time so a wrong-length buffer is a
/// type error rather than a runtime one.
pub type Frame = [i16; FRAME_SAMPLES];

/// A silent frame, handy for priming buffers.
#[must_use]
pub const fn silent_frame() -> Frame {
    [0; FRAME_SAMPLES]
}

/// Encodes 20 ms mono frames.
pub struct VoiceEncoder {
    inner: Encoder,
    bitrate_bps: i32,
}

impl VoiceEncoder {
    /// Builds an encoder at [`DEFAULT_BITRATE_BPS`].
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Codec`] if Opus rejects the configuration.
    pub fn new() -> Result<Self, AudioError> {
        Self::with_bitrate(DEFAULT_BITRATE_BPS)
    }

    /// Builds an encoder at a specific bitrate.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Bitrate`] outside the PRD's 20–40 kbps range, or
    /// [`AudioError::Codec`] if Opus rejects the configuration.
    pub fn with_bitrate(bitrate_bps: i32) -> Result<Self, AudioError> {
        let inner = Encoder::new(SAMPLE_RATE_HZ, Channels::Mono, Application::Voip)?;
        let mut encoder = Self {
            inner,
            bitrate_bps: DEFAULT_BITRATE_BPS,
        };
        encoder.set_bitrate(bitrate_bps)?;
        Ok(encoder)
    }

    /// The bitrate currently in force.
    #[must_use]
    pub const fn bitrate_bps(&self) -> i32 {
        self.bitrate_bps
    }

    /// Retunes the encoder mid-call.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Bitrate`] outside 20–40 kbps, or
    /// [`AudioError::Codec`] if Opus rejects the value.
    pub fn set_bitrate(&mut self, bitrate_bps: i32) -> Result<(), AudioError> {
        if !(MIN_BITRATE_BPS..=MAX_BITRATE_BPS).contains(&bitrate_bps) {
            return Err(AudioError::Bitrate(bitrate_bps));
        }
        self.inner.set_bitrate(Bitrate::Bits(bitrate_bps))?;
        self.bitrate_bps = bitrate_bps;
        Ok(())
    }

    /// Encodes one frame into `packet`, returning how many bytes were written.
    ///
    /// Allocates nothing: both buffers belong to the caller.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::PacketBuffer`] if `packet` is too small to hold a
    /// worst-case packet, or [`AudioError::Codec`] if Opus fails.
    pub fn encode(&mut self, frame: &Frame, packet: &mut [u8]) -> Result<usize, AudioError> {
        if packet.len() < MAX_PACKET_BYTES {
            return Err(AudioError::PacketBuffer(packet.len()));
        }
        Ok(self.inner.encode(frame, packet)?)
    }
}

/// Decodes 20 ms mono frames, including concealing lost ones.
pub struct VoiceDecoder {
    inner: Decoder,
}

impl VoiceDecoder {
    /// Builds a decoder matching [`VoiceEncoder`].
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Codec`] if Opus rejects the configuration.
    pub fn new() -> Result<Self, AudioError> {
        Ok(Self {
            inner: Decoder::new(SAMPLE_RATE_HZ, Channels::Mono)?,
        })
    }

    /// Decodes one packet into `frame`, returning the samples written.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::EmptyPacket`] for a zero-length packet — use
    /// [`VoiceDecoder::conceal`] for a lost frame — or [`AudioError::Codec`].
    pub fn decode(&mut self, packet: &[u8], frame: &mut Frame) -> Result<usize, AudioError> {
        if packet.is_empty() {
            return Err(AudioError::EmptyPacket);
        }
        Ok(self.inner.decode(packet, &mut frame[..], false)?)
    }

    /// Produces a concealment frame for a packet that never arrived, keeping
    /// the decoder's internal state coherent for the packets that follow.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Codec`] if Opus fails.
    pub fn conceal(&mut self, frame: &mut Frame) -> Result<usize, AudioError> {
        // An empty slice is how this binding spells packet loss.
        Ok(self.inner.decode(&[], &mut frame[..], false)?)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        silent_frame, Frame, VoiceDecoder, VoiceEncoder, FRAME_SAMPLES, MAX_PACKET_BYTES,
        SAMPLE_RATE_HZ,
    };
    use crate::audio::AudioError;
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
        f32::consts::TAU,
    };

    // --- allocation counting -------------------------------------------------

    thread_local! {
        static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    }

    /// Counts allocations on the calling thread only, so tests running in
    /// parallel cannot pollute each other's measurement.
    struct CountingAllocator;

    // SAFETY: every method forwards to `System` unchanged; the counter is a
    // thread-local `Cell<usize>` that never allocates.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;

    fn allocations() -> usize {
        ALLOCATIONS.with(Cell::get)
    }

    // --- signal helpers ------------------------------------------------------

    /// A frame of a sine wave, continuous across frames via `phase`.
    fn tone(frequency_hz: f32, phase: &mut f32) -> Frame {
        let mut frame = silent_frame();
        #[allow(
            clippy::cast_possible_truncation,
            reason = "amplitude is bounded to i16 range by construction"
        )]
        for sample in &mut frame {
            *sample = (phase.sin() * 8_000.0) as i16;
            #[allow(
                clippy::cast_precision_loss,
                reason = "48000 is exactly representable as f32"
            )]
            {
                *phase += TAU * frequency_hz / SAMPLE_RATE_HZ as f32;
            }
        }
        frame
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "FRAME_SAMPLES is 960 — exact in f64"
    )]
    fn rms(frame: &Frame) -> f64 {
        let sum: f64 = frame.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
        (sum / FRAME_SAMPLES as f64).sqrt()
    }

    /// Goertzel: energy of one frequency bin, enough to tell 440 Hz from noise
    /// without pulling in an FFT crate.
    #[allow(
        clippy::cast_precision_loss,
        reason = "constants and sample values are exact in f32"
    )]
    fn bin_energy(frame: &Frame, frequency_hz: f32) -> f32 {
        let coefficient = 2.0 * (TAU * frequency_hz / SAMPLE_RATE_HZ as f32).cos();
        let (mut previous, mut previous2) = (0.0_f32, 0.0_f32);
        for &sample in frame {
            let current = f32::from(sample) + coefficient * previous - previous2;
            previous2 = previous;
            previous = current;
        }
        previous.mul_add(previous, previous2 * previous2) - coefficient * previous * previous2
    }

    /// Runs `frames` frames of a tone through encode → decode and hands back
    /// the last decoded frame, once the codec has settled past its lookahead.
    fn round_trip(frequency_hz: f32, frames: usize) -> (Frame, Frame) {
        let mut encoder = VoiceEncoder::new().expect("encoder");
        let mut decoder = VoiceDecoder::new().expect("decoder");
        let mut packet = [0_u8; MAX_PACKET_BYTES];
        let mut output = silent_frame();
        let mut phase = 0.0;
        let mut source = silent_frame();

        for _ in 0..frames {
            source = tone(frequency_hz, &mut phase);
            let written = encoder.encode(&source, &mut packet).expect("encode");
            let samples = decoder
                .decode(&packet[..written], &mut output)
                .expect("decode");
            assert_eq!(samples, FRAME_SAMPLES);
        }

        (source, output)
    }

    // --- tests ---------------------------------------------------------------

    #[test]
    fn frame_is_twenty_milliseconds_at_48khz() {
        assert_eq!(FRAME_SAMPLES, 960);
    }

    #[test]
    fn round_trip_preserves_loudness() {
        let (source, decoded) = round_trip(440.0, 10);

        let ratio = rms(&decoded) / rms(&source);
        assert!(
            (0.8..1.25).contains(&ratio),
            "decoded/source RMS ratio was {ratio}"
        );
    }

    #[test]
    fn round_trip_preserves_pitch() {
        let (_, decoded) = round_trip(440.0, 10);

        let at_tone = bin_energy(&decoded, 440.0);
        let off_tone = bin_energy(&decoded, 1_500.0);
        assert!(
            at_tone > off_tone * 10.0,
            "440 Hz energy {at_tone} was not clearly above 1500 Hz energy {off_tone}"
        );
    }

    #[test]
    fn silence_costs_less_than_speech() {
        let mut encoder = VoiceEncoder::new().expect("encoder");
        let mut packet = [0_u8; MAX_PACKET_BYTES];
        let mut phase = 0.0;

        // VBR is on by default, so a quiet frame should cost less than a loud
        // one at the same bitrate. Both are measured in steady state — the
        // encoder's first few frames are larger while it settles. (DTX stays
        // off: mute already stops sending entirely — prd.md §3 F1.)
        let mut loud_bytes = 0;
        for _ in 0..10 {
            let loud = tone(440.0, &mut phase);
            loud_bytes = encoder.encode(&loud, &mut packet).expect("encode");
        }

        let mut silent_bytes = 0;
        for _ in 0..10 {
            silent_bytes = encoder
                .encode(&silent_frame(), &mut packet)
                .expect("encode");
        }

        assert!(
            silent_bytes < loud_bytes,
            "silence took {silent_bytes} bytes against {loud_bytes} for a tone"
        );
    }

    #[test]
    fn a_louder_bitrate_produces_larger_packets() {
        let sizes = [super::MIN_BITRATE_BPS, super::MAX_BITRATE_BPS].map(|bitrate| {
            let mut encoder = VoiceEncoder::with_bitrate(bitrate).expect("encoder");
            let mut packet = [0_u8; MAX_PACKET_BYTES];
            let mut phase = 0.0;
            let mut total = 0;
            for _ in 0..25 {
                let frame = tone(440.0, &mut phase);
                total += encoder.encode(&frame, &mut packet).expect("encode");
            }
            total
        });

        assert!(
            sizes[1] > sizes[0],
            "40 kbps produced {} bytes, 20 kbps produced {}",
            sizes[1],
            sizes[0]
        );
    }

    #[test]
    fn bitrate_outside_the_prd_range_is_refused() {
        assert!(matches!(
            VoiceEncoder::with_bitrate(8_000),
            Err(AudioError::Bitrate(8_000))
        ));
        assert!(matches!(
            VoiceEncoder::with_bitrate(128_000),
            Err(AudioError::Bitrate(128_000))
        ));

        let mut encoder = VoiceEncoder::new().expect("encoder");
        assert!(encoder.set_bitrate(24_000).is_ok());
        assert_eq!(encoder.bitrate_bps(), 24_000);
    }

    #[test]
    fn an_empty_packet_is_not_a_decode() {
        let mut decoder = VoiceDecoder::new().expect("decoder");
        let mut frame = silent_frame();

        assert!(matches!(
            decoder.decode(&[], &mut frame),
            Err(AudioError::EmptyPacket)
        ));
    }

    #[test]
    fn a_short_packet_buffer_is_refused() {
        let mut encoder = VoiceEncoder::new().expect("encoder");
        let mut packet = [0_u8; 16];

        assert!(matches!(
            encoder.encode(&silent_frame(), &mut packet),
            Err(AudioError::PacketBuffer(16))
        ));
    }

    #[test]
    fn a_lost_frame_is_concealed_at_full_length() {
        let mut encoder = VoiceEncoder::new().expect("encoder");
        let mut decoder = VoiceDecoder::new().expect("decoder");
        let mut packet = [0_u8; MAX_PACKET_BYTES];
        let mut output = silent_frame();
        let mut phase = 0.0;

        for _ in 0..5 {
            let frame = tone(440.0, &mut phase);
            let written = encoder.encode(&frame, &mut packet).expect("encode");
            decoder
                .decode(&packet[..written], &mut output)
                .expect("decode");
        }

        let mut concealed = silent_frame();
        let samples = decoder.conceal(&mut concealed).expect("conceal");

        assert_eq!(samples, FRAME_SAMPLES);
        // Concealment extrapolates the tone rather than inserting a gap.
        assert!(rms(&concealed) > 100.0, "conceal produced near-silence");
    }

    #[test]
    fn frame_path_never_allocates() {
        let mut encoder = VoiceEncoder::new().expect("encoder");
        let mut decoder = VoiceDecoder::new().expect("decoder");
        let mut packet = [0_u8; MAX_PACKET_BYTES];
        let mut output = silent_frame();
        let mut phase = 0.0;

        // Prime both codecs; construction and first use are allowed to allocate.
        let warmup = tone(440.0, &mut phase);
        let written = encoder.encode(&warmup, &mut packet).expect("encode");
        decoder
            .decode(&packet[..written], &mut output)
            .expect("decode");

        let before = allocations();
        for _ in 0..100 {
            let frame = tone(440.0, &mut phase);
            let written = encoder.encode(&frame, &mut packet).expect("encode");
            decoder
                .decode(&packet[..written], &mut output)
                .expect("decode");
        }
        let during = allocations() - before;

        assert_eq!(during, 0, "the frame path allocated {during} times");
    }
}
