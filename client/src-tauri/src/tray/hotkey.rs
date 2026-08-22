//! Push to talk that works while a game has focus (plan.md task 4.3).
//!
//! The in-window key from task 3.3 only exists while the webview is the thing
//! being typed into, which is exactly when nobody is playing anything. This is
//! the same key, heard from outside: a `WH_KEYBOARD_LL` hook watches the whole
//! desktop and reports the one key it was given.
//!
//! # What it deliberately does not do
//!
//! - **It does not swallow the key.** Every event is passed on to whoever it
//!   was going to. A push-to-talk key that a game stops seeing is a key nobody
//!   will bind to a weapon, and choosing for the user is not this app's job.
//! - **It does not inject anything anywhere.** No DLL in another process, no
//!   reading anyone else's memory. That is the whole of the anti-cheat
//!   argument, and DR-18 is where it is written down.
//! - **It is not installed unless it is needed.** The hook exists only while a
//!   call is in push-to-talk mode with a key bound. Out of a call, goodvoice is
//!   not in the keyboard's way at all.
//!
//! [`vk_for_code`] is the part that is worth testing, and it is portable: the
//! webview names keys the way `KeyboardEvent.code` does, and Windows names them
//! as virtual-key codes, and getting that wrong means a key that silently never
//! fires.

use super::TrayError;

/// The virtual-key code for a key named the way the webview names it.
///
/// The webview sends `KeyboardEvent.code` — a *physical* key, not the
/// character it produces, which is what push to talk wants: the key under the
/// finger stays the same key when the layout changes.
///
/// `None` for anything not in the table, which is the honest answer: a binding
/// that cannot be watched for should fail at the point it is set rather than by
/// never firing.
#[must_use]
pub fn vk_for_code(code: &str) -> Option<u16> {
    // Virtual-key codes are from `Win32/UI/Input/KeyboardAndMouse`, written out
    // rather than imported so the table is readable — and so it still compiles
    // on the hosts the test suite runs on.
    const VK_BACK: u16 = 0x08;
    const VK_TAB: u16 = 0x09;
    const VK_RETURN: u16 = 0x0D;
    const VK_CAPITAL: u16 = 0x14;
    const VK_SPACE: u16 = 0x20;
    const VK_PRIOR: u16 = 0x21;
    const VK_END: u16 = 0x23;
    const VK_LEFT: u16 = 0x25;
    const VK_INSERT: u16 = 0x2D;
    const VK_NUMPAD0: u16 = 0x60;
    const VK_MULTIPLY: u16 = 0x6A;
    const VK_F1: u16 = 0x70;
    const VK_LSHIFT: u16 = 0xA0;

    if let Some(letter) = code.strip_prefix("Key") {
        // "KeyA" is 0x41, and so on: the letter's own ASCII code.
        let mut letters = letter.bytes();
        return match (letters.next(), letters.next()) {
            (Some(letter @ b'A'..=b'Z'), None) => Some(u16::from(letter)),
            _ => None,
        };
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        return single_digit(digit).map(|digit| 0x30 + u16::from(digit));
    }
    if let Some(digit) = code.strip_prefix("Numpad") {
        return match digit {
            "Multiply" => Some(VK_MULTIPLY),
            "Add" => Some(VK_MULTIPLY + 1),
            "Subtract" => Some(VK_MULTIPLY + 3),
            "Decimal" => Some(VK_MULTIPLY + 4),
            "Divide" => Some(VK_MULTIPLY + 5),
            _ => single_digit(digit).map(|digit| VK_NUMPAD0 + u16::from(digit)),
        };
    }
    if let Some(number) = code.strip_prefix('F') {
        // F1 to F24, and nothing that merely starts with an F.
        let number: u16 = number.parse().ok()?;
        return (1..=24).contains(&number).then(|| VK_F1 + number - 1);
    }

    Some(match code {
        "Space" => VK_SPACE,
        "Tab" => VK_TAB,
        "Enter" => VK_RETURN,
        "Backspace" => VK_BACK,
        "CapsLock" => VK_CAPITAL,
        // The hook reports the hand, not just the modifier, so both sides are
        // bindable and only the one that was pressed answers.
        "ShiftLeft" => VK_LSHIFT,
        "ShiftRight" => VK_LSHIFT + 1,
        "ControlLeft" => VK_LSHIFT + 2,
        "ControlRight" => VK_LSHIFT + 3,
        "AltLeft" => VK_LSHIFT + 4,
        "AltRight" => VK_LSHIFT + 5,
        "PageUp" => VK_PRIOR,
        "PageDown" => VK_PRIOR + 1,
        "End" => VK_END,
        "Home" => VK_END + 1,
        "ArrowLeft" => VK_LEFT,
        "ArrowUp" => VK_LEFT + 1,
        "ArrowRight" => VK_LEFT + 2,
        "ArrowDown" => VK_LEFT + 3,
        "Insert" => VK_INSERT,
        "Delete" => VK_INSERT + 1,
        "Semicolon" => 0xBA,
        "Equal" => 0xBB,
        "Comma" => 0xBC,
        "Minus" => 0xBD,
        "Period" => 0xBE,
        "Slash" => 0xBF,
        "Backquote" => 0xC0,
        "BracketLeft" => 0xDB,
        "Backslash" => 0xDC,
        "BracketRight" => 0xDD,
        "Quote" => 0xDE,
        _ => return None,
    })
}

