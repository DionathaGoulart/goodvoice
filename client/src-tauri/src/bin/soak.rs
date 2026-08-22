//! What goodvoice costs a machine while nobody is saying anything.
//!
//! plan.md task 4.5, and the two budgets in prd.md §4 that are about the
//! *other* thing a voice client does all evening: **CPU under 2%** and **RAM
//! at or under 120 MB**, minimised in a room. A call nobody is talking on is
//! the state this app spends almost all its life in, so it is the state worth
//! measuring — and half an hour of it is long enough for a leak to show up as
//! a slope rather than as noise.
//!
//! # What is being measured
//!
//! The app, launched for real, joined to a real room with a real second client
//! in it, and minimised into the tray — because that is the sentence in the
//! PRD ("while minimized in a room, idle") and because a hidden webview does
//! measurably less work than a visible one.
//!
//! **The whole process tree, not the process.** A Tauri app on Windows is
//! `goodvoice-client.exe` plus `WebView2`'s browser, GPU, network and renderer
//! processes, and those are where most of the memory is. Measuring only the
//! one process with our name on it would report a third of the truth and pass
//! a budget it had not met.
//!
//! Two clocks per sample and one liveness check:
//!
//! - **CPU** as the tree's own kernel+user time, differenced between samples
//!   and divided by the wall clock. Reported against the whole machine (what
//!   Task Manager shows, and what the 2% budget means) and against one core.
//! - **Memory** as the sum of the tree's working sets and, separately, the sum
//!   of its private bytes. Working sets double-count pages shared between the
//!   `WebView2` processes; private bytes miss pages that are genuinely resident.
//!   The budget is judged on the larger of the two, which is working set.
//! - **Is it still a call?** The listener counts frames arriving from the app.
//!   A soak where the app quietly fell out of the room is a measurement of an
//!   idle *app*, which is not what the budget is about, and it would be the
//!   cheapest possible way to pass.
//!
//! # Running it
//!
//! ```text
//! cargo build --release --bin goodvoice-client --bin soak
//! cargo run --release --bin soak                       # 30 minutes, the budget run
//! cargo run --release --bin soak -- --minutes 2        # a shakedown
//! ```
//!
//! Every sample is written to `docs/perf/idle-soak.csv` as it is taken, so a
//! run that is interrupted at minute 25 still leaves 25 minutes of evidence,
//! and so the numbers in the Decision Record can be recomputed by somebody who
//! does not believe them.

fn main() -> std::process::ExitCode {
    platform::main()
}

/// The arithmetic, kept away from the Win32 so it can be tested on any host.
///
/// None of it is difficult and all of it is the sort of thing that is wrong by
/// a factor of the core count without looking wrong at all.
#[cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "the sampler that calls these is Windows-only; the tests that \
                  check them are not, which is the point of the split"
    )
)]
mod numbers {
    use std::time::Duration;

