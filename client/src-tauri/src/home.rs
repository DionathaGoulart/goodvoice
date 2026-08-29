//! Which deploy this client talks to, and where that is remembered.
//!
//! prd.md §9's self-hosting story ends "paste the Worker URL into the client's
//! settings", and until plan.md task 6.1 there was nowhere to paste it: the
//! server was `GOODVOICE_SERVER` at build time, so pointing a client at your
//! own Worker meant installing a Rust toolchain and MSVC and rebuilding — for
//! everyone in the squad, not just the person who deployed it.
//!
//! # Why it is not `localStorage`
//!
//! The window is not the only thing that joins. `GOODVOICE_AUTOJOIN` joins
//! before a webview exists (task 4.4), and task 6.2's `goodvoice://join/<room>`
//! link will too — neither can ask a window which server it had in mind. So it
//! is kept where the client can read it without one: a small JSON file in the
//! app's config directory, written whole on every change.
//!
//! # Why a file rather than a plugin
//!
//! Three small things, read once at startup and written when they change — the
//! server, the rectangle the window was last left at ([`crate::place`]), and
//! the language the tray menu is in ([`crate::lang`]).
//! `tauri-plugin-store` would bring a dependency, an ACL entry per window and
//! a second source of truth for the one setting the window is *not* the
//! authority on. The window's own position is the second, and it is written
//! from Rust for the same reason: the window is gone by the time it is known.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};

use serde::{Deserialize, Serialize};

use crate::{lang::Language, place::Placement};

/// What is stored, and all that is stored.
///
/// A struct rather than a bare string so that the next thing a self-hoster has
/// to set does not need a second file or a migration. The window's rectangle
/// was the second thing and the language is the third; neither needed either.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Stored {
    /// The Worker origin this client joins rooms on. `None` means "whatever
    /// the build was pointed at", which is what a fresh install has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// Where the window was last seen. `None` is a machine that has never had
    /// a window on it, and means "let Windows choose", which is what every
    /// window did before [`crate::place`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Placement>,
    /// What language the tray menu is in. `None` is a client no window has
    /// ever mounted on — a fresh install, whose first window tells it within
    /// the second it takes to paint (`ui/i18n.ts`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
    /// Whether crash reports may leave this machine.
    ///
    /// Three states, and the third is the point: `Some(true)` is yes,
    /// `Some(false)` is no, and **`None` is nobody has been asked yet** —
    /// which [`reports_allowed`] reads as no. A voice client that
    /// started sending on a default would be sending because somebody
    /// installed it, not because they agreed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<bool>,
}

/// The file name inside the app's config directory.
const FILE: &str = "settings.json";

/// The chosen server, remembered across runs.
///
/// Held in memory behind an `RwLock` because every join reads it and almost
/// nothing writes it. The file is the record; this is the copy that answers.
pub struct Home {
    /// Where [`Stored`] lives. `None` when the host would not name a config
    /// directory, which leaves the setting working for this run and forgotten
    /// on the next — better than refusing to start.
    path: Option<PathBuf>,
    stored: RwLock<Stored>,
    /// Where this build joins rooms when nothing has been chosen.
    fallback: &'static str,
}

impl Home {
    /// Reads what was stored, if anything.
    ///
    /// A file that cannot be read or parsed is treated as absent: a client
    /// that refused to start because a settings file was truncated would be
    /// worse than one that starts on its defaults and can be told again.
    #[must_use]
    pub fn open(directory: Option<&Path>, fallback: &'static str) -> Self {
        let path = directory.map(|directory| directory.join(FILE));
        let stored = path
            .as_deref()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<Stored>(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            stored: RwLock::new(stored),
            fallback,
        }
    }

    /// The server to join on: what was chosen, or what this build ships with.
    #[must_use]
    pub fn server(&self) -> String {
        self.stored
            .read()
            .ok()
            .and_then(|stored| stored.server.clone())
            .unwrap_or_else(|| self.fallback.to_owned())
    }

