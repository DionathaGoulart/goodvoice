//! What this machine will actually give us, measured instead of assumed.
//!
//! plan.md task 0.5. Two hardware unknowns the PRD leaves open, and both are
//! decisions somebody has to make on real silicon rather than from a
//! datasheet:
//!
//! 1. **How small a buffer WASAPI will run in shared mode.** That number is
//!    most of the ≤80 ms voice budget (prd.md §4), and it is what task 2.1
//!    weighs when it chooses between `cpal` and the `wasapi` crate — cpal takes
//!    the device's default period and never asks for the low-latency one, so
//!    the gap between the two figures below *is* the argument.
//! 2. **Which H.264 encoders are in hardware.** Task 5.2 needs a path that
//!    costs the game no frames, and a software encoder is not one.
//!
//! It changes nothing and installs nothing: it opens devices read-only, asks
//! them what they can do, and prints it in a shape that can be pasted straight
//! into a Decision Record.

#[cfg(not(windows))]
fn main() {
    // Not a silent success. Both questions are about Windows hardware, and a
    // probe that printed nothing on Linux would look like a machine with no
    // devices rather than the wrong machine (plan.md: do not fake or skip).
    eprintln!("the 0.5 probe answers questions about WASAPI and Media Foundation");
    eprintln!("and has to run on the Windows host it is asking about");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    windows_probe::run()
}