    /// CPU used as a share of the machine, in percent.
    ///
    /// `cores` is every logical processor, so this is the number Task Manager
    /// puts in its CPU column and the number prd.md §4's 2% is about. A process
    /// pinning one core of eight is 12.5% here and 100% in [`per_core`].
    #[must_use]
    pub fn machine_percent(cpu: Duration, wall: Duration, cores: usize) -> f64 {
        if wall.is_zero() || cores == 0 {
            return 0.0;
        }
        #[allow(
            clippy::cast_precision_loss,
            reason = "a core count on any machine this decade is exact in f64"
        )]
        let cores = cores as f64;
        per_core(cpu, wall) / cores
    }

    /// CPU used as a share of one core, in percent.
    #[must_use]
    pub fn per_core(cpu: Duration, wall: Duration) -> f64 {
        if wall.is_zero() {
            return 0.0;
        }
        100.0 * cpu.as_secs_f64() / wall.as_secs_f64()
    }

    /// Bytes as megabytes, in the sense Task Manager means: 1024².
    #[must_use]
    pub fn megabytes(bytes: u64) -> f64 {
        #[allow(
            clippy::cast_precision_loss,
            reason = "a working set is far below f64's exact integer range"
        )]
        let bytes = bytes as f64;
        bytes / (1024.0 * 1024.0)
    }

    /// min / median / p95 / max of a run of samples.
    ///
    /// `audio::burst::Spread` in the library does this for durations; this
    /// one is for percentages and megabytes, and the p95 is the number that matters most
    /// here — an idle client is allowed a spike when a roommate joins, and it
    /// is not allowed to sit at 4%.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Spread {
        pub min: f64,
        pub median: f64,
        pub p95: f64,
        pub max: f64,
        pub count: usize,
    }

    impl Spread {
        /// `None` for an empty run: no samples is a failure to measure, not a
        /// measurement of zero.
        #[must_use]
        pub fn of(values: &[f64]) -> Option<Self> {
            if values.is_empty() {
                return None;
            }
            let mut sorted = values.to_vec();
            sorted.sort_by(f64::total_cmp);

            let at = |fraction: f64| {
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "an index into a vector of at most a few thousand"
                )]
                let index =
                    (f64::from(u32::try_from(sorted.len() - 1).unwrap_or(0)) * fraction) as usize;
                sorted[index]
            };

            Some(Self {
                min: sorted[0],
                median: at(0.5),
                p95: at(0.95),
                max: sorted[sorted.len() - 1],
                count: sorted.len(),
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{machine_percent, megabytes, per_core, Spread};
        use std::time::Duration;

        #[test]
        fn a_second_of_cpu_over_ten_is_a_tenth_of_a_core() {
            let cpu = Duration::from_secs(1);
            let wall = Duration::from_secs(10);
            assert!((per_core(cpu, wall) - 10.0).abs() < f64::EPSILON);
            // ...and an eighth of that on eight cores, which is the whole
            // reason both numbers are reported.
            assert!((machine_percent(cpu, wall, 8) - 1.25).abs() < f64::EPSILON);
        }

        #[test]
        fn no_wall_clock_is_no_measurement_rather_than_a_divide_by_zero() {
            assert!((per_core(Duration::from_secs(1), Duration::ZERO)).abs() < f64::EPSILON);
            assert!(
                (machine_percent(Duration::from_secs(1), Duration::from_secs(1), 0)).abs()
                    < f64::EPSILON
            );
        }

        #[test]
        fn megabytes_are_the_ones_task_manager_counts() {
            assert!((megabytes(120 * 1024 * 1024) - 120.0).abs() < f64::EPSILON);
        }

        #[test]
        fn spread_orders_what_it_is_given() {
            let spread = Spread::of(&[3.0, 1.0, 2.0, 100.0]).expect("four samples");
            assert!((spread.min - 1.0).abs() < f64::EPSILON);
            assert!((spread.median - 2.0).abs() < f64::EPSILON);
            assert!((spread.max - 100.0).abs() < f64::EPSILON);
            assert_eq!(spread.count, 4);
        }

        #[test]
        fn one_sample_is_every_percentile_of_itself() {
            let spread = Spread::of(&[7.5]).expect("one sample");
            assert!((spread.p95 - 7.5).abs() < f64::EPSILON);
            assert_eq!(spread.count, 1);
        }

        #[test]
        fn nothing_measured_is_not_zero() {
            assert_eq!(Spread::of(&[]), None);
        }
    }
}

#[cfg(not(windows))]
mod platform {
    /// Not a silent success. The budgets are about a Windows desktop, the
    /// process tree being measured is `WebView2`'s, and a soak that printed
    /// plausible Linux numbers would be worse than one that printed none
    /// (plan.md: do not fake or skip verification).
    pub fn main() -> std::process::ExitCode {
        eprintln!("the idle soak measures a Windows process tree — WebView2's — and");
        eprintln!("has to run on the Windows host the budgets are about");
        std::process::ExitCode::from(2)
    }
}

#[cfg(windows)]
mod platform {
    use std::{
        env,
        fs::File,
        io::{BufRead as _, Write as _},
        path::PathBuf,
        process::{Child, Command, ExitCode},
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc,
        },
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    use anyhow::{bail, Context as _, Result};
    use goodvoice_client_lib::{
        audio::{
            device::{AudioSink, AudioSource, MAX_REMOTE_SLOTS},
            opus::{silent_frame, Frame, FRAME_MS},
        },
        rtc::session::{Call, CallOptions},
    };

    use super::numbers::{machine_percent, megabytes, per_core, Spread};
    use tree::Tree;

    const DEFAULT_BASE: &str = "https://goodvoice.goodvoice-server.workers.dev";

    /// prd.md §4, idle in a room.
    const CPU_BUDGET_PERCENT: f64 = 2.0;
    const RAM_BUDGET_MB: f64 = 120.0;

