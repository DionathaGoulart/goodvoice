//! Which language the client's own words are in.
//!
//! The window has its own catalog (`ui/strings.ts`) and does not need this.
//! What needs it is everything the window is not: the **tray menu**, which is
//! the whole of goodvoice while the webview is destroyed (task 4.6), and which
//! is built during `setup` — before any webview exists, and therefore before
//! anything could have asked one what language it is in.
//!
//! # How the two stay in step
//!
//! The window is the authority and the client is told. `i18n.ts` calls
//! `set_language` on every mount and on every change, and [`crate::home::Home`]
//! writes the tag down beside the server and the window's rectangle. So a
//! fresh install's tray is English for as long as it takes the first window to
//! mount, and correct from the second run onwards — with no OS locale API on
//! the Rust side at all, which is a Windows call that would have to be
//! `#[cfg]`-ed away for the tests that run everywhere else.
//!
//! # Why the strings are `&'static str` in a `match`
//!
//! Five words, twice. A catalog file, a lookup and a fallback for a missing
//! key would all be machinery for a table small enough to read in one screen —
//! and this way a language added without a `Leave room` in it does not
//! compile.

use serde::{Deserialize, Serialize};

/// The languages this build speaks. Kept in step with `Lang` in
/// `ui/strings.ts`; the wire value is the BCP-47 tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Language {
    /// The default, and what an unrecognised tag falls back to.
    #[default]
    #[serde(rename = "en")]
    English,
    #[serde(rename = "pt-BR")]
    BrazilianPortuguese,
}

impl Language {
    /// What a BCP-47 tag means here.
    ///
    /// Matched on the primary subtag rather than the whole thing, and
    /// case-insensitively, because `navigator.language` is `pt-BR` on one
    /// machine and `pt-br` or plain `pt` on the next. Any Portuguese gets the
    /// Brazilian catalog: it is the only Portuguese this build has, and it is
    /// far closer to a European Portuguese speaker than English is.
    #[must_use]
    pub fn of(tag: &str) -> Self {
        let primary = tag
            .split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if primary == "pt" {
            Self::BrazilianPortuguese
        } else {
            Self::English
        }
    }

    /// The tag this is stored and sent as.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::BrazilianPortuguese => "pt-BR",
        }
    }

    /// What the tray menu says in this language.
    #[must_use]
    pub const fn tray(self) -> TrayWords {
        match self {
            Self::English => TrayWords {
                open: "Open goodvoice",
                mute: "Mute",
                deafen: "Deafen",
                leave: "Leave room",
                quit: "Quit goodvoice",
            },
            // Sentence case and infinitives, which is how Windows' own menus
            // read in Brazilian Portuguese — "Abrir", not "Abra".
            Self::BrazilianPortuguese => TrayWords {
                open: "Abrir o goodvoice",
                mute: "Silenciar",
                deafen: "Desligar o áudio",
                leave: "Sair da sala",
                quit: "Fechar o goodvoice",
            },
        }
    }
}

/// The five items in the tray menu (`tray::menu`).
///
/// A struct rather than five methods so that the menu builds from one value
/// and [`crate::tray::menu::TrayMenu::relabel`] rewrites from the same one —
/// there is no way for the two to disagree about how many items there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayWords {
    pub open: &'static str,
    pub mute: &'static str,
    pub deafen: &'static str,
    pub leave: &'static str,
    pub quit: &'static str,
}

#[cfg(test)]
mod tests {
    use super::Language;

    #[test]
    fn every_shape_of_portuguese_a_browser_sends() {
        for tag in ["pt", "pt-BR", "pt-br", "pt_BR", "pt-PT"] {
            assert_eq!(
                Language::of(tag),
                Language::BrazilianPortuguese,
                "{tag} did not reach the only Portuguese this build has"
            );
        }
    }

    #[test]
    fn anything_else_is_english_rather_than_a_failure() {
        // Including the two that would be a panic if `of` indexed instead of
        // matching: nothing, and a tag that is only a separator.
        for tag in ["en", "en-GB", "es-AR", "ptx", "", "-"] {
            assert_eq!(Language::of(tag), Language::English, "{tag}");
        }
    }

    #[test]
    fn a_tag_survives_the_round_trip() {
        for language in [Language::English, Language::BrazilianPortuguese] {
            assert_eq!(Language::of(language.tag()), language);
        }
    }

    #[test]
    fn no_menu_item_is_missing_or_shared_between_languages() {
        for language in [Language::English, Language::BrazilianPortuguese] {
            let words = language.tray();
            let items = [
                words.open,
                words.mute,
                words.deafen,
                words.leave,
                words.quit,
            ];
            assert!(
                items.iter().all(|item| !item.trim().is_empty()),
                "{} has an empty menu item",
                language.tag()
            );
            let unique: std::collections::HashSet<&str> = items.into_iter().collect();
            assert_eq!(
                unique.len(),
                items.len(),
                "{} says the same thing twice, so one item is unreadable",
                language.tag()
            );
        }
    }
}