/// The one-character tail of `Digit3` or `Numpad7`, as a number.
fn single_digit(text: &str) -> Option<u8> {
    let mut characters = text.bytes();
    match (characters.next(), characters.next()) {
        (Some(digit @ b'0'..=b'9'), None) => Some(digit - b'0'),
        _ => None,
    }
}

#[cfg(windows)]
pub use windows_hook::Listener;

/// Starts watching the desktop for `code`, calling `on_change` on every press
/// and release of it.
///
/// The key is not consumed: `on_change` is told, and the keystroke carries on
/// to whatever had focus.
///
/// # Errors
///
/// [`TrayError::HotkeyUnavailable`] when the key is not one this can watch for,
/// or when Windows refuses the hook.
#[cfg(windows)]
pub fn listen<F>(code: &str, on_change: F) -> Result<Listener, TrayError>
where
    F: Fn(bool) + Send + Sync + 'static,
{
    let vk = vk_for_code(code).ok_or(TrayError::HotkeyUnavailable)?;
    windows_hook::install(vk, Box::new(on_change))
}

/// The same, on a host with no desktop to hook.
///
/// The client is a Windows application; this exists so the rest of it builds
/// and its tests run anywhere. It refuses rather than pretending, because a
/// push-to-talk key that quietly never fires is the worst of the three
/// possible outcomes.
///
/// # Errors
///
/// Always [`TrayError::HotkeyUnavailable`].
#[cfg(not(windows))]
pub fn listen<F>(code: &str, _on_change: F) -> Result<Listener, TrayError>
where
    F: Fn(bool) + Send + Sync + 'static,
{
    let _ = vk_for_code(code).ok_or(TrayError::HotkeyUnavailable)?;
    Err(TrayError::HotkeyUnavailable)
}

/// Nothing to hold on a host with no hook. Dropping it stops nothing.
#[cfg(not(windows))]
#[derive(Debug)]
pub struct Listener;

