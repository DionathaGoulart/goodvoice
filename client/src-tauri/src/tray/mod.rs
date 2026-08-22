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
    AppHandle, Manager as _, Window, WindowEvent,
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

/// Brings the window back, from hidden or from minimised or from both.
///
/// The order is what keeps it from flickering: a minimised window is
/// un-minimised while it is still hidden, so the restore animation happens
/// where nobody can see it, and only then is it shown and focused.
pub fn show(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// Leaves the room and ends the process.
///
/// Quitting is the one exit that has to be tidy: the window closing is not the
/// end of a call any more, so this is where the seat goes back.
fn quit(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = tokio::time::timeout(LEAVE_GRACE, crate::end_call(app.clone())).await;
        app.exit(0);
    });
}

/// Turns the window's close and minimise into a hide.
///
/// Wired up in [`crate::run`]. Both paths leave the call running: audio lives
/// in Rust and never depended on the webview being visible.
pub fn window_event(window: &Window, event: &WindowEvent) {
    if !window.state::<Tray>().hides_the_window() {
        return;
    }

    match event {
        // There is no "minimise requested" to intercept — what arrives is the
        // resize the minimise already did, so the window is hidden after the
        // animation rather than instead of it. The visibility check is what
        // keeps [`show`] from undoing itself: it un-minimises while still
        // hidden, and that resize must not read as a fresh minimise.
        WindowEvent::Resized(_)
            if window.is_visible().unwrap_or(false) && window.is_minimized().unwrap_or(false) =>
        {
            let _ = window.hide();
        }
        WindowEvent::CloseRequested { api, .. } => {
            // Hiding rather than closing means the webview survives, so coming
            // back is instant and the UI is still in the room it was in.
            api.prevent_close();
            let _ = window.hide();
        }
        _ => {}
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
}
