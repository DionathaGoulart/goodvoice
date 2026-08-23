//! What Windows.Graphics.Capture actually hands over, measured on real
//! silicon.
//!
//! plan.md task 5.1. Three questions, and none of them has a datasheet answer:
//!
//! 1. **What is there to capture?** Monitors and windows, after the filtering
//!    a picker needs (task 5.3) — most of what `EnumWindows` returns is not
//!    shareable and not nameable.
//! 2. **What format do frames arrive in?** The surface, the texture, and
//!    whether the two agree about size.
//! 3. **Is there an H.264 encoder in silicon, and does what comes out of it
//!    play?** `--encode` runs the capture through it, writes an Annex B
//!    elementary stream, and muxes the same packets into an `.mp4` that
//!    Windows' own player opens without anything else installed (task 5.2).
//! 4. **How does the frame pool behave?** Which is the question with the
//!    surprise in it: WGC is not a clock. It produces a frame when the content
//!    changes and nothing at all when it does not, so "frames per second" here
//!    is a property of what is on the screen, not of the capture.
//!
//! It changes nothing and installs nothing: it opens a capture session, counts
//! what comes out of it, optionally writes a few frames to disk as BMP, and
//! prints the rest in a shape that pastes into a Decision Record.
//!
//! ```text
//! cargo run -p goodvoice-harness --bin capture-spike -- --list
//! cargo run -p goodvoice-harness --bin capture-spike                     # primary monitor, 5 s
//! cargo run -p goodvoice-harness --bin capture-spike -- --seconds 10 --dump 3
//! cargo run -p goodvoice-harness --bin capture-spike -- --window "Notepad"
//! cargo run -p goodvoice-harness --bin capture-spike -- --monitor DISPLAY2
//! cargo run -p goodvoice-harness --bin capture-spike -- --encode --seconds 10
//! cargo run -p goodvoice-harness --bin capture-spike -- --software    # the fallback, and its warning
//! cargo run -p goodvoice-harness --bin capture-spike -- --encode --720
//! ```

#[cfg(not(windows))]
fn main() {
    // Not a silent success, for the same reason `bin/probe` is not: a spike
    // that printed nothing on Linux would look like a machine with nothing to
    // capture rather than the wrong machine (plan.md: do not fake or skip).
    eprintln!("the 5.1 spike asks what Windows.Graphics.Capture gives us");
    eprintln!("and has to run on the Windows host it is asking about");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    windows_spike::run()
}

