# Idle in a room: what goodvoice costs while nobody is talking

plan.md task 4.5, and two of prd.md §4's budgets: **CPU under 2%** and **RAM at
or under 120 MB**, minimised in a room. This is the state a voice client is in
for almost all of an evening, so it is the state worth half an hour of
measurement.

## What is measured

- **The real app**, release build, launched by the harness and joined to a real
  room on the live deploy — not a test rig standing in for it.
- **With somebody in the room.** A second client sits in the room for the whole
  run, sending and receiving. An app alone in a room subscribes to nothing and
  decodes nothing, which is a cheaper client than anybody actually runs.
- **Minimised into the tray**, because that is the sentence the PRD writes
  ("while minimized in a room, idle") and because a hidden webview does
  measurably less work than a visible one.
- **The whole process tree.** `goodvoice-client.exe` plus WebView2's browser,
  GPU, network and renderer processes. Measuring only the process with our name
  on it reports a fraction of the memory and passes a budget it has not met.

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
cargo build --release --bin goodvoice-client --bin soak
cargo run --release --bin soak                    # 30 minutes, the budget run
cargo run --release --bin soak -- --minutes 2     # a shakedown
```

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
seconds column. The committed capture below was written before the second of
those was fixed and had its separators stripped afterwards; the numbers in it
are the ones the run produced, and its summary recomputes from the file to the
same median and peak the script printed.

## Where the memory is

The tree, minimised in a room, mid-soak. `--type=` is what each `msedgewebview2`
process was started as:

```text
goodvoice-client       main             34.2 MB ws      7.2 MB private
msedgewebview2         main            130.6 MB ws     39.8 MB private
msedgewebview2         gpu-process      63.0 MB ws     59.8 MB private
msedgewebview2         renderer         61.3 MB ws     26.7 MB private
msedgewebview2         utility          39.6 MB ws     12.8 MB private
msedgewebview2         utility          20.1 MB ws      8.7 MB private
msedgewebview2         crashpad-handler 12.8 MB ws      2.9 MB private
```

**The voice client is 34 MB. The shell is the other 325.** Everything the
budget is about — the devices, Opus, the mixer, the transport, the roster —
lives in the first line, and the first line is a quarter of the ceiling. The
other six lines are the WebView2 runtime, and they are all there with the
window hidden: a GPU process compositing nothing, a renderer holding a
420-pixel roster nobody is looking at, and a crash handler.

It is not an artefact of counting shared pages twice, either. Private bytes —
which count nothing shared — come to about 157 MB across the tree, and 7 MB of
that is ours.

## The run: 30 minutes, 2026-08-22

Release build, DR-12 machine (Windows 11, 12 logical processors), against the
live deploy. 897 samples from `bin/soak`, 309 from the PowerShell script over
the same process tree at the same time.

```text
CPU, share of the machine        median 0.39 %   p95 0.65 %   max 1.04 %
CPU, share of one core           median 4.66 %   p95 7.79 %   max 12.48 %

memory, tree working sets        min 350.6 MB    median 361.1 MB   max 404.3 MB
memory, tree private bytes                       median 156.5 MB   max 184.8 MB

processes                        7, and 8 in five samples out of 897
the call                         897 of 897 samples carried audio from the app

CPU  WITHIN BUDGET   0.39 % against 2 %
RAM  OVER BUDGET     361 MB steady against 120 MB
```

The second opinion, over the same half hour: **CPU median 0.39%**, **RAM median
361.1 MB, peak 367.4 MB**. The two harnesses agree on the CPU number to two
decimal places and on the memory to a megabyte. The peaks differ because
`bin/soak` samples every two seconds and catches the transient eighth process —
a WebView2 utility that comes and goes, worth about 40 MB for a few seconds at
a time — which the five-second sampling mostly steps over.

**Nothing grows.** Median working set by five-minute bucket: 361.0, 363.9,
363.4, 361.1, 361.0, 361.0 MB. Private bytes: 157.4, 157.4, 156.6, 156.4,
156.2, 156.2 MB. Per process, start against end after 26 minutes, the largest
change in the tree is half a megabyte. The `+10.5 MB` drift the harness prints
is the first sample, taken while the tree was still settling, against the rest.

## What this means

Half of the task passes and half does not, and the halves are not symmetric.

**CPU: met, with room.** 0.39% of a twelve-processor machine — 4.7% of one core
— for a client that is encoding, sending, receiving, decoding and mixing
continuously (the shipped default is open-mic, prd.md's "cannot swallow the
start of a sentence"). Voice-activity or push-to-talk modes do strictly less.

**Memory: not met, and not by our code.** The budget is 120 MB; the tree is
361 MB; the part of it that is goodvoice is 34 MB. See DR-20 in
`.harness/plan.md` for what was measured, what the options are, and why this is
plan task 4.6 rather than a line in this file.
