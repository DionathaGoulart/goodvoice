//! H.264 encode, in silicon where the machine has silicon for it.
//!
//! plan.md task 5.2. The budget is prd.md §4's last and strictest: sharing a
//! screen must cost the game **~0 FPS**. A software H.264 encoder does not
//! meet that at 1080p30 and is not meant to — it is here as the thing that
//! keeps a share working on a machine without a hardware encoder, and
//! [`H264Encoder::is_hardware`] is how the caller knows to say so.
//!
//! # The path a frame takes
//!
//! ```text
//! WGC frame (BGRA8 texture) ── VideoProcessorBlt ──▶ NV12 texture
//!                                                        │
//!                                          MFCreateDXGISurfaceBuffer
//!                                                        ▼
//!                                              IMFTransform (NVENC/AMF/QSV)
//!                                                        │
//!                                                        ▼
//!                                            H.264 Annex B, on the CPU
//! ```
//!
//! Nothing crosses the bus until the bitstream comes out. The colour
//! conversion is a `VideoProcessorBlt` on the same device the capture is on,
//! and the encoder is handed the NV12 texture through a DXGI surface buffer
//! rather than a memory copy of it — that is what [`H264Encoder::encode`]
//! means by zero-copy, and it is the reason the encoder is created from the
//! *capture's* device rather than one of its own (DR-32).
//!
//! # Sync and async MFTs
//!
//! Hardware encoders are asynchronous MFTs and are driven differently from
//! software ones: unlock the async model, then follow `METransformNeedInput`
//! and `METransformHaveOutput` events instead of pushing and pulling at will.
//! [`H264Encoder`] hides the difference; both shapes come out as
//! [`Packet`]s.

use std::{sync::Once, time::Duration};

use windows::{
    core::{Interface as _, GUID, PWSTR},
    Win32::{
        Graphics::{
            Direct3D11::{
                ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
                ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor,
                ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
                ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET, D3D11_TEX2D_VPIV,
                D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
                D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
                D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
                D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
                D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
            },
            Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_RATIONAL},
        },
        Media::MediaFoundation::{
            IMFActivate, IMFDXGIDeviceManager, IMFMediaEventGenerator, IMFMediaType, IMFSample,
            IMFTransform, METransformDrainComplete, METransformHaveOutput, METransformNeedInput,
            MFCreateDXGIDeviceManager, MFCreateDXGISurfaceBuffer, MFCreateMediaType,
            MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFStartup, MFTEnumEx,
            MFT_ENUM_HARDWARE_URL_Attribute, MFT_FRIENDLY_NAME_Attribute, MFVideoFormat_H264,
            MFVideoFormat_NV12, MFVideoInterlace_Progressive,
            MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS, MFSTARTUP_NOSOCKET, MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_ASYNCMFT, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER,
            MFT_ENUM_FLAG_SYNCMFT, MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
            MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
            MFT_MESSAGE_SET_D3D_MANAGER, MFT_OUTPUT_DATA_BUFFER,
            MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
            MFT_REGISTER_TYPE_INFO, MF_EVENT_TYPE, MF_E_TRANSFORM_NEED_MORE_INPUT,
            MF_E_TRANSFORM_STREAM_CHANGE, MF_MT_ALL_SAMPLES_INDEPENDENT, MF_MT_AVG_BITRATE,
            MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
            MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_SA_D3D11_AWARE, MF_TRANSFORM_ASYNC,
            MF_TRANSFORM_ASYNC_UNLOCK, MF_VERSION,
        },
        System::Com::CoTaskMemFree,
    },
};

use super::CaptureError;

/// Media Foundation counts in 100-nanosecond units and so must anything
/// handing it a timestamp.
const HNS_PER_SECOND: i64 = 10_000_000;

/// How many NV12 textures the converter cycles through.
///
/// An asynchronous MFT can still hold the previous frame's surface when the
/// next one is converted, so one texture is one corrupted frame. Four is two
/// more than has ever been needed and costs 6 MB at 1080p.
const NV12_POOL: usize = 4;

/// Media Foundation is process-wide and starting it twice is a leak.
static MF_STARTUP: Once = Once::new();

fn start_media_foundation() -> Result<(), CaptureError> {
    let mut result = Ok(());
    MF_STARTUP.call_once(|| {
        // SAFETY: called exactly once per process; the flag asks for no
        // sockets, which a local encoder has no use for.
        if let Err(error) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) } {
            result = Err(CaptureError::Encoder(format!(
                "starting Media Foundation: {error}"
            )));
        }
    });
    result
}

