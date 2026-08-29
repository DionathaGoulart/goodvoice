//! Provokes each of the three failures on purpose, so the reporting path can
//! be proved instead of assumed.
//!
//! Everything the client reports is, by design, something nobody wants to
//! happen. That leaves one question with no cheap answer: **does any of it
//! actually arrive?** Until this existed, finding out meant editing a `panic!`
//! into a command, building, clicking it, and remembering to take it back out
//! — which is a thing you do once, before a release, and never again.
//!
//! The three kinds are not variations. They travel by completely different
//! roads and exactly one of them is the road a unit test can walk:
//!
//! | kind      | what it does                    | who carries it              |
//! |-----------|---------------------------------|-----------------------------|
//! | `handled` | `report::failure`               | this process, over HTTP     |
//! | `panic`   | `panic!`, and `panic = "abort"` | the crash reporter, a minidump |
//! | `native`  | an access violation             | the crash reporter, a minidump |
//!
//! `handled` is the one that covers a join that failed, a call that dropped
//! and a share that would not open — the failures this app actually has. The
//! other two are what happens when the process stops existing, which is the
//! part no code inside it can report on.
//!
//! # The property this exists to check
//!
//! `minidump::init` **re-executes this binary**. Everything above that call
//! runs in two processes, and in `lib.rs` that constraint is what keeps the
//! app from opening the microphone twice or claiming two tray icons. `--kind
//! processes` is the check with nothing to clean up afterwards: it starts the
//! reporter, prints both process ids, and waits while you look.
//!
//! ```text
//! # Windows, from the MSVC dev shell, with the DSN compiled in:
//! $env:GOODVOICE_SENTRY_DSN = "https://…"
//! cargo run -p goodvoice-harness --bin crash-drill -- --kind processes
//! cargo run -p goodvoice-harness --bin crash-drill -- --kind handled
//! cargo run -p goodvoice-harness --bin crash-drill -- --kind panic
//! cargo run -p goodvoice-harness --bin crash-drill -- --kind native
//! ```
//!
//! **Build it in release.** A debug build has `panic = "unwind"`, so `panic`
//! unwinds out of `main` instead of aborting, and the thing being tested does
//! not happen:
//!
//! ```text
//! cargo run --release -p goodvoice-harness --bin crash-drill -- --kind panic
//! ```
//!
//! Every event this sends carries `drill = <kind>` as a tag, so a real issue
//! is never confused with one of these and the lot can be deleted together.

use std::{env, process, thread, time::Duration};

use anyhow::{bail, Result};
use goodvoice_client_lib::report;

/// How long to hold the process open so the reporter can be looked at.
const LOOK: Duration = Duration::from_secs(30);

/// How long to wait for an event to leave before dropping the guard.
///
/// The guard flushes on drop, but a `handled` drill that exits immediately
/// gives no chance to see whether it worked in the terminal it was run from.
const FLUSH: Duration = Duration::from_secs(5);