#[cfg(windows)]
mod windows_probe {
    use anyhow::{Context as _, Result};
    use windows::{
        core::{Interface as _, GUID, PWSTR},
        Win32::{
            Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
            Media::{
                Audio::{
                    eCapture, eRender, EDataFlow, IAudioClient, IAudioClient3, IMMDevice,
                    IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
                },
                MediaFoundation::{
                    IMFActivate, MFMediaType_Video, MFStartup, MFTEnumEx,
                    MFT_ENUM_HARDWARE_URL_Attribute, MFT_FRIENDLY_NAME_Attribute,
                    MFVideoFormat_H264, MFSTARTUP_NOSOCKET, MFT_CATEGORY_VIDEO_ENCODER,
                    MFT_ENUM_FLAG_ASYNCMFT, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER,
                    MFT_ENUM_FLAG_SYNCMFT, MFT_REGISTER_TYPE_INFO, MF_VERSION,
                },
            },
            System::Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
                STGM_READ,
            },
        },
    };

    /// A device period, in the 100-nanosecond units WASAPI counts in.
    const HNS_PER_MS: f64 = 10_000.0;

    pub fn run() -> Result<()> {
        // SAFETY: called once, before any other COM call on this thread, and
        // the process is single-threaded until it exits.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .context("initialising COM")?;

        println!("# goodvoice hardware probe (plan.md task 0.5)");
        println!();
        audio_endpoints()?;
        println!();
        video_encoders()?;

        Ok(())
    }

    // --- WASAPI ------------------------------------------------------------

    fn audio_endpoints() -> Result<()> {
        // SAFETY: COM is initialised and the CLSID matches the interface.
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .context("creating the device enumerator")?;

        for (flow, label) in [(eRender, "render"), (eCapture, "capture")] {
            println!("## {label} endpoints");
            println!();
            report_flow(&enumerator, flow)?;
            println!();
        }
        Ok(())
    }

    fn report_flow(enumerator: &IMMDeviceEnumerator, flow: EDataFlow) -> Result<()> {
        // SAFETY: the enumerator is live and both arguments are valid.
        let collection = unsafe { enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) }
            .context("enumerating endpoints")?;
        let count = unsafe { collection.GetCount() }.context("counting endpoints")?;

        if count == 0 {
            println!("_none active_");
            return Ok(());
        }

        for index in 0..count {
            let device = unsafe { collection.Item(index) }.context("opening an endpoint")?;
            match describe(&device) {
                Ok(report) => println!("{report}"),
                // One endpoint refusing to open is worth saying out loud and
                // not worth abandoning the other seven for.
                Err(error) => println!("- **(unreadable)** — {error}"),
            }
        }
        Ok(())
    }

    fn describe(device: &IMMDevice) -> Result<String> {
        let name = friendly_name(device).unwrap_or_else(|_| "(unnamed)".to_owned());

        // SAFETY: the device is live; `Activate` writes an interface pointer.
        let client: IAudioClient =
            unsafe { device.Activate(CLSCTX_ALL, None) }.context("activating the audio client")?;

        let format = unsafe { client.GetMixFormat() }.context("reading the mix format")?;
        // SAFETY: WASAPI returned a valid, non-null format block.
        let (rate, channels, bits) = unsafe {
            let format: &WAVEFORMATEX = &*format;
            (
                format.nSamplesPerSec,
                format.nChannels,
                format.wBitsPerSample,
            )
        };

        let mut default_period = 0_i64;
        let mut minimum_period = 0_i64;
        unsafe {
            client.GetDevicePeriod(Some(&raw mut default_period), Some(&raw mut minimum_period))
        }
        .context("reading the device period")?;

        // What `IAudioClient3` will run at, which is the number cpal never
        // asks for. The interface arrived in Windows 10; a device that does
        // not offer it simply has nothing extra to give.
        let low_latency = client
            .cast::<IAudioClient3>()
            .ok()
            .and_then(|client| shared_mode_periods(&client, format).ok());

        // SAFETY: `GetMixFormat` allocates with CoTaskMemAlloc and hands
        // ownership over; nothing reads the block after this.
        unsafe { CoTaskMemFree(Some(format.cast())) };

        let engine = low_latency.unwrap_or_else(|| "no IAudioClient3".to_owned());
        Ok(format!(
            "- **{name}** — {rate} Hz, {channels} ch, {bits}-bit; \
             default period {:.1} ms, minimum {:.1} ms; {engine}",
            ms(default_period),
            ms(minimum_period),
        ))
    }

    /// What `IAudioClient3` reports it can run at, in frames and milliseconds.
    fn shared_mode_periods(client: &IAudioClient3, format: *const WAVEFORMATEX) -> Result<String> {
        let mut default_frames = 0_u32;
        let mut fundamental = 0_u32;
        let mut min_frames = 0_u32;
        let mut max_frames = 0_u32;

        // SAFETY: the format block is the one WASAPI just handed back and is
        // still alive; every out-pointer is a live local.
        unsafe {
            client.GetSharedModeEnginePeriod(
                format,
                &raw mut default_frames,
                &raw mut fundamental,
                &raw mut min_frames,
                &raw mut max_frames,
            )
        }
        .context("reading the shared-mode engine period")?;

        // SAFETY: same block, still alive.
        let rate = f64::from(unsafe { (*format).nSamplesPerSec });
        let as_ms = |frames: u32| f64::from(frames) * 1000.0 / rate;

        Ok(format!(
            "IAudioClient3 default {default_frames} frames ({:.1} ms), \
             minimum {min_frames} ({:.1} ms), maximum {max_frames} ({:.1} ms)",
            as_ms(default_frames),
            as_ms(min_frames),
            as_ms(max_frames),
        ))
    }

    fn friendly_name(device: &IMMDevice) -> Result<String> {
        // SAFETY: the device is live and the property store is read-only.
        let store =
            unsafe { device.OpenPropertyStore(STGM_READ) }.context("opening the property store")?;
        let value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }
            .context("reading the friendly name")?;
        let name = value.to_string();
        Ok(name)
    }

    fn ms(hundred_nanos: i64) -> f64 {
        #[allow(
            clippy::cast_precision_loss,
            reason = "a device period is microseconds, nowhere near f64's mantissa"
        )]
        {
            hundred_nanos as f64 / HNS_PER_MS
        }
    }

    // --- Media Foundation --------------------------------------------------

    fn video_encoders() -> Result<()> {
        println!("## H.264 encoders");
        println!();

        // SAFETY: COM is initialised; the flag asks for no sockets.
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }
            .context("starting Media Foundation")?;

        let wanted = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };

        let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0_u32;
        // SAFETY: `MFTEnumEx` allocates the array and writes its length; both
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
        .context("enumerating video encoders")?;

        if count == 0 {
            println!("_none_ — task 5.2 has no hardware path on this machine");
        }

        for index in 0..count as usize {
            // SAFETY: `MFTEnumEx` wrote `count` entries into the array.
            let activate = unsafe { &*activates.add(index) };
            let Some(activate) = activate.as_ref() else {
                continue;
            };
            let name = attribute(activate, &MFT_FRIENDLY_NAME_Attribute)
                .unwrap_or_else(|| "(unnamed)".to_owned());
            // The hardware URL is only set on encoders backed by a device, so
            // its presence is the answer rather than the string itself.
            let hardware = attribute(activate, &MFT_ENUM_HARDWARE_URL_Attribute).is_some();
            println!(
                "- **{name}** — {}",
                if hardware { "hardware" } else { "software" }
            );
        }

        // SAFETY: the array came from CoTaskMemAlloc inside `MFTEnumEx`. The
        // activates themselves are released by their own `Drop`.
        unsafe { CoTaskMemFree(Some(activates.cast())) };
        Ok(())
    }

    fn attribute(activate: &IMFActivate, key: &GUID) -> Option<String> {
        let mut value = PWSTR::null();
        let mut length = 0_u32;
        // SAFETY: the activate is live; the call allocates the string and
        // writes its length.
        unsafe { activate.GetAllocatedString(key, &raw mut value, &raw mut length) }.ok()?;
        // SAFETY: on success the pointer is a live, null-terminated UTF-16
        // string that this call takes ownership of.
        let owned = unsafe { value.to_string() }.ok();
        unsafe { CoTaskMemFree(Some(value.as_ptr().cast())) };
        owned
    }
}
