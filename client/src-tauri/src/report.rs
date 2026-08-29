//! What leaves this machine when something breaks, and what never does.
//!
//! # Three kinds of failure, and why only one of them reports itself
//!
//! A crash reporter catches what kills the process: a `panic!`, or a driver
//! taking the whole thing down. That is [`start`]'s job and it needs no help
//! from the rest of the client.
//!
//! It is not where this app's failures are. A join that never connected, a
//! room that refused, a share that would not open — none of those kill
//! anything. They travel back through `Result` to a window that shows them in
//! red, which is the right behaviour and also the reason nothing would ever be
//! reported: by the time a person sees the message, the failure has been
//! handled. [`failure`] is where those are sent from, by hand, at the few
//! places that know what actually went wrong.
//!
//! The third kind never becomes an error at all — audio that stutters, a
//! microphone that stops after an hour. Nothing here will catch those; the
//! rotating log is what carries them, and a person has to hand it over.
//!
//! # Nothing is sent by default
//!
//! Two independent switches, and both have to be on:
//!
//! 1. **A DSN compiled into this build.** [`DSN`] is `option_env!`, the same
//!    shape `crate::DEFAULT_SERVER` uses, so a self-hoster who builds from
//!    source gets a client that reports nowhere and needs no opt-out.
//! 2. **Consent stored on this machine.** `home::Stored::telemetry` starts as
//!    `None`, which means nobody has been asked yet and is treated as no.
//!
//! # Breadcrumbs are not a log
//!
//! [`note!`] leaves a breadcrumb *and* prints, and the printing is the part
//! that used to be the whole of it. In a release build there is no console —
//! `main.rs` sets `windows_subsystem = "windows"` — so every one of those
//! messages was written into nothing on a user's machine. They travel now, but
//! only attached to an event: a breadcrumb with no failure to explain is
//! dropped, which is why [`failure`] existing at all is what makes [`note!`]
//! worth anything.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

use sentry::{
    protocol::{Event, Level},
    ClientInitGuard, ClientOptions,
};

/// Where reports go, or nowhere.
///
/// Compiled in rather than configured, for the reason `crate::DEFAULT_SERVER`
/// is: a build is a thing somebody can point somewhere else without editing
/// source. Unset — which is every build that does not deliberately set it,
/// including every `cargo run` — and this module does nothing at all.
pub const DSN: Option<&str> = option_env!("GOODVOICE_SENTRY_DSN");

/// Whether this run is reporting. Read on paths that would otherwise pay to
/// build a message nobody will send.
static ON: AtomicBool = AtomicBool::new(false);

/// When each distinct failure was last sent, so a loop cannot spend the month.
///
/// See [`recently_sent`]. A `Mutex` rather than anything cleverer because it is
/// only ever touched from `before_send`, which the SDK calls on the reporting
/// thread and never from the audio path.
static SEEN: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

/// How long the same failure stays quiet after being reported once.
///
/// The number that matters is not this one but what it protects: the free tier
/// is 5,000 events a month, and the encode loop runs 50 times a second. One
/// error inside it is 3,000 events a minute — a month's quota in under two —
/// and the month is spent blind from there.
const QUIET: Duration = Duration::from_secs(60);

/// The most breadcrumbs an event carries.
///
/// Enough to hold a join's whole retry schedule and the reconnect that
/// followed it, which is the longest story this client tells.
const BREADCRUMBS: usize = 50;

/// Whether anything is being reported this run.
#[must_use]
pub fn is_on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Starts reporting, if there is somewhere to report to and somebody said yes.
///
/// The returned guard has to outlive the app: dropping it flushes and shuts the
/// client down. `None` means this run reports nothing, which is the default and
/// not an error.
#[must_use]
pub fn start(consented: bool) -> Option<ClientInitGuard> {
    let dsn = DSN?;
    if !consented {
        return None;
    }

    // Assigned rather than written as a literal: `ClientOptions` is
    // `#[non_exhaustive]`, so a struct expression is refused even with
    // `..Default::default()`.
    let mut options = ClientOptions::default();
    // The version in `tauri.conf.json`, which `release.yml` has already
    // checked against the tag. Symbols are uploaded under the same name, and
    // a mismatch here is what makes a stack trace resolve to nothing.
    options.release = sentry::release_name!();
    // Whether a run ended in a crash, which is the one number that says
    // whether a release is worse than the one before it.
    options.auto_session_tracking = true;
    options.max_breadcrumbs = BREADCRUMBS;
    // No usernames, no IP addresses. A room is people talking; the roster is
    // not diagnostic data.
    options.send_default_pii = false;
    options.before_send = Some(std::sync::Arc::new(|event| {
        if recently_sent(&fingerprint(&event)) {
            None
        } else {
            Some(event)
        }
    }));
    // Tracing stays off — `TracesSamplingStrategy::Disabled` is the default
    // and is left alone on purpose. It would buy a second copy of timings the
    // client already measures and prints, out of the same quota the crashes
    // come from.

    let guard = sentry::init((dsn, options));

    ON.store(true, Ordering::Relaxed);
    Some(guard)
}