/// One H.264 encoder Media Foundation is willing to hand out.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EncoderInfo {
    /// The MFT's friendly name, as the driver registered it.
    pub name: String,
    /// Whether it is backed by a device. This is the whole question: only a
    /// hardware encoder meets prd.md §4's ~0-FPS budget.
    pub hardware: bool,
    /// Whether it can take a D3D11 texture rather than a memory buffer.
    pub d3d11_aware: bool,
}

/// Every H.264 encoder on this machine, hardware first.
///
/// # Errors
///
/// [`CaptureError::Encoder`] if Media Foundation will not start or will not
/// enumerate.
pub fn encoders() -> Result<Vec<EncoderInfo>, CaptureError> {
    start_media_foundation()?;
    let (activates, count) = enumerate()?;

    let mut found = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        // SAFETY: `MFTEnumEx` wrote `count` entries into the array.
        let Some(activate) = (unsafe { &*activates.add(index) }).as_ref() else {
            continue;
        };
        found.push(describe(activate));
    }
    // SAFETY: the array itself came from CoTaskMemAlloc inside `MFTEnumEx`;
    // the activates in it are released by their own `Drop`.
    unsafe { CoTaskMemFree(Some(activates.cast())) };

    found.sort_by_key(|info| !info.hardware);
    Ok(found)
}

fn describe(activate: &IMFActivate) -> EncoderInfo {
    let name = attribute_string(activate, &MFT_FRIENDLY_NAME_Attribute)
        .unwrap_or_else(|| "(unnamed)".to_owned());
    // The hardware URL is only registered on an encoder backed by a device, so
    // its presence is the answer rather than the string in it.
    let hardware = attribute_string(activate, &MFT_ENUM_HARDWARE_URL_Attribute).is_some();
    // The remaining question cannot be answered from the activate — see
    // `is_d3d11_aware` and DR-32 — so listing it means activating it.
    // SAFETY: the activate is live; a driver that refuses to activate reports
    // an error rather than handing back a bad interface.
    let d3d11_aware = unsafe { activate.ActivateObject::<IMFTransform>() }
        .is_ok_and(|transform| is_d3d11_aware(&transform));
    EncoderInfo {
        name,
        hardware,
        d3d11_aware,
    }
}

/// Whether this transform will take a D3D11 texture rather than a memory
/// buffer.
///
/// `MF_SA_D3D11_AWARE` lives on the **transform's** attribute store, not on
/// the activate's. Read from the activate it comes back absent for every
/// encoder on the machine, including the ones that plainly are — which is how
/// a zero-copy path quietly becomes a copying one (DR-32).
fn is_d3d11_aware(transform: &IMFTransform) -> bool {
    // SAFETY: the transform is live; a transform with no attribute store, or
    // without this attribute, reports an error rather than a bad read.
    unsafe { transform.GetAttributes() }.is_ok_and(|attributes| {
        // SAFETY: the attribute store is live.
        unsafe { attributes.GetUINT32(&MF_SA_D3D11_AWARE) }.unwrap_or(0) != 0
    })
}

/// The raw `MFTEnumEx` array, which the caller must free.
fn enumerate() -> Result<(*mut Option<IMFActivate>, u32), CaptureError> {
    let wanted = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0_u32;
    // SAFETY: the call allocates the array and writes its length; both
    // out-pointers are live locals.
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE
                | MFT_ENUM_FLAG_SYNCMFT
                | MFT_ENUM_FLAG_ASYNCMFT
                | MFT_ENUM_FLAG_SORTANDFILTER,
            None,
            Some(&raw const wanted),
            &raw mut activates,
            &raw mut count,
        )
    }
    .map_err(|error| CaptureError::Encoder(format!("enumerating H.264 encoders: {error}")))?;
    Ok((activates, count))
}

fn attribute_string(activate: &IMFActivate, key: &GUID) -> Option<String> {
    let mut value = PWSTR::null();
    let mut length = 0_u32;
    // SAFETY: the activate is live; on success the call allocates the string
    // and writes its length.
    unsafe { activate.GetAllocatedString(key, &raw mut value, &raw mut length) }.ok()?;
    // SAFETY: the pointer is a live, null-terminated UTF-16 string this call
    // takes ownership of.
    let owned = unsafe { value.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(value.as_ptr().cast())) };
    owned
}

/// What to ask the encoder for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeConfig {
    /// Encoded width in pixels. Even, because NV12 is 4:2:0.
    pub width: u32,
    /// Encoded height in pixels. Even, for the same reason.
    pub height: u32,
    /// Frames per second the bitstream declares.
    pub fps: u32,
    /// Target bits per second.
    pub bitrate: u32,
}

/// Which encoders to consider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// Hardware first, software only if nothing else will take the format.
    /// What a share should always ask for.
    Auto,
    /// Skip every hardware encoder.
    ///
    /// Not a thing a share should want — it exists so the fallback path can
    /// be exercised on a machine that has hardware, which is every machine
    /// this has been developed on. Without it "the software path warns" is a
    /// claim nobody can check.
    SoftwareOnly,
}

