//! A second person in the room, made of numbers.
//!
//! Several things in plan.md are marked done-but-unverified for the same
//! reason: the last step is "and somebody hears you". Task 3.3's push to talk,
//! task 3.2's mute, and task 3.4's speaker echo all end that way, and all three
//! are one person short rather than one feature short.
//!
//! This is that person. It joins a room, publishes either silence or a tone,
//! and once a second says what it is receiving: how many frames arrived, how
//! loud they were, and — the part task 3.4 needs — how much of its *own* tone
//! came back.
//!
//! ```text
//! cargo run -p goodvoice-harness --bin listener -- --room squad
//! cargo run -p goodvoice-harness --bin listener -- --room squad --tone 1200
//! ```
//!
//! # Reading the echo column
//!
//! With `--tone`, the room hears a steady tone from here. On the other end it
//! comes out of a loudspeaker, goes into a microphone, and — if the echo
//! canceller does its job — does not come back. The `echo` column is the
//! energy at that exact frequency in what does come back, against the energy
//! at that frequency in the tone as sent. That ratio in decibels is what task
//! 3.4's definition of done is asking about, measured instead of judged.
//!
//! # Reading the roster lines
//!
//! Frames are what a second person *hears*; the roster is what they *see*.
//! plan.md §7.2 checks the tray menu against both halves of the app and
//! against a roommate, and that last column is this: whenever the room
//! broadcasts a roster whose flags differ from the last one, a line goes out
//! saying who is in it and what they are.
//!
//! ```text
//! roster @ 4s   anon muted | listener (you)
//! ```
//!
//! Only the flags are compared, so a level meter moving does not print and a
//! mute does. A line for every second would drown the table it shares.
//!
//! # One slot
//!
//! Everything is read from slot 0, which is the first remote speaker the call
//! subscribed to. That is exactly right with two people in the room and wrong
//! with three, so keep the room to two.

