//! What the tray icon offers when the window is not there (plan.md task 4.2).
//!
//! The menu is the whole app for as long as goodvoice is hidden: mute, deafen,
//! leave, and the way out. Every item here has an equivalent in the window, and
//! neither is the source of truth — the call is. Both are told what it says by
//! [`crate::Controls`], which is why a mute from the tray lights the window's
//! button up and a mute from the window ticks the tray's box.

use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    AppHandle, Wry,
};

use super::TrayError;
use crate::Controls;

/// What a click on the tray menu means.
///
/// Ids and actions are one table so a menu item cannot be added without an arm
/// to answer it — a menu with a dead button in it is worse than one item short.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Show,
    Mute,
    Deafen,
    Leave,
    Quit,
}

impl Action {
    const ALL: [Self; 5] = [
        Self::Show,
        Self::Mute,
        Self::Deafen,
        Self::Leave,
        Self::Quit,
    ];

    const fn id(self) -> &'static str {
        match self {
            Self::Show => "show",
            Self::Mute => "mute",
            Self::Deafen => "deafen",
            Self::Leave => "leave",
            Self::Quit => "quit",
        }
    }

    pub(super) fn of(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.id() == id)
    }
}

/// The tray menu, and the handles to the items that change.
///
/// Held so [`TrayMenu::apply`] can tick and untick boxes: a menu that shows
/// "Mute" while the microphone is muted is a menu that lies about the one thing
/// a hidden client most needs to know.
pub(super) struct TrayMenu {
    menu: Menu<Wry>,
    mute: CheckMenuItem<Wry>,
    deafen: CheckMenuItem<Wry>,
    leave: MenuItem<Wry>,
}

impl TrayMenu {
    /// # Errors
    ///
    /// [`TrayError::Unavailable`] when the host will not build a menu.
    pub(super) fn build(app: &AppHandle) -> Result<Self, TrayError> {
        let open = MenuItem::with_id(app, Action::Show.id(), "Open goodvoice", true, NO_KEY)?;
        // Unchecked and disabled until a call says otherwise: the tray is up
        // before anyone has joined anything.
        let mute = CheckMenuItem::with_id(app, Action::Mute.id(), "Mute", false, false, NO_KEY)?;
        let deafen =
            CheckMenuItem::with_id(app, Action::Deafen.id(), "Deafen", false, false, NO_KEY)?;
        let leave = MenuItem::with_id(app, Action::Leave.id(), "Leave room", false, NO_KEY)?;
        let quit = MenuItem::with_id(app, Action::Quit.id(), "Quit goodvoice", true, NO_KEY)?;

        // Separated into what the window does, what the call does, and the
        // way out: three groups of one thought each.
        let menu = Menu::with_items(
            app,
            &[
                &open,
                &PredefinedMenuItem::separator(app)?,
                &mute,
                &deafen,
                &leave,
                &PredefinedMenuItem::separator(app)?,
                &quit,
            ],
        )?;

        Ok(Self {
            menu,
            mute,
            deafen,
            leave,
        })
    }

    pub(super) fn menu(&self) -> &Menu<Wry> {
        &self.menu
    }

    /// Puts the menu in step with the call.
    ///
    /// Failures are dropped: a tick that will not go on is a menu slightly out
    /// of date, and there is nothing useful to do about it from here.
    pub(super) fn apply(&self, controls: Controls) {
        let _ = self.mute.set_checked(controls.muted);
        let _ = self.mute.set_enabled(controls.in_call);
        let _ = self.deafen.set_checked(controls.deafened);
        let _ = self.deafen.set_enabled(controls.in_call);
        let _ = self.leave.set_enabled(controls.in_call);
    }
}

/// No accelerators anywhere in this menu. A tray menu is opened with the mouse,
/// and the one shortcut goodvoice wants is the global push-to-talk key, which
/// is task 4.3 and nothing to do with a menu.
const NO_KEY: Option<&str> = None;

#[cfg(test)]
mod tests {
    use super::Action;
    use std::collections::HashSet;

    #[test]
    fn every_item_in_the_menu_has_an_arm_to_answer_it() {
        for action in Action::ALL {
            assert_eq!(
                Action::of(action.id()),
                Some(action),
                "{} is in the menu with nothing behind it",
                action.id()
            );
        }
    }

    #[test]
    fn no_two_items_share_an_id() {
        // Two items with one id is one dead item, and which one dies depends on
        // the order they were added in.
        let ids: HashSet<&str> = Action::ALL.iter().map(|action| action.id()).collect();
        assert_eq!(ids.len(), Action::ALL.len());
    }

    #[test]
    fn a_click_that_means_nothing_here_does_nothing() {
        assert_eq!(Action::of("share"), None);
        assert_eq!(Action::of(""), None);
    }
}
