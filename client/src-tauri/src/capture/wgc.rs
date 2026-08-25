//! Screen capture through Windows.Graphics.Capture.
//!
//! plan.md task 5.1. Two things live here: [`monitors`] and [`windows`], which
//! say what there is to capture, and [`Capturer`], which captures one of them.
//!
//! **Frames never leave the GPU on this path.** WGC hands back an
//! `IDirect3DSurface` backed by a texture on the same D3D11 device the frame
//! pool was created with, and [`Frame::texture`] is that texture. Task 5.2's
//! encoder takes it as-is; [`Frame::copy_to_cpu`] exists for the spike and for
//! tests, and is the one thing here that costs a readback.
//!
//! **Threading.** The frame pool is free-threaded, so WGC calls `FrameArrived`
//! on one of its own threads. The handler installed here does nothing but
//! signal — the thread that owns the [`Capturer`] is what calls
//! `TryGetNextFrame`, on its own schedule. That keeps every D3D11 call on one
//! thread (the immediate context is not free-threaded either), and it keeps
//! the pool's buffers from being held hostage by a slow consumer: a frame not
//! taken is a frame recycled, which is the drop policy a live share wants.
//!
//! **What WGC does not do is tick.** It delivers a frame when the content
//! changes, and nothing at all while a screen holds still. A capturer waiting
//! on a static desktop is working correctly and producing zero frames a
//! second; see [`Capturer::next_frame`] and DR-31.

use std::{
    cell::RefCell,
    sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    time::Duration,
};

use windows::{
    core::{Interface as _, BOOL},
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::{
            Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
            GraphicsCaptureSession,
        },
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
        SizeInt32,
    },
    Win32::{
        Foundation::{HMODULE, HWND, LPARAM, RECT, TRUE},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
                D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
            },
            Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED},
            Dxgi::{Common::DXGI_FORMAT, IDXGIDevice},
            Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW},
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
            IsIconic, IsWindowVisible, GWL_EXSTYLE, MONITORINFOF_PRIMARY, WS_EX_TOOLWINDOW,
        },
    },
};

use super::CaptureError;

/// How many textures the frame pool cycles through.
///
/// Two is the documented minimum for a pool that is not being drawn from a
/// swap chain, and more only buys latency: a consumer that has fallen two
/// frames behind wants the newest one, not a queue of stale ones.
const POOL_BUFFERS: i32 = 2;

/// What a [`Target`] points at.
///
/// The distinction survives into the picker (task 5.3): a monitor is a fixed
/// rectangle a viewer can rely on, a window moves, resizes and can vanish
/// mid-share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    /// A whole display.
    Monitor,
    /// One top-level window, wherever it happens to be.
    Window,
}

/// Something that can be captured, in a shape that crosses a thread and the
/// Tauri command boundary.
///
/// `handle` is an `HMONITOR` or an `HWND` depending on `kind`. Both are
/// process-local and neither is `Send`, which is why this carries the integer
/// rather than the handle type: the picker lists these, the UI serialises
/// them, and [`Capturer::start`] turns one back into a handle on the thread
/// that will do the capturing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Target {
    /// Monitor or window.
    pub kind: TargetKind,
    /// The `HMONITOR` or `HWND` as an integer.
    pub handle: isize,
    /// What to show a person choosing between these.
    pub name: String,
    /// Width in physical pixels, at the moment of enumeration.
    pub width: u32,
    /// Height in physical pixels, at the moment of enumeration.
    pub height: u32,
}

/// Whether the mouse pointer is drawn into the captured frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// Draw it. What a person sharing a screen almost always means.
    Shown,
    /// Leave it out.
    Hidden,
}

/// Is Windows.Graphics.Capture present and permitted on this machine?
///
/// False on Windows 10 before 1903, and on a machine where policy has turned
/// capture off. Worth asking before offering to share rather than after.
#[must_use]
pub fn is_supported() -> bool {
    GraphicsCaptureSession::IsSupported().unwrap_or(false)
}

// --- what there is to capture ------------------------------------------------