/// One encoded access unit.
#[derive(Debug, Clone)]
pub struct Packet {
    /// The H.264 bytes, as the encoder produced them.
    pub bytes: Vec<u8>,
    /// The presentation time carried through from the frame that made it.
    pub time: Duration,
    /// Whether this packet can be decoded without any before it — an IDR.
    /// A viewer joining mid-share has to start at one (task 5.4).
    pub keyframe: bool,
}

/// A Media Foundation H.264 encoder, fed D3D11 textures.
///
/// Not `Send`: it holds COM interfaces and shares a device with the capture.
pub struct H264Encoder {
    transform: IMFTransform,
    /// `Some` for an asynchronous MFT, which every hardware one is.
    events: Option<IMFMediaEventGenerator>,
    name: String,
    hardware: bool,
    d3d11_aware: bool,
    config: EncodeConfig,
    converter: Nv12Converter,
    /// Held so the DXGI device manager outlives the transform that was handed
    /// it.
    _manager: IMFDXGIDeviceManager,
    frames: i64,
}

impl H264Encoder {
    /// Open the first encoder that will take this configuration, hardware
    /// first.
    ///
    /// `device` must be the device the frames being encoded live on — a
    /// texture cannot cross devices without a copy, which is the copy this
    /// whole path exists to avoid.
    ///
    /// # Errors
    ///
    /// [`CaptureError::NoHardwareEncoder`] if there is no H.264 encoder at
    /// all, [`CaptureError::Encoder`] if every candidate refused the
    /// configuration.
    pub fn open(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        config: EncodeConfig,
    ) -> Result<Self, CaptureError> {
        Self::open_with(device, context, config, Selection::Auto)
    }

    /// As [`H264Encoder::open`], but told which encoders to consider.
    ///
    /// # Errors
    ///
    /// As [`H264Encoder::open`], plus [`CaptureError::NoHardwareEncoder`] when
    /// [`Selection::SoftwareOnly`] leaves nothing.
    pub fn open_with(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        config: EncodeConfig,
        selection: Selection,
    ) -> Result<Self, CaptureError> {
        start_media_foundation()?;

        // Media Foundation drives the device from its own threads. Without
        // this the encoder and the capture race on the immediate context and
        // the corruption is intermittent, which is the worst kind.
        if let Ok(multithread) = device.cast::<ID3D11Multithread>() {
            // SAFETY: the interface is live; this only sets a flag.
            let _ = unsafe { multithread.SetMultithreadProtected(true) };
        }

        let manager = device_manager(device)?;
        let (activates, count) = enumerate()?;
        if count == 0 {
            // SAFETY: the array is allocated even when empty.
            unsafe { CoTaskMemFree(Some(activates.cast())) };
            return Err(CaptureError::NoHardwareEncoder);
        }

        let mut last: Option<CaptureError> = None;
        let mut opened = None;
        for index in 0..count as usize {
            // SAFETY: `MFTEnumEx` wrote `count` entries.
            let Some(activate) = (unsafe { &*activates.add(index) }).as_ref() else {
                continue;
            };
            let info = describe(activate);
            if selection == Selection::SoftwareOnly && info.hardware {
                continue;
            }
            match Self::try_open(activate, &info, device, context, &manager, config) {
                Ok(encoder) => {
                    opened = Some(encoder);
                    break;
                }
                Err(error) => last = Some(error),
            }
        }
        // SAFETY: as above — the array is ours to free, the activates are not.
        unsafe { CoTaskMemFree(Some(activates.cast())) };

        opened.ok_or_else(|| {
            last.unwrap_or_else(|| CaptureError::Encoder("no encoder accepted the format".into()))
        })
    }

    fn try_open(
        activate: &IMFActivate,
        info: &EncoderInfo,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        manager: &IMFDXGIDeviceManager,
        config: EncodeConfig,
    ) -> Result<Self, CaptureError> {
        // SAFETY: the activate is live and `IMFTransform` is what a video
        // encoder activates into.
        let transform = unsafe { activate.ActivateObject::<IMFTransform>() }
            .map_err(|error| CaptureError::Encoder(format!("activating {}: {error}", info.name)))?;

        // SAFETY: the transform is live; a missing attribute store comes back
        // as an error.
        let attributes = unsafe { transform.GetAttributes() }.ok();
        let asynchronous = attributes.as_ref().is_some_and(|attributes| {
            // SAFETY: the attribute store is live.
            unsafe { attributes.GetUINT32(&MF_TRANSFORM_ASYNC) }.unwrap_or(0) != 0
        });
        if asynchronous {
            let attributes = attributes
                .as_ref()
                .ok_or_else(|| CaptureError::Encoder("async MFT with no attributes".into()))?;
            // Until this is set the transform refuses every call with
            // E_ILLEGAL_METHOD_CALL, and the error does not say why.
            // SAFETY: the attribute store is live.
            unsafe { attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1) }.map_err(|error| {
                CaptureError::Encoder(format!("unlocking {}: {error}", info.name))
            })?;
        }