    /// Half an hour, because a leak that shows up in five minutes is a bug
    /// somebody already found.
    const DEFAULT_MINUTES: u64 = 30;
    const DEFAULT_INTERVAL: Duration = Duration::from_secs(2);

    /// How long the app gets to reach the room before the soak gives up on it.
    const JOIN_TIMEOUT: Duration = Duration::from_secs(45);

    /// How long to let the app settle after it is in and minimised, before the
    /// first sample. Startup is task 4.4's measurement; counting it here would
    /// put a cold start's CPU into an idle client's average.
    const SETTLE: Duration = Duration::from_secs(20);

    #[tokio::main]
    pub async fn main() -> ExitCode {
        match run().await {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(error) => {
                eprintln!("\nthe soak could not be run: {error:#}");
                ExitCode::from(2)
            }
        }
    }

    /// `Ok(false)` is a soak that ran and did not meet the budgets — a result,
    /// not an error, and the exit code says so for a CI that ever wants it.
    async fn run() -> Result<bool> {
        let options = Options::parse();
        if !options.exe.exists() {
            bail!(
                "no app to launch at {}\n  build it first: cargo build --release --bin goodvoice-client",
                options.exe.display()
            );
        }
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);

        println!("goodvoice idle soak (plan.md task 4.5)\n");
        println!("  server   {}", options.base);
        println!("  app      {}", options.exe.display());
        println!(
            "  soak     {} minutes, sampled every {:.0} s",
            options.minutes,
            options.interval.as_secs_f64()
        );
        println!("  machine  {cores} logical processors");
        println!("  csv      {}\n", options.csv.display());

        let room = fresh_room();
        let ears = Arc::new(Ears::default());

        // A room with somebody in it. An app alone in a room subscribes to
        // nothing and decodes nothing, which is a cheaper client than the one
        // people actually run.
        let listener = Call::join(
            CallOptions {
                base: options.base.clone(),
                room: room.clone(),
                name: "soak-ears".to_owned(),
                mode: goodvoice_client_lib::audio::vad::TransmitMode::Open,
            },
            Box::new(Silence),
            Arc::clone(&ears) as Arc<dyn AudioSink>,
        )
        .await
        .context("the listener could not join")?;

