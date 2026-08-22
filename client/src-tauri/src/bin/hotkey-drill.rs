//! Does the talk key reach us while something else has focus?
//!
//! plan.md task 4.3. The push-to-talk key from task 3.3 is handled by the
//! webview, which only hears it while the webview is what is being typed into.
//! `tray::hotkey` watches the whole desktop instead, and this is the smallest
//! thing that can tell whether it works: no window, no call, no audio — so
//! whatever has focus, it is not this.
//!
//! ```text
//! cargo run --bin hotkey-drill                 # Space, for ten seconds
//! cargo run --bin hotkey-drill -- --key KeyV --seconds 30
//! ```
//!
//! Press and release the key in any other window. Every transition is printed
//! with the millisecond it arrived, and the exit code says whether anything
//! did: **0** if the key was seen going down and coming back up, **1** if it
//! never arrived. `docs/testing/hotkey.md` drives it without a person.

#[cfg(not(windows))]
fn main() {
    // Not a silent success: the whole question is about a Windows desktop, and
    // a drill that printed "nothing pressed" on Linux would read as a broken
    // hook rather than the wrong machine (plan.md: do not fake or skip).
    eprintln!("the global push-to-talk hook is a Windows thing, and so is this drill");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };

    use goodvoice_client_lib::tray::hotkey;

    let (key, seconds) = args();
    let started = Instant::now();
    let downs = Arc::new(AtomicUsize::new(0));
    let ups = Arc::new(AtomicUsize::new(0));

    println!("goodvoice hotkey drill (plan.md task 4.3)\n");
    println!("  watching for {key} for {seconds} s, from anywhere on the desktop");
    println!("  give focus to any other window and hold it\n");

    let listener = {
        let downs = Arc::clone(&downs);
        let ups = Arc::clone(&ups);
        hotkey::listen(&key, move |down| {
            let at = started.elapsed().as_millis();
            if down {
                downs.fetch_add(1, Ordering::Relaxed);
                println!("  {at:>6} ms  down");
            } else {
                ups.fetch_add(1, Ordering::Relaxed);
                println!("  {at:>6} ms  up");
            }
        })
    };
    let listener = match listener {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("could not watch for {key}: {error}");
            return std::process::ExitCode::from(2);
        }
    };

    std::thread::sleep(Duration::from_secs(seconds));
    drop(listener);

    let (downs, ups) = (downs.load(Ordering::Relaxed), ups.load(Ordering::Relaxed));
    println!("\n--- {downs} presses, {ups} releases ---");
    if downs > 0 && ups > 0 {
        println!("PASS — the key was heard from outside this process.");
        return std::process::ExitCode::SUCCESS;
    }
    println!(
        "FAIL — nothing arrived. Either the key was never pressed, or the hook\n\
         is not seeing the desktop (check that {key} is the key you pressed)."
    );
    std::process::ExitCode::FAILURE
}

/// `--key CODE --seconds N`, where CODE is named the way `KeyboardEvent.code`
/// names it — the same string the window stores.
#[cfg(windows)]
fn args() -> (String, u64) {
    const DEFAULT_KEY: &str = "Space";
    const DEFAULT_SECONDS: u64 = 10;

    let mut key = DEFAULT_KEY.to_owned();
    let mut seconds = DEFAULT_SECONDS;

    let mut argv = std::env::args().skip(1);
    while let Some(flag) = argv.next() {
        let Some(value) = argv.next() else {
            break;
        };
        match flag.as_str() {
            "--key" => key = value,
            "--seconds" => seconds = value.parse().unwrap_or(DEFAULT_SECONDS),
            _ => {}
        }
    }
    (key, seconds.max(1))
}