        let d3d11_aware = is_d3d11_aware(&transform);
        if d3d11_aware {
            // SAFETY: the transform is live; the message takes the manager as
            // a raw `IUnknown` pointer it does not take ownership of, and
            // `_manager` below keeps it alive for as long as the transform.
            unsafe {
                transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)
            }
            .map_err(|error| {
                CaptureError::Encoder(format!("handing {} the device: {error}", info.name))
            })?;
        }

        // Output first, then input: an H.264 encoder cannot say what it takes
        // until it knows what it is being asked to produce.
        let output = output_type(config)?;
        // SAFETY: both the transform and the media type are live.
        unsafe { transform.SetOutputType(0, &output, 0) }.map_err(|error| {
            CaptureError::Encoder(format!("{} refused the output format: {error}", info.name))
        })?;

        let input = input_type(config)?;
        // SAFETY: as above.
        unsafe { transform.SetInputType(0, &input, 0) }.map_err(|error| {
            CaptureError::Encoder(format!("{} refused NV12 input: {error}", info.name))
        })?;

        let converter = Nv12Converter::new(device, context, config)?;

        // SAFETY: the transform is live and both messages take no parameter.
        unsafe {
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .and_then(|()| transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0))
        }
        .map_err(|error| CaptureError::Encoder(format!("starting {}: {error}", info.name)))?;

        let events = if asynchronous {
            Some(
                transform
                    .cast::<IMFMediaEventGenerator>()
                    .map_err(|error| {
                        CaptureError::Encoder(format!("{} has no event queue: {error}", info.name))
                    })?,
            )
        } else {
            None
        };

        Ok(Self {
            transform,
            events,
            name: info.name.clone(),
            hardware: info.hardware,
            d3d11_aware,
            config,
            converter,
            _manager: manager.clone(),
            frames: 0,
        })
    }

    /// The encoder that was actually opened.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether that encoder is in silicon.
    ///
    /// `false` means the share will work and will cost the machine frames.
    /// The caller is expected to say so out loud rather than quietly ship a
    /// software encode (plan.md task 5.2).
    #[must_use]
    pub fn is_hardware(&self) -> bool {
        self.hardware
    }

    /// Whether the encoder took the D3D11 device, and with it the textures.
    ///
    /// `false` means Media Foundation is copying every frame out of GPU memory
    /// on the way in. It still encodes; it is no longer zero-copy, and it is
    /// no longer inside task 5.5's budget.
    #[must_use]
    pub fn is_zero_copy(&self) -> bool {
        self.d3d11_aware
    }

    /// What the encoder was configured for.
    #[must_use]
    pub fn config(&self) -> EncodeConfig {
        self.config
    }

    /// Encode one captured frame, appending whatever comes out to `out`.
    ///
    /// An encoder is allowed to answer with nothing — it holds frames while it
    /// fills its pipeline — so an empty append is normal and not a failure.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Encoder`] if the colour conversion or the transform
    /// fails.
    pub fn encode(
        &mut self,
        texture: &ID3D11Texture2D,
        time: Duration,
        out: &mut Vec<Packet>,
    ) -> Result<(), CaptureError> {
        let nv12 = self.converter.convert(texture)?;
        let sample = self.sample(&nv12, time)?;
        self.frames += 1;

        if let Some(events) = self.events.clone() {
            return self.feed_async(&events, &sample, out);
        }
        // SAFETY: the transform is live and the sample is fully built.
        unsafe { self.transform.ProcessInput(0, &sample, 0) }
            .map_err(|error| CaptureError::Encoder(format!("feeding input: {error}")))?;
        self.collect(out)
    }

    /// Wrap the NV12 texture in a sample without copying it.
    fn sample(&self, texture: &ID3D11Texture2D, time: Duration) -> Result<IMFSample, CaptureError> {
        // SAFETY: the texture is live; the buffer takes a reference to it and
        // the surface stays valid for as long as the buffer.
        let buffer = unsafe { MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false) }
            .map_err(|error| CaptureError::Encoder(format!("wrapping the surface: {error}")))?;

        // SAFETY: no arguments; returns a fresh empty sample.
        let sample = unsafe { MFCreateSample() }
            .map_err(|error| CaptureError::Encoder(format!("creating a sample: {error}")))?;

        // i64 of 100 ns units runs out after 29 000 years, so the saturation
        // is theatre; `try_from` is how it is said without an `as`.
        let stamp = i64::try_from(time.as_nanos() / 100).unwrap_or(i64::MAX);
        let duration = HNS_PER_SECOND / i64::from(self.config.fps.max(1));

        // SAFETY: sample and buffer are both live and freshly created.
        unsafe {
            sample
                .AddBuffer(&buffer)
                .and_then(|()| sample.SetSampleTime(stamp))
                .and_then(|()| sample.SetSampleDuration(duration))
        }
        .map_err(|error| CaptureError::Encoder(format!("building a sample: {error}")))?;
        Ok(sample)
    }

    /// Drive an asynchronous MFT until it has taken this frame.
    ///
    /// The transform decides when it wants input. Everything it offers before
    /// then is output from earlier frames, and dropping it on the floor would
    /// be dropping the bitstream.
    fn feed_async(
        &mut self,
        events: &IMFMediaEventGenerator,
        sample: &IMFSample,
        out: &mut Vec<Packet>,
    ) -> Result<(), CaptureError> {
        loop {
            // SAFETY: the event generator is live; a zero flag blocks until
            // there is an event, which an MFT being fed always eventually has.
            let event = unsafe { events.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0)) }
                .map_err(|error| {
                    CaptureError::Encoder(format!("waiting on the encoder: {error}"))
                })?;
            // SAFETY: the event is live.
            let kind = unsafe { event.GetType() }
                .map_err(|error| CaptureError::Encoder(format!("reading an event: {error}")))?;

            // Compared rather than matched: these are `const`s with WinRT's
            // own casing, and a `match` arm would bind a new variable instead
            // of testing against them.
            let kind = MF_EVENT_TYPE(i32::try_from(kind).unwrap_or_default());
            if kind == METransformNeedInput {
                // SAFETY: transform and sample are both live.
                unsafe { self.transform.ProcessInput(0, sample, 0) }
                    .map_err(|error| CaptureError::Encoder(format!("feeding input: {error}")))?;
                return Ok(());
            } else if kind == METransformHaveOutput {
                self.collect(out)?;
            }
        }
    }

    /// Take everything the transform is holding.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Encoder`] if a packet cannot be read off a sample.
    pub fn collect(&mut self, out: &mut Vec<Packet>) -> Result<(), CaptureError> {
        // An asynchronous MFT offers exactly one sample per
        // `METransformHaveOutput`, and asking a second time is not "nothing
        // to give" — it is `E_UNEXPECTED` (DR-32). Only a synchronous one is
        // drained in a loop.
        if self.events.is_some() {
            return self.take_one(out).map(|_| ());
        }
        while self.take_one(out)? {}
        Ok(())
    }

    /// One `ProcessOutput`. `Ok(true)` if it produced a packet.
    fn take_one(&mut self, out: &mut Vec<Packet>) -> Result<bool, CaptureError> {
        {
            // A transform that does not allocate its own output samples needs
            // one supplied, and an encoder refused an output buffer then
            // refuses the *next input* — which is where it surfaces, several
            // calls away from its cause (DR-32).
            let supplied = self.output_sample()?;
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(supplied),
                ..Default::default()
            }];
            let mut status = 0_u32;
            // SAFETY: the transform is live; the buffer array is a live local
            // and the transform either writes a sample into it or fills the
            // one supplied.
            let taken = unsafe {
                self.transform
                    .ProcessOutput(0, &mut buffers, &raw mut status)
            };

            let sample =
                std::mem::replace(&mut buffers[0].pSample, std::mem::ManuallyDrop::new(None));
            let sample = std::mem::ManuallyDrop::into_inner(sample);
            // Same for the event collection the transform may attach.
            let events =
                std::mem::replace(&mut buffers[0].pEvents, std::mem::ManuallyDrop::new(None));
            drop(std::mem::ManuallyDrop::into_inner(events));

            match taken {
                Ok(()) => {}
                // "Nothing to give yet", which is not an error: it is how a
                // pipeline fills and how every drain ends.
                Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(false),
                // The encoder renegotiated its output. Nothing here changes
                // format mid-stream, so it is worth saying rather than
                // swallowing.
                Err(error) if error.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    return Err(CaptureError::Encoder(
                        "the encoder changed its output format mid-stream".into(),
                    ))
                }
                Err(error) => return Err(CaptureError::Encoder(format!("taking output: {error}"))),
            }
            let Some(sample) = sample else {
                return Ok(false);
            };
            out.push(read_packet(&sample)?);
            Ok(true)
        }
    }

    /// An output sample, for transforms that do not allocate their own.
    ///
    /// `None` for the ones that do — every hardware encoder here — because
    /// supplying a buffer to a transform that allocates is an error rather
    /// than a courtesy.
    fn output_sample(&self) -> Result<Option<IMFSample>, CaptureError> {
        // SAFETY: the transform is live and stream 0 is the only one.
        let info = unsafe { self.transform.GetOutputStreamInfo(0) }
            .map_err(|error| CaptureError::Encoder(format!("sizing the output: {error}")))?;
        let provides = MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0.unsigned_abs()
            | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0.unsigned_abs();
        if info.dwFlags & provides != 0 {
            return Ok(None);
        }

        // SAFETY: neither call takes an interface; both return a fresh object.
        let sample = unsafe { MFCreateSample() }
            .map_err(|error| CaptureError::Encoder(format!("output sample: {error}")))?;
        // SAFETY: as above.
        let buffer = unsafe { MFCreateMemoryBuffer(info.cbSize.max(1)) }
            .map_err(|error| CaptureError::Encoder(format!("output buffer: {error}")))?;
        // SAFETY: both are live and freshly created.
        unsafe { sample.AddBuffer(&buffer) }
            .map_err(|error| CaptureError::Encoder(format!("output sample: {error}")))?;
        Ok(Some(sample))
    }

    /// Flush the encoder and take the last packets out of it.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Encoder`] if the transform refuses to drain.
    pub fn drain(&mut self, out: &mut Vec<Packet>) -> Result<(), CaptureError> {
        // SAFETY: the transform is live; neither message takes a parameter.
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
                .and_then(|()| self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0))
        }
        .map_err(|error| CaptureError::Encoder(format!("draining: {error}")))?;

        let Some(events) = self.events.clone() else {
            return self.collect(out);
        };
        loop {
            // SAFETY: the event generator is live and a drain always ends in
            // `METransformDrainComplete`.
            let event = unsafe { events.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0)) }
                .map_err(|error| CaptureError::Encoder(format!("waiting on the drain: {error}")))?;
            // SAFETY: the event is live.
            let kind = unsafe { event.GetType() }
                .map_err(|error| CaptureError::Encoder(format!("reading an event: {error}")))?;
            let kind = MF_EVENT_TYPE(i32::try_from(kind).unwrap_or_default());
            if kind == METransformHaveOutput {
                self.collect(out)?;
            } else if kind == METransformDrainComplete {
                return Ok(());
            }
        }
    }
}

