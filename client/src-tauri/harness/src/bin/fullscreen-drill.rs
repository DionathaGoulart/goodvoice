//! A window that takes the screen the way a game does, and says what keys
//! reached it (plan.md §7.5).
//!
//! `bin/hotkey-drill` proves the desktop hook hears a key from a process with
//! no window. What it cannot show is the case the feature exists for
//! (prd.md §3 F2): a *fullscreen exclusive* game on the screen, the talk key
//! held, and the game still getting the key it binds to a weapon. That has
//! been "a person and a game" in every checklist since task 4.3.
//!
//! It does not need one. A game in fullscreen exclusive is a D3D11 swap chain
//! with `SetFullscreenState(TRUE)` on it — that call *is* what the phrase
//! means — and a window that owns the display is a window whose `WM_KEYDOWN`
//! is the answer to "did the game still receive it". So this is the smallest
//! honest stand-in: it takes the display through DXGI, draws, and counts the
//! edges of one key.
//!
//! ```text
//! cargo run -p goodvoice-harness --bin fullscreen-drill -- --key F13 --seconds 12
//! cargo run -p goodvoice-harness --bin fullscreen-drill -- --windowed   # the same, without taking the display
//! ```
//!
//! Run beside `hotkey-drill` on the same key, it answers both halves at once:
//! goodvoice hears the key from behind the fullscreen window, and the window
//! in front still gets it. `docs/testing/hotkey-fullscreen.ps1` is the pair,
//! driven without a person.
//!
//! What it is not: a game. It does not load an anti-cheat, and DR-18 is where
//! that argument is written down. What it measures is Windows' input path and
//! DXGI's display ownership, which is what "over a fullscreen game" means for
//! everything in this repo.

#[cfg(not(windows))]
fn main() {
    // The same refusal as `hotkey-drill`: a drill that printed "no keys
    // arrived" on Linux would read as a broken hook rather than the wrong
    // machine (plan.md: do not fake or skip).
    eprintln!("taking the display and watching for keys is a Windows thing, and so is this drill");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    platform::main()
}

#[cfg(windows)]
mod platform {
    use std::{
        sync::{
            atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
            OnceLock,
        },
        time::{Duration, Instant},
    };