/// A stable name for "the same failure as last time".
///
/// The exception's type and value where there is one, the message otherwise.
/// Deliberately not the stack: two arrivals at the same failure through
/// different call paths are the same bug for this purpose, and telling them
/// apart is what would let a loop through.
fn fingerprint(event: &Event<'static>) -> String {
    if let Some(exception) = event.exception.first() {
        return format!(
            "{}:{}",
            exception.ty,
            exception.value.as_deref().unwrap_or_default()
        );
    }
    event
        .message
        .clone()
        .unwrap_or_else(|| event.event_id.to_string())
}

/// Whether this failure has already been sent inside [`QUIET`].
///
/// Records it as sent when it has not, so the caller drops the event.
fn recently_sent(key: &str) -> bool {
    let seen = SEEN.get_or_init(|| Mutex::new(HashMap::new()));
    // A poisoned lock here would mean a panic *inside* the reporter. Reporting
    // nothing is better than panicking again in the middle of the panic that
    // poisoned it, so the failure to lock is read as "let it through".
    let Ok(mut seen) = seen.lock() else {
        return false;
    };
    let now = Instant::now();
    seen.retain(|_, at| now.duration_since(*at) < QUIET);
    if let Some(at) = seen.get(key) {
        if now.duration_since(*at) < QUIET {
            return true;
        }
    }
    seen.insert(key.to_owned(), now);
    false
}

/// Leaves a breadcrumb: something happened that will matter if this run ends
/// badly.
///
/// Prefer the [`note!`] macro, which also prints under a debug build.
pub fn breadcrumb(category: &'static str, message: String) {
    // The file first, and unconditionally. It is the only one of the two that
    // works on a machine that consented to nothing, has no network, and is
    // sitting in front of somebody who can say what they saw.
    log::info!(target: "goodvoice", "[{category}] {message}");
    if !is_on() {
        return;
    }
    sentry::add_breadcrumb(sentry::Breadcrumb {
        category: Some(category.to_owned()),
        message: Some(message),
        level: Level::Info,
        ..Default::default()
    });
}

/// Reports a failure the client handled and recovered from.
///
/// `category` groups the issue and is written at the call site rather than
/// derived, so two failures that mean the same thing land in one issue however
/// their messages differ. `tags` are the axes this app's bugs actually
/// separate along — which audio device, which encoder, how many people in the
/// room — and are worth more here than any stack trace, because several
/// different failures share the same one.
pub fn failure(category: &'static str, message: &str, tags: &[(&str, String)]) {
    log::error!(target: "goodvoice", "[{category}] {message}");
    if !is_on() {
        return;
    }
    let tags: Vec<(String, String)> = tags
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect();
    sentry::with_scope(
        |scope| {
            scope.set_tag("failure", category);
            for (key, value) in &tags {
                scope.set_tag(key, value);
            }
        },
        || {
            sentry::capture_message(&format!("{category}: {message}"), Level::Error);
        },
    );
}

/// Records something true for the rest of this run.
///
/// The Windows build, the CPU and how much memory the machine has arrive on
/// their own; these are the ones only this client knows.
pub fn tag(key: &str, value: &str) {
    if !is_on() {
        return;
    }
    sentry::configure_scope(|scope| scope.set_tag(key, value));
}

/// A diagnostic that is printed while developing and carried on the next
/// failure in release.
///
/// Every call site was an `eprintln!` before this existed, and read well; the
/// only thing wrong with them was that a release build has no console to print
/// to. The `eprintln!` is kept deliberately — a debug run should still say what
/// it is doing in a terminal, without a log file or a Sentry project existing.
///
/// It is *not* guarded on reporting being switched on. The rotating log takes
/// every one of these whatever the consent says, and a machine with no network
/// and no Sentry project is exactly the one whose owner is going to describe a
/// problem out loud and need the file to back it up.
///
/// **Not for the audio callback path.** styleguide.md forbids allocation,
/// locking and logging there, and this does all three. What a device callback
/// has to say travels over the channel it already has, and is noted on the
/// other side.
#[macro_export]
macro_rules! note {
    ($category:literal, $($arg:tt)*) => {{
        let message = format!($($arg)*);
        #[cfg(debug_assertions)]
        eprintln!("{}", message);
        $crate::report::breadcrumb($category, message);
    }};
}

#[cfg(test)]
mod tests {
    use super::{fingerprint, recently_sent, DSN};
    use sentry::protocol::Event;

    #[test]
    fn a_build_reports_nowhere_unless_told_to() {
        // The default every `cargo run`, every test and every self-hoster's
        // build gets. A DSN here would mean the test suite reports to it.
        assert!(DSN.is_none());
    }

    #[test]
    fn the_same_failure_is_sent_once_then_held() {
        let key = "test:the_same_failure";
        assert!(!recently_sent(key), "the first one is always sent");
        assert!(
            recently_sent(key),
            "the second one inside the window is not"
        );
        assert!(recently_sent(key));
    }

    #[test]
    fn different_failures_do_not_hold_each_other_back() {
        assert!(!recently_sent("test:one"));
        assert!(!recently_sent("test:two"));
    }

    #[test]
    fn a_message_event_is_named_by_its_message() {
        let event = Event {
            message: Some("join failed".to_owned()),
            ..Default::default()
        };
        assert_eq!(fingerprint(&event), "join failed");
    }

    #[test]
    fn an_event_with_neither_is_never_confused_with_another() {
        let one = Event::default();
        let two = Event::default();
        assert_ne!(fingerprint(&one), fingerprint(&two));
    }
}