#[cfg(windows)]
mod windows_spike {
    use std::{
        env,
        fs::{self, File},
        io::{BufWriter, Write as _},
        os::windows::ffi::OsStrExt as _,
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    use anyhow::{bail, Context as _, Result};
    use windows::Win32::Media::MediaFoundation::{
        IMFSinkWriter, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
        MFCreateSinkWriterFromURL, MFMediaType_Video, MFSampleExtension_CleanPoint,
        MFVideoFormat_H264, MFVideoInterlace_Progressive, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE,
        MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO,
        MF_MT_SUBTYPE,
    };

    use goodvoice_client_lib::capture::{
        encoder::{self, EncodeConfig, H264Encoder, Packet, Quality, Selection},
        wgc::{self, Capturer, Cursor, Target, TargetKind},
    };

    /// How long to sit on the target if nothing says otherwise.
    const DEFAULT_SECONDS: u64 = 5;

    /// How many frames to write out. Three is enough to see that the content
    /// is real and moving, and small enough that a 1440p capture does not fill
    /// a directory with 14 MB files.
    const DEFAULT_DUMP: usize = 3;

    /// Long enough that a genuinely idle screen is reported as idle rather
    /// than as a hang, short enough to keep the progress line moving.
    const FRAME_WAIT: Duration = Duration::from_millis(500);

    /// What `--encode` asks the encoder for. The bitrate comes with the
    /// quality; 30 fps is what task 5.3 publishes at.
    const ENCODE_FPS: u32 = 30;

    /// Media Foundation counts time in 100-nanosecond units.
    const HNS_PER_SECOND: i32 = 10_000_000;

    pub fn run() -> Result<()> {
        let args = Args::parse();

        println!("# goodvoice capture spike (plan.md task 5.1)");
        println!();
        println!(
            "Windows.Graphics.Capture is {}",
            if wgc::is_supported() {
                "supported"
            } else {
                "**not supported on this machine**"
            }
        );
        println!();

        list_encoders();

        if args.list {
            return list();
        }

        let target = choose(&args)?;
        println!(
            "## capturing {} — {} ({}×{})",
            match target.kind {
                TargetKind::Monitor => "monitor",
                TargetKind::Window => "window",
            },
            target.name,
            target.width,
            target.height,
        );
        println!();
        measure(&target, &args)
    }

    // --- what there is to capture ------------------------------------------

    fn list() -> Result<()> {
        println!("## monitors");
        println!();
        for target in wgc::monitors().context("listing monitors")? {
            println!(
                "- **{}** — {}×{}, handle {:#x}",
                target.name, target.width, target.height, target.handle
            );
        }

        println!();
        println!("## windows");
        println!();
        let found = wgc::windows().context("listing windows")?;
        if found.is_empty() {
            println!("_none shareable_");
        }
        for target in found {
            println!(
                "- **{}** — {}×{}, handle {:#x}",
                target.name, target.width, target.height, target.handle
            );
        }
        Ok(())
    }

    fn choose(args: &Args) -> Result<Target> {
        if let Some(wanted) = &args.window {
            return pick(wgc::windows().context("listing windows")?, wanted)
                .with_context(|| format!("no shareable window matching {wanted:?}"));
        }
        let monitors = wgc::monitors().context("listing monitors")?;
        match &args.monitor {
            Some(wanted) => {
                pick(monitors, wanted).with_context(|| format!("no monitor matching {wanted:?}"))
            }
            // The primary, which `wgc::monitors` sorts first.
            None => monitors.into_iter().next().context("no monitors"),
        }
    }

    /// The first target whose name contains `wanted`, case-insensitively.
    fn pick(targets: Vec<Target>, wanted: &str) -> Option<Target> {
        let needle = wanted.to_lowercase();
        targets
            .into_iter()
            .find(|target| target.name.to_lowercase().contains(&needle))
    }

    // --- what comes out of it ----------------------------------------------

    fn measure(target: &Target, args: &Args) -> Result<()> {
        let capturer = Capturer::start(target, Cursor::Shown).context("starting the capture")?;

        let mut encode = if args.encode {
            Some(Encode::start(&capturer, args)?)
        } else {
            None
        };

        let mut arrivals: Vec<Duration> = Vec::new();
        let mut idle_waits = 0_u32;
        let mut dumped = 0_usize;
        let mut shape: Option<Shape> = None;

        let started = Instant::now();
        let deadline = started + Duration::from_secs(args.seconds);
        while Instant::now() < deadline {
            let Some(frame) = capturer
                .next_frame(FRAME_WAIT)
                .context("waiting for a frame")?
            else {
                // Not an error: nothing on the screen moved. Counting these
                // separately is the whole of question 3.
                idle_waits += 1;
                continue;
            };

            if shape.is_none() {
                shape = Some(Shape {
                    texture: frame.size(),
                    content: frame.content_size(),
                    format: frame.format().0,
                });
            }
            arrivals.push(frame.time());

            if dumped < args.dump {
                let path = args.out.join(format!("frame-{dumped:02}.bmp"));
                let mut pixels = Vec::new();
                let stride = frame.copy_to_cpu(&mut pixels).context("reading a frame")?;
                let (width, height) = frame.size();
                write_bmp(&path, &pixels, width, height, stride)
                    .with_context(|| format!("writing {}", path.display()))?;
                dumped += 1;
                println!("  wrote {}", path.display());
            }

            if let Some(encode) = encode.as_mut() {
                encode.push(&frame)?;
            }

            // Keeping the console busy is not decoration: on an otherwise
            // still desktop this progress line is the only thing changing, and
            // WGC has nothing to send until something does.
            print!("\r  {} frames", arrivals.len());
            let _ = std::io::stdout().flush();
        }
        let elapsed = started.elapsed();
        println!("\r  {} frames                ", arrivals.len());
        println!();

        report(&arrivals, idle_waits, elapsed, shape.as_ref());
        if let Some(encode) = encode {
            encode.finish(elapsed)?;
        }
        Ok(())
    }

    // --- and through the encoder -------------------------------------------

    fn list_encoders() {
        println!("## H.264 encoders");
        println!();
        match encoder::encoders() {
            Ok(found) if found.is_empty() => {
                println!("_none_ — a share on this machine would have no encoder at all");
            }
            Ok(found) => {
                for info in found {
                    println!(
                        "- **{}** — {}{}",
                        info.name,
                        if info.hardware {
                            "hardware"
                        } else {
                            "software"
                        },
                        if info.d3d11_aware {
                            ", takes D3D11 textures"
                        } else {
                            ", memory buffers only"
                        }
                    );
                }
            }
            Err(error) => println!("_unavailable_ — {error}"),
        }
        println!();
    }

    /// An encoder, the file it is writing, and what it cost.
    struct Encode {
        encoder: H264Encoder,
        packets: Vec<Packet>,
        path: PathBuf,
        bytes: usize,
        keyframes: usize,
        frames: usize,
        spent: Duration,
        slowest: Duration,
    }

    impl Encode {
        fn start(capturer: &Capturer, args: &Args) -> Result<Self> {
            let (width, height) = capturer.size().context("sizing the capture")?;
            // NV12 is 4:2:0, so both dimensions have to be even. Rounding down
            // loses at most one row and one column of a screen.
            let config = args.quality.plan((width, height), ENCODE_FPS);
            let selection = if args.software {
                Selection::SoftwareOnly
            } else {
                Selection::Auto
            };
            let encoder =
                H264Encoder::open_with(capturer.device(), capturer.context(), config, selection)
                    .context("opening an H.264 encoder")?;

            println!("### encoding with {}", encoder.name());
            println!();
            if encoder.is_hardware() {
                println!("- hardware, which is the only path inside prd.md §4's FPS budget");
            } else {
                println!(
                    "- **software fallback** — this will work and it will cost the machine frames"
                );
            }
            if encoder.is_zero_copy() {
                println!("- takes the D3D11 texture; nothing crosses the bus but the bitstream");
            } else {
                println!("- **copies every frame out of GPU memory** — not D3D11-aware");
            }
            println!(
                "- {}×{} at {} fps, {} kbps requested",
                config.width,
                config.height,
                config.fps,
                config.bitrate / 1_000
            );
            println!();

            Ok(Self {
                encoder,
                packets: Vec::new(),
                path: args.out.join("capture.h264"),
                bytes: 0,
                keyframes: 0,
                frames: 0,
                spent: Duration::ZERO,
                slowest: Duration::ZERO,
            })
        }

        fn push(&mut self, frame: &goodvoice_client_lib::capture::wgc::Frame<'_>) -> Result<()> {
            let started = Instant::now();
            self.encoder
                .encode(frame.texture(), frame.time(), &mut self.packets)
                .context("encoding a frame")?;
            let spent = started.elapsed();
            self.spent += spent;
            self.slowest = self.slowest.max(spent);
            self.frames += 1;
            Ok(())
        }

        fn finish(mut self, elapsed: Duration) -> Result<()> {
            self.encoder
                .drain(&mut self.packets)
                .context("draining the encoder")?;

            if let Some(parent) = self.path.parent() {
                fs::create_dir_all(parent).context("creating the output directory")?;
            }
            let mut out =
                BufWriter::new(File::create(&self.path).context("creating the bitstream")?);
            for packet in &self.packets {
                out.write_all(&packet.bytes).context("writing a packet")?;
                self.bytes += packet.bytes.len();
                if packet.keyframe {
                    self.keyframes += 1;
                }
            }
            out.flush().context("flushing the bitstream")?;

            println!();
            println!("### encode");
            println!();
            println!(
                "- {} frames in, {} packets out",
                self.frames,
                self.packets.len()
            );
            println!("- {} keyframes, {} bytes total", self.keyframes, self.bytes);
            #[allow(
                clippy::cast_precision_loss,
                reason = "a byte count in the megabytes, nowhere near f64's mantissa"
            )]
            let bits = self.bytes as f64 * 8.0;
            println!(
                "- {:.0} kbps over {:.1} s of wall clock",
                bits / 1_000.0 / elapsed.as_secs_f64().max(f64::EPSILON),
                elapsed.as_secs_f64()
            );
            if self.frames > 0 {
                println!(
                    "- {:.2} ms per frame in `encode`, slowest {:.2} ms",
                    self.spent.as_secs_f64() * 1_000.0
                        / f64::from(u32::try_from(self.frames).unwrap_or(1)),
                    self.slowest.as_secs_f64() * 1_000.0,
                );
            }
            println!("- wrote {}", self.path.display());

            let mp4 = self.path.with_extension("mp4");
            match write_mp4(&mp4, &self.packets, self.encoder.config()) {
                Ok(()) => println!("- wrote {}", mp4.display()),
                // A missing muxer does not invalidate the encode, and the
                // elementary stream above is still the real output.
                Err(error) => println!("- no mp4: {error}"),
            }
            Ok(())
        }
    }

    /// Mux the encoded packets into an `.mp4`, without re-encoding them.
    ///
    /// The point is the `DoD`'s word "standard": an Annex B elementary stream
    /// needs a player that knows what to do with a bare bitstream, and an mp4
    /// is what a person can double-click. Input type and output type are the
    /// same H.264, so the sink writer muxes and does not transcode.
    fn write_mp4(path: &Path, packets: &[Packet], config: EncodeConfig) -> Result<()> {
        if packets.is_empty() {
            bail!("nothing was encoded");
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("creating the output directory")?;
        }
        // The sink writer will not overwrite, and a stale file from the last
        // run is worse than no file.
        let _ = fs::remove_file(path);

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // SAFETY: every call below is on a live interface, and each media-type
        // key matches the type of the value handed with it — the attribute
        // store rejects a mismatch rather than misreading it.
        unsafe {
            let media = MFCreateMediaType().context("creating the mp4 media type")?;
            media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            media.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            media.SetUINT64(
                &MF_MT_FRAME_SIZE,
                (u64::from(config.width) << 32) + u64::from(config.height),
            )?;
            media.SetUINT64(&MF_MT_FRAME_RATE, (u64::from(config.fps) << 32) + 1)?;
            media.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1_u64 << 32) + 1)?;
            media.SetUINT32(
                &MF_MT_INTERLACE_MODE,
                MFVideoInterlace_Progressive.0.unsigned_abs(),
            )?;
            media.SetUINT32(&MF_MT_AVG_BITRATE, config.bitrate)?;

            let writer: IMFSinkWriter =
                MFCreateSinkWriterFromURL(windows::core::PCWSTR(wide.as_ptr()), None, None)
                    .context("creating the mp4 sink")?;
            let stream = writer
                .AddStream(&media)
                .context("adding the video stream")?;
            writer
                .SetInputMediaType(stream, &media, None)
                .context("the sink refused a passthrough mux")?;
            writer.BeginWriting().context("beginning the mp4")?;

            // The encoder's own timestamps start wherever the capture session
            // did; an mp4 wants them from zero.
            let origin = packets[0].time;
            let duration = i64::from(HNS_PER_SECOND) / i64::from(config.fps.max(1));
            for packet in packets {
                let buffer =
                    MFCreateMemoryBuffer(u32::try_from(packet.bytes.len()).unwrap_or(u32::MAX))
                        .context("allocating a mux buffer")?;
                let mut data = std::ptr::null_mut();
                buffer
                    .Lock(&raw mut data, None, None)
                    .context("locking a mux buffer")?;
                std::ptr::copy_nonoverlapping(packet.bytes.as_ptr(), data, packet.bytes.len());
                let _ = buffer.Unlock();
                buffer.SetCurrentLength(u32::try_from(packet.bytes.len()).unwrap_or(u32::MAX))?;

                let sample = MFCreateSample().context("creating a mux sample")?;
                sample.AddBuffer(&buffer)?;
                sample.SetSampleTime(
                    i64::try_from(packet.time.saturating_sub(origin).as_nanos() / 100)
                        .unwrap_or(i64::MAX),
                )?;
                sample.SetSampleDuration(duration)?;
                if packet.keyframe {
                    sample.SetUINT32(&MFSampleExtension_CleanPoint, 1)?;
                }
                writer
                    .WriteSample(stream, &sample)
                    .context("writing a sample")?;
            }
            writer.Finalize().context("finalising the mp4")?;
        }
        Ok(())
    }

    /// What the first frame said it was, which every later frame agreed with.
    struct Shape {
        texture: (u32, u32),
        content: (u32, u32),
        format: i32,
    }

    fn report(arrivals: &[Duration], idle_waits: u32, elapsed: Duration, shape: Option<&Shape>) {
        println!("### surface");
        println!();
        match shape {
            Some(shape) => {
                println!(
                    "- texture {}×{}, content {}×{}",
                    shape.texture.0, shape.texture.1, shape.content.0, shape.content.1
                );
                println!(
                    "- DXGI format {} ({})",
                    shape.format,
                    format_name(shape.format)
                );
            }
            None => println!("_no frame arrived_ — nothing on the target changed"),
        }

        println!();
        println!("### frame pool");
        println!();
        println!(
            "- {} frames in {:.1} s",
            arrivals.len(),
            elapsed.as_secs_f64()
        );
        println!(
            "- {idle_waits} × {} ms waits that timed out with no frame",
            FRAME_WAIT.as_millis()
        );

        let mut gaps: Vec<f64> = arrivals
            .windows(2)
            .map(|pair| (pair[1].saturating_sub(pair[0])).as_secs_f64() * 1_000.0)
            .collect();
        if gaps.is_empty() {
            println!("- no interval to measure (fewer than two frames)");
            return;
        }
        gaps.sort_by(f64::total_cmp);

        println!(
            "- interval min {:.1} ms, median {:.1} ms, p95 {:.1} ms, max {:.1} ms",
            gaps[0],
            percentile(&gaps, 0.50),
            percentile(&gaps, 0.95),
            gaps[gaps.len() - 1],
        );
        println!(
            "- {:.1} frames/s while the content was moving (from the median interval)",
            1_000.0 / percentile(&gaps, 0.50).max(f64::EPSILON)
        );
    }

    fn percentile(sorted: &[f64], fraction: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a sample count in the thousands, indexing its own slice"
        )]
        let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
        sorted[index.min(sorted.len() - 1)]
    }

    /// The handful of DXGI formats a capture can plausibly be in.
    fn format_name(format: i32) -> &'static str {
        match format {
            87 => "DXGI_FORMAT_B8G8R8A8_UNORM",
            28 => "DXGI_FORMAT_R8G8B8A8_UNORM",
            10 => "DXGI_FORMAT_R16G16B16A16_FLOAT",
            24 => "DXGI_FORMAT_R10G10B10A2_UNORM",
            _ => "unrecognised",
        }
    }

    // --- writing one out ---------------------------------------------------

    /// A 32-bit top-down BMP, which is the shortest path from BGRA to
    /// something a person can double-click.
    fn write_bmp(path: &Path, pixels: &[u8], width: u32, height: u32, stride: usize) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("creating the output directory")?;
        }
        let expected = stride * height as usize;
        if pixels.len() < expected {
            bail!("{} bytes for a {width}×{height} frame", pixels.len());
        }

        let header = 14_u32 + 40;
        let size = header + u32::try_from(expected).unwrap_or(u32::MAX);
        let mut out = BufWriter::new(File::create(path).context("creating the file")?);

        // BITMAPFILEHEADER
        out.write_all(b"BM")?;
        out.write_all(&size.to_le_bytes())?;
        out.write_all(&0_u32.to_le_bytes())?;
        out.write_all(&header.to_le_bytes())?;
        // BITMAPINFOHEADER. Negative height is what says top-down, which is
        // the order the texture is already in — the alternative is copying it
        // backwards for nothing.
        out.write_all(&40_u32.to_le_bytes())?;
        out.write_all(&i32::try_from(width).unwrap_or(i32::MAX).to_le_bytes())?;
        out.write_all(&(-i32::try_from(height).unwrap_or(i32::MAX)).to_le_bytes())?;
        out.write_all(&1_u16.to_le_bytes())?;
        out.write_all(&32_u16.to_le_bytes())?;
        out.write_all(&0_u32.to_le_bytes())?;
        out.write_all(&u32::try_from(expected).unwrap_or(u32::MAX).to_le_bytes())?;
        out.write_all(&[0_u8; 16])?;

        out.write_all(&pixels[..expected])?;
        out.flush().context("flushing")?;
        Ok(())
    }

    // --- arguments ---------------------------------------------------------

    struct Args {
        list: bool,
        encode: bool,
        software: bool,
        quality: Quality,
        monitor: Option<String>,
        window: Option<String>,
        seconds: u64,
        dump: usize,
        out: PathBuf,
    }

    impl Args {
        fn parse() -> Self {
            let mut args = Self {
                list: false,
                encode: false,
                software: false,
                quality: Quality::P1080,
                monitor: None,
                window: None,
                seconds: DEFAULT_SECONDS,
                dump: DEFAULT_DUMP,
                out: env::temp_dir().join("goodvoice-capture"),
            };

            let mut rest = env::args().skip(1);
            while let Some(flag) = rest.next() {
                match flag.as_str() {
                    "--list" => args.list = true,
                    "--encode" => args.encode = true,
                    // Exercises the fallback on a machine that has hardware,
                    // which is the only way to see that it warns.
                    "--software" => {
                        args.encode = true;
                        args.software = true;
                    }
                    "--720" => args.quality = Quality::P720,
                    "--1080" => args.quality = Quality::P1080,
                    "--monitor" => args.monitor = rest.next(),
                    "--window" => args.window = rest.next(),
                    "--seconds" => {
                        args.seconds = rest
                            .next()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(DEFAULT_SECONDS);
                    }
                    "--dump" => {
                        args.dump = rest
                            .next()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(DEFAULT_DUMP);
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