/// Copy one encoded sample out of Media Foundation's memory.
///
/// This is the one copy on the path, and it is the right one: the bitstream is
/// kilobytes and it has to reach the network stack as bytes anyway.
fn read_packet(sample: &IMFSample) -> Result<Packet, CaptureError> {
    // SAFETY: the sample is live and came from the transform.
    let buffer = unsafe { sample.ConvertToContiguousBuffer() }
        .map_err(|error| CaptureError::Encoder(format!("flattening a sample: {error}")))?;

    let mut data = std::ptr::null_mut();
    let mut length = 0_u32;
    // SAFETY: the buffer is live; `Lock` hands back a pointer valid until
    // `Unlock`, and the two are paired below.
    unsafe { buffer.Lock(&raw mut data, None, Some(&raw mut length)) }
        .map_err(|error| CaptureError::Encoder(format!("locking a sample: {error}")))?;
    // SAFETY: `Lock` succeeded, so `data` points at `length` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data, length as usize) }.to_vec();
    // SAFETY: paired with the `Lock` above.
    let _ = unsafe { buffer.Unlock() };

    // SAFETY: the sample is live; a sample with no time reports an error
    // rather than a bad read.
    let time = unsafe { sample.GetSampleTime() }.map_or(Duration::ZERO, |stamp| {
        Duration::from_nanos(stamp.unsigned_abs() * 100)
    });
    // Set on every sample that can be decoded on its own. Absent is the
    // common case and means a P-frame, not a failure.
    // SAFETY: the sample is live.
    let keyframe = unsafe { sample.GetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT) }.unwrap_or(0) != 0
        || starts_with_idr(&bytes);

    Ok(Packet {
        bytes,
        time,
        keyframe,
    })
}

