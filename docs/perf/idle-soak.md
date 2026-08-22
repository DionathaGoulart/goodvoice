# Idle in a room: what goodvoice costs while nobody is talking

plan.md tasks 4.5 (measure it) and 4.6 (fix what the measurement found), and two
of prd.md §4's budgets: **CPU under 2%** and **RAM at or under 120 MB**,
minimised in a room. This is the state a voice client is in for almost all of an
evening, so it is the state worth half an hour of measurement.

## What is measured

- **The real app**, release build, launched by the harness and joined to a real
  room on the live deploy — not a test rig standing in for it.
- **With somebody in the room.** A second client sits in the room for the whole
  run, sending and receiving. An app alone in a room subscribes to nothing and
  decodes nothing, which is a cheaper client than anybody actually runs.
- **Minimised into the tray**, because that is the sentence the PRD writes
  ("while minimized in a room, idle") — and since task 4.6 that is also the
  state where there is no webview to measure at all.
- **The whole process tree.** `goodvoice-client.exe` plus whatever WebView2 has
  running. Measuring only the process with our name on it was how a 361 MB app
  would have reported 34 and passed — and now that 4.6 has made those two the
  same number, the tree is still what is measured, because "the webview really
  did go" is the claim being checked.

Both numbers, every two seconds:

| | how |
|---|---|
| CPU | the tree's kernel+user time, differenced between samples, over the wall clock and the machine's logical processors — the share Task Manager shows |
| memory | the sum of the tree's working sets, and separately the sum of its private bytes |

And one check that the run means anything: the second client counts frames
arriving from the app. **A soak where the app quietly fell out of the room is a
measurement of an idle process, not of an idle call** — and it would be the
cheapest possible way to pass.

## Running it

```text
cargo build --release --features custom-protocol --bin goodvoice-client
cargo build --release --bin soak
cargo run --release --bin soak                    # 30 minutes, the budget run
cargo run --release --bin soak -- --minutes 2     # a shakedown
```

**`--features custom-protocol` is not optional.** Without it
`generate_context!` points the webview at the Vite dev server instead of
embedding `../dist`, and what gets launched is a goodvoice-titled window
containing Edge's "localhost refused to connect". It still starts, still joins,
still minimises, and every number taken from it is about an error page. The
2026-08-22 run below is the first one that is not (DR-22).

Every sample is appended to `docs/perf/idle-soak.csv` as it is taken, so a run
stopped at minute 25 still leaves 25 minutes of evidence behind it.

The second opinion, against an app that is already running:

```text
powershell -ExecutionPolicy Bypass -File docs\perf\idle-soak.ps1 -Minutes 30
```

`bin/soak` measures through Toolhelp and `GetProcessTimes`; the script measures
through CIM and .NET. They share no code and no API, so a budget met by one and
not the other is a bug in the arithmetic rather than a fact about the app.

Two things the script has to say out loud because this desktop is pt-BR: it
formats every number for a reader rather than for the machine's locale (a
decimal comma inside a comma-separated file is not a file), and it uses `F`
rather than `N`, which groups thousands and would put a second comma in the
seconds column.

## The run: 30 minutes, 2026-08-22, after task 4.6

Release build with `custom-protocol`, DR-12 machine (Windows 11, 12 logical
processors), against the live deploy. 896 samples from `bin/soak`, 309 from the
PowerShell script over the same process tree at the same time.

```text
CPU, share of the machine        median 0.39 %   p95 0.65 %   max 0.97 %
CPU, share of one core           median 4.65 %   p95 7.77 %   max 11.67 %

memory, tree working sets        min 33.8 MB     median 34.0 MB    max 34.1 MB
memory, tree private bytes                       median  7.5 MB    max  7.8 MB

processes                        1, for all 896 samples
the call                         896 of 896 samples carried audio from the app

CPU  WITHIN BUDGET   0.39 % against 2 %
RAM  WITHIN BUDGET    34.1 MB peak against 120 MB
```

The second opinion, over the same half hour through CIM and .NET rather than
Toolhelp and `GetProcessTimes`: **CPU median 0.39%**, **RAM median 34.0 MB, peak
34.1 MB**. Two implementations that share no code and no API, agreeing to a
tenth of a megabyte.

**Nothing grows.** Median working set by five-minute bucket: 33.8, 34.0, 34.0,
34.0, 34.0, 34.0 MB. Private bytes: 7.5, 7.6, 7.5, 7.5, 7.5, 7.5. Drift from
the first sample to the last: +0.0 MB.

**And nothing accumulates across rebuilds**, which is the new thing 4.6 gave
this app to get wrong. `docs/testing/tray-roundtrip.ps1 -Cycles 12`, tree
working set read in the tray after each round trip: 33.7, 34.1, 34.2, 34.3,
34.4, 34.5, 34.8, 34.7, 34.7, 34.9, 34.9, 34.9 MB. It settles and stays, so the
first few cycles are a cost paid once rather than a webview that never quite
goes away.

## What this means

**Both budgets met, and the memory one by a factor of three.** 34 MB against
120, and 0.39% of a twelve-processor machine against 2% — for a client that is
encoding, sending, receiving, decoding and mixing continuously, because the
shipped default is open-mic (prd.md's "cannot swallow the start of a
sentence"). Voice-activity and push-to-talk do strictly less.

There is nothing clever in the 34 MB. It is what goodvoice always cost; the
other 327 was a WebView2 runtime sitting behind a window nobody was looking at,
and task 4.6 closes the window rather than shrinking the browser. See DR-21 for
why the two cheaper levers were not enough, and what closing it costs.

## What it used to be (task 4.5, before 4.6)

For comparison, and because the shape of the old number is the whole argument
for the change. Same machine, same method, window hidden rather than destroyed:

```text
memory, tree working sets        median 361 MB     max 404 MB     OVER 120 MB
CPU, share of the machine        median 0.39 %                  WITHIN   2 %

goodvoice-client       main              34.2 MB ws      7.2 MB private
msedgewebview2         main             130.6 MB ws     39.8 MB private
msedgewebview2         gpu-process       63.0 MB ws     59.8 MB private
msedgewebview2         renderer          61.3 MB ws     26.7 MB private
msedgewebview2         utility           39.6 MB ws     12.8 MB private
msedgewebview2         utility           20.1 MB ws      8.7 MB private
msedgewebview2         crashpad-handler  12.8 MB ws      2.9 MB private
```

The voice client was 34 MB then too. Everything the budget is about — the
devices, Opus, the mixer, the transport, the roster — was in the first line,
and the first line was a quarter of the ceiling. The other six were the runtime,
all resident with the window hidden: a GPU process compositing nothing, a
renderer holding a 420-pixel roster nobody was looking at, and a crash handler.
Not an artefact of counting shared pages twice, either — private bytes came to
157 MB across the tree, and 7 MB of that was ours.

One caveat on that table, found later: it was measured against a build without
`custom-protocol`, so the renderer was holding Edge's error page rather than
goodvoice's UI (DR-22). The runtime is the runtime either way and the
conclusion did not move, but the numbers are not the app's.
