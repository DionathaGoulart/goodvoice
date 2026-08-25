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
//! One string, read once at startup and written when somebody changes it.
//! `tauri-plugin-store` would bring a dependency, an ACL entry per window and
//! a second source of truth for the one setting the window is *not* the
//! authority on.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};

use serde::{Deserialize, Serialize};

/// What is stored, and all that is stored.
///
/// A struct rather than a bare string so that the next thing a self-hoster has
/// to set does not need a second file or a migration.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Stored {
    /// The Worker origin this client joins rooms on. `None` means "whatever
    /// the build was pointed at", which is what a fresh install has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
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
    fn save(&self) {
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
            eprintln!("could not remember the server: {error}");
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

#[cfg(test)]
mod tests {
    use super::{normalise, Home};

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

        let second = Home::open(Some(&directory), FALLBACK);
        assert_eq!(second.server(), "https://squad.workers.dev");
        assert!(second.is_chosen());

        let _ = std::fs::remove_dir_all(&directory);
    }
}