    /// Whether the answer above came from a person rather than from the build.
    #[must_use]
    pub fn is_chosen(&self) -> bool {
        self.stored
            .read()
            .is_ok_and(|stored| stored.server.is_some())
    }

    /// What this build ships with, for a window offering to go back to it.
    #[must_use]
    pub const fn fallback(&self) -> &'static str {
        self.fallback
    }

    /// The language the client's own words are in, or English until a window
    /// has said otherwise.
    #[must_use]
    pub fn language(&self) -> Language {
        self.stored
            .read()
            .ok()
            .and_then(|stored| stored.language)
            .unwrap_or_default()
    }

    /// Records the language and writes the file.
    ///
    /// Returns whether this actually changed anything, so the caller can skip
    /// rewriting a menu that already says the right thing — `i18n.ts` sends
    /// this on **every** window mount, and a window is rebuilt on every trip
    /// back from the tray (task 4.6), so the unchanged case is the common one.
    pub fn choose_language(&self, language: Language) -> bool {
        {
            let Ok(mut stored) = self.stored.write() else {
                return false;
            };
            if stored.language == Some(language) {
                return false;
            }
            stored.language = Some(language);
        }
        self.save();
        true
    }

    /// Whether crash reports may be sent, and whether the question has been
    /// put yet.
    ///
    /// `None` is the fresh install the window turns into a question.
    #[must_use]
    pub fn telemetry(&self) -> Option<bool> {
        self.stored.read().ok().and_then(|stored| stored.telemetry)
    }

    /// Records the answer and writes the file.
    ///
    /// Returns whether anything changed, for the same reason
    /// [`Home::choose_language`] does: the window sends its whole state on
    /// every mount, and a mount happens on every trip back from the tray.
    pub fn choose_telemetry(&self, allowed: bool) -> bool {
        {
            let Ok(mut stored) = self.stored.write() else {
                return false;
            };
            if stored.telemetry == Some(allowed) {
                return false;
            }
            stored.telemetry = Some(allowed);
        }
        self.save();
        true
    }

    /// Where the window was last seen, as it was written.
    ///
    /// Unchecked: whether that rectangle is still on a screen is
    /// [`crate::place::remembered`]'s question, and it needs monitors this
    /// type has never heard of.
    #[must_use]
    pub fn window(&self) -> Option<Placement> {
        self.stored.read().ok().and_then(|stored| stored.window)
    }

    /// Records the window's rectangle **without writing the file**.
    ///
    /// Every move and every resize comes through here (`place::note`), which
    /// is hundreds of calls for one drag. [`Home::save`] is the trip to the
    /// disk, and it happens once, when the window goes away.
    pub fn note_window(&self, place: Placement) {
        if let Ok(mut stored) = self.stored.write() {
            stored.window = Some(place);
        }
    }

    /// Points this client at `url`, or back at the build's own server when it
    /// is empty.
    ///
    /// Returns the origin as it was stored, which is not always what was
    /// typed: see [`normalise`].
    ///
    /// # Errors
    ///
    /// The prose to show the person who typed it, when what they typed is not
    /// an origin this client could ever join.
    pub fn choose(&self, url: &str) -> Result<String, String> {
        let chosen = if url.trim().is_empty() {
            None
        } else {
            Some(normalise(url)?)
        };

        {
            let mut stored = self
                .stored
                .write()
                .map_err(|_| "the settings are unreadable".to_owned())?;
            stored.server.clone_from(&chosen);
        }
        self.save();
        Ok(chosen.unwrap_or_else(|| self.fallback.to_owned()))
    }

    /// Writes the whole file, and says nothing if it cannot.
    ///
    /// A setting that did not persist is worth less than one that did and is
    /// not worth failing the change over: the client is already pointed at the
    /// new server for this run, and the alternative is a dialog about a
    /// directory nobody can do anything about.
    ///
    /// Public because [`Home::note_window`] deliberately does not call it —
    /// see there.
    pub fn save(&self) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        let Ok(stored) = self.stored.read() else {
            return;
        };
        let Ok(text) = serde_json::to_string_pretty(&*stored) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(error) = fs::write(path, text) {
            crate::note!("settings", "could not write the settings: {error}");
        }
    }
}

