//! System tray icon, tray menu and the global push-to-talk hotkey.
//!
//! The window is not the app. A voice client spends almost all of its time with
//! nothing to show — the call runs in Rust and does not care whether a webview
//! exists — so closing the window puts goodvoice in the notification area
//! rather than ending the call (plan.md task 4.1). The menu that grows out of
//! this is task 4.2 and the global hotkey is 4.3.
//!
//! # The trap this must not set
//!
//! An app whose close button hides the window and whose tray icon failed to
//! appear cannot be quit at all. So the two are wired together: close-to-tray
//! is only in force while [`Tray::installed`] says there is a tray to close
//! into, and a host that could not give us one keeps an ordinary window that
//! ordinarily closes.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use tauri::{
    menu::{Menu, MenuEvent, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager as _, Window, WindowEvent,
};
use thiserror::Error;

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
#[derive(Debug, Default)]
pub struct Tray {
    installed: AtomicBool,
}

impl Tray {
    fn mark_installed(&self) {
        self.installed.store(true, Ordering::Release);
    }

    /// Whether closing the window should hide it instead.
    #[must_use]
    pub fn hides_the_window(&self) -> bool {
        self.installed.load(Ordering::Acquire)
    }
}

/// What a click on the tray menu means.
///
/// The mapping from a menu id to an action is the part of this module that is
/// worth testing, and the part task 4.2 grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Show,
    Quit,
}

const SHOW_ID: &str = "show";
const QUIT_ID: &str = "quit";

fn action_of(id: &str) -> Option<Action> {
    match id {
        SHOW_ID => Some(Action::Show),
        QUIT_ID => Some(Action::Quit),
        _ => None,
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
    let open_item = MenuItem::with_id(app, SHOW_ID, "Open goodvoice", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, QUIT_ID, "Quit goodvoice", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_item, &quit_item])?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or(TrayError::NoIcon)?;

    TrayIconBuilder::with_id("goodvoice")
        .icon(icon)
        .tooltip("goodvoice")
        .menu(&menu)
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

    app.state::<Tray>().mark_installed();
    Ok(())
}

fn on_menu_event(app: &AppHandle, event: &MenuEvent) {
    match action_of(event.id.as_ref()) {
        Some(Action::Show) => show(app),
        Some(Action::Quit) => quit(app),
        None => {}
    }
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
        let _ = tokio::time::timeout(LEAVE_GRACE, crate::end_call(&app)).await;
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
    use super::{action_of, Action, Tray, QUIT_ID, SHOW_ID};

    #[test]
    fn the_menu_ids_are_the_ones_the_menu_is_built_with() {
        assert_eq!(action_of(SHOW_ID), Some(Action::Show));
        assert_eq!(action_of(QUIT_ID), Some(Action::Quit));
    }

    #[test]
    fn an_unknown_menu_id_does_nothing() {
        // Task 4.2 adds mute, deafen and leave. Until it does, a click that
        // means nothing here has to mean nothing at all rather than falling
        // through to whichever arm happens to be last.
        assert_eq!(action_of("mute"), None);
        assert_eq!(action_of(""), None);
    }

    #[test]
    fn a_window_with_no_tray_behind_it_closes_normally() {
        // The trap this guards: close-to-tray plus a tray that never appeared
        // is an app that cannot be quit.
        let tray = Tray::default();
        assert!(
            !tray.hides_the_window(),
            "close-to-tray was in force before a tray existed"
        );

        tray.mark_installed();
        assert!(tray.hides_the_window());
    }
}