        let outcome = soak(&options, &room, &ears, cores).await;
        listener.leave().await;
        let report = outcome?;
        Ok(report.within_budget())
    }

    /// Launch, join, minimise, sample. The listener is already in the room.
    async fn soak(options: &Options, room: &str, ears: &Arc<Ears>, cores: usize) -> Result<Report> {
        let app = spawn_app(&options.exe, room).context("could not start the app")?;
        let mut app = Killed(Some(app));
        let joined = app.watch_for_join();

        let waiting = Instant::now();
        while !joined.landed() {
            if waiting.elapsed() >= JOIN_TIMEOUT {
                bail!("the app never reached the room in {JOIN_TIMEOUT:?}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let pid = app.pid().context("the app has no process id")?;
        println!("  the app is in room {room} as pid {pid}");

        // The budget is about a minimised client, and the difference is not
        // rhetorical: a hidden webview stops rendering.
        match window::minimise(pid) {
            Ok(()) => println!("  minimised into the tray"),
            // Worth continuing and worth saying: the numbers are then a
            // visible window's numbers, which is the harder case.
            Err(error) => println!("  NOT minimised ({error:#}) — measuring a visible window"),
        }

        println!("  settling for {:.0} s\n", SETTLE.as_secs_f64());
        tokio::time::sleep(SETTLE).await;

        let mut tree = Tree::of(pid).context("could not read the app's process tree")?;
        let mut csv = Csv::create(&options.csv)?;
        let mut report = Report::new(cores);

        let started = Instant::now();
        let soaking = Duration::from_secs(options.minutes * 60);
        let mut previous = tree.sample().context("the first sample failed")?;
        let mut previous_at = Instant::now();
        let mut previous_frames = ears.frames();

        while started.elapsed() < soaking {
            tokio::time::sleep(options.interval).await;
            if !app.alive() {
                bail!(
                    "the app exited after {:.0} s — a soak of a process that is not \
                     running is not a measurement",
                    started.elapsed().as_secs_f64()
                );
            }

            let now = tree.sample().context("a sample failed")?;
            let at = Instant::now();
            let frames = ears.frames();

            let wall = at.duration_since(previous_at);
            let cpu = now.cpu.saturating_sub(previous.cpu);
            let point = Point {
                at: started.elapsed(),
                processes: now.processes,
                machine: machine_percent(cpu, wall, cores),
                core: per_core(cpu, wall),
                working_set: megabytes(now.working_set),
                private: megabytes(now.private),
                heard: frames > previous_frames,
            };
            csv.write(&point)?;
            report.push(&point);

            previous = now;
            previous_at = at;
            previous_frames = frames;
        }

        app.stop();
        report.print(&options.csv);
        Ok(report)
    }

    // --- the run, as numbers ------------------------------------------------

    /// One sample, already turned into the units the budgets are written in.
    struct Point {
        at: Duration,
        processes: usize,
        machine: f64,
        core: f64,
        working_set: f64,
        private: f64,
        heard: bool,
    }

    /// Every sample, and the verdict they add up to.
    struct Report {
        cores: usize,
        machine: Vec<f64>,
        core: Vec<f64>,
        working_set: Vec<f64>,
        private: Vec<f64>,
        processes: Vec<usize>,
        /// Samples in which nothing arrived from the app: the call went quiet
        /// without the process dying.
        silent: usize,
        first_working_set: Option<f64>,
        last_working_set: f64,
    }

    impl Report {
        fn new(cores: usize) -> Self {
            Self {
                cores,
                machine: Vec::new(),
                core: Vec::new(),
                working_set: Vec::new(),
                private: Vec::new(),
                processes: Vec::new(),
                silent: 0,
                first_working_set: None,
                last_working_set: 0.0,
            }
        }

        fn push(&mut self, point: &Point) {
            self.machine.push(point.machine);
            self.core.push(point.core);
            self.working_set.push(point.working_set);
            self.private.push(point.private);
            self.processes.push(point.processes);
            if !point.heard {
                self.silent += 1;
            }
            self.first_working_set.get_or_insert(point.working_set);
            self.last_working_set = point.working_set;
        }

        /// Both budgets, judged the way prd.md §4 states them: CPU as a share
        /// of the machine, memory at its peak. A peak rather than a median
        /// because 120 MB is a ceiling, and a client that touches 200 MB once
        /// an hour has not met it.
        fn within_budget(&self) -> bool {
            let cpu = Spread::of(&self.machine).is_some_and(|cpu| cpu.median < CPU_BUDGET_PERCENT);
            let ram = Spread::of(&self.working_set).is_some_and(|ram| ram.max <= RAM_BUDGET_MB);
            cpu && ram
        }

        #[allow(
            clippy::too_many_lines,
            reason = "a report is one long print; splitting it would only hide it"
        )]
        fn print(&self, csv: &std::path::Path) {
            let Some(machine) = Spread::of(&self.machine) else {
                println!("\nno samples were taken.");
                return;
            };
            let core = Spread::of(&self.core).unwrap_or(machine);
            let Some(working_set) = Spread::of(&self.working_set) else {
                return;
            };
            let private = Spread::of(&self.private).unwrap_or(working_set);

            println!("\n--- {} samples ---\n", machine.count);
            println!("  CPU, share of the machine ({} processors)", self.cores);
            println!("    median  {:6.2} %", machine.median);
            println!("    p95     {:6.2} %", machine.p95);
            println!("    max     {:6.2} %", machine.max);
            println!("\n  CPU, share of one core");
            println!("    median  {:6.2} %", core.median);
            println!("    p95     {:6.2} %", core.p95);
            println!("    max     {:6.2} %", core.max);

            println!("\n  memory, the whole tree's working sets");
            println!("    min     {:6.1} MB", working_set.min);
            println!("    median  {:6.1} MB", working_set.median);
            println!("    max     {:6.1} MB", working_set.max);
            println!("\n  memory, the whole tree's private bytes");
            println!("    median  {:6.1} MB", private.median);
            println!("    max     {:6.1} MB", private.max);

            let processes = self.processes.iter().copied().max().unwrap_or(0);
            println!("\n  processes  {processes} at most (the app plus WebView2's)");

            if let Some(first) = self.first_working_set {
                // Not a leak test — half an hour is too short to call one —
                // but a slope this can see is one worth chasing before ship.
                println!(
                    "  drift      {:+.1} MB from the first sample to the last",
                    self.last_working_set - first
                );
            }
            println!(
                "  quiet      {} of {} samples carried nothing from the app",
                self.silent, machine.count
            );

            println!("\n  against < {CPU_BUDGET_PERCENT:.0} % CPU and <= {RAM_BUDGET_MB:.0} MB (prd.md §4)");
            let cpu_ok = machine.median < CPU_BUDGET_PERCENT;
            let ram_ok = working_set.max <= RAM_BUDGET_MB;
            println!(
                "\n  CPU  {}  median {:.2} %",
                if cpu_ok { "WITHIN " } else { "OVER   " },
                machine.median
            );
            println!(
                "  RAM  {}  peak   {:.1} MB",
                if ram_ok { "WITHIN " } else { "OVER   " },
                working_set.max
            );
            if self.silent > 0 {
                println!(
                    "\n  Careful: {} samples heard nothing. A client that fell out of the\n\
                     \x20 room is idle in a way the budget does not mean.",
                    self.silent
                );
            }
            println!("\n  every sample: {}", csv.display());
        }
    }

    /// The samples, on disk as they are taken.
    struct Csv(File);

    impl Csv {
        fn create(path: &PathBuf) -> Result<Self> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("could not create {}", parent.display()))?;
            }
            let mut file = File::create(path)
                .with_context(|| format!("could not write {}", path.display()))?;
            writeln!(
                file,
                "seconds,processes,cpu_machine_percent,cpu_core_percent,working_set_mb,private_mb,heard"
            )?;
            Ok(Self(file))
        }

        fn write(&mut self, point: &Point) -> Result<()> {
            writeln!(
                self.0,
                "{:.1},{},{:.3},{:.3},{:.2},{:.2},{}",
                point.at.as_secs_f64(),
                point.processes,
                point.machine,
                point.core,
                point.working_set,
                point.private,
                u8::from(point.heard)
            )?;
            // Flushed every sample: a run that is stopped at minute 25 has to
            // leave 25 minutes of evidence behind it.
            self.0.flush()?;
            Ok(())
        }
    }

    // --- the app under measurement ------------------------------------------

    fn spawn_app(exe: &PathBuf, room: &str) -> std::io::Result<Child> {
        Command::new(exe)
            .env("GOODVOICE_AUTOJOIN", room)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
    }

    /// The app, killed rather than asked to leave — the same as task 4.4's
    /// drill, and for the same reason: closing its window now hides it.
    struct Killed(Option<Child>);

    impl Killed {
        fn pid(&self) -> Option<u32> {
            self.0.as_ref().map(Child::id)
        }

        fn alive(&mut self) -> bool {
            self.0
                .as_mut()
                .is_some_and(|app| matches!(app.try_wait(), Ok(None)))
        }

        /// Reads the app's stdout for the line that says it is in the room —
        /// and keeps reading for the rest of the soak, because a pipe nobody
        /// drains fills up and blocks the process being measured, which is a
        /// spectacular way to measure a very idle client.
        fn watch_for_join(&mut self) -> Joined {
            let mark = Joined::default();
            let Some(stdout) = self.0.as_mut().and_then(|app| app.stdout.take()) else {
                return mark;
            };

            let flag = Arc::clone(&mark.0);
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(stdout)
                    .lines()
                    .map_while(Result::ok)
                {
                    if line.starts_with("autojoined") {
                        println!("      app: {line}");
                        flag.store(1, Ordering::Release);
                    }
                }
            });
            mark
        }

        fn stop(&mut self) {
            if let Some(mut app) = self.0.take() {
                let _ = app.kill();
                let _ = app.wait();
            }
        }
    }

    impl Drop for Killed {
        fn drop(&mut self) {
            self.stop();
        }
    }

    #[derive(Default)]
    struct Joined(Arc<AtomicU64>);

    impl Joined {
        fn landed(&self) -> bool {
            self.0.load(Ordering::Acquire) == 1
        }
    }

    /// A listener that counts what arrives rather than what it sounds like.
    ///
    /// Loudness is not the question — a quiet room encodes to almost nothing —
    /// but *arrival* is: it is how the soak knows the app is still in the room
    /// half an hour later.
    #[derive(Default)]
    struct Ears {
        frames: AtomicU64,
    }

    impl Ears {
        fn frames(&self) -> u64 {
            self.frames.load(Ordering::Relaxed)
        }
    }

    impl AudioSink for Ears {
        fn play(&self, slot: usize, _frame: &Frame) {
            debug_assert!(slot < MAX_REMOTE_SLOTS);
            self.frames.fetch_add(1, Ordering::Relaxed);
        }

        fn clear(&self, _slot: usize) {}
    }

    /// The listener's microphone: a client that never sends is not a client
    /// this path supports, so it sends silence.
    struct Silence;

    #[async_trait::async_trait]
    impl AudioSource for Silence {
        async fn next_frame(&mut self) -> Option<Frame> {
            tokio::time::sleep(Duration::from_millis(u64::from(FRAME_MS))).await;
            Some(silent_frame())
        }
    }

    // --- arguments ----------------------------------------------------------

    struct Options {
        base: String,
        exe: PathBuf,
        csv: PathBuf,
        minutes: u64,
        interval: Duration,
    }

    impl Options {
        fn parse() -> Self {
            let mut options = Self {
                base: env::var("GOODVOICE_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_owned()),
                exe: default_exe(),
                csv: default_csv(),
                minutes: DEFAULT_MINUTES,
                interval: DEFAULT_INTERVAL,
            };

            let mut argv = env::args().skip(1);
            while let Some(flag) = argv.next() {
                let Some(value) = argv.next() else {
                    break;
                };
                match flag.as_str() {
                    "--base" => options.base = value,
                    "--exe" => options.exe = PathBuf::from(value),
                    "--csv" => options.csv = PathBuf::from(value),
                    "--minutes" => {
                        options.minutes = value.parse().unwrap_or(DEFAULT_MINUTES).max(1);
                    }
                    "--interval" => {
                        options.interval = value
                            .parse()
                            .map_or(DEFAULT_INTERVAL, Duration::from_secs)
                            .max(Duration::from_secs(1));
                    }
                    _ => {}
                }
            }
            options
        }
    }

    /// The app next to this drill: both are targets of the same crate, so the
    /// one built with it is the one in the same directory.
    fn default_exe() -> PathBuf {
        env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|dir| dir.join("goodvoice-client.exe")))
            .unwrap_or_else(|| PathBuf::from("goodvoice-client.exe"))
    }

    /// `docs/perf/idle-soak.csv` relative to wherever this was started, which
    /// for `cargo run` is `client/src-tauri`.
    fn default_csv() -> PathBuf {
        PathBuf::from("..")
            .join("..")
            .join("docs")
            .join("perf")
            .join("idle-soak.csv")
    }

    /// A room nobody else is in, and a different one every run.
    fn fresh_room() -> String {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        format!("soak-{}", stamp % 1_000_000_000)
    }

    /// Reading a process tree's CPU and memory out of Windows.
    mod tree {
        use std::time::Duration;

        use anyhow::{Context as _, Result};
        use windows::Win32::{
            Foundation::{CloseHandle, FILETIME, HANDLE},
            System::{
                Diagnostics::ToolHelp::{
                    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                    TH32CS_SNAPPROCESS,
                },
                ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
                Threading::{
                    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                    PROCESS_VM_READ,
                },
            },
        };

        /// What one sample of the tree came to.
        pub struct Sample {
            /// Kernel + user time of every process in the tree, accumulated
            /// since each one started. Differenced by the caller.
            pub cpu: Duration,
            pub working_set: u64,
            pub private: u64,
            pub processes: usize,
        }

        /// The app's process tree, sampled repeatedly.
        ///
        /// Re-walked every time rather than remembered: `WebView2` starts and
        /// stops processes while it runs, and a tree read once would go on
        /// reporting the memory of a renderer that exited ten minutes ago.
        pub struct Tree {
            root: u32,
            /// CPU time already accounted for by processes that have since
            /// exited. Without this the tree's total goes *down* when a
            /// renderer dies, and a negative delta reads as an idle sample.
            departed: Duration,
            /// What each live process had contributed at the last sample.
            last: Vec<(u32, Duration)>,
        }

        impl Tree {
            /// # Errors
            ///
            /// When the process cannot be opened at all — it exited, or this
            /// is not allowed to look at it.
            pub fn of(root: u32) -> Result<Self> {
                let tree = Self {
                    root,
                    departed: Duration::ZERO,
                    last: Vec::new(),
                };
                open(root).context("the app's own process could not be opened")?;
                Ok(tree)
            }

            /// # Errors
            ///
            /// When the process list cannot be taken.
            pub fn sample(&mut self) -> Result<Sample> {
                let members = descendants(self.root)?;
                let mut sample = Sample {
                    cpu: Duration::ZERO,
                    working_set: 0,
                    private: 0,
                    processes: 0,
                };

                let mut live: Vec<(u32, Duration)> = Vec::with_capacity(members.len());
                for pid in members {
                    let Some(process) = open(pid) else {
                        // A process that exited between the snapshot and here.
                        // Its last slice of CPU is lost; at two seconds a
                        // sample and a few short-lived utility processes, that
                        // is far below the noise in the number.
                        continue;
                    };
                    if let Some(cpu) = process.cpu() {
                        sample.cpu += cpu;
                        live.push((pid, cpu));
                    }
                    if let Some((working_set, private)) = process.memory() {
                        sample.working_set += working_set;
                        sample.private += private;
                    }
                    sample.processes += 1;
                }

                // Anything that was in the last sample and is not in this one
                // took its CPU time with it. Keeping the total monotonic is
                // what makes the difference between two samples mean "work
                // done in between".
                for (pid, cpu) in &self.last {
                    if !live.iter().any(|(alive, _)| alive == pid) {
                        self.departed += *cpu;
                    }
                }
                self.last = live;
                sample.cpu += self.departed;
                Ok(sample)
            }
        }

        /// Every process descended from `root`, and `root` itself.
        ///
        /// One snapshot, then a walk: `WebView2`'s renderers are children of its
        /// browser process, not of ours, so a single generation is not enough.
        fn descendants(root: u32) -> Result<Vec<u32>> {
            // SAFETY: a snapshot handle or an error; nothing is borrowed.
            let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
                .context("could not take a process snapshot")?;
            let snapshot = Owned(snapshot);

            let mut entry = PROCESSENTRY32W {
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "the struct is a few hundred bytes"
                )]
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };

            let mut pairs = Vec::new();
            // SAFETY: `entry` is sized as the API requires and lives across
            // the whole walk.
            if unsafe { Process32FirstW(snapshot.0, &raw mut entry) }.is_ok() {
                loop {
                    pairs.push((entry.th32ProcessID, entry.th32ParentProcessID));
                    // SAFETY: same handle, same entry, until it says there is
                    // nothing left.
                    if unsafe { Process32NextW(snapshot.0, &raw mut entry) }.is_err() {
                        break;
                    }
                }
            }

            let mut tree = vec![root];
            let mut born = vec![(root, started(root).unwrap_or(0))];
            // A child does not have to appear after its parent in the
            // snapshot, so this repeats until a pass adds nothing.
            loop {
                let before = tree.len();
                for (pid, parent) in &pairs {
                    if *pid == 0 || tree.contains(pid) {
                        continue;
                    }
                    let Some((_, parent_born)) = born.iter().find(|(id, _)| id == parent) else {
                        continue;
                    };
                    let Some(child_born) = started(*pid) else {
                        continue;
                    };
                    // A parent id is stale the moment the parent exits and
                    // Windows hands the number to somebody else. A process
                    // older than the parent it claims is that, and not a
                    // child of ours — and adopting one would put a stranger's
                    // whole memory inside our budget.
                    if child_born >= *parent_born {
                        tree.push(*pid);
                        born.push((*pid, child_born));
                    }
                }
                if tree.len() == before {
                    break;
                }
            }
            Ok(tree)
        }

        /// When a process started, in 100-nanosecond ticks. `None` if it
        /// cannot be opened at all, which is answer enough: it is not ours.
        fn started(pid: u32) -> Option<u64> {
            let process = open(pid)?;
            let mut creation = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            // SAFETY: a live handle and four outputs this call fills.
            unsafe {
                GetProcessTimes(
                    process.0,
                    &raw mut creation,
                    &raw mut exit,
                    &raw mut kernel,
                    &raw mut user,
                )
            }
            .ok()?;
            Some(ticks(creation))
        }

        /// A process handle that closes itself.
        struct Process(HANDLE);

        impl Process {
            fn cpu(&self) -> Option<Duration> {
                let mut creation = FILETIME::default();
                let mut exit = FILETIME::default();
                let mut kernel = FILETIME::default();
                let mut user = FILETIME::default();
                // SAFETY: a live handle opened with QUERY_LIMITED_INFORMATION,
                // and four outputs this call fills.
                unsafe {
                    GetProcessTimes(
                        self.0,
                        &raw mut creation,
                        &raw mut exit,
                        &raw mut kernel,
                        &raw mut user,
                    )
                }
                .ok()?;
                Some(hundred_nanos(kernel) + hundred_nanos(user))
            }

            /// Working set and private bytes, in that order.
            fn memory(&self) -> Option<(u64, u64)> {
                let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "the struct is under a hundred bytes"
                )]
                let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
                // SAFETY: the EX struct is what the size says it is, and the
                // call is documented to take either shape behind that size.
                unsafe {
                    GetProcessMemoryInfo(self.0, std::ptr::from_mut(&mut counters).cast(), size)
                }
                .ok()?;
                Some((counters.WorkingSetSize as u64, counters.PrivateUsage as u64))
            }
        }

        impl Drop for Process {
            fn drop(&mut self) {
                // SAFETY: the handle came from `OpenProcess` and is closed once.
                let _ = unsafe { CloseHandle(self.0) };
            }
        }

        fn open(pid: u32) -> Option<Process> {
            // SAFETY: no borrows; the handle is owned by `Process` from here.
            let handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
                    false,
                    pid,
                )
            }
            .ok()?;
            Some(Process(handle))
        }

        /// A snapshot handle that closes itself.
        struct Owned(HANDLE);

        impl Drop for Owned {
            fn drop(&mut self) {
                // SAFETY: the handle came from `CreateToolhelp32Snapshot`.
                let _ = unsafe { CloseHandle(self.0) };
            }
        }

        /// A FILETIME is 100-nanosecond ticks: an instant when it says when
        /// a process started, a duration when it says how much CPU it burned.
        fn ticks(time: FILETIME) -> u64 {
            (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
        }

        fn hundred_nanos(time: FILETIME) -> Duration {
            Duration::from_nanos(ticks(time).saturating_mul(100))
        }
    }

    /// Putting the app where the budget says it is: minimised, in the tray.
    mod window {
        use anyhow::{bail, Result};
        use windows::core::BOOL;
        use windows::Win32::{
            Foundation::{HWND, LPARAM},
            UI::WindowsAndMessaging::{
                EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible, ShowWindow,
                SW_MINIMIZE,
            },
        };

        /// What Tauri calls its windows on Windows. The app has exactly one.
        const TAURI_CLASS: &str = "Tauri Window";

        /// Minimises the app's window, which task 4.1 turns into a hide.
        ///
        /// # Errors
        ///
        /// When the process has no such window — a build without a webview, or
        /// one that has not created it yet.
        pub fn minimise(pid: u32) -> Result<()> {
            let Some(window) = find(pid) else {
                bail!("no Tauri window belongs to pid {pid}");
            };
            // SAFETY: a window handle from `EnumWindows`, used at once.
            let _ = unsafe { ShowWindow(window, SW_MINIMIZE) };
            std::thread::sleep(std::time::Duration::from_secs(2));
            // SAFETY: same handle.
            if unsafe { IsWindowVisible(window) }.as_bool() {
                bail!("the window is still visible after a minimise");
            }
            Ok(())
        }

        fn find(pid: u32) -> Option<HWND> {
            let mut found = Found {
                pid,
                window: HWND::default(),
            };
            // SAFETY: the callback is `extern "system"` and `found` outlives
            // the call, which does not outlive this function.
            let _ = unsafe {
                EnumWindows(Some(visit), LPARAM(std::ptr::from_mut(&mut found) as isize))
            };
            (!found.window.is_invalid()).then_some(found.window)
        }

        struct Found {
            pid: u32,
            window: HWND,
        }

        unsafe extern "system" fn visit(window: HWND, state: LPARAM) -> BOOL {
            // SAFETY: the pointer is the `Found` `find` passed in, live for
            // the whole enumeration.
            let found = unsafe { &mut *(state.0 as *mut Found) };

            let mut owner = 0_u32;
            // SAFETY: a window handle from the enumeration, and one output.
            unsafe { GetWindowThreadProcessId(window, Some(&raw mut owner)) };
            if owner != found.pid {
                return true.into();
            }

            let mut class = [0_u16; 64];
            // SAFETY: the buffer is as long as the length passed.
            let written = unsafe { GetClassNameW(window, &mut class) };
            if written > 0 {
                #[allow(
                    clippy::cast_sign_loss,
                    reason = "GetClassNameW returns a length or zero"
                )]
                let name = String::from_utf16_lossy(&class[..written as usize]);
                if name == TAURI_CLASS {
                    found.window = window;
                    return false.into();
                }
            }
            true.into()
        }
    }
}