/// What a person typed, as an origin the client can join.
///
/// Deliberately strict about two things and forgiving about the rest. A
/// trailing slash is removed, because every path this client builds starts with
/// one and `https://x//rooms/y` is a 404 that reads like a bug in the room
/// code. A path, a query or a fragment is refused outright rather than
/// silently dropped: somebody who pasted a dashboard URL should be told, not
/// quietly pointed at its origin.
///
/// # Errors
///
/// A sentence to put in front of the person, never a code.
pub fn normalise(url: &str) -> Result<String, String> {
    let trimmed = url.trim().trim_end_matches('/');
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .ok_or_else(|| "start with https:// (or http:// for a local Worker)".to_owned())?;

    if rest.is_empty() {
        return Err("that is a scheme with no host after it".to_owned());
    }
    if let Some(index) = rest.find(['/', '?', '#']) {
        return Err(format!(
            "the client wants the origin only — drop the {} and everything after it",
            &rest[index..=index]
        ));
    }
    if rest.contains(' ') {
        return Err("a host cannot contain a space".to_owned());
    }
    // A host with no dot and no port is a hostname like `localhost`, which is
    // fine, or a typo like `https:goodvoice`, which is not — and cannot be
    // told apart here. Both reach the join and fail with the connection error
    // they deserve.
    Ok(trimmed.to_owned())
}

/// This bundle's identifier, spelled out rather than asked for.
///
/// `app.path().app_config_dir()` is the authority and is what [`Home`] is
/// given in `setup`. The consent gate cannot wait for it: the crash reporter
/// has to start before the Tauri builder exists (`report`), and it
/// must not start on a machine where nobody agreed to it. So the directory is
/// worked out twice, and [`tests::the_identifier_matches_the_manifest`] is
/// what keeps the two spellings from drifting.
pub const IDENTIFIER: &str = "art.good.goodvoice";

/// Where [`Stored`] lives, worked out before Tauri can be asked.
///
/// Windows only, deliberately: `%APPDATA%` is where Tauri's own resolution
/// lands on the one platform this client ships on, and a wrong guess anywhere
/// else would be worse than no guess. `None` elsewhere means the early read
/// finds nothing, which is read as "not consented" — the safe direction.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join(IDENTIFIER))
    } else {
        None
    }
}

