//! System tray icon, tray menu and the global push-to-talk hotkey.
//!
//! The window is not the app. A voice client spends almost all of its time with
//! nothing to show — the call runs in Rust and does not care whether a webview
//! exists — so closing the window puts goodvoice in the notification area
//! rather than ending the call (plan.md task 4.1). What it offers while hidden
//! is [`menu`] (task 4.2); the global hotkey is 4.3.
//!
//! # The trap this must not set
//!
//! An app whose close button hides the window and whose tray icon failed to
//! appear cannot be quit at all. So the two are wired together: close-to-tray
//! is only in force while [`Tray::installed`] says there is a tray to close
//! into, and a host that could not give us one keeps an ordinary window that
//! ordinarily closes.

pub mod hotkey;
mod menu;

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};

use tauri::{
    menu::MenuEvent,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager as _, WebviewWindowBuilder, Window, WindowEvent,
};
use thiserror::Error;

use crate::Controls;
use menu::{Action, TrayMenu};

/// How long the call gets to hand its seat back when quitting from the tray.
///
/// Leaving is one HTTP request and a socket close; a seat nobody gives back is
/// what makes the *next* join get refused by a room full of this client's own
/// ghosts (DR-5). But a quit that hangs on a dead network is worse than a
/// stranded seat, which the room reclaims on its own.
const LEAVE_GRACE: Duration = Duration::from_secs(3);

/// Failures on the tray / hotkey path.
#[derive(Debug, Error)]
pub enum TrayError {
    /// The low-level keyboard hook could not be installed.
    #[error("global hotkey registration failed")]
    HotkeyUnavailable,
    /// The host would not give us a notification-area icon.
    #[error("tray icon unavailable: {0}")]
    Unavailable(#[from] tauri::Error),
    /// The build has no icon to put in the tray.
    #[error("this build has no window icon to use as a tray icon")]
    NoIcon,
}

/// Whether there is a tray to hide into.
///
/// Managed state rather than a global: the window event handler is handed a
/// window and can reach it, and nothing else has any business knowing.
#[derive(Default)]
pub struct Tray {
    installed: AtomicBool,
    /// Set while [`show`] is part-way through opening. See the comment there:
    /// without it, a double click on the tray icon wedges the event loop.
    opening: AtomicBool,
    /// The menu, once there is one. Held so the call can tick its boxes;
    /// [`apply_controls`] is the only thing that reads it.
    menu: Mutex<Option<TrayMenu>>,
}

impl Tray {
    /// Whether closing the window should hide it instead.
    #[must_use]
    pub fn hides_the_window(&self) -> bool {
        self.installed.load(Ordering::Acquire)
    }
}

/// Puts the tray menu in step with the call: what is ticked, and what can be
/// clicked at all.
///
/// Called from [`crate::push_controls`], which is also what tells the window.
/// Neither is the source of truth; the call is.
pub fn apply_controls(app: &AppHandle, controls: Controls) {
    if let Ok(menu) = app.state::<Tray>().menu.lock() {
        if let Some(menu) = menu.as_ref() {
            menu.apply(controls);
        }
    }
}

/// Puts goodvoice in the notification area.
///
/// # Errors
///
/// [`TrayError::NoIcon`] when the build has no icon to use, and
/// [`TrayError::Unavailable`] when the host refuses the icon — a Linux desktop
/// with no status-notifier host, most often. Neither is fatal: the caller is
/// expected to carry on with a window that closes normally.
pub fn install(app: &AppHandle) -> Result<(), TrayError> {
    let menu = TrayMenu::build(app)?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or(TrayError::NoIcon)?;

    TrayIconBuilder::with_id("goodvoice")
        .icon(icon)
        .tooltip("goodvoice")
        .menu(menu.menu())
        // The left button belongs to "show me the window", which is what a
        // person clicking a tray icon means nine times in ten. The menu is on
        // the right button, where Windows puts it.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| on_menu_event(app, &event))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show(tray.app_handle());
            }
        })
        .build(app)?;

    let tray = app.state::<Tray>();
    if let Ok(mut held) = tray.menu.lock() {
        *held = Some(menu);
    }
    // Last, and only once the menu is where `apply_controls` can find it: this
    // is what puts close-to-tray in force (see the module docs).
    tray.installed.store(true, Ordering::Release);
    Ok(())
}