    use windows::{
        core::w,
        Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
            Graphics::{
                Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP},
                Direct3D11::{
                    D3D11CreateDeviceAndSwapChain, ID3D11Device, ID3D11DeviceContext,
                    ID3D11RenderTargetView, ID3D11Texture2D, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    D3D11_SDK_VERSION,
                },
                Dxgi::{
                    Common::{
                        DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_MODE_DESC,
                        DXGI_RATIONAL, DXGI_SAMPLE_DESC,
                    },
                    IDXGISwapChain, DXGI_FRAME_STATISTICS, DXGI_SWAP_CHAIN_DESC,
                    DXGI_SWAP_CHAIN_FLAG_ALLOW_MODE_SWITCH, DXGI_SWAP_EFFECT_FLIP_DISCARD,
                    DXGI_USAGE_RENDER_TARGET_OUTPUT,
                },
            },
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
                GetForegroundWindow, GetSystemMetrics, LoadCursorW, PeekMessageW, PostQuitMessage,
                RegisterClassW, SetForegroundWindow, ShowWindow, TranslateMessage, CS_HREDRAW,
                CS_VREDRAW, IDC_ARROW, MSG, PM_REMOVE, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW,
                WM_DESTROY, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW,
                WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
            },
        },
    };

    /// The key being watched for, as a virtual-key code. Zero until parsed.
    ///
    /// Statics because a window procedure is a bare `extern "system" fn` with
    /// nowhere to put a closure — the same shape, and for the same reason, as
    /// `tray::hotkey`'s hook.
    static TARGET: AtomicU32 = AtomicU32::new(0);
    /// Whether the key is currently held, so a repeat is not a second press.
    static DOWN: AtomicBool = AtomicBool::new(false);
    static DOWNS: AtomicUsize = AtomicUsize::new(0);
    static UPS: AtomicUsize = AtomicUsize::new(0);
    /// When the run started, so each edge can be printed with the millisecond
    /// it arrived — the same shape `hotkey-drill` prints, so the two
    /// transcripts of one keystroke can be laid side by side.
    static STARTED: OnceLock<Instant> = OnceLock::new();

    pub fn main() -> std::process::ExitCode {
        let options = Options::parse();
        let Some(vk) = goodvoice_client_lib::tray::hotkey::vk_for_code(&options.key) else {
            eprintln!(
                "{} is not a key this can watch for — the table is `vk_for_code` in tray/hotkey.rs",
                options.key
            );
            return std::process::ExitCode::from(2);
        };
        TARGET.store(u32::from(vk), Ordering::Release);

        let screen = screen_size();
        println!("goodvoice fullscreen talk-key drill (plan.md §7.5)\n");
        println!(
            "  {} (VK {vk:#04X}) for {} s, {} at {}x{}",
            options.key,
            options.seconds,
            if options.windowed {
                "in a plain window"
            } else {
                "with the display taken"
            },
            screen.0,
            screen.1,
        );
        println!("  a key pressed anywhere on the desktop arrives here as WM_KEYDOWN\n");

        let window = match Window::open(screen) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("could not open the window: {error}");
                return std::process::ExitCode::from(2);
            }
        };

        let mut mode = Mode::Windowed;
        if !options.windowed {
            mode = window.take_the_display();
            if mode == Mode::Refused {
                // Not a failure of the app: DXGI would not give this process
                // the display, and every key count below would be about a
                // window that never became a game.
                println!("MODE=refused");
                println!(
                    "INCONCLUSIVE — DXGI would not go exclusive on this display, so nothing\n\
                     here says anything about the talk key. See hotkey-fullscreen.ps1's\n\
                     header for the two known reasons."
                );
                return std::process::ExitCode::from(3);
            }
        }

        let started = *STARTED.get_or_init(Instant::now);
        // Drawn at for a moment before the first reading, and asked more than
        // once: the call answers DXGI_ERROR_FRAME_STATISTICS_DISJOINT for a
        // chain that has just changed mode, and a drill that took that for
        // "no display here" would be reporting on its own impatience.
        let mut before = None;
        for _ in 0..10 {
            window.run(Duration::from_millis(300), Instant::now());
            if let Ok(stats) = window.frame_stats() {
                before = Some(stats);
                break;
            }
        }
        let counting = Instant::now();
        let drawn = window.run(Duration::from_secs(options.seconds), started);
        let after = window.frame_stats();

        let foreground_at_end = window.has_the_foreground();
        // Reported as measured rather than as expected: a windowed run has no
        // display to hold, and printing `true` for it would put a claim in the
        // transcript that nothing checked.
        let exclusive_at_end = window.still_exclusive();
        let held_on = options.windowed || exclusive_at_end;
        // Exclusive fullscreen is left explicitly: DXGI holds the display
        // until it is told otherwise, and a drill that exits owning the mode
        // leaves a desktop somebody has to fix.
        window.give_the_display_back();

        let (downs, ups) = (DOWNS.load(Ordering::Relaxed), UPS.load(Ordering::Relaxed));
        println!(
            "\n--- {downs} presses, {ups} releases in {} frames ---",
            drawn.presented
        );
        report(
            &window,
            mode,
            &drawn,
            Readings {
                exclusive_at_end,
                foreground_at_end,
                started,
                counting,
                before,
                after,
            },
        );
        println!("DOWNS={downs} UPS={ups}");

        if downs == 0 || ups == 0 {
            println!(
                "FAIL — the window on the screen never saw {}. Either it was never\n\
                 pressed, or something between the keyboard and this window ate it.",
                options.key
            );
            return std::process::ExitCode::FAILURE;
        }
        if !held_on {
            // The counts are real but they were taken by a window that had
            // stopped being a game, which is not the question §7.5 asks.
            println!(
                "INCONCLUSIVE — the key arrived, but the display had been handed back\n\
                 before the run ended (something else took the foreground)."
            );
            return std::process::ExitCode::from(3);
        }
        println!("PASS — the display was taken and the key still arrived here.");
        std::process::ExitCode::SUCCESS
    }

    /// `--key CODE --seconds N [--windowed]`, with CODE named the way
    /// `KeyboardEvent.code` names it — the same string the window stores and
    /// the same one `hotkey-drill` takes.
    struct Options {
        key: String,
        seconds: u64,
        windowed: bool,
    }

    impl Options {
        fn parse() -> Self {
            const DEFAULT_KEY: &str = "F13";
            const DEFAULT_SECONDS: u64 = 12;

            let mut options = Self {
                key: DEFAULT_KEY.to_owned(),
                seconds: DEFAULT_SECONDS,
                windowed: false,
            };
            let mut argv = std::env::args().skip(1);
            while let Some(flag) = argv.next() {
                if flag == "--windowed" {
                    options.windowed = true;
                    continue;
                }
                let Some(value) = argv.next() else {
                    break;
                };
                match flag.as_str() {
                    "--key" => options.key = value,
                    "--seconds" => options.seconds = value.parse().unwrap_or(DEFAULT_SECONDS),
                    _ => {}
                }
            }
            options.seconds = options.seconds.max(1);
            options
        }
    }

    /// What the swap chain ended up being, which is the difference between a
    /// measurement about a game and one about a big window.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        Exclusive,
        Windowed,
        Refused,
    }

    /// What the drawing loop managed, which is how a screen being drawn is
    /// told from a loop merely turning over.
    #[derive(Default)]
    struct Drawn {
        presented: u64,
        refused: u64,
        why: String,
    }

    /// What was true around the run, gathered so [`report`] can be one
    /// argument list rather than seven.
    struct Readings {
        exclusive_at_end: bool,
        foreground_at_end: bool,
        started: Instant,
        counting: Instant,
        before: Option<DXGI_FRAME_STATISTICS>,
        after: windows::core::Result<DXGI_FRAME_STATISTICS>,
    }

    /// Everything the transcript says about the display, in the order a reader
    /// needs it: what was asked for, what was got, and what the display itself
    /// says about who has it.
    fn report(window: &Window, mode: Mode, drawn: &Drawn, readings: Readings) {
        println!(
            "MODE={}",
            match mode {
                Mode::Exclusive => "exclusive",
                Mode::Windowed => "windowed",
                Mode::Refused => "refused",
            }
        );
        println!("EXCLUSIVE_AT_END={}", readings.exclusive_at_end);
        println!("FOREGROUND_AT_END={}", readings.foreground_at_end);
        println!("DRIVER={}", window.driver);
        println!("FRAMES={}", drawn.presented);
        #[allow(
            clippy::cast_precision_loss,
            reason = "a frame count over a few seconds is exact in f64"
        )]
        let rate = drawn.presented as f64 / readings.started.elapsed().as_secs_f64();
        // The rate is the other half of "it behaved like a game": a chain that
        // owns the display presents at the display's rate, and one presenting
        // thousands of times a second is not being shown to anybody.
        println!("FPS={rate:.0}");
        match (readings.before, readings.after) {
            (Some(before), Ok(after)) => {
                let refreshes = after.SyncRefreshCount.wrapping_sub(before.SyncRefreshCount);
                let presents = after.PresentCount.wrapping_sub(before.PresentCount);
                let hz = f64::from(refreshes) / readings.counting.elapsed().as_secs_f64();
                // The display's own vblank counter, which a chain that does
                // not own a display cannot read at all. A number here is the
                // display saying who has it.
                println!("DISPLAY_REFRESHES={refreshes} ({hz:.0} Hz)");
                println!("PRESENTS={presents}");
                if u64::from(presents) > u64::from(refreshes).saturating_mul(2) {
                    // Not a fault, and worth saying out loud: something is
                    // overriding the sync interval this asked for, so the
                    // frame rate above is the GPU's and not the screen's.
                    println!("VSYNC=off (presents are not being held to the refresh)");
                }
            }
            (_, Err(error)) => {
                println!("DISPLAY_REFRESHES=unavailable ({error})");
            }
            (None, Ok(after)) => {
                println!("DISPLAY_REFRESHES=unavailable (no reading to count from)");
                println!("PRESENTS={}", after.PresentCount);
            }
        }
        if drawn.refused > 0 {
            // Named rather than hidden: frames the display refused are frames
            // this window was not a game for.
            println!("FRAMES_REFUSED={} ({})", drawn.refused, drawn.why);
        }
    }

    /// The window, its device, and the swap chain that owns the display.
    struct Window {
        hwnd: HWND,
        swap_chain: IDXGISwapChain,
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        /// Which D3D driver made it. A run that fell back to WARP took the
        /// display in software, which is a different thing to have measured
        /// and has to be said rather than averaged into the frame rate.
        driver: &'static str,
    }

    impl Window {
        /// A borderless window the size of the primary display, with a swap
        /// chain on it that is allowed to change the display mode.
        fn open(screen: (i32, i32)) -> windows::core::Result<Self> {
            let module = unsafe { GetModuleHandleW(None) }?;
            let instance = HINSTANCE::from(module);
            let class = w!("goodvoice-fullscreen-drill");

            let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }?;
            let wndclass = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                hCursor: cursor,
                lpszClassName: class,
                ..WNDCLASSW::default()
            };
            // SAFETY: the class lives for the process; its name is a static.
            // A zero return here is picked up by CreateWindowExW failing.
            unsafe { RegisterClassW(&raw const wndclass) };

            // WS_POPUP, no frame: the shape a game's window has before DXGI is
            // asked for the display, so the mode switch is the only thing
            // being measured.
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOPMOST,
                    class,
                    w!("goodvoice fullscreen drill"),
                    WS_POPUP | WS_VISIBLE,
                    0,
                    0,
                    screen.0,
                    screen.1,
                    None,
                    None,
                    Some(instance),
                    None,
                )
            }?;
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
                // A window that is not the foreground one is a window DXGI
                // will not give the display to, and is not what a game on
                // screen looks like either.
                let _ = SetForegroundWindow(hwnd);
            }

            let desc = DXGI_SWAP_CHAIN_DESC {
                BufferDesc: DXGI_MODE_DESC {
                    Width: u32::try_from(screen.0).unwrap_or(0),
                    Height: u32::try_from(screen.1).unwrap_or(0),
                    // 0/1 asks DXGI for the display's own rate: the mode
                    // switch should not change the refresh a person is using.
                    RefreshRate: DXGI_RATIONAL {
                        Numerator: 0,
                        Denominator: 1,
                    },
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    ..DXGI_MODE_DESC::default()
                },
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                OutputWindow: hwnd,
                Windowed: true.into(),
                // The bitblt model, which is the one `SetFullscreenState`
                // takes to a real mode switch; ALLOW_MODE_SWITCH is what lets
                // it change the display rather than stretch into it.
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                #[allow(
                    clippy::cast_sign_loss,
                    reason = "the flag constants are small positive bits in an i32 newtype"
                )]
                Flags: DXGI_SWAP_CHAIN_FLAG_ALLOW_MODE_SWITCH.0 as u32,
            };

            let mut swap_chain = None;
            let mut device = None;
            let mut context = None;
            // A machine with no hardware device is still a machine that can
            // answer the input question, so WARP is a fallback rather than a
            // failure — it is named in the output if it is what ran.
            let mut driver = "hardware";
            let mut created = create(
                D3D_DRIVER_TYPE_HARDWARE,
                &desc,
                &mut swap_chain,
                &mut device,
                &mut context,
            );
            if created.is_err() {
                driver = "warp";
                created = create(
                    D3D_DRIVER_TYPE_WARP,
                    &desc,
                    &mut swap_chain,
                    &mut device,
                    &mut context,
                );
            }
            if let Err(error) = created {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return Err(error);
            }

            match (swap_chain, device, context) {
                (Some(swap_chain), Some(device), Some(context)) => Ok(Self {
                    hwnd,
                    swap_chain,
                    device,
                    context,
                    driver,
                }),
                _ => Err(windows::core::Error::from_thread()),
            }
        }

        /// `SetFullscreenState(TRUE)` — the call that *is* "fullscreen
        /// exclusive", and whether this display let us have it.
        fn take_the_display(&self) -> Mode {
            // SAFETY: the swap chain is live; None means "the output the
            // window is already on".
            match unsafe { self.swap_chain.SetFullscreenState(true, None) } {
                Ok(()) => {
                    // DXGI can accept the call and drop straight back out —
                    // ask it rather than assume.
                    if !self.still_exclusive() {
                        return Mode::Refused;
                    }
                    // The chain was made windowed and has just been given a
                    // display mode; without this the buffers are still the
                    // ones the window had, and the present path stays the
                    // windowed one however true GetFullscreenState reads.
                    // SAFETY: zeroes mean "keep what the mode says"; the flag
                    // has to match the one the chain was created with.
                    if let Err(error) = unsafe {
                        self.swap_chain.ResizeBuffers(
                            0,
                            0,
                            0,
                            DXGI_FORMAT_UNKNOWN,
                            DXGI_SWAP_CHAIN_FLAG_ALLOW_MODE_SWITCH,
                        )
                    } {
                        println!("  the buffers would not follow the mode: {error}");
                    }
                    Mode::Exclusive
                }
                Err(error) => {
                    println!("  DXGI refused the display: {error}");
                    Mode::Refused
                }
            }
        }

        fn still_exclusive(&self) -> bool {
            let mut fullscreen = windows::core::BOOL::default();
            // SAFETY: the out-parameter is a live local.
            unsafe {
                self.swap_chain
                    .GetFullscreenState(Some(&raw mut fullscreen), None)
            }
            .is_ok()
                && fullscreen.as_bool()
        }

        fn give_the_display_back(&self) {
            // SAFETY: the swap chain is live. Failing here means it was
            // already windowed, which is the state being asked for.
            unsafe {
                let _ = self.swap_chain.SetFullscreenState(false, None);
            }
        }

        /// The display's own counters, or `None` if this chain does not have
        /// a display to count.
        ///
        /// This is the hardest evidence here that the window is a game and not
        /// merely a big rectangle: `GetFrameStatistics` answers for a
        /// full-screen swap chain and refuses one that is windowed and
        /// bitblt-presented. `SyncRefreshCount` is the *display's* vblank
        /// counter, so its rate over the run is the monitor's refresh —
        /// something a window borrowing the desktop has no access to at all.
        fn frame_stats(&self) -> windows::core::Result<DXGI_FRAME_STATISTICS> {
            let mut stats = DXGI_FRAME_STATISTICS::default();
            // SAFETY: the out-parameter is a live local.
            unsafe { self.swap_chain.GetFrameStatistics(&raw mut stats) }.map(|()| stats)
        }

        fn has_the_foreground(&self) -> bool {
            let foreground = unsafe { GetForegroundWindow() };
            foreground == self.hwnd
        }

        /// Pumps messages and draws until the time is up, returning the frame
        /// count.
        ///
        /// The drawing is not decoration: a swap chain that is never presented
        /// is a fullscreen window Windows is entitled to composite away, and
        /// the colour says what the key is doing — dark while it is up, bright
        /// while it is held — so a photograph of the screen is a second,
        /// independent reading of the same thing.
        fn run(&self, duration: Duration, started: Instant) -> Drawn {
            let mut drawn = Drawn::default();
            let mut message = MSG::default();
            while started.elapsed() < duration {
                // SAFETY: the message is a live local; PM_REMOVE takes each
                // message once.
                while unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                    if message.message == WM_QUIT {
                        return drawn;
                    }
                    unsafe {
                        let _ = TranslateMessage(&raw const message);
                        DispatchMessageW(&raw const message);
                    }
                }
                match self.draw() {
                    Ok(()) => drawn.presented += 1,
                    Err(error) => {
                        drawn.refused += 1;
                        if drawn.why.is_empty() {
                            drawn.why = error.to_string();
                        }
                    }
                }
            }
            drawn
        }

        fn draw(&self) -> windows::core::Result<()> {
            // SAFETY: the swap chain is live, and buffer 0 is the back buffer
            // by definition.
            let back_buffer = unsafe { self.swap_chain.GetBuffer::<ID3D11Texture2D>(0) }?;
            let mut view: Option<ID3D11RenderTargetView> = None;
            // SAFETY: the back buffer is live; the out-parameter is a local.
            unsafe {
                self.device
                    .CreateRenderTargetView(&back_buffer, None, Some(&raw mut view))
            }?;
            let Some(view) = view else {
                return Err(windows::core::Error::from_thread());
            };

            let held = DOWN.load(Ordering::Relaxed);
            let colour: [f32; 4] = if held {
                [0.15, 0.75, 0.35, 1.0]
            } else {
                [0.05, 0.06, 0.10, 1.0]
            };
            // SAFETY: the view was just made from this device's back buffer.
            unsafe {
                self.context.ClearRenderTargetView(&view, &colour);
                // Present with a sync interval of 1: a game's cadence, and the
                // reason this loop is not a spinning core. Its HRESULT is the
                // whole point of the frame count — a drill that counted turns
                // of the loop instead would report thousands of frames a
                // second and be reporting on a screen nobody was drawing.
                self.swap_chain
                    .Present(1, windows::Win32::Graphics::Dxgi::DXGI_PRESENT(0))
                    .ok()?;
            }
            Ok(())
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            self.give_the_display_back();
            // SAFETY: the window belongs to this thread.
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }

    fn create(
        driver: windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE,
        desc: &DXGI_SWAP_CHAIN_DESC,
        swap_chain: &mut Option<IDXGISwapChain>,
        device: &mut Option<ID3D11Device>,
        context: &mut Option<ID3D11DeviceContext>,
    ) -> windows::core::Result<()> {
        // SAFETY: every out-parameter is a live local; no adapter and no
        // feature-level list means "pick the default ones".
        unsafe {
            D3D11CreateDeviceAndSwapChain(
                None,
                driver,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&raw const *desc),
                Some(&raw mut *swap_chain),
                Some(&raw mut *device),
                None,
                Some(&raw mut *context),
            )
        }
    }

    fn screen_size() -> (i32, i32) {
        // SAFETY: no state, no handles.
        unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
    }

    /// Every keystroke that reaches the window on the screen passes here.
    ///
    /// This is the half of §7.5 that is about the *game*: goodvoice's hook is
    /// somewhere else entirely, and if it were swallowing the key, this is
    /// where the key would stop arriving.
    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
                let target = TARGET.load(Ordering::Acquire);
                // `wparam` is the virtual-key code for all four messages.
                if target != 0 && u32::try_from(wparam.0).unwrap_or_default() == target {
                    let down = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
                    // Holding a key repeats WM_KEYDOWN; only the edges count,
                    // the same way `tray::hotkey` reports them.
                    if DOWN.swap(down, Ordering::AcqRel) != down {
                        let at = STARTED
                            .get()
                            .map_or(0, |started| started.elapsed().as_millis());
                        if down {
                            DOWNS.fetch_add(1, Ordering::Relaxed);
                            println!("  {at:>6} ms  down");
                        } else {
                            UPS.fetch_add(1, Ordering::Relaxed);
                            println!("  {at:>6} ms  up");
                        }
                    }
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                // SAFETY: no state.
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            // SAFETY: forwarding the arguments this procedure was given.
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }
}