/// Whether crash reports may be sent, read straight off the disk.
///
/// For the one caller that runs before there is an app to hold a [`Home`].
/// Every way this can fail — no directory, no file, a truncated file, the key
/// absent because nobody has been asked — answers the same way: no.
#[must_use]
pub fn reports_allowed() -> bool {
    config_dir()
        .map(|directory| directory.join(FILE))
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Stored>(&text).ok())
        .and_then(|stored| stored.telemetry)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{normalise, Home, IDENTIFIER};
    use crate::lang::Language;

    const FALLBACK: &str = "https://goodvoice.example.workers.dev";

    #[test]
    fn an_origin_survives_and_a_trailing_slash_does_not() {
        assert_eq!(
            normalise("https://gv.me.workers.dev/"),
            Ok("https://gv.me.workers.dev".to_owned())
        );
        assert_eq!(
            normalise("  http://localhost:8787  "),
            Ok("http://localhost:8787".to_owned())
        );
    }

    #[test]
    fn what_a_person_is_likely_to_paste_is_refused_with_a_reason() {
        // The dashboard URL of a Worker, which is the thing on screen when
        // somebody is looking for its address.
        let refused = normalise("https://dash.cloudflare.com/abc/workers/services/view/goodvoice")
            .expect_err("a path is not an origin");
        assert!(refused.contains("origin only"), "{refused}");

        assert!(normalise("goodvoice.workers.dev").is_err(), "no scheme");
        assert!(normalise("https://").is_err(), "no host");
    }

    #[test]
    fn nothing_chosen_means_the_build_answers() {
        let home = Home::open(None, FALLBACK);
        assert_eq!(home.server(), FALLBACK);
        assert!(!home.is_chosen());
    }

    #[test]
    fn choosing_and_unchoosing_a_server() {
        let home = Home::open(None, FALLBACK);

        assert_eq!(
            home.choose("https://mine.workers.dev/"),
            Ok("https://mine.workers.dev".to_owned())
        );
        assert_eq!(home.server(), "https://mine.workers.dev");
        assert!(home.is_chosen());

        // Empty is how a window says "back to whatever this build shipped
        // with", which is not the same as an invalid URL.
        assert_eq!(home.choose("  "), Ok(FALLBACK.to_owned()));
        assert!(!home.is_chosen());
        assert_eq!(home.server(), FALLBACK);
    }

    #[test]
    fn a_refused_url_changes_nothing() {
        let home = Home::open(None, FALLBACK);
        home.choose("https://mine.workers.dev").expect("chosen");
        assert!(home.choose("nonsense").is_err());
        assert_eq!(home.server(), "https://mine.workers.dev");
    }

    #[test]
    fn a_fresh_client_is_english_until_a_window_says_otherwise() {
        let home = Home::open(None, FALLBACK);
        assert_eq!(home.language(), Language::English);

        // The answer a window's every mount gets: nothing changed, so nothing
        // is rewritten.
        assert!(home.choose_language(Language::BrazilianPortuguese));
        assert_eq!(home.language(), Language::BrazilianPortuguese);
        assert!(!home.choose_language(Language::BrazilianPortuguese));
    }

    #[test]
    fn what_was_written_is_read_back() {
        let directory = std::env::temp_dir().join(format!(
            "goodvoice-home-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos())
        ));
        std::fs::create_dir_all(&directory).expect("a directory to write into");

        let first = Home::open(Some(&directory), FALLBACK);
        first.choose("https://squad.workers.dev").expect("chosen");
        first.choose_language(Language::BrazilianPortuguese);

        let second = Home::open(Some(&directory), FALLBACK);
        assert_eq!(second.server(), "https://squad.workers.dev");
        assert!(second.is_chosen());
        // The whole file is rewritten on every change, so the thing to check
        // is that the *other* settings survived the language being written.
        assert_eq!(second.language(), Language::BrazilianPortuguese);

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// The consent gate reads the settings file before Tauri exists to say
    /// where it is, so `IDENTIFIER` is a second copy of the manifest's. A
    /// rename that changed one and not the other would move the file and
    /// leave the gate reading an empty directory — which fails closed, and so
    /// would silently stop reporting rather than break.
    #[test]
    fn the_identifier_matches_the_manifest() {
        let manifest = include_str!("../tauri.conf.json");
        let config: serde_json::Value =
            serde_json::from_str(manifest).expect("tauri.conf.json parses");
        assert_eq!(
            config["identifier"].as_str(),
            Some(IDENTIFIER),
            "home::IDENTIFIER and tauri.conf.json have drifted apart"
        );
    }

    #[test]
    fn nobody_asked_means_no() {
        let directory = std::env::temp_dir().join("goodvoice-telemetry-test");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("temp dir");

        let home = Home::open(Some(&directory), FALLBACK);
        assert_eq!(home.telemetry(), None, "a fresh install has not been asked");

        assert!(home.choose_telemetry(true));
        assert!(
            !home.choose_telemetry(true),
            "saying yes twice changes nothing"
        );
        assert_eq!(home.telemetry(), Some(true));

        assert!(home.choose_telemetry(false));
        assert_eq!(
            Home::open(Some(&directory), FALLBACK).telemetry(),
            Some(false),
            "the answer outlives the run"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }
}