fn on_menu_event(app: &AppHandle, event: &MenuEvent) {
    match Action::of(event.id.as_ref()) {
        Some(Action::Show) => show(app),
        // The call's own toggles are async and this is the event loop, so each
        // is handed to the runtime rather than waited on. The tick in the menu
        // follows from the call changing, not from the click (`push_controls`).
        Some(Action::Mute) => on_call(app, crate::toggle_muted),
        Some(Action::Deafen) => on_call(app, crate::toggle_deafened),
        Some(Action::Leave) => on_call(app, crate::end_call),
        Some(Action::Quit) => quit(app),
        None => {}
    }
}

/// Runs one of the call's own operations off the event loop.
fn on_call<F, Fut>(app: &AppHandle, operation: F)
where
    F: FnOnce(AppHandle) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let app = app.clone();
    tauri::async_runtime::spawn(async move { operation(app).await });
}

/// Brings the window back — from minimised, from hidden, or from not existing.
///
/// There are two ways back because there are two ways away. A window that is
/// merely hidden is un-minimised while it is still hidden, so the restore
/// animation happens where nobody can see it, and only then shown and focused.
/// A window that was **destroyed** (see [`window_event`]) has to be built
/// again from the same config the app declares, which is what makes the rebuilt
/// one the same size, title and shape as the original rather than a
/// second-class copy of it — and from [`crate::place`], which is what puts it
/// back where the last one was instead of wherever Windows would cascade it to.
///
/// The rebuild costs what a webview costs to start — 127 ms from click to
/// window on the DR-12 machine — and buys back 327 MB (DR-21). It is also why
/// the window asks for [`crate::Snapshot`] on mount: a window built in the
/// middle of a call has missed every event that ever described it.
pub fn show(app: &AppHandle) {
    // One Open at a time.
    //
    // Building a webview pumps the message loop, and Tauri registers the
    // window before the build returns — so a second Open arriving in the
    // middle of the first finds a half-built window and asks it to
    // un-minimise, which is a dispatch to the main thread, which is the thread
    // inside the build. What comes back is a window on screen and an event
    // loop that answers nothing: no close, no quit, no tray,
    // `IsHungAppWindow` true, and the call still running underneath. That is
    // the worst version of it, because nothing looks wrong until you try to
    // put goodvoice away.
    //
    // Honestly: a real double click on the icon does not do this here, because
    // the build finishes in ~130 ms and the second click lands after it. What
    // does it every time is UI Automation's `Invoke` twice in a row, which is
    // why `tray-roundtrip.ps1` clicks once. A slower machine closes that gap
    // on its own. Dropping the second Open costs nothing and is what the
    // second Open wanted anyway — a window, which is on its way.
    let tray = app.state::<Tray>();
    if tray.opening.swap(true, Ordering::AcqRel) {
        return;
    }
    let opened = open(app);
    tray.opening.store(false, Ordering::Release);

    if let Err(error) = opened {
        eprintln!("the window could not be opened: {error}");
    }
}

/// Shows the window, or builds one. [`show`] is what serialises the calls.
fn open(app: &AppHandle) -> Result<(), tauri::Error> {
    if let Some(window) = app.get_webview_window("main") {
        window.unminimize()?;
        window.show()?;
        return window.set_focus();
    }

    let Some(mut config) = app.config().app.windows.first().cloned() else {
        // Nothing declares a window, so there is nothing to rebuild. Not fatal
        // and not silent: a tray whose Open does nothing needs explaining.
        eprintln!("no window is declared in the app config; nothing to open");
        return Ok(());
    };
    // Where the last one was, written into the config the new one is built
    // from — so the window is *born* in place rather than moved there. The
    // config declares a size and no position, which is what let Windows
    // cascade every rebuild down the screen (DR-38); moving it afterwards
    // would fix the walk and add a jump, because a window has a rectangle
    // from the moment it exists and `tray-flicker.ps1` counts every one of
    // them.
    if let Some(place) = crate::place::remembered(app) {
        config.x = Some(place.x);
        config.y = Some(place.y);
        config.width = place.width;
        config.height = place.height;
        config.maximized = place.maximized;
    }
    let built = WebviewWindowBuilder::from_config(app, &config)?.build()?;
    // No `set_focus` here: the window is built hidden and shows itself once
    // the webview has painted (DR-38), and focus comes with that. Asking for
    // it now would put an empty window in front of whatever the person is
    // doing, ~400 ms before there is anything in it.
    crate::reveal_after_grace(&built.as_ref().window());
    Ok(())
}

