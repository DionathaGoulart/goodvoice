//! A goodvoice call with no window: real microphone, real speakers, one room.
//!
//! This is plan.md task 2.4's manual check and the fastest way to hear the
//! voice path work. Run it on two machines — or twice on one, with headphones,
//! since there is no echo cancellation until task 3.4:
//!
//! ```text
//! cargo run --bin call -- --room squad --name rafael
//! cargo run --bin call -- --room squad --name dio --base http://localhost:8787
//! ```
//!
//! `m` toggles mute, `d` toggles deafen, `q` leaves. The roster is printed
//! whenever it changes.

use std::{env, io::BufRead as _, sync::Arc, time::Duration};

use anyhow::{Context as _, Result};
use goodvoice_client_lib::{
    audio::{device::AudioSink, hardware, vad::TransmitMode},
    rtc::session::{Call, CallOptions},
};

const DEFAULT_BASE: &str = "https://goodvoice.goodvoice-server.workers.dev";
const DEFAULT_ROOM: &str = "goodvoice";

#[tokio::main]
async fn main() -> Result<()> {
    let options = args();
    println!(
        "joining {} as {} on {}",
        options.room, options.name, options.base
    );

    let (microphone, speakers) = hardware::open().context("no usable audio device")?;
    println!("audio devices open at 48 kHz");

    let call = Call::join(
        options,
        Box::new(microphone),
        Arc::new(speakers) as Arc<dyn AudioSink>,
    )
    .await
    .context("could not join the room")?;

    println!("connected — you are {}\n", call.self_id());
    println!("m = mute · d = deafen · q = quit\n");

    let call = Arc::new(call);
    tokio::spawn(watch_roster(Arc::clone(&call)));

    read_commands(&call).await;

    // `leave` tells the room before closing, so everyone else sees the
    // departure now rather than on the next heartbeat sweep. The roster
    // watcher holds the other handle; dropping ours first is what lets the
    // call be unwrapped out of the `Arc`.
    if let Ok(call) = Arc::try_unwrap(call) {
        call.leave().await;
    } else {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    println!("left the room");
    Ok(())
}

/// Prints the room whenever it changes.
async fn watch_roster(call: Arc<Call>) {
    let mut roster = call.roster();
    loop {
        let line = {
            let participants = roster.borrow_and_update();
            participants
                .iter()
                .map(|peer| {
                    let mut label = peer.name.clone();
                    if peer.id == call.self_id() {
                        label.push_str(" (you)");
                    }
                    if peer.muted {
                        label.push_str(" [muted]");
                    }
                    if !peer.publishes("mic") {
                        label.push_str(" [no mic]");
                    }
                    label
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!("  room: {line}");

        if roster.changed().await.is_err() {
            return;
        }
    }
}

/// Reads single-letter commands off stdin until the user quits.
///
/// Blocking reads live on a blocking thread: stdin has no async form that
/// works the same on every platform, and starving the runtime would stall the
/// audio tasks.
async fn read_commands(call: &Call) {
    let (lines_tx, mut lines) = tokio::sync::mpsc::channel::<String>(4);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if lines_tx.blocking_send(line).is_err() {
                return;
            }
        }
    });

    while let Some(line) = lines.recv().await {
        match line.trim() {
            "m" => {
                let muted = !call.is_muted();
                call.set_muted(muted).await;
                println!("  {}", if muted { "muted" } else { "unmuted" });
            }
            "d" => {
                let deafened = !call.is_deafened();
                call.set_deafened(deafened).await;
                println!("  {}", if deafened { "deafened" } else { "undeafened" });
            }
            "q" => return,
            "" => {}
            other => println!("  unknown command {other:?} — try m, d or q"),
        }
    }
}

fn args() -> CallOptions {
    let mut options = CallOptions {
        base: env::var("GOODVOICE_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_owned()),
        room: DEFAULT_ROOM.to_owned(),
        name: whoami(),
        // A windowless client has no key to hold and nobody watching a
        // setting, so it talks whenever its microphone does.
        mode: TransmitMode::Open,
    };

    let mut argv = env::args().skip(1);
    while let Some(flag) = argv.next() {
        let Some(value) = argv.next() else {
            eprintln!("{flag} needs a value");
            break;
        };
        match flag.as_str() {
            "--base" => options.base = value,
            "--room" => options.room = value,
            "--name" => options.name = value,
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }

    options
        .base
        .truncate(options.base.trim_end_matches('/').len());
    options
}

/// The host's idea of who is running this, for a display name nobody has to
/// type. Falls back to something rather than refusing to start.
fn whoami() -> String {
    env::var("USER")
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_else(|_| "goodvoice".to_owned())
}