/// Whether an Annex B access unit carries an IDR slice or a parameter set.
///
/// Read from the bitstream rather than from the sample's attributes because
/// the attribute is not set by every encoder, and a viewer that guesses wrong
/// about where it can start (task 5.4) shows a grey rectangle.
fn starts_with_idr(bytes: &[u8]) -> bool {
    let mut index = 0;
    while index + 4 <= bytes.len() {
        // Annex B start codes are 00 00 01 or 00 00 00 01.
        let (offset, header) = match bytes[index..] {
            [0, 0, 1, header, ..] => (4, header),
            [0, 0, 0, 1, header, ..] => (5, header),
            _ => {
                index += 1;
                continue;
            }
        };
        // 5 = IDR slice, 7 = SPS, 8 = PPS. Any of the three means a decoder
        // can start here.
        if matches!(header & 0x1f, 5 | 7 | 8) {
            return true;
        }
        index += offset;
    }
    false
}

/// The `DXGI` device manager an MFT needs before it will take textures.
fn device_manager(device: &ID3D11Device) -> Result<IMFDXGIDeviceManager, CaptureError> {
    let mut token = 0_u32;
    let mut manager = None;
    // SAFETY: both out-parameters are live locals.
    unsafe { MFCreateDXGIDeviceManager(&raw mut token, &raw mut manager) }
        .map_err(|error| CaptureError::Encoder(format!("creating a device manager: {error}")))?;
    let manager =
        manager.ok_or_else(|| CaptureError::Encoder("device manager came back null".into()))?;
    // SAFETY: the manager is live and `token` is the one it just issued.
    unsafe { manager.ResetDevice(device, token) }
        .map_err(|error| CaptureError::Encoder(format!("binding the device: {error}")))?;
    Ok(manager)
}