/// Every active display, primary first.
///
/// # Errors
///
/// [`CaptureError::Enumerate`] if `EnumDisplayMonitors` fails outright. A
/// single monitor that will not describe itself is skipped, not fatal.
pub fn monitors() -> Result<Vec<Target>, CaptureError> {
    let mut found: Vec<(bool, Target)> = Vec::new();
    // SAFETY: `collect_monitor` has the signature `MONITORENUMPROC` names, and
    // the `LPARAM` carries a pointer to `found`, which outlives the call —
    // `EnumDisplayMonitors` is synchronous and does not retain it.
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(std::ptr::from_mut(&mut found) as isize),
        )
    };
    if !ok.as_bool() {
        return Err(CaptureError::Enumerate("EnumDisplayMonitors failed".into()));
    }

    // Primary first: it is what a person means by "my screen" nine times out
    // of ten.
    found.sort_by_key(|(primary, _)| !primary);
    Ok(found.into_iter().map(|(_, target)| target).collect())
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _dc: HDC,
    _clip: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    // SAFETY: `monitors` passed a pointer to a live `Vec` and does not touch
    // it again until this enumeration has finished.
    let found = unsafe { &mut *(lparam.0 as *mut Vec<(bool, Target)>) };

    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = u32::try_from(size_of::<MONITORINFOEXW>()).unwrap_or_default();
    // SAFETY: `info` is a live, correctly sized `MONITORINFOEXW`; the callee
    // is told its size through `cbSize`.
    let described = unsafe { GetMonitorInfoW(monitor, std::ptr::addr_of_mut!(info).cast()) };
    if !described.as_bool() {
        // One display refusing to describe itself is not a reason to abandon
        // the others. Keep enumerating.
        return TRUE;
    }

    let rect = info.monitorInfo.rcMonitor;
    let primary = info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0;
    let device = String::from_utf16_lossy(
        &info.szDevice[..info.szDevice.iter().position(|c| *c == 0).unwrap_or(0)],
    );

    let width = rect.right.saturating_sub(rect.left);
    let height = rect.bottom.saturating_sub(rect.top);
    let name = if primary {
        format!("{device} ({width}×{height}, primary)")
    } else {
        format!("{device} ({width}×{height})")
    };

    found.push((
        primary,
        Target {
            kind: TargetKind::Monitor,
            handle: monitor.0 as isize,
            name,
            width: width.unsigned_abs(),
            height: height.unsigned_abs(),
        },
    ));
    TRUE
}

/// Every top-level window a person could plausibly mean to share.
///
/// Filtered down from what `EnumWindows` returns, because most of that list is
/// not shareable and none of it is nameable: invisible windows, tool windows,
/// minimised windows (WGC has nothing to capture from one), and the cloaked
/// shells that back every UWP app and every virtual desktop the user is not
/// looking at.
///
/// # Errors
///
/// [`CaptureError::Enumerate`] if `EnumWindows` fails outright.
pub fn windows() -> Result<Vec<Target>, CaptureError> {
    let mut found: Vec<Target> = Vec::new();
    // SAFETY: same contract as `monitors` — a synchronous enumeration handed a
    // pointer to a `Vec` that outlives it.
    unsafe {
        EnumWindows(
            Some(collect_window),
            LPARAM(std::ptr::from_mut(&mut found) as isize),
        )
    }
    .map_err(|error| CaptureError::Enumerate(error.to_string()))?;
    Ok(found)
}

unsafe extern "system" fn collect_window(window: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: as `collect_monitor`.
    let found = unsafe { &mut *(lparam.0 as *mut Vec<Target>) };
    if let Some(target) = describe_window(window) {
        found.push(target);
    }
    TRUE
}

/// `Some` if this window is worth offering, `None` for everything filtered out.
fn describe_window(window: HWND) -> Option<Target> {
    // SAFETY: every call below reads a property of a window handle Windows
    // just handed us, and none of them retains anything.
    unsafe {
        if !IsWindowVisible(window).as_bool() || IsIconic(window).as_bool() {
            return None;
        }

        // Tool windows are palettes and tooltips: they exist, they are
        // visible, and nobody means them by "share a window".
        let extended = u32::try_from(GetWindowLongW(window, GWL_EXSTYLE)).unwrap_or_default();
        if extended & WS_EX_TOOLWINDOW.0 != 0 {
            return None;
        }

        // Cloaked is the one that matters most: DWM keeps a window for every
        // suspended UWP app and every virtual desktop you are not on, and all
        // of them are visible, titled and unshareable.
        let mut cloaked = 0_u32;
        let asked = DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            std::ptr::addr_of_mut!(cloaked).cast(),
            u32::try_from(size_of::<u32>()).unwrap_or_default(),
        );
        if asked.is_ok() && cloaked != 0 {
            return None;
        }

        let length = GetWindowTextLengthW(window);
        if length <= 0 {
            return None;
        }
        let mut title = vec![0_u16; usize::try_from(length).unwrap_or_default() + 1];
        let written = GetWindowTextW(window, &mut title);
        if written <= 0 {
            return None;
        }
        let name = String::from_utf16_lossy(&title[..usize::try_from(written).unwrap_or_default()]);

        let mut rect = RECT::default();
        GetWindowRect(window, &raw mut rect).ok()?;
        let width = rect.right.saturating_sub(rect.left);
        let height = rect.bottom.saturating_sub(rect.top);
        if width <= 0 || height <= 0 {
            return None;
        }

        Some(Target {
            kind: TargetKind::Window,
            handle: window.0 as isize,
            name,
            width: width.unsigned_abs(),
            height: height.unsigned_abs(),
        })
    }
}

