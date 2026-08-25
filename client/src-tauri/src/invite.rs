//! `goodvoice://join/<room>` — the link somebody sends a friend.
//!
//! plan.md task 6.2. Two halves, and the second one is the reason this is a
//! module rather than three lines in `lib.rs`:
//!
//! - **Making one.** The window's *copy invite* button, which puts a room and
//!   the server it is on into somebody's clipboard.
//! - **Reading one.** What Windows hands the client when a link is clicked —
//!   an argument on a command line, from a browser, an email or a chat window,
//!   which is to say from somewhere entirely outside this program.
//!
//! # Why the server travels with the room
//!
//! goodvoice is self-hosted (prd.md §9), so a room code means nothing without
//! knowing which deploy it is on: two squads on two Workers can both be in
//! `squad-night` and never hear each other. The link carries the origin so
//! that a mismatch can be *said* rather than silently joined.
//!
//! **It is never followed.** A link that could repoint a client at a server of
//! the sender's choosing is a link that can put somebody in a room they did
//! not know was somebody else's, with their microphone open. So an invite for
//! another server is refused with the address in the message, and changing
//! servers stays where it was: a person, typing, in the settings screen
//! (DR-36).

/// What a link asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    /// The room to join, lowercased the way the room itself will.
    pub room: String,
    /// The deploy the sender was on, when the link says.
    pub server: Option<String>,
}

/// The scheme this client registers, and the only one it answers.
pub const SCHEME: &str = "goodvoice";

/// The one action a link can ask for. A second one would be a second verb
/// here rather than a second scheme.
const JOIN: &str = "join";

/// Builds the link the *copy invite* button copies.
#[must_use]
pub fn format(room: &str, server: &str) -> String {
    format!("{SCHEME}://{JOIN}/{room}?s={server}")
}

/// Reads a link, or says why it is not one.
///
/// Deliberately strict: this is the one input to the client that arrives from
/// outside it, and everything here ends in a microphone being opened in a room.
///
/// # Errors
///
/// A sentence for the window to show. Nothing here is recoverable by retrying,
/// so the message says what was wrong with the link rather than what to do.
pub fn parse(url: &str) -> Result<Invite, String> {
    let rest = url
        .trim()
        .strip_prefix(&format!("{SCHEME}://"))
        .ok_or_else(|| format!("that is not a {SCHEME} link"))?;

    // `goodvoice://join/squad` reaches here as `join/squad`, and Windows is
    // fond of adding the trailing slash a URL parser would leave on an
    // authority — `join/squad/` means the same thing.
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    };
    let mut parts = path.trim_end_matches('/').split('/');

    match parts.next() {
        Some(JOIN) => {}
        Some(other) => return Err(format!("this client does not know how to {other}")),
        None => return Err("that link says nothing".to_owned()),
    }

    let room = parts.next().unwrap_or_default();
    if parts.next().is_some() {
        return Err("that link has more in it than a room code".to_owned());
    }
    let room = valid_room(room)?;

    Ok(Invite {
        room,
        server: query.and_then(server_of),
    })
}

/// The room code, as the room itself would have it.
///
/// Mirrors `roomCodeSchema` in `server/src/protocol.ts`, which is what will
/// reject it anyway — the point of checking here is that "that is not a room
/// code" is a better answer than a failed join against a room nobody could
/// ever be in.
fn valid_room(room: &str) -> Result<String, String> {
    let room = room.trim();
    if room.is_empty() {
        return Err("that link has no room in it".to_owned());
    }
    if room.len() < 4 || room.len() > 24 {
        return Err("a room code is 4 to 24 characters".to_owned());
    }
    if !room.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("a room code is letters, numbers and hyphens".to_owned());
    }
    Ok(room.to_ascii_lowercase())
}

/// The `s=` parameter, if there is one worth having.
///
/// Anything unparseable is dropped rather than refused: a link that lost its
/// query string somewhere between a chat client and here is still a link to a
/// room, and the client's own server is the sensible thing to try. What is
/// *not* dropped is a server that disagrees — see [`crate::open_invite`].
fn server_of(query: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == "s")
        .and_then(|(_, value)| crate::home::normalise(value).ok())
}

#[cfg(test)]
mod tests {
    use super::{format, parse, Invite};

    #[test]
    fn a_link_carries_a_room_and_a_server() {
        assert_eq!(
            parse("goodvoice://join/squad-night?s=https://gv.me.workers.dev"),
            Ok(Invite {
                room: "squad-night".to_owned(),
                server: Some("https://gv.me.workers.dev".to_owned()),
            })
        );
    }

    #[test]
    fn a_room_on_its_own_is_a_link() {
        assert_eq!(
            parse("goodvoice://join/squad"),
            Ok(Invite {
                room: "squad".to_owned(),
                server: None
            })
        );
    }

    #[test]
    fn what_windows_does_to_a_url_does_not_matter() {
        // The shell hands over what was clicked, and what was clicked has been
        // through a browser, a chat client and a mail reader first. A trailing
        // slash and surrounding space are the two that actually happen.
        assert_eq!(
            parse("  goodvoice://join/squad/  ").map(|it| it.room),
            Ok("squad".to_owned())
        );
        // And a room code is case-insensitive because the room lowercases it.
        assert_eq!(
            parse("goodvoice://join/SQUAD").map(|it| it.room),
            Ok("squad".to_owned())
        );
    }

    #[test]
    fn a_link_that_is_not_one_is_refused_by_what_is_wrong_with_it() {
        assert!(
            parse("https://example.com/join/squad").is_err(),
            "wrong scheme"
        );
        assert!(parse("goodvoice://leave/squad").is_err(), "unknown verb");
        assert!(parse("goodvoice://join/").is_err(), "no room");
        assert!(parse("goodvoice://join/ab").is_err(), "too short");
        assert!(
            parse("goodvoice://join/squad/extra").is_err(),
            "too much path"
        );
        assert!(
            parse("goodvoice://join/squad night").is_err(),
            "a space is not in the alphabet a room code is written in"
        );
    }

    #[test]
    fn a_server_that_is_not_an_origin_is_dropped_rather_than_refused() {
        // The link still names a room, and the client's own server is the
        // sensible thing to try with it.
        assert_eq!(
            parse("goodvoice://join/squad?s=notaurl"),
            Ok(Invite {
                room: "squad".to_owned(),
                server: None
            })
        );
    }

    #[test]
    fn what_is_copied_is_what_is_read_back() {
        let link = format("squad-night", "https://gv.me.workers.dev");
        assert_eq!(
            link,
            "goodvoice://join/squad-night?s=https://gv.me.workers.dev"
        );
        assert_eq!(
            parse(&link),
            Ok(Invite {
                room: "squad-night".to_owned(),
                server: Some("https://gv.me.workers.dev".to_owned()),
            })
        );
    }
}