fn output_type(config: EncodeConfig) -> Result<IMFMediaType, CaptureError> {
    let media = new_media_type(config, &MFVideoFormat_H264)?;
    // SAFETY: the media type is live and both keys take a UINT32.
    unsafe { media.SetUINT32(&MF_MT_AVG_BITRATE, config.bitrate) }
        .map_err(|error| CaptureError::Encoder(format!("setting the bitrate: {error}")))?;
    Ok(media)
}

fn input_type(config: EncodeConfig) -> Result<IMFMediaType, CaptureError> {
    new_media_type(config, &MFVideoFormat_NV12)
}

fn new_media_type(config: EncodeConfig, subtype: &GUID) -> Result<IMFMediaType, CaptureError> {
    // SAFETY: no arguments; returns a fresh empty media type.
    let media = unsafe { MFCreateMediaType() }
        .map_err(|error| CaptureError::Encoder(format!("creating a media type: {error}")))?;
    // SAFETY: the media type is live and every key below matches its value's
    // type — the attribute store rejects a mismatch rather than misreading it.
    unsafe {
        media
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .and_then(|()| media.SetGUID(&MF_MT_SUBTYPE, subtype))
            .and_then(|()| media.SetUINT64(&MF_MT_FRAME_SIZE, pack(config.width, config.height)))
            .and_then(|()| media.SetUINT64(&MF_MT_FRAME_RATE, pack(config.fps, 1)))
            .and_then(|()| media.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)))
            .and_then(|()| {
                media.SetUINT32(
                    &MF_MT_INTERLACE_MODE,
                    MFVideoInterlace_Progressive.0.unsigned_abs(),
                )
            })
    }
    .map_err(|error| CaptureError::Encoder(format!("describing the format: {error}")))?;
    Ok(media)
}

/// Media Foundation packs a pair of `u32` into one `u64` attribute, high word
/// first. Size, frame rate and aspect ratio all use it.
const fn pack(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | low as u64
}

// --- BGRA to NV12, on the GPU ------------------------------------------------

/// A `D3D11` video processor doing the one thing the encoder cannot do itself.
///
/// Hardware H.264 encoders take NV12 and WGC produces BGRA8 (DR-31). The
/// conversion is a `VideoProcessorBlt` on the capture's own device, so the
/// pixels never leave the GPU between the two.
struct Nv12Converter {
    context: ID3D11VideoContext,
    processor: ID3D11VideoProcessor,
    enumerator: ID3D11VideoProcessorEnumerator,
    device: ID3D11VideoDevice,
    pool: Vec<ID3D11Texture2D>,
    outputs: Vec<ID3D11VideoProcessorOutputView>,
    next: usize,
}