// --- capturing one of them ---------------------------------------------------

/// A running capture of one [`Target`].
///
/// Not `Send`: it owns COM interfaces and a D3D11 immediate context, and both
/// want to stay on the thread that made them. Start it on the thread that will
/// pull from it.
pub struct Capturer {
    /// Held for the capturer's lifetime: the frame pool's textures belong to
    /// it, and so does every staging texture a readback makes.
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    item: GraphicsCaptureItem,
    /// Signalled once per arriving frame by a pool thread. Carries nothing:
    /// the frame itself is fetched by the consumer.
    arrivals: Receiver<()>,
    token: i64,
    /// Reused across [`Frame::copy_to_cpu`] calls; `None` until the first one.
    staging: RefCell<Option<ID3D11Texture2D>>,
}

impl Capturer {
    /// Start capturing `target`.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Unsupported`] if WGC is not available on this machine,
    /// [`CaptureError::Start`] if the D3D11 device, the capture item, the
    /// frame pool or the session refuses.
    pub fn start(target: &Target, cursor: Cursor) -> Result<Self, CaptureError> {
        if !is_supported() {
            return Err(CaptureError::Unsupported);
        }

        let (device, context) = create_device()?;
        let interop_device = winrt_device(&device)?;
        let item = capture_item(target)?;

        let size = item
            .Size()
            .map_err(|error| CaptureError::Start(format!("reading the item's size: {error}")))?;

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &interop_device,
            // The only format WGC produces, and — usefully for task 5.2 — the
            // one the Media Foundation H.264 encoders take as input.
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            POOL_BUFFERS,
            size,
        )
        .map_err(|error| CaptureError::Start(format!("creating the frame pool: {error}")))?;