use std::{
    env,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use goodvoice_client_lib::{
    audio::{
        device::{AudioSink, RecordingSink, ToneSource},
        mixer::peak,
        opus::Frame,
        prefs::AudioPrefs,
        tone::{bin_energy, db_below, frame as tone_frame, DEFAULT_HZ},
        vad::TransmitMode,
    },
    rtc::{
        session::{Call, CallOptions},
        signaling::Participant,
    },
};

const DEFAULT_BASE: &str = "https://goodvoice.goodvoice-server.workers.dev";
const DEFAULT_ROOM: &str = "goodvoice";

#[tokio::main]
async fn main() -> Result<()> {
    let Args {
        base,
        room,
        name,
        tone,
        seconds,
    } = args();

    println!("joining {room} on {base} as {name}");
    match tone {
        Some(hz) => println!("publishing a {hz:.0} Hz tone — the room will hear it steadily"),
        None => println!("publishing silence — nothing here will be heard"),
    }

    let ears = RecordingSink::new();
    let call = Call::join(
        CallOptions {
            base,
            room,
            name,
            // Open, always: a gate here would make the tone come and go and
            // the far end's echo column meaningless.
            mode: TransmitMode::Open,
            prefs: std::sync::Arc::new(AudioPrefs::default()),
        },
        // 0 Hz is a sine of zero: a source that is genuinely silent rather
        // than one that is merely quiet.
        Box::new(ToneSource::new(tone.unwrap_or(0.0))),
        Arc::clone(&ears) as Arc<dyn AudioSink>,
    )
    .await
    .context("could not join the room")?;

    println!("connected — {}\n", call.self_id());
    println!("  time   frames/s   peak   {}", header(tone));

    let reference = tone.map(|hz| bin_energy(&tone_frame(hz), hz));
    let started = Instant::now();
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    let mut previous = 0_usize;
    let roster = call.roster();
    let mut seen_roster = String::new();

    while started.elapsed() < Duration::from_secs(seconds) {
        ticker.tick().await;
        let record = ears.slot(0);
        let arrived = record.frames.saturating_sub(previous);
        previous = record.frames;

        // Before the table row, because a mute is the reason the row after it
        // reads zero and a line that explains a silence belongs above it.
        let now = flags(&roster.borrow(), &call.self_id());
        if now != seen_roster {
            println!("  roster @ {}s   {now}", started.elapsed().as_secs());
            seen_roster = now;
        }

        println!(
            "  {:>4}s   {arrived:>8}   {:>4}   {}",
            started.elapsed().as_secs(),
            peak(&record.last),
            column(&record.last, tone, reference, arrived),
        );
    }

    call.leave().await;
    println!("\nleft the room.");
    Ok(())
}

/// The room as this second person sees it: everyone in it, and what each of
/// them is.
///
/// Sorted by arrival rather than left in the order the broadcast happened to
/// use, because the caller prints this only when it *changes* and two orderings
/// of the same room would read as a change.
fn flags(roster: &[Participant], me: &str) -> String {
    let mut room: Vec<&Participant> = roster.iter().collect();
    room.sort_unstable_by(|a, b| a.joined_at.cmp(&b.joined_at).then(a.id.cmp(&b.id)));
    room.iter()
        .map(|peer| {
            let mut line = peer.name.clone();
            if peer.muted {
                line.push_str(" muted");
            }
            if peer.deafened {
                line.push_str(" deafened");
            }
            if peer.sharing {
                line.push_str(" sharing");
            }
            if peer.id == me {
                line.push_str(" (you)");
            }
            line
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn header(tone: Option<f32>) -> &'static str {
    if tone.is_some() {
        "echo (dB below what was sent — higher is better)"
    } else {
        ""
    }
}

/// What came back of our own tone, as decibels below what went out.
fn column(frame: &Frame, tone: Option<f32>, reference: Option<f32>, arrived: usize) -> String {
    let (Some(hz), Some(reference)) = (tone, reference) else {
        return String::new();
    };
    if arrived == 0 {
        // Nothing arrived, so there is nothing to say about it. Reporting a
        // clean cancellation here would be reporting on silence.
        return "— nothing arriving".to_owned();
    }

    let returned = bin_energy(frame, hz);
    let Some(db) = db_below(reference, returned) else {
        return ">60 (nothing came back)".to_owned();
    };
    format!("{db:.1}")
}

struct Args {
    base: String,
    room: String,
    name: String,
    tone: Option<f32>,
    seconds: u64,
}

fn args() -> Args {
    let mut base = env::var("GOODVOICE_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_owned());
    let mut room = DEFAULT_ROOM.to_owned();
    let mut name = "listener".to_owned();
    let mut tone = None;
    let mut seconds = 120;

    let mut argv = env::args().skip(1);
    while let Some(flag) = argv.next() {
        match (flag.as_str(), argv.next()) {
            ("--base", Some(value)) => base = value,
            ("--room", Some(value)) => room = value,
            ("--name", Some(value)) => name = value,
            ("--tone", Some(value)) => tone = value.parse().ok().or(Some(DEFAULT_HZ)),
            ("--seconds", Some(value)) => seconds = value.parse().unwrap_or(seconds),
            (other, _) => eprintln!("ignoring unknown argument {other}"),
        }
    }

    Args {
        base: base.trim_end_matches('/').to_owned(),
        room,
        name,
        tone,
        seconds,
    }
}

#[cfg(test)]
mod tests {
    use super::{flags, Participant};

    fn peer(id: &str, name: &str, joined_at: u64) -> Participant {
        Participant {
            id: id.to_owned(),
            name: name.to_owned(),
            joined_at,
            muted: false,
            deafened: false,
            sharing: false,
            session_id: None,
            tracks: Vec::new(),
        }
    }

    #[test]
    fn a_quiet_room_is_just_names() {
        let room = vec![peer("a", "anon", 1), peer("b", "listener", 2)];
        assert_eq!(flags(&room, "b"), "anon | listener (you)");
    }

    #[test]
    fn every_flag_the_tray_menu_can_set_is_named() {
        // plan.md §7.2 walks mute, deafen and leave against a roommate. This
        // is the roommate's half of that table, so all three have to show.
        let mut room = vec![peer("a", "anon", 1)];
        room[0].muted = true;
        room[0].deafened = true;
        room[0].sharing = true;
        assert_eq!(flags(&room, "b"), "anon muted deafened sharing");
    }

    #[test]
    fn the_order_is_arrival_and_not_the_broadcast_s() {
        // The caller prints only on a change, so two orderings of one room
        // would read as somebody muting.
        let early = peer("z", "early", 1);
        let late = peer("a", "late", 2);
        assert_eq!(
            flags(&[early.clone(), late.clone()], "?"),
            flags(&[late, early], "?")
        );
    }

    #[test]
    fn an_empty_room_says_nothing_rather_than_something_wrong() {
        assert_eq!(flags(&[], "b"), "");
    }
}