impl Nv12Converter {
    fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        config: EncodeConfig,
    ) -> Result<Self, CaptureError> {
        let video_device = device
            .cast::<ID3D11VideoDevice>()
            .map_err(|error| CaptureError::Encoder(format!("no video device: {error}")))?;
        let video_context = context
            .cast::<ID3D11VideoContext>()
            .map_err(|error| CaptureError::Encoder(format!("no video context: {error}")))?;

        let rate = DXGI_RATIONAL {
            Numerator: config.fps.max(1),
            Denominator: 1,
        };
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: config.width,
            InputHeight: config.height,
            OutputFrameRate: rate,
            OutputWidth: config.width,
            OutputHeight: config.height,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        // SAFETY: the video device is live and `desc` is fully initialised.
        let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&raw const desc) }
            .map_err(|error| {
                CaptureError::Encoder(format!("no video processor for this size: {error}"))
            })?;
        // SAFETY: the enumerator is live; index 0 is the default rate
        // conversion, which every driver offers.
        let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }
            .map_err(|error| CaptureError::Encoder(format!("creating the converter: {error}")))?;

        let mut converter = Self {
            context: video_context,
            processor,
            enumerator,
            device: video_device,
            pool: Vec::with_capacity(NV12_POOL),
            outputs: Vec::with_capacity(NV12_POOL),
            next: 0,
        };
        for _ in 0..NV12_POOL {
            converter.add_target(device, config)?;
        }
        Ok(converter)
    }

    /// One NV12 texture and the output view that writes into it.
    fn add_target(
        &mut self,
        device: &ID3D11Device,
        config: EncodeConfig,
    ) -> Result<(), CaptureError> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: config.width,
            Height: config.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0.unsigned_abs(),
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        // SAFETY: the device is live, the description is fully initialised,
        // and there is no initial data.
        unsafe { device.CreateTexture2D(&raw const desc, None, Some(&raw mut texture)) }
            .map_err(|error| CaptureError::Encoder(format!("creating an NV12 texture: {error}")))?;
        let texture =
            texture.ok_or_else(|| CaptureError::Encoder("NV12 texture came back null".into()))?;

        let view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut view = None;
        // SAFETY: every argument is live and the description matches a 2D
        // texture, which is what was just created.
        unsafe {
            self.device.CreateVideoProcessorOutputView(
                &texture,
                &self.enumerator,
                &raw const view_desc,
                Some(&raw mut view),
            )
        }
        .map_err(|error| CaptureError::Encoder(format!("creating an output view: {error}")))?;
        let view =
            view.ok_or_else(|| CaptureError::Encoder("output view came back null".into()))?;

        self.pool.push(texture);
        self.outputs.push(view);
        Ok(())
    }

    /// Convert one BGRA texture, returning the NV12 one it landed in.
    fn convert(&mut self, source: &ID3D11Texture2D) -> Result<ID3D11Texture2D, CaptureError> {
        let slot = self.next % self.pool.len().max(1);
        self.next = self.next.wrapping_add(1);

        let input_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let mut input = None;
        // SAFETY: the source texture and the enumerator are live and the
        // description matches a 2D texture.
        unsafe {
            self.device.CreateVideoProcessorInputView(
                source,
                &self.enumerator,
                &raw const input_desc,
                Some(&raw mut input),
            )
        }
        .map_err(|error| CaptureError::Encoder(format!("creating an input view: {error}")))?;
        let input: ID3D11VideoProcessorInputView =
            input.ok_or_else(|| CaptureError::Encoder("input view came back null".into()))?;

        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(input.clone())),
            ..Default::default()
        };
        // SAFETY: processor, output view and input view are all live and all
        // came from the same enumerator.
        let blitted = unsafe {
            self.context
                .VideoProcessorBlt(&self.processor, &self.outputs[slot], 0, &[stream])
        };
        blitted.map_err(|error| CaptureError::Encoder(format!("converting to NV12: {error}")))?;

        Ok(self.pool[slot].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_size_packs_high_word_first() {
        // The halves do not overlap, so the packing is an addition written
        // as a shift — which is what makes the expected values readable.
        assert_eq!(pack(1920, 1080), (1920_u64 << 32) + 1080);
        assert_eq!(pack(30, 1), (30_u64 << 32) + 1);
        assert_eq!(pack(0, 0), 0);
        assert_eq!(pack(u32::MAX, u32::MAX), u64::MAX);
    }

    #[test]
    fn an_idr_is_found_behind_either_start_code() {
        // Four-byte start code, NAL header 0x65: IDR slice.
        assert!(starts_with_idr(&[0, 0, 0, 1, 0x65, 0xff]));
        // Three-byte start code, NAL header 0x67: SPS.
        assert!(starts_with_idr(&[0, 0, 1, 0x67, 0x42]));
        // 0x41 is a non-IDR slice: a P-frame, and not a place to start.
        assert!(!starts_with_idr(&[0, 0, 0, 1, 0x41, 0x9a, 0x00]));
        assert!(!starts_with_idr(&[]));
    }

    #[test]
    fn an_sps_after_a_p_slice_still_counts() {
        let bytes = [0, 0, 0, 1, 0x41, 0x9a, 0, 0, 0, 1, 0x67, 0x42];
        assert!(starts_with_idr(&bytes));
    }

    #[test]
    fn this_machine_lists_its_encoders_hardware_first() {
        let Ok(found) = encoders() else { return };
        let mut seen_software = false;
        for info in &found {
            if info.hardware {
                assert!(!seen_software, "hardware must sort before software");
            } else {
                seen_software = true;
            }
            assert!(!info.name.is_empty());
        }
    }
}