        // A rendezvous channel of depth one. The pool thread must never block
        // on a consumer that has stopped pulling, and a backlog of arrival
        // notices is worthless anyway: what the consumer wants is "there is a
        // frame now", not a count of how many it missed.
        let (tx, arrivals) = mpsc::sync_channel::<()>(1);
        let token = pool
            .FrameArrived(&TypedEventHandler::new(
                move |_pool: windows::core::Ref<'_, Direct3D11CaptureFramePool>, _| {
                    notify(&tx);
                    Ok(())
                },
            ))
            .map_err(|error| CaptureError::Start(format!("subscribing to frames: {error}")))?;

        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|error| CaptureError::Start(format!("creating the session: {error}")))?;
        let _ = session.SetIsCursorCaptureEnabled(cursor == Cursor::Shown);
        // Windows 11 draws a yellow border around anything being captured and
        // only lets some callers turn it off. Best effort: a border is a
        // cosmetic complaint, a failed share is not.
        let _ = session.SetIsBorderRequired(false);

        session
            .StartCapture()
            .map_err(|error| CaptureError::Start(format!("starting capture: {error}")))?;

        Ok(Self {
            device,
            context,
            pool,
            session,
            item,
            arrivals,
            token,
            staging: RefCell::new(None),
        })
    }

    /// The `D3D11` device the captured frames live on.
    ///
    /// An encoder has to be built on this one: a texture cannot cross devices
    /// without a copy, which is the copy the whole path exists to avoid
    /// ([`crate::capture::encoder`]).
    #[must_use]
    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    /// The immediate context that device was created with.
    #[must_use]
    pub fn context(&self) -> &ID3D11DeviceContext {
        &self.context
    }

    /// What the capture item says it is, right now.
    ///
    /// A window's size follows the window. Task 5.3's encoder is fixed-size,
    /// so this is what it scales from.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Start`] if the item has closed underneath us.
    pub fn size(&self) -> Result<(u32, u32), CaptureError> {
        let size = self
            .item
            .Size()
            .map_err(|error| CaptureError::Start(format!("reading the item's size: {error}")))?;
        Ok((size.Width.unsigned_abs(), size.Height.unsigned_abs()))
    }

    /// Wait up to `timeout` for the next frame.
    ///
    /// `Ok(None)` is the ordinary answer on a screen that is not changing —
    /// **WGC only produces a frame when the content moves.** A timeout here is
    /// not an error and not a dropped capture; it means nothing happened.
    ///
    /// # The pool is asked anyway
    ///
    /// A notice is how a *change* announces itself, and it is not the only way
    /// a frame exists: the session's first frame is the screen as it already
    /// is, and on a screen that then never moves no notice ever follows. A
    /// capture that only ever read on a notice published nothing at all in
    /// that case — 0 access units in 20 seconds, measured (DR-34) — and a
    /// viewer would sit on "nobody is sharing" until somebody moved a window.
    ///
    /// So the timeout asks the pool directly. It answers once, with the
    /// content as it stands, and then has nothing until something changes
    /// again — so this is one frame on a still screen rather than ten a
    /// second: the same 15 seconds went from 0 access units to 9.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Stopped`] if the pool thread has gone away, which only
    /// happens once the session is closed. Failures reading the arrived frame
    /// come back as [`CaptureError::Frame`].
    pub fn next_frame(&self, timeout: Duration) -> Result<Option<Frame<'_>>, CaptureError> {
        match self.arrivals.recv_timeout(timeout) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => {
                // Nothing announced itself. There may still be a frame sitting
                // in the pool — see above — and if there is not, this is the
                // `Ok(None)` it always was.
                let Ok(frame) = self.pool.TryGetNextFrame() else {
                    return Ok(None);
                };
                return Frame::new(self, frame).map(Some);
            }
            Err(RecvTimeoutError::Disconnected) => return Err(CaptureError::Stopped),
        }

        // A notice does not guarantee a frame: the pool may have recycled it
        // between the signal and here. That is a miss, not a failure.
        let Ok(frame) = self.pool.TryGetNextFrame() else {
            return Ok(None);
        };
        Frame::new(self, frame).map(Some)
    }

    /// Resize the pool's textures to match the item.
    ///
    /// A window that is resized keeps producing frames at the old size until
    /// something asks for this; the pool does not follow on its own.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Start`] if the item or the pool refuses.
    pub fn resize(&self) -> Result<(u32, u32), CaptureError> {
        let (width, height) = self.size()?;
        let interop = winrt_device(&self.device)?;
        self.pool
            .Recreate(
                &interop,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                POOL_BUFFERS,
                SizeInt32 {
                    Width: i32::try_from(width).unwrap_or(i32::MAX),
                    Height: i32::try_from(height).unwrap_or(i32::MAX),
                },
            )
            .map_err(|error| CaptureError::Start(format!("resizing the frame pool: {error}")))?;
        Ok((width, height))
    }
}

impl Drop for Capturer {
    fn drop(&mut self) {
        // Order matters: stop the session first, then unsubscribe, then close
        // the pool. The other way round and a frame can arrive against a
        // handler whose channel has already been dropped.
        let _ = self.session.Close();
        let _ = self.pool.RemoveFrameArrived(self.token);
        let _ = self.pool.Close();
    }
}

/// Send without blocking and without caring whether it landed.
///
/// This runs on a WGC pool thread. If the consumer has not taken the last
/// notice yet it already knows there is work, and if it has gone away there is
/// nobody to tell.
fn notify(tx: &SyncSender<()>) {
    let _ = tx.try_send(());
}

/// One captured frame, alive until it is dropped.
///
/// Holding it holds one of the pool's [`POOL_BUFFERS`] textures, so a consumer
/// that keeps frames around starves the capture. Take what you need and let it
/// go.
pub struct Frame<'a> {
    capturer: &'a Capturer,
    inner: Direct3D11CaptureFrame,
    texture: ID3D11Texture2D,
    desc: D3D11_TEXTURE2D_DESC,
    time: Duration,
    content: (u32, u32),
}