#[cfg(windows)]
mod windows_hook {
    use std::sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU32 as AtomicThreadId, Ordering},
        Mutex, OnceLock,
    };

    use windows::Win32::{
        Foundation::{LPARAM, LRESULT, WPARAM},
        UI::WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
            TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
            WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    };

    use super::TrayError;

    /// What the hook is watching for, and who to tell.
    ///
    /// Statics because a hook procedure is a bare `extern "system" fn` with
    /// nowhere to put a closure. Only one listener exists at a time — the app
    /// holds it — so there is nothing here to key by.
    static TARGET: AtomicU32 = AtomicU32::new(0);
    static DOWN: AtomicBool = AtomicBool::new(false);
    /// Who to tell when the key moves. Boxed because a hook procedure has
    /// nowhere to keep a closure.
    type Handler = Box<dyn Fn(bool) + Send + Sync>;
    static HANDLER: Mutex<Option<Handler>> = Mutex::new(None);
    static HOOK_THREAD: OnceLock<AtomicThreadId> = OnceLock::new();

    /// The hook, and the thread pumping the messages that drive it.
    ///
    /// Dropping this takes the hook off the desktop: goodvoice is only in the
    /// keyboard's way while somebody is holding a key to talk.
    #[derive(Debug)]
    pub struct Listener {
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            TARGET.store(0, Ordering::Release);
            if let Ok(mut handler) = HANDLER.lock() {
                *handler = None;
            }
            if let Some(id) = HOOK_THREAD.get().map(|id| id.load(Ordering::Acquire)) {
                // The hook lives on that thread and only it can take the hook
                // off, so it is asked to leave its own message loop.
                unsafe {
                    let _ = PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0));
                }
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    pub(super) fn install(
        vk: u16,
        on_change: Box<dyn Fn(bool) + Send + Sync>,
    ) -> Result<Listener, TrayError> {
        TARGET.store(u32::from(vk), Ordering::Release);
        DOWN.store(false, Ordering::Release);
        *HANDLER.lock().map_err(|_| TrayError::HotkeyUnavailable)? = Some(on_change);

        let (ready, started) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("goodvoice-hotkey".to_owned())
            .spawn(move || pump(&ready))
            .map_err(|_| TrayError::HotkeyUnavailable)?;

        // A hook is owned by the thread that set it, so the caller waits here
        // to learn whether it went on rather than finding out at the first
        // keystroke that never arrives.
        if let Ok(Ok(())) = started.recv() {
            return Ok(Listener {
                thread: Some(thread),
            });
        }
        TARGET.store(0, Ordering::Release);
        Err(TrayError::HotkeyUnavailable)
    }

    /// Sets the hook and pumps messages until asked to stop.
    fn pump(ready: &std::sync::mpsc::Sender<Result<(), ()>>) {
        let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), None, 0) };
        let Ok(hook) = hook else {
            let _ = ready.send(Err(()));
            return;
        };

        HOOK_THREAD.get_or_init(|| AtomicThreadId::new(0)).store(
            unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
            Ordering::Release,
        );
        let _ = ready.send(Ok(()));

        // A low-level hook is only called while its thread is pumping
        // messages: this loop is not idle bookkeeping, it *is* the hook.
        let mut message = MSG::default();
        while unsafe { GetMessageW(&raw mut message, None, 0, 0) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }

        unsafe {
            let _ = UnhookWindowsHookEx(hook);
        }
    }

    /// Every keystroke on the desktop passes through here.
    ///
    /// It has to be cheap: this runs on the input path of every process on the
    /// machine, and a slow hook is felt as a laggy keyboard everywhere. The
    /// common case — a key that is not ours — is one atomic load and a
    /// comparison. Nothing is ever consumed; the event always carries on.
    unsafe extern "system" fn keyboard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // Negative means "not for us to inspect", and the documentation is
        // explicit that the event must be passed on untouched.
        if code >= 0 {
            let target = TARGET.load(Ordering::Acquire);
            if target != 0 {
                let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
                // The message id fits a `u32` by definition; `WPARAM` is
                // pointer-sized because the same field carries pointers
                // elsewhere.
                if event.vkCode == target {
                    report(u32::try_from(wparam.0).unwrap_or_default());
                }
            }
        }

        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    /// Turns a stream of key messages into the two edges anyone cares about.
    ///
    /// Holding a key repeats `WM_KEYDOWN`, and a caller told "down" fifty times
    /// a second would be told to start talking fifty times a second. Only the
    /// transitions are passed on.
    fn report(message: u32) {
        let down = match message {
            WM_KEYDOWN | WM_SYSKEYDOWN => true,
            WM_KEYUP | WM_SYSKEYUP => false,
            _ => return,
        };
        if DOWN.swap(down, Ordering::AcqRel) == down {
            return;
        }
        if let Ok(handler) = HANDLER.lock() {
            if let Some(handler) = handler.as_ref() {
                handler(down);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::vk_for_code;

    #[test]
    fn the_letters_are_their_own_codes() {
        assert_eq!(vk_for_code("KeyA"), Some(0x41));
        assert_eq!(vk_for_code("KeyV"), Some(0x56));
        assert_eq!(vk_for_code("KeyZ"), Some(0x5A));
    }

    #[test]
    fn the_default_key_is_one_that_can_be_watched_for() {
        // "Space" is `DEFAULT_TALK_KEY` in App.tsx. A default that cannot be
        // bound would leave push to talk silently dead for everyone who never
        // changed it.
        assert_eq!(vk_for_code("Space"), Some(0x20));
    }

    #[test]
    fn both_hands_of_a_modifier_are_separate_keys() {
        // The hook reports which one was pressed, so binding the left control
        // and pressing the right one must not talk.
        assert_eq!(vk_for_code("ControlLeft"), Some(0xA2));
        assert_eq!(vk_for_code("ControlRight"), Some(0xA3));
        assert_eq!(vk_for_code("ShiftLeft"), Some(0xA0));
        assert_eq!(vk_for_code("AltRight"), Some(0xA5));
    }

    #[test]
    fn the_function_keys_stop_at_f24() {
        assert_eq!(vk_for_code("F1"), Some(0x70));
        assert_eq!(vk_for_code("F13"), Some(0x7C));
        assert_eq!(vk_for_code("F24"), Some(0x87));
        assert_eq!(vk_for_code("F25"), None);
        assert_eq!(vk_for_code("F0"), None);
    }

    #[test]
    fn the_two_kinds_of_number_key_are_not_the_same_key() {
        assert_eq!(vk_for_code("Digit4"), Some(0x34));
        assert_eq!(vk_for_code("Numpad4"), Some(0x64));
        assert_eq!(vk_for_code("NumpadAdd"), Some(0x6B));
        assert_eq!(vk_for_code("NumpadDivide"), Some(0x6F));
    }

    #[test]
    fn a_prefix_is_not_a_key() {
        // The parsing is by prefix, so these are the ways it could say yes to
        // something that is not a key at all.
        assert_eq!(vk_for_code("Key"), None);
        assert_eq!(vk_for_code("KeyAB"), None);
        assert_eq!(vk_for_code("Keya"), None);
        assert_eq!(vk_for_code("Digit10"), None);
        assert_eq!(vk_for_code("Numpad"), None);
        assert_eq!(vk_for_code("Fn"), None);
        assert_eq!(vk_for_code(""), None);
    }

    #[test]
    fn a_key_nobody_should_bind_is_not_in_the_table() {
        // Escape and the Windows key are how a person gets out of a game and
        // out of a stuck binding. Binding push to talk to either is a way to
        // lose an afternoon.
        assert_eq!(vk_for_code("Escape"), None);
        assert_eq!(vk_for_code("MetaLeft"), None);
    }
}