fn main() -> Result<()> {
    // # Nothing above this comment, and the arguments are parsed below it
    //
    // This is not style. The first version of this drill read `--kind` here,
    // before starting the reporter, and it did not work: `minidump::init`
    // re-executes the binary with **its own** arguments, so the child found no
    // `--kind`, printed the usage and exited — never reaching the call that
    // would have made it a reporter. The parent then died with
    // `Failed to create client with socket name`.
    //
    // That is the same trap `lib.rs` is written around, and it caught this
    // file first. Anything that inspects the command line, opens a device or
    // claims a resource has to sit below the two calls that follow.

    // The consent this passes is `true`, unconditionally and on purpose. On a
    // real client the answer comes off the disk and a `None` means no; running
    // this binary *is* the answer, and a drill that silently did nothing
    // because a settings file said so would be a drill that lies.
    let Some(guard) = report::start(true) else {
        bail!(
            "this build has no DSN compiled in, so nothing would be sent.\n\
             Set GOODVOICE_SENTRY_DSN and rebuild:\n\
             \n    $env:GOODVOICE_SENTRY_DSN = \"https://…\"\n\
             \n(see report::DSN — it is an option_env!, read at compile time)"
        );
    };

    // Unlike `lib.rs`, a failure here is fatal rather than degraded. The app
    // has somewhere useful to go without a crash reporter; this binary does
    // not, and a drill that quietly ran without one would report success for
    // the exact thing it exists to disprove.
    let reporter = match report::minidump_reporter(&guard) {
        Ok(handle) => handle,
        Err(error) => bail!("the crash reporter would not start: {error}"),
    };

    // Below the reporter, for the reason at the top of this function: the
    // child process gets here too, and only stops at the line above.
    let Some(kind) = kind_from_args() else {
        eprintln!("{USAGE}");
        process::exit(2);
    };

    let me = process::id();
    println!("reporting is on. this process is {me}.");
    println!("the crash reporter is a second process running this same exe.");

    report::tag("drill", kind.name());

    match kind {
        Kind::Processes => {
            println!();
            println!("look at Task Manager now. what should be there:");
            println!("  - exactly two `crash-drill` processes, this one and the reporter");
            println!("  - nothing else new");
            println!();
            println!("in the app, the same check is two `goodvoice-client`, one tray");
            println!("icon and one window. three processes, two icons or two windows");
            println!("means something moved above `minidump::init` in `run()`.");
            println!();
            println!("waiting {} seconds, then exiting cleanly.", LOOK.as_secs());
            thread::sleep(LOOK);
        }

        Kind::Handled => {
            println!();
            println!("sending a handled failure — the road a failed join takes.");
            report::failure(
                "crash-drill",
                "a handled failure, sent on purpose by the crash drill",
                &[("drill", "handled".to_owned())],
            );
            println!("sent. flushing for {} seconds.", FLUSH.as_secs());
            thread::sleep(FLUSH);
            println!("look for `crash-drill:` in the issue list.");
        }

        Kind::Panic => {
            println!();
            println!("panicking in 1s. under `panic = \"abort\"` this process dies");
            println!("and the reporter sends the minidump — so nothing after the");
            println!("next line ever runs.");
            thread::sleep(Duration::from_secs(1));
            panic!("crash drill: a panic, on purpose");
        }

        Kind::Native => {
            println!();
            println!("writing to a null pointer in 1s. this is the shape of a driver");
            println!("or GPU fault: no panic, no unwind, no Rust involved — the");
            println!("process is simply killed by Windows.");
            thread::sleep(Duration::from_secs(1));
            // SAFETY: there is none, and that is the entire point. This is the
            // one failure a Rust program cannot cause by accident and the one
            // an in-process SDK cannot survive, so it is the only honest test
            // of whether the separate reporter is doing its job.
            unsafe {
                std::ptr::null_mut::<u8>().write_volatile(1);
            }
            unreachable!("the write above ends the process");
        }
    }

    // Explicit rather than left to scope end: both of these flush on drop, and
    // the order is what makes the drill's last words true.
    drop(reporter);
    drop(guard);
    println!("done.");
    Ok(())
}

/// What this drill was asked to provoke.
#[derive(Clone, Copy)]
enum Kind {
    Processes,
    Handled,
    Panic,
    Native,
}

impl Kind {
    const fn name(self) -> &'static str {
        match self {
            Self::Processes => "processes",
            Self::Handled => "handled",
            Self::Panic => "panic",
            Self::Native => "native",
        }
    }
}

const USAGE: &str = "\
crash-drill — provoke a failure on purpose and watch it arrive

    cargo run --release -p goodvoice-harness --bin crash-drill -- --kind <kind>

  processes   start the reporter and wait, so both processes can be counted
  handled     report::failure — the road a failed join or share takes
  panic       panic!, which `panic = \"abort\"` turns into a minidump
  native      an access violation, the shape of a driver fault

Needs GOODVOICE_SENTRY_DSN set at *build* time, not run time.";

/// Reads `--kind <kind>`, and nothing else.
///
/// Hand-rolled rather than pulling in an argument parser, the same as every
/// other drill in this package: one flag with four values does not need a
/// dependency.
fn kind_from_args() -> Option<Kind> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--kind" {
            return match args.next()?.as_str() {
                "processes" => Some(Kind::Processes),
                "handled" => Some(Kind::Handled),
                "panic" => Some(Kind::Panic),
                "native" => Some(Kind::Native),
                _ => None,
            };
        }
    }
    None
}