impl<'a> Frame<'a> {
    fn new(capturer: &'a Capturer, inner: Direct3D11CaptureFrame) -> Result<Self, CaptureError> {
        let surface = inner
            .Surface()
            .map_err(|error| CaptureError::Frame(format!("reading the surface: {error}")))?;
        let access = surface
            .cast::<IDirect3DDxgiInterfaceAccess>()
            .map_err(|error| CaptureError::Frame(format!("reaching the DXGI surface: {error}")))?;
        // SAFETY: the surface is live and `ID3D11Texture2D` is what a WGC
        // surface is backed by.
        let texture = unsafe { access.GetInterface::<ID3D11Texture2D>() }
            .map_err(|error| CaptureError::Frame(format!("reaching the texture: {error}")))?;

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: the texture is live and `desc` is a live local.
        unsafe { texture.GetDesc(&raw mut desc) };

        // `SystemRelativeTime` is QPC-based and shares an origin with every
        // other frame from this session, which is all a frame interval needs.
        let time = inner.SystemRelativeTime().map_or(Duration::ZERO, |span| {
            Duration::from_nanos(span.Duration.unsigned_abs() * 100)
        });
        let content = inner
            .ContentSize()
            .map_or((desc.Width, desc.Height), |size| {
                (size.Width.unsigned_abs(), size.Height.unsigned_abs())
            });

        Ok(Self {
            capturer,
            inner,
            texture,
            desc,
            time,
            content,
        })
    }

    /// The frame, where it already is: on the GPU.
    #[must_use]
    pub fn texture(&self) -> &ID3D11Texture2D {
        &self.texture
    }

    /// The texture's allocated size, which is the frame pool's size and not
    /// necessarily what is drawn in it — see [`Frame::content_size`].
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.desc.Width, self.desc.Height)
    }

    /// How much of the texture the captured content actually fills.
    ///
    /// A window that shrank still arrives in a pool-sized texture with the
    /// remainder undefined. Anything that scales or encodes wants this.
    #[must_use]
    pub fn content_size(&self) -> (u32, u32) {
        self.content
    }

    /// The surface format, which is always BGRA8 on this path.
    #[must_use]
    pub fn format(&self) -> DXGI_FORMAT {
        self.desc.Format
    }

    /// When WGC produced this frame, on the session's own clock.
    #[must_use]
    pub fn time(&self) -> Duration {
        self.time
    }

    /// Copy the frame off the GPU into `out` as tightly packed BGRA8.
    ///
    /// This is a full readback and a stall: it is here for the spike, for
    /// tests, and for the software fallback. The encoder path does not use it.
    /// Returns the row stride in bytes, which after packing is `width * 4`.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Frame`] if the staging texture cannot be made or the
    /// map fails.
    pub fn copy_to_cpu(&self, out: &mut Vec<u8>) -> Result<usize, CaptureError> {
        let staging = self.staging()?;
        let context = &self.capturer.context;

        // SAFETY: both textures are live, on the same device, and identical in
        // size and format — which is what `CopyResource` requires.
        unsafe { context.CopyResource(&staging, &self.texture) };

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the staging texture was created `D3D11_USAGE_STAGING` with
        // `D3D11_CPU_ACCESS_READ`, which is what makes it mappable.
        unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped)) }.map_err(
            |error| CaptureError::Frame(format!("mapping the staging texture: {error}")),
        )?;

        let width = self.desc.Width as usize;
        let height = self.desc.Height as usize;
        let packed = width * 4;
        out.clear();
        out.reserve(packed * height);

        // The GPU picks its own row pitch and it is rarely `width * 4`. Copy
        // row by row so what comes out is what a consumer expects.
        for row in 0..height {
            // SAFETY: `Map` succeeded, so `pData` points at
            // `RowPitch * Height` readable bytes, and this reads the first
            // `packed` of each row.
            let line = unsafe {
                std::slice::from_raw_parts(
                    mapped
                        .pData
                        .cast::<u8>()
                        .add(row * mapped.RowPitch as usize),
                    packed,
                )
            };
            out.extend_from_slice(line);
        }

        // SAFETY: paired with the `Map` above, same resource and subresource.
        unsafe { context.Unmap(&staging, 0) };
        Ok(packed)
    }

    /// The staging texture, made on first use and reused after that.
    fn staging(&self) -> Result<ID3D11Texture2D, CaptureError> {
        let mut slot = self.capturer.staging.borrow_mut();
        if let Some(existing) = slot.as_ref() {
            let mut existing_desc = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: the texture is live and `existing_desc` is a live local.
            unsafe { existing.GetDesc(&raw mut existing_desc) };
            if existing_desc.Width == self.desc.Width
                && existing_desc.Height == self.desc.Height
                && existing_desc.Format == self.desc.Format
            {
                return Ok(existing.clone());
            }
        }

        let desc = D3D11_TEXTURE2D_DESC {
            Width: self.desc.Width,
            Height: self.desc.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: self.desc.Format,
            SampleDesc: self.desc.SampleDesc,
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0.unsigned_abs(),
            MiscFlags: 0,
        };
        let mut created = None;
        // SAFETY: the device is live, the description is fully initialised,
        // and there is no initial data to supply.
        unsafe {
            self.capturer
                .device
                .CreateTexture2D(&raw const desc, None, Some(&raw mut created))
        }
        .map_err(|error| CaptureError::Frame(format!("creating the staging texture: {error}")))?;

        let created =
            created.ok_or_else(|| CaptureError::Frame("staging texture came back null".into()))?;
        *slot = Some(created.clone());
        Ok(created)
    }
}