/// Leaves the room and ends the process.
///
/// Quitting is the one exit that has to be tidy: the window closing is not the
/// end of a call any more, so this is where the seat goes back.
fn quit(app: &AppHandle) {
    // The last chance to write where the window is: quitting from the tray
    // menu is the one exit that does not destroy a window first, so nothing
    // else on this path would have saved it.
    if let Some(window) = app.get_webview_window("main") {
        crate::place::note(&window.as_ref().window());
    }
    crate::place::keep(app);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = tokio::time::timeout(LEAVE_GRACE, crate::end_call(app.clone())).await;
        app.exit(0);
    });
}

/// Turns the window's close and minimise into goodvoice going away entirely.
///
/// Wired up in [`crate::run`]. Both paths leave the call running: audio lives
/// in Rust and never depended on the webview existing, let alone being visible.
///
/// **The window is destroyed, not hidden** (task 4.6). Hidden was cheaper to
/// write and cost 327 MB: a hidden webview keeps its browser process, its GPU
/// process, its renderer and two utilities resident, and DR-20 measured a
/// client idling at 361 MB against a 120 MB budget with nothing on screen.
/// Destroying it leaves the 34 MB that is actually goodvoice. What it costs is
/// [`show`] having to build a new one, and the new one having to be told what
/// it missed ([`crate::Snapshot`]).
///
/// Nothing here fires when there is no tray: an app whose window closes into
/// nothing is an app that cannot be reopened.
pub fn window_event(window: &Window, event: &WindowEvent) {
    // The screen viewer (task 5.4) is a window in its own right, and none of
    // what follows applies to it: one that destroyed itself when minimised
    // would end a live share every time somebody put it out of the way. What
    // it does owe is the subscription it opened, which nothing else can give
    // back — a destroyed webview does not get to run its own cleanup.
    if window.label() == crate::VIEWER_LABEL {
        if matches!(event, WindowEvent::Destroyed) {
            crate::viewer_closed(window.app_handle());
        }
        return;
    }
    if window.label() != "main" {
        return;
    }

    // Where it is, and where it was when it went. Above the tray check on
    // purpose: a host that gave us no tray still has a person who put the
    // window somewhere, and their next run should get it back.
    match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => crate::place::note(window),
        WindowEvent::Destroyed => crate::place::keep(window.app_handle()),
        _ => {}
    }

    if !window.state::<Tray>().hides_the_window() {
        return;
    }

    // A close is not intercepted at all any more: it destroys the window,
    // `crate::run` refuses the exit that would otherwise follow the last one
    // closing, and the tray is what is left of goodvoice until somebody clicks
    // it. Minimise is the one that needs help — there is no "minimise
    // requested" to answer, only the resize the minimise already did, so the
    // window goes after the animation rather than instead of it. The
    // visibility check is what keeps a restore from undoing itself.
    if matches!(event, WindowEvent::Resized(_))
        && window.is_visible().unwrap_or(false)
        && window.is_minimized().unwrap_or(false)
    {
        destroy(window);
    }
}

/// Takes the window and its webview apart.
///
/// `destroy` rather than `close`: closing asks, and asking arrives back here as
/// another [`WindowEvent::CloseRequested`]. This is the answer, not the
/// question.
fn destroy(window: &Window) {
    if let Err(error) = window.destroy() {
        // Leave it on screen rather than in some half state — a window that
        // will not go away is a bug worth seeing, and the call is unaffected.
        eprintln!("the window would not close: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::Tray;

    #[test]
    fn a_window_with_no_tray_behind_it_closes_normally() {
        // The trap this guards: close-to-tray plus a tray that never appeared
        // is an app that cannot be quit.
        let tray = Tray::default();
        assert!(
            !tray.hides_the_window(),
            "close-to-tray was in force before a tray existed"
        );

        tray.installed
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(tray.hides_the_window());
    }

    #[test]
    fn a_second_open_arriving_inside_the_first_is_dropped() {
        // What this guards is not a race between threads: it is one click
        // arriving while the other is still opening, on the same thread,
        // because building a webview pumps the message loop. The nested call
        // dispatches to a main thread that is busy building, and the event
        // loop stops answering anything at all.
        use std::sync::atomic::Ordering;

        let tray = Tray::default();
        assert!(
            !tray.opening.swap(true, Ordering::AcqRel),
            "the first Open should have found the way clear"
        );
        assert!(
            tray.opening.swap(true, Ordering::AcqRel),
            "a second Open got past the guard and would wedge the event loop"
        );

        tray.opening.store(false, Ordering::Release);
        assert!(
            !tray.opening.swap(true, Ordering::AcqRel),
            "the guard did not let go once the window was up"
        );
    }
}