impl Drop for Frame<'_> {
    fn drop(&mut self) {
        // Returns the texture to the pool. Skip it and the capture stalls
        // after `POOL_BUFFERS` frames.
        let _ = self.inner.Close();
    }
}

// --- the plumbing underneath -------------------------------------------------

/// A hardware D3D11 device, BGRA-capable because WGC's format demands it.
fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext), CaptureError> {
    let mut device = None;
    let mut context = None;
    // SAFETY: every out-parameter is a live local; no adapter and no feature
    // level list means "pick the default ones".
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&raw mut device),
            None,
            Some(&raw mut context),
        )
    }
    .map_err(|error| CaptureError::Start(format!("creating the D3D11 device: {error}")))?;

    match (device, context) {
        (Some(device), Some(context)) => Ok((device, context)),
        _ => Err(CaptureError::Start(
            "D3D11 returned no device or no context".into(),
        )),
    }
}

/// The `WinRT` face of a `D3D11` device, which is what the frame pool takes.
fn winrt_device(device: &ID3D11Device) -> Result<IDirect3DDevice, CaptureError> {
    let dxgi = device
        .cast::<IDXGIDevice>()
        .map_err(|error| CaptureError::Start(format!("reaching the DXGI device: {error}")))?;
    // SAFETY: the DXGI device is live; the call returns a new reference.
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .map_err(|error| CaptureError::Start(format!("wrapping the device for WinRT: {error}")))?;
    inspectable
        .cast::<IDirect3DDevice>()
        .map_err(|error| CaptureError::Start(format!("casting to IDirect3DDevice: {error}")))
}

/// A `GraphicsCaptureItem` for a monitor or a window.
///
/// The activation factory route rather than `TryCreateFromDisplayId`: the
/// interop interface is the one that takes the `HMONITOR`/`HWND` this process
/// already has, and it works back to Windows 10 1903.
fn capture_item(target: &Target) -> Result<GraphicsCaptureItem, CaptureError> {
    let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
        .map_err(|error| CaptureError::Start(format!("no capture interop: {error}")))?;

    let handle = target.handle as *mut std::ffi::c_void;
    // SAFETY: the handle came from this process's own enumeration. A stale
    // one — a window closed since the picker listed it — comes back as an
    // error rather than undefined behaviour.
    let item = unsafe {
        match target.kind {
            TargetKind::Monitor => {
                interop.CreateForMonitor::<GraphicsCaptureItem>(HMONITOR(handle))
            }
            TargetKind::Window => interop.CreateForWindow::<GraphicsCaptureItem>(HWND(handle)),
        }
    };
    item.map_err(|error| {
        CaptureError::Start(format!("no capture item for {}: {error}", target.name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_machine_has_a_primary_monitor_first() {
        let Ok(found) = monitors() else {
            // A headless CI runner is allowed to have none.
            return;
        };
        if let Some(first) = found.first() {
            assert!(first.name.contains("primary"), "primary sorts first");
            assert_eq!(first.kind, TargetKind::Monitor);
            assert!(first.width > 0 && first.height > 0);
        }
    }

    #[test]
    fn enumerated_windows_are_named_and_sized() {
        let Ok(found) = windows() else { return };
        for target in found {
            assert_eq!(target.kind, TargetKind::Window);
            assert!(!target.name.is_empty(), "a nameless window got through");
            assert!(target.width > 0 && target.height > 0);
        }
    }

    #[test]
    fn a_target_survives_a_round_trip_through_json() {
        let target = Target {
            kind: TargetKind::Monitor,
            handle: 0x1234,
            name: "\\\\.\\DISPLAY1 (2560×1440, primary)".to_owned(),
            width: 2560,
            height: 1440,
        };
        let json = serde_json::to_string(&target).expect("serialise");
        let back: Target = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(target, back);
    }
}
