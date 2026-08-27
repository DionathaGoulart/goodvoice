# GoodVoice — Execution Plan

> **Reader: an AI agent** (Claude, in Claude Code) executing across many sessions.
> Rules of engagement:
> - Work top-to-bottom inside a phase unless a task says it can run in parallel.
> - Mark a task `[x]` **in the same commit** that completes it.
> - Every task states its files, definition of done (DoD), and the exact
>   command(s) that prove it works. If the proof command fails, the task is not done.
> - Windows-only tasks are tagged **[WIN]** — they need a Windows machine/VM with
>   the app's target hardware APIs. Do not fake or skip verification; if no Windows
>   host is available, stop and report.
> - Hit an unknown (crate missing a feature, API changed, SFU quirk)? Write a
>   **Decision Record** (§DR below), propose alternatives, do NOT silently swap stack.
> - Performance is the tiebreaker. Budgets (PRD §4): ≤80 ms voice latency, <2% idle
>   CPU, ≤120 MB RAM, ≤6% FPS impact sharing 1080p30 (DR-35), <3 s cold start.
> - **Read "Start here" below and nothing else first.** It says where the last
>   session stopped and what the next thing to do is. Keep it true: update it
>   in the same commit that changes what it says.

---

## Start here

**Where this stopped:** 2026-08-25, with §7.6's instrument built and its
answer taken: this machine cannot host that test.
Phases 0–5 are closed, and so are §6.1, §6.2, §7.1, §7.2, §7.3, §7.4 and §7.5.
What is open is §6.3, §6.4 and the rest of Phase 7 — and **Phase 7 opens with
the order to do all of it in**, whose rows 1–6 are done and whose row 7 is now
blocked on an object rather than on work. Nothing else in this file needs
reading first.

**Two of the three things below now wait on hardware nobody here has**, and
that is the shape of the release rather than a thing to keep re-discovering:
§7.6 wants a loudspeaker the microphone can hear, §6.3 wants a second Windows
machine. Both were asked for on 2026-08-25 and neither was available. §6.4 is
not blocked, and it is the row that decides what a release with those two
unrun is allowed to claim.

### The next three things, in this order

1. **Phase 7 row 7 — §7.6, a room hearing itself. Blocked on a loudspeaker,
   and on nothing else.** The question this file asked — *can an instrument
   reach it before anybody is scheduled* — has been answered: yes, all of it
   except the room. `bin/echo-room` makes the tone take the whole trip through
   a real transducer and a real microphone, and it **refused twice**, at
   0.4 dB and 2.0 dB of coupling against the 6 dB it needs (DR-41). The only
   active render endpoint on this machine is a headset earphone; the analog
   jack DR-23 measured through now reports `not present`. Lay an earcup
   face-down against the fifine, or put a speaker back in the jack, and the
   row is one command: `cargo run -p goodvoice-harness --bin echo-room --
   --record <dir>`. **Do not re-derive this** — the drill's own control says
   it in one line, and the canceller-off segment is there so that a room with
   no loudspeaker in it can never be mistaken for a canceller that works.
   4.7's other half — does the suppressor sound better on than off — needs no
   loudspeaker at all: `--record` writes the WAVs even when the verdict is
   refused, and `quiet.wav` against `suppressed.wav` is a listening test that
   can be done today.
   **Injection is refused while an *elevated* program holds the foreground.**
   Not, as this file said twice, while somebody is at the machine: measured
   with the desktop idle 31 minutes and still refused, and a click on an
   ordinary window cleared it (DR-39). This blocks every drill that clicks, and
   the drills now name the program in the way rather than blaming the app.
2. **§6.3 — the clean-VM install**, which needs a second Windows machine that
   has never had the toolchain on it. The bundle is built and installs here;
   what is unproven is that it carries everything a machine without MSVC needs.
   **Rebuild the bundle first:** DR-38 changed the window and the frontend, and
   the installed app on this machine is older than both.
3. **§6.4 — the README, the tag, the release. The README and the release CI
   are written; what is left is `git tag v0.1.0` and a push.** The README
   prints the measured numbers, §7.1's ≤ 6% among them, says which of rows
   10–15 were never run, and writes §7.6 and §6.3 the way this file asked —
   not "untested" but "tested up to the hardware the test needs, and here is
   the command that finishes it". `.github/workflows/release.yml` builds both
   bundles from the tagged commit, checks the tag against
   `tauri.conf.json` before it builds anything, writes `SHA256SUMS.txt`, and
   opens the release as a **draft**. Pushing the tag is therefore reversible up
   to the moment somebody presses publish. **What the tag does not do is the
   DoD's verification** — a download from the release page, an install, a
   call — and on this machine that install proves only what §6.3 already
   proved here.

### What this machine needs

- **Dot-source `$env:USERPROFILE\gv\env.ps1` before any cargo command.** It enters the MSVC dev
  shell, puts LLVM after it (DR-30), and points `CARGO_TARGET_DIR` at a path
  short enough for the vendored C++ (DR-30) and outside OneDrive.
- **`--features custom-protocol` is not optional** for any binary that will be
  looked at or measured: without it the webview points at the Vite dev server
  and every number is about Edge's error page (DR-22).
- **A `goodvoice://` link only reaches an installed client.** The scheme is
  registered by the installer, so testing links means building the bundle and
  installing it, not running from `target/release`.
- The measurement tools are their own package: `cargo run -p goodvoice-harness
  --bin <name>` (DR-29). `bin/listener` is the second person in the room that
  most drills lean on (DR-26).

### The gates, all green before a commit

```powershell
. $env:USERPROFILE\gv\env.ps1     # leaves you in client\src-tauri
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace              # 181 tests
cd ..;         npm run format:check; npm run typecheck
cd ..\server;  npm run format:check; npm run typecheck; npm test   # 85 tests
```

Run them **unfiltered**. A formatting violation was committed once because the
gate's output was piped through a filter that hid its diff.

### How commits are written here

Conventional Commits, English, imperative subject, and a body that says what
was measured rather than what was intended. The author and committer are the
repository owner: **no AI co-author, committer or session trailers.** Mark a
task `[x]` in the same commit that completes it.

### The remote

`origin` is GitHub over SSH, and SSH works from **Windows git**, not from WSL —
`git fetch` from WSL fails with a public-key error while the same command
through `powershell.exe` succeeds. The two histories diverged once (the same
work under different hashes) and were rebased onto `origin/main` on 2026-08-25;
local has been ahead-only since, so a push is a fast-forward.

---

## Phase 0 — Scaffolding

Goal: repo builds, CI green, hello-world client and worker exist and deploy.

- [x] **0.1 Rust workspace + Tauri v2 hello world** — `client/src-tauri/*`,
  `client/ui/*`. Init Tauri v2 app (SolidJS + TS + Vite template) in `client/`;
  workspace compiles with empty `audio/capture/rtc/tray` modules declared.
  DoD: dev build opens a window showing "goodvoice".
  Verify: `cd client && npm install && npm run tauri build -- --debug` (on
  non-Windows dev hosts: `cargo check` inside `client/src-tauri` at minimum).
- [x] **0.2 Worker hello world** — `server/src/index.ts`, `server/wrangler.toml`,
  `server/package.json`, `server/tsconfig.json`. Minimal Hono-less router (plain
  `fetch` handler) returning `{"ok":true}` on `GET /health`; Durable Object class
  `Room` declared and bound (empty shell).
  DoD: local dev server answers health check.
  Verify: `cd server && npm install && npx wrangler dev` + `curl localhost:8787/health`.
- [x] **0.3 Lint/format gates** — `client/src-tauri` (clippy pedantic per
  styleguide), Prettier config for `server/` and `client/ui`.
  DoD: all four commands pass clean:
  `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `npx prettier --check .` (server and ui), `npx tsc --noEmit` (server and ui).
- [x] **0.4 CI** — `.github/workflows/ci.yml`. On push/PR: rust fmt+clippy+test
  (windows-latest runner), server/ui prettier+tsc, `wrangler deploy --dry-run`.
  DoD: workflow file is valid and passes on push.
  Verify: `gh run watch` on the push, or `act` locally if available.
  Green on run 32386485723 — all four jobs, including rust on windows-latest.
  **Green again on run 32617030554, which is the first run since task 3.4
  vendored a C++ tree** — the twenty-seven commits between the two were never
  pushed, so four separate breakages sat unseen and arrived together. DR-30
  has them: prettier reformatting somebody else's source, and three different
  ways for a Windows runner to build C++ with the wrong toolchain.
- [x] **0.5 [WIN] Hardware risk probe (spike)** — `client/src-tauri/harness/src/bin/probe.rs`.
  Tiny binary that (a) enumerates WASAPI render/capture devices + their shared-mode
  min buffer sizes, (b) enumerates Media Foundation H.264 encoders and flags which
  are hardware (NVENC/AMF/QuickSync). Front-loads the two hardware unknowns.
  DoD: probe output from a real Windows gaming machine pasted into a Decision
  Record in this file (devices found, min buffer ms, hw encoders found).
  Verify: `cargo run -p goodvoice-harness --bin probe` on Windows host.
  Run on the RTX 2060 machine, output in DR-12. Two answers worth having
  early: `IAudioClient3` offers **nothing beyond the default period** on this
  hardware, which is most of task 2.1's argument, and NVENC is present, which
  is task 5.2's. Off Windows the binary refuses and exits 2 rather than
  printing an empty report that would read like a machine with no devices.
  It also lists the endpoints that are present and **not** active, with their
  state — added for DR-41, where the question was not what the app would open
  but whether this machine has a loudspeaker at all.
- [x] **0.6 README + LICENSE + .gitignore checked in** — root. README states the
  three features, perf budgets, self-host one-liner; MIT LICENSE.
  DoD: files exist, README renders. Verify: `git ls-files | grep -E 'README|LICENSE'`.

## Phase 1 — Signaling core

Goal: rooms exist server-side; a mock client can join/leave and get SFU credentials.
No Rust in this phase — Worker + DO only, integration-tested with vitest.

- [x] **1.1 Room Durable Object: roster + cap** — `server/src/room.ts`. In-memory
  only (no storage API): participants map (id → {name, joinedAt}), join rejects at
  8 with a typed error, leave removes, last-leave resets state. WebSocket per
  participant for roster pushes. Zod-validate every inbound message.
  DoD: unit tests cover join/leave/cap/last-out-reset.
  Verify: `cd server && npx vitest run` (uses `@cloudflare/vitest-pool-workers`).
- [x] **1.2 Worker routes** — `server/src/index.ts`. `POST /rooms/:code/join` →
  routes to DO by room code (idFromName), `GET /rooms/:code/ws` → WebSocket upgrade
  proxied to DO, `GET /health`. No other routes. CORS: `origin: *`.
  DoD: route tests pass. Verify: `npx vitest run`.
- [x] **1.3 Realtime SFU credential exchange** — `server/src/room.ts` + new
  `server/src/sfu.ts`. On join, DO calls Cloudflare Realtime API
  (`CALLS_APP_ID`/`CALLS_APP_SECRET` secrets) to create a session; returns session
  credentials + TURN credentials to the client. Bitrate caps read from
  `MAX_VIDEO_BITRATE`/`MAX_AUDIO_BITRATE` env vars (cloudflare/meet pattern).
  DoD: mocked-API tests pass; a real `wrangler dev --remote` join returns live
  credentials. Verify: `npx vitest run` + manual curl against dev deploy.
  Verified live against the deploy from task 1.5: a join answers a real
  `sessionId` plus Cloudflare TURN `iceServers` with username/credential (all
  four secrets set, see DR-1). Mocked tests still cover the failure paths.
- [x] **1.4 Timeout cleanup + room death** — `server/src/room.ts`. Heartbeat over
  WS; missed heartbeats (e.g. 30 s) evict the participant; DO alarm as backstop;
  empty room leaves zero state behind.
  DoD: tests simulate silent disconnect → eviction → empty-room reset.
  Verify: `npx vitest run`.
- [x] **1.5 Deploy to real Cloudflare** — `server/`. `wrangler deploy` to a test
  account; document required secrets in `wrangler.toml` comments.
  DoD: deployed URL passes health + join flow with curl/websocat script committed
  as `server/scripts/smoke.sh`. Verify: `bash server/scripts/smoke.sh <url>`.
  Live at **https://goodvoice.goodvoice-server.workers.dev** (account
  `de05ba5339883a30422ae126363028fc`), all four smoke checks green. See DR-5 for
  why the run right after a `wrangler secret put` is expected to flake.

## Phase 2 — Voice MVP (2 people)

Goal: two clients talking through the SFU. Riskiest phase — spikes first.

- [x] **2.1 [WIN] WASAPI capture/playback spike** — `client/src-tauri/src/audio/`.
  Event-driven shared-mode capture → ring buffer → playback (loopback monitor).
  Decides `cpal` vs `wasapi` crate (PRD open question 4) — write the Decision Record.
  DoD: mic loopback audible; round-trip device latency measured and recorded in DR.
  Verify: `cargo test -p goodvoice-client audio` + manual loopback run
  (`cargo run -p goodvoice-harness --bin audio-spike`).
  **Narrowed by 2.4.** The seam is in (`audio/device.rs`) with a working cpal
  backend behind it (`audio/hardware.rs`), so this task is no longer "build the
  audio layer" — it is the measurement it was always about: does cpal's WASAPI
  backend hit the 80 ms budget, or does the `wasapi` crate's control over
  shared-mode buffer sizes buy enough to justify a second backend? Whatever wins
  lands beside `hardware.rs` and nothing above the seam moves. See DR-8.
  **The crate decision is made: cpal stays — DR-15.** The driver reports
  minimum = default = maximum = 480 frames (10 ms) for shared mode (DR-12), and
  cpal runs event-driven at exactly that period, so a `wasapi` backend would
  land on the same number. It is parked until a device reports something
  smaller *and* the budget needs it; DR-14's 41.4 ms leaves 38 ms of headroom.
  Built: `bin/audio-spike` — `--seconds N` plays the microphone back out of the
  speakers through the same `hardware::open()` the app uses (2250 frames in
  45.0 s on the DR-12 machine, no drift), and `--roundtrip` times a 5 ms burst
  from the speakers to the microphone. Both print the negotiated configuration
  (`hardware::describe()`), and the round trip refuses to report a spread when
  fewer than half its bursts came back — room noise is not a measurement.
  Also new: `audio/burst.rs`, the burst and its bookkeeping, shared with
  `bin/latency` and tested (nine tests) where it used to be untested private
  code in a binary.
  **Both halves are now done, on the DR-12 machine, and the answer is worse
  than DR-15 expected — see DR-23.** The loopback is audible, confirmed by a
  person wearing the headset while the meter peaked at 14 837. The round trip
  measured **84.7–86.1 ms** across four runs, of which our own rings hold
  0.0 ms at the median: it is all below cpal. A control run moved the render
  device off the USB DAC onto the motherboard's analog codec and the number did
  not move, so the render leg is not where it goes.
  That makes a call's mouth-to-ear about **106 ms against the 80 ms budget**
  (21.4 ms of measured wire plus 84.7 ms of measured devices), and it retires
  DR-14's 41.4 ms, which had added DR-12's engine period as a stand-in for the
  device cost. `wasapi` still loses — the control run cleared the leg it would
  have helped — so what DR-23 sends back is the budget, not the backend.
  **The one experiment still owed:** the same round trip with a capture device
  that is not the fifine, which separates the microphone from the stack. This
  machine has no second microphone.
- [x] **2.2 Opus encode/decode pipeline** — `client/src-tauri/src/audio/opus.rs`.
  20 ms frames, 48 kHz, 32 kbps start; encode→decode round-trip preserves audio.
  DoD: unit tests with synthetic tones; no allocation on the frame path (assert
  with a counting allocator in test builds).
  Verify: `cargo test -p goodvoice-client opus`.
  Done out of order — it is not [WIN] and does not depend on 2.1's outcome.
  See DR-3 (crate choice) and DR-4 (CMake pin).
- [x] **2.3 webrtc-rs ↔ Cloudflare SFU spike** — `client/src-tauri/harness/src/bin/rtc-spike.rs`.
  Prove webrtc-rs can complete ICE/DTLS with Realtime SFU and push an Opus track
  (PRD open question 2). This is the project's biggest unknown — if blocked,
  Decision Record + libwebrtc FFI evaluation, and STOP for user input.
  DoD: a published track from client A is pulled by a throwaway web page or second
  client. Verify: `cargo run -p goodvoice-harness --bin rtc-spike -- --room test` against Phase 1 deploy.
  **Server half:** the Worker proxy DR-2 called for is in —
  `POST /rooms/:code/sfu/tracks/new`, `PUT …/renegotiate`, `PUT …/tracks/close`,
  signed with the app secret, scoped to the caller's own session, and refusing
  any track that pulls from a session outside the room (58 tests green).
  **Client half:** the spike runs both ends in one process — a speaker publishes
  a 440 Hz tone as `mic`, a listener finds it on the roster, pulls it, and
  decodes it back. It passed against the live deploy on the first run, and the
  `[WIN]` tag it used to carry was wrong: nothing on this path is
  Windows-specific. See DR-7. It now also passes on the Windows host, which it
  could not before DR-14.
- [x] **2.4 Wire join flow end-to-end** — `rtc/session.rs`, `audio/`, minimal UI
  (`client/ui`): room code input → join → publish mic → subscribe/playback peers.
  DoD: two Windows machines (or machine+VM) hold a conversation.
  Verify: manual two-client call + `cargo test` green.
  Built: `rtc/signaling.rs` (HTTP join + roster WebSocket + heartbeat),
  `rtc/session.rs` (`Call`: join, publish, subscribe as the roster changes,
  mute, deafen, leave), `audio/device.rs` (the `AudioSource`/`AudioSink` seam),
  `audio/hardware.rs` (cpal behind it), Tauri commands, and the SolidJS UI.
  `bin/call.rs` is a windowless client for two-machine testing; `bin/rtc-spike.rs`
  now drives the real `Call` and checks a **two-way** conversation with crossed
  tones. See DR-8 for what the live runs turned up.
  **Not verified:** the DoD's two Windows machines. Everything here is hardware
  agnostic and was exercised on macOS; the Windows leg needs a Windows host.
- [x] **2.5 [WIN] Latency measurement harness** — `client/src-tauri/harness/src/bin/latency.rs`
  or in-app debug overlay. Measure mouth-to-ear latency (loopback tone timestamp
  method) across the real SFU path.
  DoD: measured number recorded in a Decision Record vs the 80 ms budget; if over
  budget, the DR lists the suspects (buffer sizes, jitter buffer depth) and next steps.
  Verify: committed measurement notes + reproducible run instructions.
  `bin/latency.rs` runs both ends in one process against the live SFU, so both
  timestamps come off one clock and there is no synchronisation to get wrong.
  One side is silent but for a 5 ms burst once a second; the other stops the
  clock on the burst's leading edge. It reports min/median/p95/max for the wire
  path, adds DR-12's 20 ms of device period, and compares the total against the
  80 ms budget.
  **Measured, on native Windows: 41.4 ms mouth to ear, against 80 ms** — a
  21.4 ms median wire path plus DR-12's 20 ms of device period, 30 bursts, none
  lost. See DR-14 for the run and for what the number leaves out (no jitter
  buffer exists yet, and it will spend some of the 38 ms of headroom).
  DR-13's blocker is closed by DR-14: one ICE URL this network cannot reach was
  keeping webrtc-rs' gathering from ever completing. The same fix is what let
  `bin/rtc-spike.rs` and `bin/reconnect-drill.rs` run on Windows at all — both
  PASS there now, where DR-7 and DR-8 had only ever seen them pass on macOS.

## Phase 3 — Full rooms

- [x] **3.1 N-party audio** — `rtc/`, `audio/mixer.rs`. Subscribe to up to 7 remote
  tracks, mix for playback; roster UI shows who's in the room and who's speaking.
  DoD: 4+ clients (mix of machines/VMs) converse; CPU stays in budget.
  Verify: manual multi-client session + `cargo test`.
  Task 2.4 already subscribes to all seven and sums them, one ring per slot, in
  the render callback — so nobody is inaudible today. What this task adds is the
  real mixer: per-speaker gain, and the level metering the roster needs to show
  who is talking. It also owns the CPU budget at four-plus clients, which has
  never been measured.
  Built: `audio/mixer.rs` — per-speaker gain in Q8.8, a saturating sum, and a
  decaying level per slot, all read and written from the render callback
  without a lock. `hardware.rs` is now only the cpal callback handing that
  mixer a buffer. `Call` exposes it as `speaking()`, `level_of()` and
  `set_gain_of()` (per-listener gain: turning someone down needs nobody's
  agreement).
  The UI half is in too: `goodvoice://speaking` carries the set of talking
  participant ids to the webview — its own event and not part of the roster's,
  because a roster changes when somebody joins and this changes with every
  sentence. The roster dot grows a halo for whoever is talking; muted wins over
  talking, since a peer's last buffered frames can still be playing when their
  mute flag arrives. A seat that ends now drops its speaking set and its slot
  map (`Shared::silence`), so a reconnect does not leave the roster lit up with
  people nobody is hearing.
  **Not verified:** the DoD itself — four-plus clients conversing, with the CPU
  measured while they do. That needs four hosts; everything above is hardware
  agnostic and `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
  and `cargo test` (70 tests) are green on Linux. Until the measurement exists,
  the room's CPU budget at four-plus clients is still an assumption.
  **§7.8** owns this now.
- [x] **3.2 Mute / deafen** — `audio/`, `ui`. Mute halts encoding+sending (packets
  stop, not zeroed — assert in test via packet counter); deafen halts playback;
  state visible in roster for everyone (signaling message).
  DoD: tests + two-client visual/audio confirmation. Verify: `cargo test` + `npx vitest run`.
  Task 2.4 shipped the mechanism; what was missing was the proof. `cargo test`
  covers the client half (packets stop rather than going silent, and start
  again on unmute), and `server/test/presence.test.ts` covers the room's half:
  both flags reach everyone else's roster, they stack, they survive a late
  joiner, and a message the room cannot parse moves nothing.
  **The two-client confirmation is in, and it is measured rather than heard:**
  `bin/mute-drill.rs` puts two real `Call`s through the live SFU with crossed
  tones and watches the far sink's frame counter across four transitions.
  Counting frames at the far end is the strongest form the "packets stop, not
  zeroed" assertion can take — a client sending silence would keep the count
  climbing; only one sending nothing freezes it. Two consecutive runs:

  | Transition | What the far end saw |
  |---|---|
  | alice mutes | 0 frames in 2 s, and bob's roster reads `muted=true` |
  | alice unmutes | 100 frames in 2 s, roster back to `muted=false` |
  | bob deafens | 0 frames at bob, 100 at alice — deafen is one ear, not a drop |
  | bob undeafens | 100 frames, roster back to `deafened=false` |

  Each flag is read from the *other* client's roster, which is what the DoD's
  "visible for everyone" means. Verify: `cargo run -p goodvoice-harness --bin mute-drill`, plus
  `cargo test` (116) and `npx vitest run` (85, `presence.test.ts` among them).
  **Both halves closed on the real app — DR-26.** A person muted the shipping
  client with its own microphone in a live room, and a listener in the same
  room measured 0 frames for as long as the mute lasted and 50 a second again
  after it. The `MUTED` tag was seen in the roster, in a screenshot. Nobody
  put their ear to the silence, and nothing would be learned by it: there is no
  path by which the packets stop and the sound does not.
  Found by the drill and fixed here: leaving a call printed the whole RTP
  packet it failed to send, because a channel `SendError`'s `Display` embeds
  it. A client on its way out has no track by definition, so that first
  failure is expected rather than news.
- [x] **3.3 Push-to-talk + VAD modes** — `audio/vad.rs`, `ui` settings. PTT
  (in-window key first; global hotkey is Phase 4), VAD via webrtc-audio-processing's
  voice detection with hangover time; mode persisted locally.
  DoD: both modes demonstrably gate transmission. Verify: `cargo test audio::vad` + manual.
  **The named crate could not supply the detector.** `webrtc-audio-processing`
  2.1 still declares `voice_detected` and never populates it — its own tests
  assert the field is empty. The detector is the standalone WebRTC VAD
  (libfvad) instead; see DR-10 for what was compared and what it measured.
  Built: `audio/vad.rs` — `TransmitMode` (open / push-to-talk / voice activity)
  and `Gate`, which answers one question per frame. `Shared` carries the mode
  and the key as atomics so a reconnect keeps both, and the publish loop asks
  the gate before it encodes: a frame nobody will send is not encoded, exactly
  as with mute. The speaking indicator follows the gate rather than the meter,
  so releasing the key puts the light out at once. In the UI the picker is on
  both panels — someone who wants push-to-talk wants it *before* they join —
  and the mode and key persist in the webview's storage.
  Verified: `cargo test` (85 tests) covers the gate on its own and through the
  publish loop — the key up sends nothing, the key down sends every frame, a
  silent microphone in voice mode sends nothing across 200 frames, and mute
  still wins over a held key.
  **The manual half is closed, measured rather than heard — DR-26.** With the
  keyboard focus on another application entirely, the talk key up gives 0
  frames at a listener in the room and the key held gives 50 a second, with the
  transitions landing in the second the key moves. Which is the same check by
  ear, minus the ear.
- [x] **3.4 AEC/NS/AGC integration** — `audio/processing.rs`. webrtc-audio-processing
  between capture and encode; loudspeaker echo cancelled (needs render-stream
  reference feed).
  DoD: speaker-echo test call shows no self-echo; DR records config chosen.
  Verify: manual echo test + `cargo test`.
  **Unparked: vendored, and it builds on Windows — DR-24.** The implementation
  is `e50e674` restored unchanged — `audio/processing.rs` (AEC3 + noise
  suppression + AGC2 over the 20 ms frame in two 10 ms passes) and the render
  tap that gives the canceller its far end. What was missing was a build, and
  `vendor/webrtc-audio-processing-sys` is 2.1.0 with the fixes: DR-11's five,
  plus a sixth it could not have seen. The sixth is the interesting one — the
  vendored `meson.build` marks every `RTC_EXPORT` symbol `dllexport` even in a
  static library, so each object carries `/EXPORT:` directives that `objcopy`'s
  symbol prefixing then leaves pointing at names it has just renamed. 156
  unresolved externals, all of them in the export table.
  Measured on both hosts, and the test now prints it rather than only asserting
  it: **echo cancelled by 31.8 dB** (residual 125 of 4872 played). `cargo test`
  is 122 green on Windows and on Linux; `cargo fmt --check` and
  `cargo clippy --all-targets --all-features -- -D warnings` are clean.
  Windows builds now need meson, ninja and LLVM; CI installs them.
  **Not verified:** the DoD's own words — a speaker-echo *test call*. The echo
  test is synthetic and perfect, which is the hard case for a canceller but not
  the real one. A person on loudspeakers in a room is what is still owed.
  **§7.6** owns this now.
- [x] **3.5 Auto-reconnect** — `rtc/reconnect.rs`. Exponential backoff, rejoin same
  room, resubscribe all tracks; UI shows reconnecting state.
  DoD: kill network 10 s mid-call → call resumes without restart.
  Verify: scripted netdown test documented + manual run.
  Task 2.4 covers reconnecting *into* a call — the join retries up to three
  times, handing its seat back between attempts. What this task owns is a call
  that drops after it started, which today just ends. Two known cases from DR-8
  to cover: the ~1-in-6 handshake that still fails after three tries, and the
  roommate holding a session id that a peer's own retry has already replaced.
  Built: `rtc/reconnect.rs` (the retry schedule and `CallState`), and `Call`
  turned into a supervisor — the microphone and the user's mute/deafen outlive
  any one seat, and sessions come and go underneath. `bin/reconnect-drill.rs`
  is the automated proof; docs/testing/reconnect.md has both it and the
  `pfctl` run that takes the network away for real. Both DR-8 cases are
  covered, and both were seen happening in the drill's output. See DR-9. The
  drill passes on the Windows host too as of DR-14.
  **Not verified:** the netdown run itself, on either host. The drill kills the
  session; only pulling the network checks that the client *notices* — see
  docs/testing/reconnect.md.
  One thing the drill could not see, found while wiring the tray menu (4.2):
  the *app* kept holding a call that had ended, so a client whose call dropped
  for good could not join anything again without being restarted. The drill
  drives `Call` directly and never went through `CurrentCall`, which is where
  it was stuck. **§7.7** owns this now.

## Phase 4 — Tray & polish

- [x] **4.1 Minimize-to-tray** — `tray/`, Tauri config. Close/minimize hides window,
  tray icon persists, voice continues; restore on click.
  DoD: manual flow works; no window flicker. Verify: manual + `cargo clippy` clean.
  Built: `tray/mod.rs` — the icon, a two-item menu (Open / Quit), and the
  window event handler that turns close and minimise into a hide. `Call` never
  hears about any of it: audio is in Rust and never depended on the webview
  being visible.
  Two things the task did not ask for and needs anyway. **Quit hands the seat
  back** before exiting (`crate::end_call`, three-second grace) — the close
  button no longer ends a call, so something else has to, or the next join
  meets a room full of this client's own ghosts (DR-5). And **close-to-tray is
  only in force while a tray exists**: a host that refuses the icon keeps a
  window that closes normally, because close-to-tray plus a missing icon is an
  app that cannot be quit at all.
  Verified, scripted, on Windows (docs/testing/tray.md): closing hides the
  window and the process lives (`ALIVE_AFTER_CLOSE=True`,
  `VISIBLE_AFTER_CLOSE=False`), minimising does the same, and a window in the
  state the hide leaves it in can be brought back. `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings` and `cargo test` (100) are green
  on Windows and Linux. See DR-16 for the two traps the scripts are shaped
  around — both of them cost an hour of chasing a bug that was not there.
  **The DoD's own sentence — "voice continues" — is now measured, not assumed
  (DR-26).** With a live call running, the window was closed, the process
  stayed up with **zero WebView2 processes** left, and a listener in the room
  kept receiving **50 frames a second throughout**, with real audio in them.
  Clicking the tray icon rebuilt the window inside the same call, roster and
  all. The icon was found by name under `Shell_TrayWnd` without opening the
  chevron, so it starts visible on this machine.
  **Not verified:** whether the rebuild flickers. That is one row of
  docs/testing/tray.md and the only one left, because it is about what an eye
  catches between two frames. **§7.3** owns this now.
- [x] **4.2 Tray menu** — `tray/menu.rs`. Mute/unmute, deafen, leave room, quit —
  all functional and state-synced with UI.
  DoD: each item verified against in-room state. Verify: manual checklist in PR.
  Built: `tray/menu.rs` — Open, a separator, **Mute** and **Deafen** as check
  items, **Leave room**, and Quit. Ids and actions are one table, so an item
  cannot be added without an arm to answer it; a menu with a dead button in it
  is worse than one item short.
  **Neither half owns the state.** The call does, and `Controls`
  (`{in_call, muted, deafened}`) is the copy pushed to both: `push_controls`
  forwards every change to the webview as `goodvoice://controls` *and* to the
  menu's ticks. So a mute from the tray lights the window's button and a mute
  from the window ticks the tray's box, without either one knowing the other
  exists. `in_call` is what greys the three items out — a menu offering to
  leave a room you are not in is a menu that lies.
  **Fixed on the way past:** a call that ended on its own was still being held
  as if it were running, so the next join was refused with "already in a call"
  and the only way back into a room was to restart the app. `push_state` lets
  go of it now, which is also what greys the menu out when a call drops.
  **`cargo test` had to be rescued to say any of that.** Mutating a menu item
  pulls comctl32 v6 into the library, and the library's unit-test executable is
  the one target `tauri_build`'s manifest does not reach — so every test on
  Windows became one unexplained `0xc0000139`. `build.rs` delay-loads comctl32;
  DR-17 has why the obvious fix does not work.
  Verified: `cargo test` (103) covers the id table and the `Controls` payload
  the window parses; `cargo fmt --check`, `cargo clippy --all-targets -- -D
  warnings`, `npm run typecheck` and `prettier --check` are green, and 4.1's
  two drills still pass on Windows after the refactor.
  **Not verified:** the checklist itself — docs/testing/tray.md, "The menu",
  seven rows, each one checked against the window and against a roommate.
  **§7.2** owns this now.
- [x] **4.3 [WIN] Global push-to-talk hotkey** — `tray/hotkey.rs`. Low-level
  keyboard hook (`WH_KEYBOARD_LL`); works while a fullscreen game has focus.
  Write the anti-cheat Decision Record (EAC/BattlEye/Vanguard stance, PRD open q3).
  DoD: PTT works over a running game; DR committed.
  Verify: manual in-game test.
  Built: `tray/hotkey.rs` — the hook, and `vk_for_code`, which turns the key
  name the webview stores (`KeyboardEvent.code`, a physical key) into the
  virtual-key code Windows reports. That table is the part that can be wrong
  without anything crashing, so it is where the tests are (7 of them, and they
  run anywhere).
  Three things it deliberately does not do: **it does not swallow the key** —
  a talk key a game stops seeing is a key nobody would bind to a weapon; **it
  injects nothing anywhere**, which is the whole of the anti-cheat argument
  (DR-18); and **it is not installed unless it is needed** — the hook goes on
  when a call is in push-to-talk mode and comes off with the call.
  The window says which kind of push to talk it has, because "heard from
  anywhere" and "heard only in this window" look identical until somebody is
  inside a game.
  Verified, scripted: `docs/testing/hotkey.ps1` starts `bin/hotkey-drill` —
  a process with no window at all — and synthesises three F13 presses from
  another process. All six edges arrive, in order, and the drill exits 0. The
  in-window handler from 3.3 is still there and still works if the hook cannot
  be installed.
  Steps 1–3 of docs/testing/hotkey.md are closed (DR-26): the scripted drill
  hears the key from a process with no window, a listener in a live room hears
  the *voice* gated by that key while another application holds the keyboard,
  and the key is not swallowed — `hotkey.rs` calls `CallNextHookEx`
  unconditionally after reporting, which is a stronger answer than watching a
  text field fill up.
  **Not verified — the DoD itself:** the key held over a running fullscreen
  game, and the game still receiving it. Steps 4–5. **§7.5** owns this now.
- [x] **4.4 [WIN] Cold-start budget** — measure app-launch → audible-in-room; must
  be <3 s. Optimize (lazy UI, parallel join+audio-init) until it is.
  DoD: measurement in DR, budget met. Verify: scripted timing run, 5-run median.
  **Met: 2692 ms median of five runs, 308 ms inside the budget.** It started at
  9.3 s. `bin/coldstart` is the harness — it launches the real app and stops
  the clock when a client already in the room decodes its first frame, so the
  number covers process start, WebView2, the devices, the join, ICE, DTLS, the
  SFU and the far end's subscribe. Nobody clicks anything: the app joins the
  room named in `GOODVOICE_AUTOJOIN`.
  Where the 6.6 s went, and what took it out (DR-19): **ICE gathering was 1.5 s
  of it and is now 40 ms** — it stops once there is a direct path and one
  fallback, instead of waiting out all six of Cloudflare's TURN ports for
  fallbacks it will never use. A renegotiation used to pay a whole quiet window
  again for an answer that cannot change; it does not now. The roster socket
  and the peer connection are opened together rather than one after the other.
  Two of the optimisations the task suggested were measured and refused:
  opening the audio devices takes 72 ms, so there is nothing to parallelise,
  and the window is already up 320 ms in, so a lazy UI would be racing
  something that is not on the path.
  Verified: five runs, all five heard, on the DR-12 machine against the live
  deploy — and `rtc-spike`, `reconnect-drill` and `latency` all still pass on
  the same build, because the gathering rule decides what goes in every SDP.
- [x] **4.5 [WIN] Idle CPU/RAM budget verification** — 30-min idle-in-room soak:
  CPU <2%, RAM ≤120 MB.
  DoD: numbers in DR, budgets met (or DR explains the gap + fix tasks added).
  Verify: soak script + Task Manager/ETW capture committed to `docs/perf/`.
  **CPU met, with room: 0.39% median of a twelve-processor machine against a
  2% budget. RAM not met: 361 MB against 120.** Nothing in those two sentences
  is about the same code. `goodvoice-client.exe` — the devices, Opus, the
  mixer, the transport, the roster — is **34 MB**; the other 327 are the six
  WebView2 processes, all of them resident with the window hidden. DR-20 has
  the breakdown, the options and what each is worth; **task 4.6** is the fix.
  Built: `bin/soak` — launches the release app, joins it to a real room with a
  second client already in it, minimises it into the tray, and reads the
  **whole process tree** every two seconds. Measuring only the process with our
  name on it would have reported 34 MB and passed a budget the app misses by
  three times.
  Two things it refuses to measure without. **Somebody in the room**: an app
  alone in a room subscribes to nothing and decodes nothing, which is a cheaper
  client than anybody runs. And **liveness** — the second client counts frames
  arriving from the app, because a soak where the app quietly fell out of the
  room is a measurement of an idle process and would be the cheapest possible
  way to pass. 897 of 897 samples carried audio.
  `docs/perf/idle-soak.ps1` measures the same tree through CIM and .NET while
  the soak runs: no shared code, no shared API. The two agree on CPU to two
  decimal places and on memory to a megabyte, so the numbers are about the app
  rather than about one implementation of the arithmetic.
  **Nothing grows.** Working set by five-minute bucket: 361.0, 363.9, 363.4,
  361.1, 361.0, 361.0 MB. Per process over 26 minutes the largest change in the
  tree is half a megabyte.
  Verified: 897 samples on the DR-12 machine against the live deploy, both
  captures committed to `docs/perf/`; `cargo fmt --check`, `cargo clippy
  --all-targets -- -D warnings` and `cargo test` green on Windows and Linux
  (the soak's arithmetic is six tests that run on any host).
- [x] **4.6 [WIN] Get the idle client under 120 MB** — `tray/`, `lib.rs`,
  `tauri.conf.json`. 4.5 measured 361 MB idle, of which 34 is goodvoice and 327
  is WebView2 with the window hidden (DR-20). Three levers, cheapest first:
  **(a)** `additionalBrowserArgs` — no GPU process for a 420-pixel roster, one
  renderer, no features nobody uses; **(b)** `ICoreWebView2_3::TrySuspend` while
  the window is hidden and `Resume` on show, reached through
  `WebviewWindow::with_webview` and `webview2-com` (**pinned to tauri's `windows`
  0.61, not this crate's 0.62** — they are different crates and the COM types
  do not interconvert); **(c)** dropping the webview entirely while in the tray
  and rebuilding it on show — the only one that reaches the 34 MB floor, and
  the one that gives up 4.1's "coming back is instant".
  DoD: `bin/soak` reports a peak at or under 120 MB with the call audible in
  every sample — or a DR says which levers were tried, what each measured, and
  why the budget is being restated instead.
  Verify: `cargo run -p goodvoice-harness --release --bin soak`, 30 minutes, both captures updated.
  **Met with room to spare: 34.1 MB peak, 34.0 median, against 120.** (c), and
  only (c). (a) and (b) share a ceiling neither can pass — they make an idle
  browser cheaper and the browser is still running, which lands somewhere near
  200 MB against a 120 MB budget and a 34 MB floor. A cheaper lever that does
  not reach the budget is not a cheaper way to pass it. DR-21 has the reasoning.
  Built: close and minimise both **destroy** the window; `run()` refuses the
  exit that the last window closing would otherwise cause (`code: None` is what
  tells it apart from the tray's Quit, which is meant); `tray::show` rebuilds
  from the app config in ~130 ms. A window built mid-call has missed every
  event that ever described it, so it asks — `current_status` returns a
  `Snapshot` and App.tsx applies it on mount, behind anything an event has
  already said. One hole of the same shape found on the way: a call joined
  without a window asking for it (`GOODVOICE_AUTOJOIN`, and 6.2's invite links)
  was never announced to the window at all, and no event carried the room name.
  `CALL_EVENT` does now.
  **Two defects found while verifying it, both older than this task and neither
  catchable by anything that was in the repo** (DR-22). `cargo build --release`
  produced a client pointed at the Vite dev server, because `Cargo.toml` never
  declared the `custom-protocol` feature — so 4.5's measurements, and every
  screenshot ever taken of a hand-built release, were of a WebView2 hosting
  Edge's "localhost refused to connect". And there was no `capabilities/`
  directory, so Tauri v2's ACL refused every `listen` in App.tsx, in dev as
  well as release: the window had never received a roster change, a talking
  dot, a reconnect, or anything the tray did to mute (task 4.2's whole point).
  Both fixed here, because 4.6 cannot be verified without them.
  Built: `docs/testing/tray-roundtrip.ps1` replaces the two 4.1 scripts and
  checks the round trip in about a minute — joined to a real room, close (or
  `-Via minimise`), then the notification-area icon **clicked for real**, since
  Windows 11's tray is a XAML island with no `ToolbarWindow32` to hit-test.
  7 processes and 333 MB with the window up; 1 process and 33.7 MB in the tray;
  a new window 130 ms after the click, showing the room it left. It screenshots
  both ends, because whether the rebuilt window shows the call or the join form
  is the one thing no assertion covers. It quits through the tray menu too — an
  app that closes into a tray it cannot be quit from is the trap this whole
  design is shaped around.
  **Nothing accumulates across rebuilds.** Twelve cycles: 33.7, 34.1, 34.2,
  34.3, 34.4, 34.5, 34.8, 34.7, 34.7, 34.9, 34.9, 34.9 MB. It settles at 34.9
  and stays, which is a fixed cost paid once, not 0.1 MB a cycle forever.
  Verified: 896 samples over 30 minutes on the DR-12 machine against the live
  deploy, 896 of them carrying audio, both captures committed to `docs/perf/`;
  the PowerShell second opinion agrees to 0.1 MB and to two decimals of CPU;
  `tray-roundtrip.ps1` PASS both ways; `cargo fmt --check`, `cargo clippy
  --all-targets -- -D warnings` (both feature sets), `cargo test` (121),
  `npm run typecheck` and `npm run format:check` green.

- [x] **4.7 A settings screen, and an indicator that shows a level** —
  `audio/prefs.rs`, `audio/processing.rs`, `audio/vad.rs`, `audio/mixer.rs`,
  `rtc/session.rs`, `lib.rs`, `ui`. Everything WebRTC does to a microphone was
  a compile-time constant until somebody sat in a real room with it. Four
  things move now: input sensitivity (the detector, or a threshold the user
  sets), noise suppression, echo cancellation, and where the transmit mode
  lives.
  DoD: each switch reaches the microphone mid-call and is measured doing it;
  the roster indicator follows a level rather than a threshold.
  Verify: `cargo test`, plus `bin/call --mode voice --threshold` against
  `bin/listener` on the live deploy.
  Built: `audio/prefs.rs` is the one shared thing — atomics, because every
  field is read on the frame path fifty times a second and written when a human
  moves a slider. `Processing::reconfigure` switches WebRTC's stages on the
  generation change rather than per frame, since `set_config` allocates. The
  gate takes a `Sensitivity` now: libfvad, or a level. The window grew a
  `settings` screen holding both the audio controls and the appearance ones,
  and the transmit picker moved into it from the two panels that each carried a
  copy.
  **The indicator is a level, not a flag.** `Meter` keeps two decays — a slow
  one for "is this person talking", which has to ride out the gap between two
  words, and a fast one for what gets drawn, because a meter that takes seconds
  to fall is showing the loudest recent thing rather than the voice. The push
  went from 10 Hz to 20 Hz and carries numbers; a quiet room still pushes
  nothing at all, because every level in it quantises to zero. See DR-28,
  including the integer-division floor that had left the old meter resting at
  63/32767 for the rest of every call.
  Verified: `cargo test` 136 (15 new), `cargo fmt --check` and `cargo clippy
  --all-targets -- -D warnings` green on Windows and Linux; `npm run
  typecheck` and `prettier --check` green. **Measured through the live SFU:**
  the same client in voice mode reports **0 frames for 26 seconds** at a
  threshold nothing in the room reaches, and **50 frames a second** at one the
  room's hum crosses — the slider's own effect, at the far end. Screenshots of
  the panel and of a lit roster dot in `docs/ui/`.
  **Not verified:** whether the noise suppressor and the echo canceller sound
  better switched on than off, which is 3.4's outstanding row and needs the
  same person on loudspeakers. **§7.6** owns this now.

## Phase 5 — Screen share

- [x] **5.1 [WIN] WGC capture spike** — `capture/wgc.rs`. Enumerate
  monitors/windows, capture frames via Windows.Graphics.Capture, report fps/format.
  DoD: spike bin dumps N frames + timing stats; DR records surface format and
  frame-pool behavior. Verify: `cargo run -p goodvoice-harness --bin capture-spike`.
  Built: `capture/wgc.rs` is the whole capture half — `monitors()` and
  `windows()` say what there is to share, `Capturer` captures one of them, and
  `Frame` hands back the D3D11 texture the frame already lives in. Nothing on
  this path copies: `Frame::copy_to_cpu` exists for the spike, for tests and
  for a software fallback, and task 5.2's encoder takes `Frame::texture`
  instead.
  **The pool is free-threaded and the handler only signals.** WGC calls
  `FrameArrived` on its own thread; the consumer's thread is what calls
  `TryGetNextFrame`. That keeps every D3D11 call on one thread and makes "a
  frame nobody took" a recycled frame rather than a stalled pool — the drop
  policy a live share wants.
  Verified on the DR-12 machine, output in DR-31: **B8G8R8A8_UNORM, texture
  size equal to content size, 141 frames in 8.0 s from the primary monitor
  (36 fps at the median interval)** and 105 in 6.0 s from a window. Three
  1920×1080 frames written to disk and opened — real desktop, right colours,
  cursor drawn. `cargo test --workspace` 145 (3 new), fmt and clippy green.
  **Not measured here:** what capture costs the machine. WGC's own overhead is
  part of task 5.5's FPS budget and only means anything with an encoder behind
  it.
- [x] **5.2 [WIN] Hardware encode paths** — `capture/encoder.rs`. Media Foundation
  H.264: NVENC, AMF, QuickSync; pick first available hw MFT; zero-copy
  (GPU texture → encoder) where possible; software fallback flagged to caller.
  DoD: encoded bitstream plays in a standard player from at least one hw path
  (per 0.5 probe results); fallback path warns.
  Verify: `cargo run -p goodvoice-harness --bin capture-spike -- --encode` + committed sample analysis.
  Built: `capture/encoder.rs`. `encoders()` says what the machine has;
  `H264Encoder` opens the first that takes the format, hardware first. The one
  thing between the capture and the encoder is a `D3D11` video processor:
  hardware encoders take NV12, WGC produces BGRA8 (DR-31), and a
  `VideoProcessorBlt` on the capture's own device converts without the pixels
  leaving the GPU. `is_hardware()` and `is_zero_copy()` are how a caller knows
  which path it got, and `Selection::SoftwareOnly` is how the fallback gets
  exercised on a machine that has three hardware encoders.
  Verified on the DR-12 machine, analysis in `docs/perf/screenshare-encode.md`,
  findings in DR-32. **NVENC at 0.42 ms a frame against the software MFT's
  8.43 ms** — 20×, and the worst software frame (50.6 ms) is longer than a
  frame period. One packet per frame from both. The bitstream is Main profile
  level 4.0, 1920×1088 coded and cropped to 1080, with SPS and PPS repeated at
  every keyframe — which is what task 5.4's mid-share viewer needs. Both the
  elementary stream and the muxed mp4 **decode in VLC**, an unrelated decoder,
  showing the desktop that was captured.
  **Not measured here:** what capture and encode cost a game. That is 5.5, and
  0.42 ms is an argument rather than a demonstration.
- [x] **5.3 720p/1080p selection + publish** — `capture/`, `rtc/`, `ui`. Picker UI
  (monitor/window + quality), scale in encoder, publish H.264 track to SFU;
  server enforces one-sharer-at-a-time (DO rejects second share).
  DoD: share visible to a second client; `npx vitest run` covers the DO rule.
  Verify: manual two-client share + tests.
  Built, in four places. `capture/share.rs` owns a capturer and an encoder on a
  thread of their own and hands back H.264 packets — neither is `Send`, and
  that is the whole reason the thread exists. `capture/encoder.rs` grew
  `Quality`, which fits a source inside 720 or 1080 rows without ever
  upscaling; the scaling is the same `VideoProcessorBlt` that was already
  converting to NV12, so it costs nothing new. `rtc/screen.rs` is the seam —
  the transport publishes video through a trait and never sees a texture — and
  `rtc/session.rs` publishes the `screen` track, subscribes to a remote one,
  and reassembles RTP back into access units.
  **The server half was already done.** `room.ts` has refused a second `screen`
  since Phase 1, on both roads into it — the `share` message and the SFU
  proxy's `tracks/new` — and `test/tracks.test.ts` has covered it since. This
  task found nothing to add there, which is what the closed track vocabulary in
  `protocol.ts` was for.
  **A share outlives the session under it.** What the user picked is intent
  held on the call; each session opens its own capture from it. A reconnect
  restarts the encode rather than trying to carry a dead track across, which is
  also what a viewer needs — a new sequence starts with a keyframe.
  Verified, `bin/share-drill` against the live deploy: ana shared the primary
  display at **1280×720 on NVENC, live 2.9 s after the pick**; bruno received
  **141 access units, 878 KB, 4 keyframes**; carla was refused with *"ana is
  already sharing (already_sharing)"*. The bytes bruno received were written
  out and **decoded by VLC into ana's desktop at 1280×720** — an unrelated
  decoder, from the second client's own copy, which is what "visible to a
  second client" has to mean. `cargo test --workspace` 155, `npm test` in
  `server/` unchanged and green, fmt/clippy/tsc/prettier green.
  The picker and the live-share panel are in `docs/ui/share-picker.png` and
  `docs/ui/share-live.png`, driven through the real app: pick a quality, pick a
  monitor or window, and the panel says what is being shared and offers the way
  out. A software encoder would add a warning line there (prd.md §3 F3); this
  machine has NVENC, so the line is not in the shot.
  **Since verified, by §7.4:** the picker under the `retro` skin is
  `docs/ui/share-picker-retro.png`, driven through the installed app by
  `docs/testing/share-picker-shot.ps1`. The shared base holds on its own —
  nothing about the picker needed the `terminal` block to lay out.
- [x] **5.4 Viewer window** — `ui`, new Tauri window. Opt-in subscribe on open,
  unsubscribe on close, resizable, aspect-correct; audio unaffected throughout.
  DoD: open/close viewer repeatedly during live share, voice never glitches.
  Verify: manual + `npx tsc --noEmit`.
  **Closed by measurement — which found two bugs before it could close
  anything.** `docs/testing/viewer.ps1` drives the shipping client through UI
  Automation: it opens and closes the viewer four times against a live share
  while `bin/viewer-drill` shares a screen from the other end of the call and
  counts the voice coming back. **101 seconds measured, none of them below 45
  frames a second, lowest second 49** — and the same again on a second run and
  under the other skin. That is the DoD's sentence, measured the way DR-26
  measures things rather than listened to.
  The rest of the run is the picture: 954×534 inside a 960×540 window,
  675×375 stretched into 1084×381, 498×282 squashed into 504×661 —
  **1.77 to 1.80 every time**, which is 16:9 keeping its shape while the
  letterbox grows around it. Both skins, `retro` (the default, and the one
  §5.3 left unlooked-at) and `terminal`. `docs/ui/viewer-letterbox.png` is the
  stretched one as a person sees it.
  Built: `ui/Viewer.tsx` is a second Tauri window (label `screen`) rendered
  from the same bundle, routed on `location.hash` in `main.tsx`.
  `open_screen_viewer` and `watch_screen` in `lib.rs` are the commands behind
  it, and the main window grows a *watch N's screen* button whenever the roster
  shows somebody else sharing.
  **The decode happens in the webview.** WebView2 ships WebCodecs, so
  `VideoDecoder` decodes H.264 on the same silicon that encoded it, and what
  crosses the IPC is what came off the wire — tens of kilobytes an access unit
  — rather than 8 MB of pixels a frame. That is why there is no
  `capture/decoder.rs`: the alternative was more code and slower.
  The channel carries raw bytes (`tauri::ipc::Channel<Response>`), not JSON: a
  `Vec<u8>` as a JSON array of numbers is four times the size. **The first byte
  is the keyframe flag** — RTP does not carry one, a decoder cannot start on a
  P-frame, and sending it as a second message would let it arrive out of order.
  An empty message means the share ended.
  **Opt-in is the window's lifetime, and now literally so.** There is no
  `stop_watching_screen` command any more: a window is *destroyed*, and a
  webview taken down with its window never runs the cleanup that used to send
  it. `tray::window_event` sees the destruction and gives the subscription up
  (DR-33), so the only way to be subscribed is to have a viewer open, whichever
  way the window goes.
  `tray::window_event` also ignores every other event on that window: without
  that, task 4.6's minimise-into-the-tray would destroy the viewer and end a
  live share every time somebody put it out of the way.
  **Two bugs, both invisible to every gate.** Only the first viewer ever got a
  picture — DR-33, and every one after it sat on "nobody is sharing" while a
  share was live. And a screen that was not moving published nothing at all —
  DR-34, 0 access units in 20 seconds, which made the viewer black for anyone
  sharing a document rather than a game. Both are fixed and both have a
  regression that fails without the fix: `a_second_viewer_takes_the_frames_over`
  in `rtc/session.rs`, and the cycle table in `docs/testing/viewer.md`.
  Verified: `npx tsc --noEmit`, `npm run format:check`, `cargo fmt --all
  --check`, `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` (151) all green; the drills above against the live
  deploy.
  **Left unmeasured:** whether Cloudflare stops *sending* the video once a
  viewer closes. The client stops pulling and stops decoding, but nothing tells
  the SFU — `tracks/close` is allowlisted by the Worker and unused by the
  client — so the "Cloudflare is not sending video to a client with no viewer"
  half of §3 F3 holds until the first viewer opens and is unproven after one
  closes. Windows' per-process IO counters do not see the media sockets
  (5.2 / 5.4 / 5.6 kB/s across never-opened, open and closed-again: the voice
  path and nothing else), so it needs an instrument this task did not have.
  **§7.9** owns this now.
- [x] **5.5 [WIN] FPS-impact benchmark** — `docs/perf/screenshare-bench.md`.
  Run a GPU-bound game (e.g. built-in benchmark), record FPS with/without 1080p
  share (hw encode). Target ~0 delta.
  DoD: methodology + numbers committed; budget met or DR with fix plan.
  Verify: committed benchmark doc reproducible by another dev.
  **Numbers committed, and the budget is missed.** *Far Far West* at 57 fps
  with the GPU 95% busy, three 30-second PresentMon captures a run —
  game alone, game sharing, game alone again:

  | share | fps alone | fps sharing | delta | GPU ms a frame | noise |
  |---|---|---|---|---|---|
  | 1080p30 | 57.0 | 53.8 | **−5.6%** | 16.64 → 17.57 | −0.5 fps |
  | 720p30 | 56.9 | 54.4 | **−4.4%** | 16.66 → 17.32 | −0.4 fps |
  | 1080p15 | 56.5 | 54.7 | −3.2% | 16.77 → 17.25 | −2.0 fps |

  prd.md §4 asks for ~0 and this is 5.6%, five to eight times the run's own
  idle-to-idle noise. The third row is the fix plan's first step, measured:
  **0.93 ms → 0.48 ms when the share rate halves**, linear, because every part
  of the cost is per-frame. Where it goes: 1.8 ms of GPU per *shared* frame, of
  which DR-32's NVENC is 0.42 — the rest is WGC handing over a 1920×1080 BGRA
  texture and the video processor making NV12 of it (DR-31), which is also why
  720p saves so little. Methodology, the whole table and four things worth
  trying are in `docs/perf/screenshare-bench.md`; the decision is DR-35.
  The instrument is Intel's PresentMon 2.5.1 (signed, verified), driven by
  `docs/perf/screenshare-bench.ps1` — **elevated**, because an ETW session
  needs it.

## Phase 6 — Ship

- [x] **6.1 Self-hosting guide** — `docs/self-hosting.md`. Cloudflare account →
  Calls app → secrets → `wrangler deploy` → point client at Worker URL. A
  non-Cloudflare-user must succeed following only this doc.
  DoD: guide tested from scratch on a fresh account. Verify: clean-account walkthrough.
  **The guide is written, and writing it found that its fourth step did not
  exist.** prd.md §9 ends "paste the Worker URL into the client's settings" and
  there was nowhere to paste it: `GOODVOICE_SERVER` was a *build-time* value,
  so pointing a client at your own Worker meant installing Rust and MSVC and
  rebuilding — for everyone in the squad, not just whoever deployed it. There
  is a **server** section in the settings screen now, and `home.rs` remembers
  what is typed there, on disk, where the things that join without a window can
  read it (autojoin today, task 6.2's invite links next). DR-36.
  Measured rather than asserted, by `docs/testing/server-setting.ps1`: a URL
  that is not an origin is refused with a sentence and **nothing is written**;
  a valid-but-wrong origin is kept, and the restarted client **fails there**
  rather than falling back to the bundled address — which is the only way to
  tell "the setting works" from "the setting is ignored"; and the real one is
  kept the same way, joined, and **heard at 50 frames a second** by
  `bin/listener` in the same room.
  Three things the smoke test owed a self-hoster, all found by running it the
  way one would: it printed **`all checks passed` while a check was failing**
  (a command on the left of `&&` is exempt from `set -e`), `curl -o /dev/stdout`
  cannot run on Windows curl at all, and a clone with Git for Windows'
  default `core.autocrlf` broke the script before Cloudflare was ever reached
  (`.gitattributes` now pins `*.sh` to LF). It passes four for four from Git
  Bash against the live deploy.
  **Not verified — the DoD itself:** the walkthrough, from the beginning, by
  somebody with a fresh Cloudflare account. Steps 1 and 5 are dashboard
  journeys, and a dashboard is the one thing here that cannot be measured from
  a terminal. The guide says so in its own last section rather than pretending
  otherwise. **§7.11** owns this now.
- [x] **6.2 Invite links** — `client` (deep link `goodvoice://join/<room>`),
  Windows protocol registration via Tauri config; UI "copy invite" button.
  DoD: clicking a link on a machine with the app installed joins the room.
  Verify: manual link test.
  **Built, measured, and drilled three times running.**
  `invite.rs` reads and writes the links, `tauri-plugin-deep-link` registers
  `goodvoice://` from the installer — verified in the registry after a real
  install: `HKCU\Software\Classes\goodvoice` with `URL Protocol` and the
  installed exe behind `"%1"` — and `tauri-plugin-single-instance` (with its
  `deep-link` feature) folds the second process Windows starts for a link back
  into the running one.
  Seen directly, on the installed app, with a warmed UI Automation tree:
  clicking `goodvoice://join/<room>` opens goodvoice **into that room** —
  window reading `ROOM SHA-37539`, roster showing `anon (you)` — and
  `bin/listener` heard it at **50 frames a second** in the same room. A second
  link arriving during that call does **not** take it: the window stays in the
  first room and shows *an invite to shb-13576 — you are already in a call*
  with **leave and join** and **dismiss**. A link naming another deploy is not
  followed at all and says which server it was for.
  The launching link is handled by the first process and nothing in the plugin
  does that on its own — `deep_link().handle_cli_arguments(std::env::args())`
  in `setup` is what makes a *clicked* link work at all, and without it the
  room in the link is silently lost.
  **A link that cannot join says so now, and so do the other two refusals —
  DR-37.** The client had been answering links before there was a webview to
  hear the answer: Windows starts the process *for* the link, so `open_invite`
  has usually finished — refused it, or failed on a microphone another program
  was holding — while the window is still being built, and an event emitted
  then reaches nobody. Measured on the installed build: a cold click on a link
  for another deploy left the window saying `ROOM | NAME | JOIN` and nothing
  else; the same click now reads *an invite to cold-refuse-31 — that invite is
  for https://goodvoice-elsewhere.invalid, and this client is on …*. The offer
  is kept as well as emitted and `Snapshot` carries it, which is the mechanism
  task 4.6 already needed; the window says when it has answered
  (`dismiss_invite`) so a rebuilt webview does not resurrect an hour-old
  banner. A failed join is the third refusal, offered back as *try `<room>`
  again*.
  **`docs/testing/invite.ps1` passes, three times consecutively**, against the
  installed `goodvoice_0.1.0_x64-setup.exe`, with `bin/listener` hearing the
  room at up to 51 frames a second on each run. Two of its three historical
  failures were the drill rather than the client, and the last one was the
  worst: it waited for the window to *say something*, and a join form is twelve
  accessible names within a second while a cold start plus a join is up to
  thirty — so it read the form it had just waited for and called a working link
  broken. It waits for an *answer* now. Its `Explain` step is gone: a release
  Tauri build is a GUI-subsystem process, so the two runs where it handed the
  URL to the binary and read the redirected output were always going to print
  nothing.
  What it also settled: the room in the masthead carries `aria-label="room
  <code>"`, because a bare room code is ambiguous to anything reading this
  window — a screen reader included.
- [ ] **6.3 Installer** — Tauri bundler MSI/NSIS config; icon, version, protocol
  registration included.
  DoD: `npm run tauri build` yields installer; clean-VM install → join a call.
  Verify: clean-VM install test.
  **Both bundles build, and the installed app joins a real call.**
  `goodvoice_0.1.0_x64-setup.exe` (3.1 MB, NSIS) and
  `goodvoice_0.1.0_x64_en-US.msi` (5.1 MB) come out of `npm run tauri build`.
  NSIS installs per-user into `%LOCALAPPDATA%\goodvoice` with no admin prompt
  and carries Microsoft's WebView2 bootstrapper, so the only prerequisite left
  on a target machine is the VC++ 2015-2022 x64 runtime — the exe imports
  `VCRUNTIME140.dll`, `VCRUNTIME140_1.dll` and `MSVCP140.dll`.
  Verified the way DR-26 verifies things: the *installed* binary, launched with
  `GOODVOICE_AUTOJOIN`, was heard by `bin/listener` in the same room at
  **50 frames a second with real microphone audio**, 8 seconds, against the
  live deploy. That is the DoD's "join a call", from an install rather than
  from `target/release`. Rebuilt and re-measured after task 4.7, so the bundle
  that exists carries the settings screen.
  Found and fixed on the way — DR-27: the first installer packaged
  `audio-spike.exe` **as the app**, because nothing in the manifest said which
  of twelve binaries is the app. `default-run` says so now.
  **Protocol registration is done, by 6.2.** The installer writes
  `HKCU\Software\Classes\goodvoice` with `URL Protocol` and the installed exe
  behind `"%1"`, read back from the registry after a real install, and a
  clicked link opens the app into the room it names.
  **Not done — the clean VM.** The install was done on this machine, which
  proves the bundle runs; it does not prove it carries everything a machine
  without the toolchain needs.
  **The bundle is the app and nothing else, as of DR-29.** It used to drop a
  1.1 MB `audio-spike.exe` beside it. NSIS is 3.0 MB, MSI 4.3 MB, and the
  installed directory is `goodvoice-client.exe` and `uninstall.exe`.
- [ ] **6.4 README final + first release** — README (features, budgets, measured
  numbers, self-host pointer, screenshots), tag `v0.1.0`, GitHub release with
  installer artifact via CI.
  DoD: release page has installer + checksums; CI built it.
  Verify: download from release page, install, join call.
  **The README is written and the CI that builds the release exists. What is
  left is the tag.** The README prints the five measured numbers beside their
  budgets, says which two of them have a story (the FPS budget moved, DR-35;
  the RAM budget is met by destroying the window, DR-21), carries six
  screenshots out of `docs/ui/`, and points at `docs/self-hosting.md`.
  **It says what has not been through anything**, which is the half of this
  task that is not a README: §7.6 and §6.3 are written as *tested up to the
  hardware the test needs*, each with the command that finishes it, and
  §7.7–§7.12 are a six-row table of things never run — so a reader cannot
  assume a measurement nobody took.
  Built: `.github/workflows/release.yml`, on `push: tags: v*`. It is ci.yml's
  Windows environment — the MSVC dev shell, LLVM *after* it (DR-30), meson and
  ninja from pip (DR-11), `CMAKE_POLICY_VERSION_MINIMUM` (DR-4) — and then
  `npm run tauri build`, which is what turns `custom-protocol` on so the
  bundle is the app rather than a webview pointed at a dev server (DR-22).
  Three things it refuses rather than ships: a tag whose version disagrees with
  `tauri.conf.json` (checked *before* the twenty-minute build), a bundle
  directory that yielded fewer than both installers, and a file whose name does
  not carry the version. It writes `SHA256SUMS.txt` beside them, uploads them
  as a run artifact whatever happens next, and opens the release **as a draft** —
  because what a release claims about untaken measurements is not a thing CI
  can know.
  **Not done — the tag, the publish, and the DoD's own verification**, which is
  a download from the release page onto a machine, an install, and a call. That
  last one is §6.3's clean VM if it is to prove anything the install on this
  machine has not already proven.

---

## Phase 7 — Closing out

Goal: nothing left owed that is not a task somebody can tick.

Everything here was a footnote inside a finished task — *not verified: …*, *left
unmeasured: …* — and a footnote cannot be ticked, chased or scheduled. The
footnotes stay where they are, because they are the record of what that task
knew about itself. The checkbox is here.

### The order

| # | task | needs | blocks v0.1.0? |
|---|---|---|---|
| ~~1~~ | ~~§6.2 invite links — the silent failure, and a drill that passes twice~~ — done 2026-08-25, DR-37 | this machine | — |
| ~~2~~ | ~~§7.1 decide DR-35: what a screen share costs a game~~ — decided 2026-08-25, the budget moved to ≤ 6% | a decision | — |
| ~~3~~ | ~~§7.2 the tray menu checklist~~ — passed 2026-08-25, a drill after all; DR-39 | a drill | — |
| ~~4~~ | ~~§7.3 the rebuilt window's flicker~~ — measured and fixed 2026-08-25, DR-38 | a drill, after all | — |
| ~~5~~ | ~~§7.4 the share picker under the `retro` skin~~ — shot and looked at 2026-08-25 | a person, two minutes | — |
| ~~6~~ | ~~§7.5 the talk key over a fullscreen game~~ — passed twice 2026-08-25, a drill; DR-40 | ~~a person and a game~~ a swap chain | — |
| 7 | §7.6 a room hearing itself on loudspeakers — **instrument built and refused: this machine has no loudspeaker** (DR-41) | ~~a person in a room~~ a loudspeaker the microphone can hear | **yes** — prd.md §3 F4 |
| 8 | §6.3 the clean-VM install | a second Windows machine | **yes** |
| 9 | §6.4 README, tag, release | — | **it is the release** |
| 10 | §7.7 the netdown run | a second machine, or a person and a cable | no |
| 11 | §7.8 four clients conversing, with the CPU measured | four hosts | no |
| 12 | §7.9 does Cloudflare stop sending video to a closed viewer | an instrument, then code | no |
| 13 | §7.10 keyframe on demand, by PLI | code | no |
| 14 | §7.11 the self-hosting walkthrough on a fresh account | a Cloudflare account nobody here has | no |
| 15 | §7.12 the rebuilt window comes back somewhere else | code | no |

Rows 10–15 do not block the release **as long as the README says so**: a
measured number nobody has taken is not a promise, and 6.4 has to say which of
these were never run rather than let a reader assume all of them were.

### Before the release

- [x] **7.1 Decide DR-35 — the share's cost to a game.** A 1080p30 share costs
  a GPU-bound game 5.6% of its frame rate, against prd.md §4's "~0". Three
  options are written up and one is already measured: the rate is linear in the
  cost (0.93 ms → 0.48 ms of GPU a frame when it halves). Pick one — make the
  share rate follow what is being shared, cheapen the BGRA→NV12 convert, or
  change the budget and say so.
  DoD: the decision appended to DR-35, and prd.md §4 edited if the budget moved.
  Verify: `docs/perf/screenshare-bench.ps1` re-run if code changed; otherwise
  the record itself.
  **Decided: the budget moves.** prd.md §4 and F3 now say **≤ 6% while sharing
  1080p30**, with a paragraph under the table saying which number moved and
  why; the README carries the same until 6.4 rewrites it. No code changed, so
  there is nothing to re-measure — the record is the verification. The two
  options that make the cost smaller are not refuted: the shader
  scale-and-convert (DR-35 option 2) is where 1.4 of the 1.8 ms a shared frame
  is, and it is left as its own post-release task with its own before-and-after
  rather than as a change made against a release.
- [x] **7.2 The tray menu, seven rows** — `docs/testing/tray.md`, "The menu".
  Every item checked against the window and against a roommate: the ticks
  follow the call, leave leaves, quit quits.
  DoD: seven rows checked off in the doc, with anything surprising written down.
  Verify: `docs\testing\tray-menu.ps1`, or manual with `bin/listener` in the
  room as the second person.
  **It did not need a person after all. Seven rows, three columns, `PASS`.**
  `docs/testing/tray-menu.ps1` walks the whole table against the live deploy:
  the tray's ticks and greying, the window's buttons, and what an independent
  client in the same room sees — `ROW5_ROOMMATE=coldstart muted deafened |
  roommate (you)`, taken while the window said both buttons were lit and the
  menu said `[x] Mute | [x] Deafen`. Both directions, which is what the table
  is for: two of the rows are set from the tray and read in the window, two are
  set in the window and read from the tray.
  **It was blocked for most of the session, and by nothing in this repo.**
  DR-39: an elevated program held the foreground, and UIPI stops a
  medium-integrity drill injecting a mouse or a key while one does. Measured on
  a desktop idle for 31 minutes, so the diagnosis this file carried twice —
  *`SetCursorPos` fails while somebody is at the machine* — was wrong. A click
  on an ordinary window cleared it and the drill passed on the next run. It
  reports `POINTER=refused (UIPI: <program> holds the foreground and is
  elevated)` and stops, rather than reporting a tray menu that does not work.
  **The tick was the hard column, and it turned out to be readable.**
  `tray.md` had it right that the popup is a `TrackPopupMenu` UIA reports as a
  `#32768` pane with no children — but the pane answers `MN_GETHMENU` with the
  `HMENU` it is drawing, and a menu handle is a USER object rather than a
  pointer, so `GetMenuItemInfo` reads its text, `MFS_CHECKED` and `MFS_GRAYED`
  from another process. Every "✔ next to Mute" and "all three greyed again" in
  the table is a measurement, not a photograph. The photograph is taken anyway
  (`MENU_SHOT_*`), because a menu right in its handle and wrong on the screen
  is a thing that could happen and nothing else here would catch it.
  **The third column is `bin/listener`, which can now see.** It prints a
  `roster @ Ns` line whenever the room's flags change — `roster @ 7s
  roommate (you) | coldstart muted` — so "someone else sees you go muted" is
  read off an independent client rather than off the window that did the
  muting. Flags only, so a level meter moving does not print. Four unit tests
  pin the rendering, including that the order is arrival rather than the
  broadcast's — two orderings of one room would otherwise read as somebody
  muting.
  **And one thing the drill found about the app, which is not a bug and would
  have wasted the next session.** A client that arrived by `GOODVOICE_AUTOJOIN`
  has an empty join form: `room` is a signal that starts empty and autojoin
  never sets it. So after a tray → Leave the field is blank, `join` is
  disabled, and the first version of this drill clicked it and reported the app
  refusing to re-join — which is precisely the failure the last row exists to
  catch, arriving from the instrument instead. It types the code now.
- [x] **7.3 Does the rebuilt window flicker** — task 4.1's last row. The window
  is destroyed into the tray and built again on the way back (DR-21); whether
  that is a flash is the one thing no counter can answer.
  DoD: a yes or a no in `docs/testing/tray.md`, and a DR if it is a yes.
  Verify: `docs\testing\tray-roundtrip.ps1 -Cycles 5`, watched rather than read.
  **Yes, and it was white — 394 ms of it. It does not any more.** DR-38.
  It turned out not to need eyes after all: `docs/testing/tray-flicker.ps1`
  photographs the rebuild at **141 frames a second** — about one frame per
  refresh on this 144 Hz screen (DR-40 corrects the 60 Hz this assumed) — and
  scores each one on whether it is the bare desktop, the finished window, or
  neither. The
  answer was fifty-five consecutive frames of *neither* — a flat white
  rectangle, luma 248 against a settled window's 33 — and then the whole app in
  one frame. WebView2's own background, before the document exists.
  The window is built hidden now and shows itself once it has painted, with a
  1500 ms fallback in `lib.rs` because a window that never shows is an app
  nobody can reach. Re-measured on the same build: **zero frames of neither**,
  desktop to finished window in one frame, twice over, and the webview beat the
  fallback by a factor of three. `bin/coldstart` unchanged at 2681 ms median of
  five (2692 ms recorded, 3000 ms budget), 170 tests, all gates clean.
  Two things about the *old* drill this found, both in DR-38: its
  `REBUILT_IN_MS=146` was timing `InvokePattern.Invoke()`, which does not return
  for 2008 ms on this desktop, and its `QUIT_CLICKED=False` was `SetCursorPos`
  being refused because somebody was at the machine. Both now say so.
  **A finding left open: the window walks.** It comes back somewhere else every
  time — `104,104 → 208,208 → 52,52 → 130,130` — because the config names no
  position and Windows cascades. **§7.12** owns it.
- [x] **7.4 The share picker under `retro`** — task 5.3 shipped two screenshots
  and both are `terminal`. The picker's CSS is the shared base plus a
  `terminal` block, so `retro` is the unmodified base and has never been looked
  at.
  DoD: `docs/ui/share-picker-retro.png` committed, or a fix if it is wrong.
  Verify: open the picker with the `retro` skin selected.
  **Looked at, and nothing is wrong with it.** `docs/ui/share-picker-retro.png`
  is the picker under `neobrutal`, driven through the installed app by
  `docs/testing/share-picker-shot.ps1` — joined to `pickershot` the same way
  the `terminal` shot was, skin chosen by clicking it in the settings screen.
  The base CSS holds: quality buttons side by side with 1080p selected, the
  monitor and window list under *what to share* with its own scroll, and the
  skin's hard shadow and thick frame on every control. Nothing overflows, and
  the two shots differ only in the ways the two skins are meant to differ.
  **Two traps for the next drill that drives this window**, both in the
  script's header where the next person will hit them. `InvokePattern.Invoke()`
  succeeds on every button here and does *nothing* — a WebView2 acts on input,
  not on automation patterns — so a drill that trusts the return value walks
  away believing it opened a screen it never opened. And UI Automation reports
  bounding rectangles for elements *below the fold* as readily as for visible
  ones: the skin buttons are at the bottom of a settings screen taller than its
  window, so clicking where UIA said clicked the desktop behind the app. The
  script scrolls the element into view and refuses to click a point outside the
  window frame. Same family as DR-26.
- [x] **7.5 [WIN] The talk key over a fullscreen game** — task 4.3, steps 4–5 of
  `docs/testing/hotkey.md`. The hook is proven from a windowless process and
  while another app has focus; a *fullscreen exclusive* game is the case the
  feature exists for (prd.md §3 F2) and the one nobody has tried.
  DoD: the key opens the mic while the game has the screen, and the game still
  receives it.
  Verify: `bin/listener` in the room, a game in fullscreen, the key held.
  **It did not need a game, and it passed twice.** DR-40.
  `docs/testing/hotkey-fullscreen.ps1` drives the real client into push to talk
  on F13, joins a room `bin/listener` is already in, puts
  `bin/fullscreen-drill` — a D3D11 swap chain with `SetFullscreenState(TRUE)`
  on it, which is what the phrase *fullscreen exclusive* names — on the display,
  and holds the key twice. All three halves are read off instruments rather
  than judged:

  ```
  GLOBAL=heard from anywhere, including over a game
    game MODE=exclusive     DISPLAY_REFRESHES=4810 (144 Hz)     DOWNS=2 UPS=2
    room 0 ... 54 50 26 0 0 0 0 0 0 22 50 50 50 28 0 0 0 0 21 50 50 50 29 0 ... 0
  HEARD_BURSTS=3 (one control, two over the game)
  ```

  The `room` line is the roommate's frames-a-second column, and it has three
  bursts because the drill holds the key three times: **once before the display
  is touched**, then twice with the game on it. A gated client sends *no
  packets* rather than silent ones (3.2), so a row is either a stream or
  nothing — and the first burst is what makes the others admissible. `DOWNS=2
  UPS=2` is the fullscreen window's own `WM_KEYDOWN`, which is the half nobody
  had a way to see: the hook passes every key on, and this is the count that
  would fall to zero if it did not. `-Windowed` runs the same walk with the
  display left alone, which is the third row of `hotkey.md`'s table and also
  passed.
  **The control hold is there because one run had none of it.** The room heard
  nothing at all while everything else passed, which reads as a talk key that
  stopped working over a game and is not: a capture device that will not open
  gives exactly the same column of zeroes. That run is now `INCONCLUSIVE` with
  the app's own stderr under it rather than a failure of the feature.
  **What is left for a person is two lines, not five**, and neither is the DoD:
  that the key still *types* where it is aimed, and that the hook comes off at
  the end of a call. `hotkey.md`'s "By hand" now says so.
  **Two traps found on the way, both in DR-40 and in the script's header.** A
  key synthesised with `keybd_event(vk, 0, ...)` has no scan code, and a DOM
  `KeyboardEvent.code` is derived from the scan code rather than the virtual
  key — so it arrives in the webview as an empty string, is stored as the talk
  key, and leaves the window honestly reporting *heard only while this window
  has focus*. And that state survives a restart: the settings button then reads
  `key:` with nothing after it, so a drill matching `key: ` walks away
  reporting an app with no key button.
- [ ] **7.6 A room hearing itself** — tasks 3.4 and 4.7. The echo canceller is
  measured against a synthetic loopback, which is the *hard* case and not the
  real one; nobody has put a microphone and a loudspeaker in one room and
  listened. Same run answers 4.7's question about whether noise suppression
  sounds better on than off.
  DoD: a paragraph in `docs/testing/` with what was heard, and the echo column
  from `bin/listener --tone` beside it.
  Verify: `cargo run -p goodvoice-harness --bin echo-room -- --record <dir>`,
  with a loudspeaker in front of the microphone.
  **The instrument exists and reaches everything but the room — DR-41.**
  `bin/echo-room` puts two clients in one process the way `bin/latency` does:
  the room holds the real devices through the same `hardware::open` the app
  uses, the far end publishes a 1 200 Hz tone and keeps every frame that comes
  back, and the tone makes the whole trip — SFU, transducer, air, microphone,
  SFU. Four segments: the room silent, the room with the suppressor on, the
  tone with the canceller **off**, the tone with it on. `docs/testing/echo.md`
  is how to read it.
  **It refuses, and the refusal is about this machine.** COUPLING **0.4 dB**
  and then **2.0 dB** against the 6 dB it asks for before it will say anything
  about a canceller — because the only active render endpoint here is a headset
  earphone, and the analog jack DR-23 measured through at 84.7 ms now reports
  `not present`. An earcup nobody is holding is not a room. The canceller-off
  segment is the control and it is the whole point: a canceller that works and
  a room with no loudspeaker in it produce the same number, which is §7.5's
  silent capture device again.
  **What is left is not "a person in a room" but one object.** A loudspeaker
  the fifine can hear — an earcup laid face-down against it, or a speaker back
  in the analog jack — and then the row is one command. Everything else is
  built.
  **4.7's half does not wait on that.** `--record` writes a WAV per segment
  *even when the run refuses a verdict*, so `quiet.wav` against
  `suppressed.wav` is a listening test that can be done today. The level cannot
  answer it: `NOISE_SUPPRESSED` came out at −1.3, 0.1 and −0.4 dB across three
  runs of the same room, because the gain controller sits after the suppressor
  and answers a quieter frame by turning it up.
  **One trap, in DR-41 and in the drill's header.** The obvious metric — the
  tone's bin now against the tone's bin in the silent room — reported
  `COUPLING=17.2 dB` on the first run and there was no coupling: the room got
  louder between the two segments (median level 121.0 → 453.3) and the bin rose
  with it. The tone is read against its own neighbours in the same 200 ms now,
  which a chair or a gain change cannot move.

### After the release

- [ ] **7.7 The netdown run** — task 3.5. `bin/reconnect-drill` kills the
  session from the inside; only pulling the network checks that the client
  *notices* — see `docs/testing/reconnect.md`.
  DoD: the doc's netdown row filled in, on either host.
  Verify: unplug it, or `Disable-NetAdapter`, mid-call.
- [ ] **7.8 Four clients conversing** — task 3.1's own DoD, with the CPU
  measured while they do. Everything else about N-party audio is tested; this
  needs four hosts.
  DoD: numbers in the task, or a DR saying why the budget cannot be met.
  Verify: four machines, `bin/soak` on each.
- [ ] **7.9 What a closed viewer still costs the room** — task 5.4's open
  question. The client stops pulling the video and stops decoding it, but
  nothing tells the SFU: `tracks/close` is allowlisted by the Worker and unused
  by the client, so "Cloudflare is not sending video to a client with no
  viewer" holds until the first viewer opens and is unproven after one closes.
  Windows' per-process IO counters cannot see the media sockets — 5.2 / 5.4 /
  5.6 kB/s across never-opened, open and closed-again — so this needs an
  instrument first and code second.
  DoD: the bandwidth measured on both sides of a close; `tracks/close`
  implemented if it turns out the video keeps arriving.
  Verify: an instrument that can see UDP per socket, then a repeat of the
  measurement.
- [ ] **7.10 Keyframe on demand** — DR-34's remaining half. A viewer opening
  mid-share waits up to two seconds for the repeated keyframe, and the sharer
  re-sends one every two seconds whether anybody is watching or not, because
  the H.264 codec is registered with `rtcp_feedback: vec![]` and Cloudflare has
  no way to ask. Negotiating `nack pli` / `ccm fir` and answering a PLI would
  make both go away.
  DoD: a viewer gets its first picture in well under a second on a still
  screen, and a still share with nobody watching sends nothing.
  Verify: `bin/rewatch --rounds 4`, and `bin/share-drill` under the drill's grey
  sheet with no viewer open.
- [ ] **7.11 The self-hosting walkthrough** — task 6.1's DoD. `docs/self-hosting.md`
  is written and its client half is measured, but nobody has followed it from
  the beginning on a fresh Cloudflare account, and steps 1 and 5 are dashboard
  journeys that cannot be measured from a terminal.
  DoD: somebody who has never deployed this reaches a working room using only
  that document; whatever tripped them up is fixed in it.
  Verify: a clean account, and a second pair of eyes.
- [ ] **7.12 The rebuilt window comes back somewhere else** — DR-38's other
  finding. `tauri.conf.json` gives the window a size and no position, so every
  window Windows makes for this process is cascaded from the last one: four
  rebuilds in one run landed at `104,104`, `208,208`, `52,52` and `130,130`. A
  person who puts goodvoice where they want it and closes it to the tray does
  not get it back there, and over a session it walks down the screen. Nobody
  had noticed because no drill compared two rebuilds' rectangles.
  DoD: the window comes back where it was left, across a close-and-reopen and
  across a restart; `tray-flicker.ps1`'s `WALK` line repeats one position.
  Verify: `docs\testing\tray-flicker.ps1 -Cycles 3`, whose `WALK` is exactly
  this measurement.

---

## Decision Records (§DR)

Append-only log. Format:

```
### DR-<n>: <title> (<date>)
Context / Options considered / Decision / Consequences / Measurements (if any)
```

### DR-1: TURN needs its own credential pair (2026-08-20)

**Context.** prd.md §7 says NAT traversal is "covered by same account/
credentials" as the Realtime SFU app. Task 1.3 needed the exact API to call.

**Findings.** Cloudflare exposes two unrelated credential pairs:

- SFU: `POST https://rtc.live.cloudflare.com/v1/apps/{APP_ID}/sessions/new`,
  `Authorization: Bearer {APP_SECRET}` → `{ sessionId }`.
- TURN: `POST https://rtc.live.cloudflare.com/v1/turn/keys/{TURN_KEY_ID}/credentials/generate-ice-servers`,
  `Authorization: Bearer {TURN_KEY_API_TOKEN}`, body `{ "ttl": 86400 }` →
  `{ iceServers }`.

The TURN key is created separately in the dashboard and is *not* derivable from
the Calls app id/secret. Confirmed against Cloudflare's docs and against
`cloudflare/meet` (`app/utils/getIceServers.server.ts`, which reads
`TURN_SERVICE_ID`/`TURN_SERVICE_TOKEN`) plus `partytracks/server`.

**Decision.** Four secrets, not two. `TURN_KEY_ID` and `TURN_KEY_API_TOKEN` are
**optional**: with them absent the Worker answers Cloudflare's public STUN
servers, so a self-hoster can get a call up with two secrets and add TURN when
a squadmate turns out to be behind a symmetric NAT. A TURN request that fails
at runtime also degrades to STUN rather than failing the join — losing relay
candidates must not drop a call that STUN alone would have connected.

**Consequences.** prd.md §7 and §9 overstate the simplicity ("two secrets");
docs/self-hosting.md (task 6.1) must document all four and say which are
optional. A session that the SFU refuses *does* fail the join (502
`sfu_unavailable`) and the reserved roster slot is rolled back.

### DR-3: `opus` crate instead of `audiopus` (2026-08-20)

**Context.** prd.md §7 names `audiopus` for the Opus binding. Task 2.2 needed it
to build on macOS (dev) and windows-latest (CI).

**Options considered.**

| | `audiopus` 0.2.0 | `opus` 0.3.1 |
|---|---|---|
| Last release | Apr 2021 | Jan 2026 |
| `-sys` crate | `audiopus_sys` 0.1.8 | `audiopus_sys` 0.2.2 |
| libopus source | **not vendored** — needs a system libopus found via `pkg-config` | vendored, built with CMake |
| Extra build tooling | `pkg-config` **and** `bindgen` (so libclang) on every host | CMake only |
| Default features | broken: the feature is named `default_features`, not `default`, so `coder` is off unless named explicitly | none needed |

**Decision.** Use `opus` 0.3.1. It is the same libopus behind an equally thin
safe wrapper — `encode(&[i16], &mut [u8]) -> usize`, no hidden allocation — so
the runtime cost is identical and the PRD's performance tiebreaker does not
separate them. What separates them is that `audiopus` would put libclang and a
system libopus on the critical path of every dev machine and CI runner.

**Consequences.** prd.md §7's "Opus via `audiopus`" row is now wrong; the
wrapper is confined to `audio/opus.rs`, so swapping back is a one-file change if
`audiopus` is ever revived. Packet loss is spelled `decode(&[], …)` in this
binding, wrapped as `VoiceDecoder::conceal`.

**Measurements (macOS arm64, debug).** 20 ms mono frames, 48 kHz: 100
encode+decode round trips allocate **zero** times (counting global allocator in
the test build). Steady-state packet at 32 kbps ≈ 115 bytes for a 440 Hz tone,
≈ 61 bytes for silence.

### DR-4: CMake policy pin for the vendored libopus (2026-08-20)

**Context.** `audiopus_sys` 0.2.2 vendors a libopus whose `CMakeLists.txt`
declares `cmake_minimum_required(VERSION 3.1)`. CMake 4 removed compatibility
with anything below 3.5 and fails the configure step outright:
`Compatibility with CMake < 3.5 has been removed from CMake.`

**Decision.** `client/src-tauri/.cargo/config.toml` sets
`CMAKE_POLICY_VERSION_MINIMUM = "3.5"` for every cargo invocation. Committed, so
a fresh clone and CI build identically instead of each machine needing the
export. Remove it when `audiopus_sys` ships a libopus with a modern
`CMakeLists.txt`.

**Consequences.** Building the client needs `cmake` on PATH — add it to
docs/self-hosting.md's contributor section (task 6.1). GitHub's runners already
have it.

### DR-6: track signalling is read, not announced (2026-08-20)

**Context.** DR-2 left task 2.4 with a hole: to pull a roommate's audio a client
needs `{ location: "remote", sessionId, trackName }`, and nothing told it either
value. The obvious fix is a `publish` message on the room WebSocket.

**Options considered.**

| | client announces (`{type:"publish"}`) | room reads the proxy |
|---|---|---|
| Can announce a track it never published | yes (harmless — only hurts itself) | no |
| Can publish and forget to announce | yes, and the peer hears silence | no |
| Knows the media kind | client says so | derived from the track name |
| New message types | 2 (`publish`, `unpublish`) | 0 |

**Decision.** The room reads publishes out of the SFU proxy it already signs.
`tracks/new` names the tracks going up and `tracks/close` names the mids coming
down, so the room does not need to be told twice, and the two states cannot
drift. The roster grew two fields — `sessionId` and `tracks` — and stays the
single message peers listen to; `welcome` carries them too, so a late joiner
subscribes from its first frame.

The roster follows what the SFU *accepted*, not what was asked: a `tracks/new`
can come back 200 with individual tracks rejected, so the answer is buffered and
per-track errors are skipped. Announcing a track that failed to publish would
have every peer subscribe to silence.

Track names are a closed vocabulary, `mic → audio` and `screen → video`. That is
what buys the media kind without parsing SDP, and it lets the room reject a
publish it has no model for instead of relaying a track no peer can interpret.
Widening it is one line here plus a client that names the track.

**Consequences.**

- A participant's `sessionId` is visible to their room. It grants nothing: the
  proxy refuses to pull from a session that is not in the room, so the only
  thing a roommate can do with it is subscribe to media they joined to hear.
- `attachSession` broadcasts. `join()` announces a participant before their
  session exists, and a roster without the address is not usable.
- One screen per room is now enforced on the track, which is where it belongs.
  The `share` message from task 1.1 still exists as the UI's intent signal and a
  screen track moves its flag; **task 5.3 should collapse the two.**
- Cloudflare's per-track error shape is inferred, not observed — no real SDP has
  crossed this path yet. Task 2.3's spike is the first run that will confirm it;
  an unrecognised answer shape falls back to trusting the HTTP status.

**Update (2026-08-20).** Real SDP has now crossed it (DR-7). The read-side of
this design works live: the listener's join answer carried the speaker's
`sessionId` and `mic`/`audio` track without any client announcing anything. The
*success* shape is confirmed — `tracks: [{ mid, trackName }]`, no error field,
which is exactly what `#rejectedTracks` treats as accepted. The **rejection**
shape is still unobserved, because nothing was rejected; that half of the
inference stands until a publish actually fails.

**Update (2026-08-20, task 2.4).** Now observed too, and the inference held:
`errorCode` plus `errorDescription`, never `error`. DR-8 has the payload.

### DR-5: an in-memory roster does not survive a redeploy (2026-08-20)

**Context.** The first `smoke.sh` run against the live Worker failed at the last
check with `the room never filled up` — twelve joins in a row, never a 409. The
same loop against a fresh room a minute later filled at exactly eight. The run
had followed four back-to-back `wrangler secret put` calls.

**Cause.** Every `secret put` publishes a new Worker version, which restarts the
Durable Object. Room state is in memory only (prd.md §7), so each restart drops
the roster back to zero and the cap can never be reached while versions keep
rolling. Ordinary DO eviction after an idle period does the same thing.

**Decision.** Keep the in-memory design — this is the documented trade for rooms
that leave nothing behind. Treat it as an operational fact instead: run
`smoke.sh` *after* the deploy has settled, never interleaved with secret
uploads, and expect a live call to drop on redeploy.

**Consequences.** Task 3.5 (auto-reconnect) is what makes this survivable for
users: a client whose room evaporated mid-call must rejoin the same code rather
than surface an error. docs/self-hosting.md (task 6.1) should say plainly that
pushing a new Worker version ends every call in progress.

### DR-7: webrtc-rs clears the Realtime SFU, and does it off Windows (2026-08-20)

**Context.** prd.md §10 open question 2 — can `webrtc-rs` complete ICE/DTLS with
Cloudflare Realtime and carry an Opus track? Task 2.3 called this the project's
biggest unknown and gated the whole voice path on it.

**What was run.** `client/src-tauri/src/bin/rtc-spike.rs`, against the live
deploy from task 1.5. One process, two participants in one room: a speaker
publishes a 440 Hz tone as `mic`, a listener reads the roster to find it, pulls
it through the Worker proxy, and runs a Goertzel bin over the decoded PCM. That
covers signalling, the proxy (DR-2), ICE, DTLS, SRTP, Opus and the roster's
track bookkeeping (DR-6) in one pass, without a second machine or a throwaway
web page.

**Decision.** `webrtc` 0.20.3 — a thin async layer over the Sans-I/O `rtc` core,
both pinned together. It connected on the first attempt; no libwebrtc FFI
evaluation is needed and prd.md §7's stack row stands.

**Findings.**

- **Cloudflare is `ice-lite`.** It answers one host candidate
  (`141.101.90.0:1473`) followed by `a=end-of-candidates`, so there is no
  trickle to implement and no TURN on the happy path — the client must simply
  send a fully-gathered offer. DTLS roles: `a=setup:passive` on the publish
  answer (our side is the DTLS client), `a=setup:actpass` on the pull offer.
- **Opus is payload type 111 both directions**, `a=rtpmap:111 opus/48000/2` with
  `minptime=10;useinbandfec=1`. Offering 111 rather than letting the media
  engine pick keeps the two sides from renumbering. RFC 7587 fixes that `/2`
  even for a mono stream: our 20 ms mono frames decoded intact through it, so
  the SDP channel count and the payload channel count are allowed to disagree.
- **Publish and pull are different flows.** `tracks/new` with a local track
  takes our offer and answers `requiresImmediateRenegotiation: false` plus an
  answer. `tracks/new` with a remote track takes *no* SDP and answers
  `requiresImmediateRenegotiation: true` plus an **offer**, which the client
  answers via `PUT renegotiate`. The proxy's method allowlist already matches.
- **The `[WIN]` tag on this task was wrong.** `webrtc-rs` is pure Rust and the
  SFU handshake is identical on every host; only capture (2.1) and the hardware
  APIs behind it are Windows-bound. Tagging a task `[WIN]` because it lives in
  the Windows client, rather than because it needs Windows, cost this project
  its critical path. Tasks 2.5, 4.3–4.5, 5.1–5.2 and 5.5 keep the tag on merit —
  they measure or drive real hardware.

**Measurements (macOS arm64, debug build, live deploy).** Two runs: 100 of 100
RTP packets decoded as Opus in both. 440 Hz bin energy `1.53e13` / `1.43e13`
against `2.62e9` / `2.59e8` at 1500 Hz — three to four orders of magnitude of
separation, i.e. what arrived is the tone that was sent, not noise that happened
to be loud. Decoded RMS 5723 / 5622 against a source amplitude of 8000.
No latency number here: that is task 2.5's job and it needs real devices.

**Consequences.**

- Task 2.4 loses its transport risk. What remains is real capture in place of
  the synthesised tone, real playback in place of the Goertzel check, and the
  UI.
- `TrackLocalStaticSample::write_sample` takes a `Bytes`, so the spike allocates
  once per 20 ms frame — banned on the real voice path by styleguide.md. Task
  2.4 must either pool those buffers or drop to `TrackLocalStaticRTP` and
  packetise itself. Noted here because the spike reads like a template and this
  is the one line that must not be copied.
- New client dependencies: `webrtc`, `rtc`, `tokio`, `reqwest` (rustls only — a
  desktop client should not depend on the host's OpenSSL), `anyhow`,
  `serde_json`, `async-trait`, `bytes`. They are `[dependencies]` rather than
  dev-only because 2.4 needs all of them in the shipped client.
- The spike leaves its two participants in the room until the heartbeat sweep
  evicts them (~30 s), because there is no HTTP leave route. It therefore
  defaults to a fresh random room code per run; `--room` overrides it, and
  re-running the same code inside the sweep window walks toward the 8-person
  cap.

### DR-8: what a real call needed that the spike did not (2026-08-20)

**Context.** Task 2.4 turned DR-7's one-way spike into a client that publishes
and subscribes at once. That is a different problem: the same peer connection
now has to keep sending while it renegotiates to receive. Four things broke, and
none of them were visible in a spike that only did one direction.

**1. The DTLS role reverses on renegotiation.** Our first offer is answered by
Cloudflare with `a=setup:passive` — they are the DTLS server, we are the client.
Every pull after that is *their* offer with `a=setup:actpass`, and `webrtc-rs`
answers `passive` by default, claiming the server role for ourselves. The
handshake restarts and the sender carrying the microphone dies with it. Fixed
with `SettingEngine::set_answering_dtls_role(RTCDtlsRole::Client)`, so the
answer says `active` and the role established at publish time survives.

**2. A renegotiation rebuilds the sender.** Even with the role held, the SSRC
and payload type captured at publish time can stop routing:
`write_sample` starts returning `SendError(SenderRtp(…))` and the client goes
silent while still believing it is talking. `Published` now holds both in
atomics and re-reads them from the peer connection after every renegotiation.
This was the single biggest win — 8/10 runs to 11/12.

**3. The roster announces a track before it carries packets.** The room records
a publish when Cloudflare accepts the publisher's `tracks/new`, which is before
their DTLS finishes and well before their first RTP packet. A peer that
subscribes on that announcement gets a per-track failure. Three codes mean "not
yet" — `not_found_track_error`, `empty_track_error`,
`transport_unavailable_error` — and are retried with backoff. Anything else is
reported.

**4. A failed subscription was permanent.** The roster is pushed only when it
changes, so a peer whose pull failed stayed silent for the rest of the call. The
subscribe loop now re-reconciles against the roster it already has every two
seconds; it is a no-op when everyone is subscribed.

**DR-6's open question, answered.** The per-track rejection shape is
`errorCode` + `errorDescription`, never `error`:

```json
{"requiresImmediateRenegotiation":false,
 "tracks":[{"errorCode":"not_found_track_error",
            "errorDescription":"Track not found on remote peer. Make sure the
              publisher peer is connected and sending packets for this track",
            "mid":"","sessionId":"…","trackName":"mic"}]}
```

The Worker's `#rejectedTracks` already keys off `errorCode`, so it was right.
Also newly observed: `requiresImmediateRenegotiation` is **not** always true on
a pull — Cloudflare only offers when it needs a new m-section, and answering an
answer that has no SDP in it was a real bug in the first draft.

**The transport is not reliable on the first try, and never was.** Roughly one
attempt in six fails to reach `Connected`. This is **not** new: the task 2.3
spike, run unmodified from its own commit, fails the same way (1 in 6 over six
runs). Two fixes and one deferral:

- ICE now gathers from the routable local address rather than `0.0.0.0`.
  Binding the wildcard offers a candidate per interface, and a machine with a
  VPN or a container bridge has several that cannot reach Cloudflare. Since the
  SFU is ice-lite — one remote candidate, no checks of its own (DR-7) — a bad
  local pick has nothing to fall back to.
- `Call::join` retries the whole exchange up to three times. The room WebSocket
  is opened *before* the peer connection so a failed attempt can hand its seat
  back with a `leave`; otherwise every retry would strand a participant in one
  of the room's eight slots until the sweep (DR-5).
- What is left belongs to **task 3.5**: a call that drops mid-conversation still
  ends the call. 3.5 should also cover the narrow case seen here — a peer that
  retried its join has a new session id, and a roommate holding the old roster
  is refused by the proxy until the next push.

**Measurements (macOS arm64, release, live deploy, two clients per run).**
15 consecutive spike runs: **13 pass, 2 fail** — one join that exhausted its
three attempts, one publisher that never started sending. The same measurement
before this work was 6 of 10. Two real-device clients on one machine joined,
published, saw each other's tracks appear on the roster, and left cleanly.
No latency number: that is task 2.5 and it needs 2.1's devices.

**Audio backend.** `cpal` sits behind `audio::device`'s seam as the backend,
**not** as task 2.1's answer. It is cross-platform, so the voice path can be
exercised off Windows, and it was: devices open at 48 kHz and frames flow. 2.1
still owns the measurement and may add a `wasapi` backend beside it — the seam
is what makes that a one-file change. goodvoice does not resample, so a device
that cannot do 48 kHz is refused rather than silently degraded.

**Consequences.**

- DR-7 asked task 2.4 to move off `write_sample` because it allocates per
  frame. It has not, and should not: the allocation is in a task on the far side
  of the capture ring buffer, never inside a device callback, which is where
  styleguide.md's rule applies. The callbacks themselves take no lock and make
  no allocation.
- Playback mixes up to seven remote speakers by saturating sum, one ring per
  slot, summed in the render callback. Task 3.1 replaces that with a real mixer
  — per-speaker gain and the level metering the roster needs to show who is
  talking — but every peer is audible now rather than only the first.
- `AudioSink::clear` asks the render callback to drain via an atomic flag
  rather than draining from the producer side: only the consumer end can throw
  queued audio away, and a peer who left should stop mid-word.
- New client dependencies: `cpal`, `ringbuf`, `tokio-tungstenite` (rustls only),
  `futures-util`.
- `GOODVOICE_TRACE_SDP=1` dumps every negotiated SDP. Every hard problem on this
  path so far has been visible there and nowhere else.
- `GOODVOICE_SERVER` at build time points a client at a different Worker, which
  docs/self-hosting.md (task 6.1) needs.

### DR-10: the VAD the PRD named does not exist any more (2026-08-20)

**Context.** Task 3.3 says "VAD via webrtc-audio-processing's voice detection
with hangover time", and prd.md §7 lists the same crate for AEC/NS/AGC (task
3.4). The intent was one dependency serving both.

**Finding.** It cannot serve the first. `webrtc-audio-processing` 2.1.0 wraps
PulseAudio's repackaging of the modern WebRTC `AudioProcessing` module, where
the legacy VAD is gone. The `voice_detected` field survives in
`AudioProcessingStats` and in the crate's FFI struct, but nothing writes it —
the crate's own test says so in as many words:

```rust
// Fields declared in upstream but never populated in v2.1.
assert!(!stats.voice_detected.has_value);
```

Enabling it is not a config away; the module that produced it was deleted
upstream. So task 3.3 needed a detector of its own regardless of what 3.4 does.

**Options considered.**

| | `webrtc-audio-processing` 2.1 | `webrtc-vad` 0.4 (libfvad) | an energy gate |
|---|---|---|---|
| Voice detection | **none** — never populated | yes, the original WebRTC GMM VAD | RMS threshold only |
| Build tooling | `bindgen` (libclang) + `meson`/`ninja` | `cc` only, bindings pre-generated | none |
| C source | vendored, built with meson | vendored, built with `cc` | — |
| Windows | meson build unproven with MSVC | plain C, `cc` handles MSVC | — |
| Last release | May 2026 | Oct 2019 | — |
| Tells noise from speech | — | yes | no |

**Decision.** `webrtc-vad` 0.4. It is the same WebRTC VAD the PRD wanted, taken
from before it was deleted, and its build profile is the one DR-3 already
settled for: vendored C, no libclang, nothing for a contributor to install
beyond a C compiler. An energy gate needs no dependency at all but cannot tell
a fan from a voice, which is the entire job.

The crate is six years old and that is the cost. It is 12 C files and one
`unsafe` FFI call behind `is_voice_segment`, all of it confined to
`audio/vad.rs`, so replacing it is a one-file change — the same containment
DR-3 relied on for `opus`.

**Consequences.**

- `Gate` holds a `*mut Fvad` and is therefore `!Send`; the publish loop is a
  spawned task, so `audio/vad.rs` carries an `unsafe impl Send` with the
  argument for it. libfvad keeps all its state in the instance and the instance
  is created inside that loop and never shared.
- Task 3.4 still brings `webrtc-audio-processing` in for AEC/NS/AGC, and it
  will bring libclang and meson with it. That is now 3.4's risk alone, taken
  where it buys something, rather than 3.3's as well.
- `mode: TransmitMode` joined `CallOptions`. A client joins in the mode the
  user last chose, so push-to-talk never has one hot frame at the start.

**Measurements.** libfvad in `Aggressive` mode at 48 kHz, 20 ms frames: a voiced
frame registers on the **first** frame — no onset delay to compensate for — and
after the sound stops the detector keeps answering "voice" for **four more
frames** (80 ms) before it drops. That overhang renews goodvoice's own 300 ms
hangover before it starts counting down, so the real tail after a sentence is
about 380 ms. Digital silence from a cold detector never reads as voice.

### DR-13: ICE gathering never completes on the Windows host (2026-08-21)

**Context.** Task 2.5's harness was written to answer the ≤80 ms budget
(prd.md §4). It has produced no number, because neither client can join.

**Symptom.** Every `Call::join` fails all three attempts with
`webrtc: ICE gathering never completed`. Reproducible on this host with
`cargo run --bin latency`, and identically from WSL and from native Windows.

**What has been ruled out.**

- *The deploy.* `GET /health` answers `{"ok":true}` and `POST /rooms/:code/join`
  returns a real `sessionId` plus eight ICE URLs — two STUN, six TURN, with
  username and credential.
- *UDP being blocked outright.* A raw STUN binding request to
  `stun.cloudflare.com:3478` gets 32 bytes back. `stun.l.google.com:19302`
  answers too. **`stun.cloudflare.com:53` times out**, which is ordinary — a
  great many networks drop outbound UDP/53 to anything that is not their
  resolver — and the server hands that URL out anyway, along with
  `turn:turn.cloudflare.com:53?transport=udp`.
- *Slowness.* `CONNECT_TIMEOUT` was raised from 25 s to 120 s. Three attempts,
  376 seconds, same failure. Gathering does not finish late; it does not finish.

**The lead.** With an `eprintln` in `Events::on_ice_gathering_state_change`,
**nothing is ever printed** — not `Gathering`, not `Complete`. The callback
never fires at all, so the `watch` this code waits on never leaves `New`. That
points at the handler or at `PeerConnectionBuilder`, not at the network.

Two candidates, untested:

1. `with_udp_addrs(vec![format!("{}:0", local_ip())])` pins gathering to one
   interface (DR-8's fix for machines with VPNs and container bridges). This
   host has WSL virtual adapters; if the pinned address is wrong, or if pinning
   interacts badly with the state callback in webrtc-rs 0.20 on Windows,
   gathering may never be driven to completion.
2. `PeerConnectionEventHandler::on_ice_gathering_state_change` may simply not be
   invoked by webrtc-rs 0.20 in this configuration, in which case waiting on it
   is the bug and the SDP should be taken once candidates stop arriving instead.

**Why it matters beyond 2.5.** Every automated proof that needs a live call
runs through this path: `bin/rtc-spike.rs` (task 2.3), `bin/reconnect-drill.rs`
(task 3.5), and the multi-client runs that tasks 2.4 and 3.1 still owe. DR-7
and DR-8 recorded those passing — **from macOS**. Nothing has re-run them on
Windows, and this is the first attempt that did.

**Next step.** Print the gathering state from inside webrtc-rs (`RUST_LOG`
plus a subscriber, which this client does not install yet), or bisect by
building a peer connection with no ICE servers and with the wildcard address,
and see which of the two candidates above moves it. Until then no measurement
in Phase 2 can be taken on this machine.

**Resolved by DR-14 (2026-08-21).** Neither candidate was it: the callback is
only ever invoked once, with `Complete`, and this network never let it get
there. The peer connection and the pinned address were both innocent.

### DR-12: what the target machine actually offers (2026-08-20)

> **Superseded in part by DR-23.** The audio half of this record reads
> `IAudioClient3`'s engine period as "what the devices cost" and puts 20 ms
> of the budget against it. Measured, the two device legs cost 85 ms. The
> engine period is the callback cadence and one term of the latency, not the
> sum of it. The encoder half is unaffected.

**Context.** Task 0.5 front-loads two hardware unknowns: how small a buffer
WASAPI will run in shared mode (most of the ≤80 ms budget, prd.md §4, and the
crux of task 2.1) and whether H.264 encoding can happen in silicon (task 5.2).
`src/bin/probe.rs` asks the machine rather than a datasheet.

**The machine.** Windows 11 (10.0.26200), NVIDIA GeForce RTX 2060, HyperX
headset out, fifine USB microphone in. One gaming desktop — the caveats below
matter because of that.

**Output**, verbatim:

```
## render endpoints
- Headset Earphone (HyperX Virtual Surround Sound) — 48000 Hz, 2 ch, 32-bit;
  default period 10.0 ms, minimum 3.0 ms;
  IAudioClient3 default 480 frames (10.0 ms), minimum 480 (10.0 ms), maximum 480 (10.0 ms)

## capture endpoints
- Microphone (fifine Microphone) — 48000 Hz, 2 ch, 32-bit;
  default period 10.0 ms, minimum 3.0 ms;
  IAudioClient3 default 480 frames (10.0 ms), minimum 480 (10.0 ms), maximum 480 (10.0 ms)

## H.264 encoders
- NVIDIA H.264 Encoder MFT — hardware
- Microsoft AVC DX12 Encoder — hardware
- H264 Encoder MFT — software
```

**Reading the audio numbers.** The two periods answer different questions and
it is easy to quote the wrong one. `GetDevicePeriod`'s *minimum* of 3.0 ms is
the **exclusive-mode** floor — it needs the device to itself, which a client
that must coexist with a game's audio cannot have. What shared mode will
actually run is `IAudioClient3`, and here it reports minimum = default =
maximum = **480 frames, 10.0 ms**. There is no low-latency mode to unlock.

That matters because unlocking it is the entire case for the `wasapi` crate
over `cpal` (DR-8): cpal takes the default period and never calls
`GetSharedModeEnginePeriod`. On this hardware the two would land on the same
10 ms, so a second backend would buy nothing. **This is not a general result** —
an interface with a low-latency-capable driver can report 128 frames (2.67 ms),
and the probe should be re-run on one before task 2.1 closes the question for
everyone. But the burden of proof has moved: `wasapi` now has to show a device
where it wins.

Both endpoints are natively 48 kHz, which is what `opus::SAMPLE_RATE_HZ`
assumes and refuses to resample around. Both are 32-bit float, so `pick_config`
takes its `f32` path, not the preferred `i16` one.

**Reading the encoder list.** NVENC is present and Media Foundation exposes it
as a regular MFT, so task 5.2 has a hardware path that does not need NVIDIA's
own SDK. The DX12 encoder is a second hardware option and a useful fallback on
machines without NVIDIA. The software encoder exists and is exactly what the
~0-FPS-impact budget rules out.

**Consequences.**

- Task 2.1's question is mostly settled on this class of hardware; what it
  still owes is the loopback and the round-trip number, which overlaps 2.5.
- The client depends on `windows` for the probe alone, behind
  `[target.'cfg(windows)'.dependencies]` and at the same major tauri already
  pulls, so it costs no extra build.
- 10 ms of device period each way is 20 ms of the 80 ms budget before a single
  packet moves. Task 2.5 measures what the rest of the path adds.

### DR-11: the echo canceller works; its build script does not know Windows (2026-08-20)

**Context.** Task 3.4 wanted `webrtc-audio-processing` between capture and
encode. Unlike its VAD (DR-10), the parts this task needs — AEC3, noise
suppression, gain control — are all present and current in 2.1.

**It works.** Written, integrated and measured (commit `e50e674`, reverted).
Deterministic noise played into the reference ring and fed straight back as the
capture frame — a perfect echo, the hardest case, with no near-end speech to
hide behind. RMS in ≈ 4 800 throughout, on Linux x86-64, debug:

| Frame (20 ms each) | Residual | Down by |
|---|---|---|
| 0 | 4 229 | nothing — it has heard no far end yet |
| 10 (200 ms) | 52 | ~39 dB |
| 50 (1 s) | 92 | ~34 dB |
| 90 (1.8 s) | 114 | ~32 dB |

It converges inside 200 ms, then gives a little back as the adaptive gain
controller lifts what is left. `residual_echo_likelihood` reads 0.0 at the end.

**It does not build on Windows.** Upstream's CI is `ubuntu-latest` and
`macos-latest`; its `build.rs` contains no `target_os = "windows"` branch and
`main` has none either. Run on a real Windows host with MSVC 14.44, the Windows
SDK, meson, ninja and LLVM all present, it fails in five places — none of them
a real incompatibility:

| # | What | Why it is shallow |
|---|---|---|
| 1 | `Command::new("cp")` to copy the sources | The crate already has `fs_extra` in its build-dependencies |
| 2 | `Command::new("nm")` to list symbols | GNU spelling; `llvm-nm` answers identically |
| 3 | meson's `cpp_std=c++17` | One file, `agc2/input_volume_stats_reporter.cc:89`, uses designated initializers. GCC and Clang take them in C++17 as an extension; MSVC wants `/std:c++20` |
| 4 | `.flag("-std=c++17").flag("-Wno-unused-parameter")` on `wrapper.cpp` | GCC flags handed to `cl.exe`, which reads `-Wno-...` as `/W` plus a non-number. `cc::Build::flag_if_supported` is the portable call |
| 5 | Looks for `libwebrtc_audio_processing_wrapper.a` | MSVC writes `.lib`, so the wrapper's symbol prefixing is skipped silently and the link would then not resolve |

1, 2 and 3 can be worked around from outside (Git for Windows on the tail of
`PATH`, an `nm.exe` copied from `llvm-nm`, `CXXFLAGS=/std:c++20`). **4 and 5
cannot** — they are `.flag()` calls and a hardcoded filename inside the crate.

**Everything hard already worked.** With 1–3 worked around, all 440 objects of
the WebRTC C++ compiled under MSVC, abseil built as a meson subproject, and
`rust-objcopy` prefixed 60 248 symbols in the resulting COFF archive. What is
left is five lines of build script.

**Decision: parked, not swapped.** The alternatives are worse than waiting:

- **Voice Capture DSP** (Windows' own AEC, `CLSID_CWMAudioAEC`) — no dependency
  at all, and disqualified by one line of its documentation: output is
  *"8,000; 11,025; 16,000; or 22,050"* samples per second. goodvoice runs at 48
  kHz and does not resample (`opus::SAMPLE_RATE_HZ`). Routing every call through
  16 kHz and back would put a permanent telephone-quality ceiling on the product's
  main feature, plus resampler latency, to fix an echo only speaker users have.
- **`speexdsp`** — the crate does not vendor its C. `speexdsp-sys` 0.1.2 (2018)
  finds a system library through pkg-config and its `build` feature uses
  autotools, which is worse on Windows than meson. Vendoring speex's `mdf.c`
  ourselves is writing a `-sys` crate from scratch.
- **`nnnoiseless`** — pure Rust and builds anywhere, but it is noise suppression
  only. No echo cancellation, so it does not answer this task.

**The two ways back**, for whoever picks this up:

1. **Vendor.** `webrtc-audio-processing-sys` unpacks to 5.3 MB across 677 files.
   Copy it under `vendor/`, apply the five fixes, point `[patch.crates-io]` at
   the path. Self-contained, no external hosting, and the fixes to 1–3 mean the
   `PATH` workarounds go away too. The cost is a C++ tree in the repository
   larger than goodvoice itself, and somebody remembering to drop it when
   upstream catches up.
2. **Upstream.** All five are obviously-correct patches against
   `tonarino/webrtc-audio-processing`. A PR that lands makes this a version bump.

**Consequences of parking.**

- A call has no echo cancellation. On a headset — the PRD's user, §2 — nothing
  changes. On speakers the room hears itself, and that is a known hole rather
  than a bug to hunt.
- No noise suppression or gain control either; both came from the same module.
- The build needs no meson, ninja or libclang after all. cmake stays, for the
  vendored libopus (DR-4).
- `audio/vad.rs`'s detector sees the raw microphone rather than a denoised one,
  which is what its `Aggressive` setting was already chosen for (DR-10).

### DR-2: the client cannot talk to the SFU directly (2026-08-20)

**Context.** Publishing and pulling tracks means calling
`/apps/{APP_ID}/sessions/{id}/tracks/new`, which needs the app secret.

**Decision.** The secret stays in the Worker. Task 1.3 ships session creation
only; track negotiation (task 2.3/2.4) must go through a Worker proxy route
under `/rooms/:code/sfu/*` that injects the `Authorization` header, the same
shape `partytracks/server` uses. Shipping the secret to a desktop client would
hand every user the keys to the whole Realtime app.

**Consequences.** Task 2.3's spike must budget for that proxy route.

**Update (2026-08-20).** The route is written and tested. Three operations are
allowlisted — `POST /rooms/:code/sfu/tracks/new`, `PUT …/renegotiate`,
`PUT …/tracks/close` — each pinned to one method, because a request the Worker
signs is a request the account owner made. The path's session id is always the
caller's own (looked up from `?p=<participant>`, never taken from the client),
and a body that pulls from a `sessionId` belonging to nobody in the room is
refused: a stranger's session id is not secret enough to be a capability.
Everything else in the body is forwarded untouched, so Cloudflare can extend
their request model without a Worker release. The SFU's own status codes pass
through rather than being flattened into a 502.

Still open for task 2.4: peers cannot yet *learn* each other's session ids and
track names. The roster deliberately does not carry them — publishing a track
needs its own signaling message, and that is 2.4's design to make.

### DR-9: a dropped call is rebuilt, not repaired (2026-08-20)

**Context.** Task 3.5. Until now a call that lost its transport ended, which is
the wrong ending for the three most ordinary ways it happens: a Worker redeploy
(DR-5), the ~1-in-6 handshake that fails to reach `Connected` (DR-8), and ten
seconds of dead network.

**Decision: reconnecting means joining again from the top.** A goodvoice room
holds no storage, so a seat that stopped existing has nothing to resume — new
participant id, new Realtime session, microphone republished, every roommate
pulled afresh. The room code is the only thing carried across, and it is the
only thing the user ever typed.

**Decision: `Call` became a supervisor.** What the user calls "the call" — the
room, the microphone, mute and deafen, what the UI is being told — now lives in
one `Shared` above the session, and sessions come and go underneath it. Two
consequences worth stating: mute pressed while reconnecting is replayed onto
the new seat rather than lost, and the encode loop keeps draining the capture
ring while there is nowhere to send, so a reconnect cannot overflow it. In
`lib.rs` the `Call` is no longer behind an `Arc`; the tasks that push at the
webview hold `watch` receivers instead, so leaving can consume the call.

**What counts as dropped.** Four ways a session ends, and only the last is not
a drop: ICE sitting in `Disconnected` past a three-second grace, the room
closing the WebSocket, the microphone's sender failing 50 frames in a row
(one second), and the user leaving. The grace exists because `Disconnected` is
recoverable in principle but not against an ice-lite SFU that offers one
candidate and runs no checks of its own (DR-7) — there is nothing to re-pair
with, and waiting the ~30 s webrtc-rs takes to say `Failed` is 30 s of talking
to nobody.

**The schedule.** 0.5 s, 1, 2, 4, 8, then 15 s, ten attempts, about 90 seconds
of trying — long enough to ride out a redeploy or a router reboot, short enough
that a client left running overnight on a dead link stops rather than spinning
until morning. No jitter: it decorrelates a herd, and eight clients already
spread by whenever each of them noticed are not one.

**A full room is retried on reconnect, refused on a first join.** The seat that
filled it may be this client's own, held by the room until the heartbeat sweep
clears it (DR-5). Refusals that saying again would not change — a bad code, an
answer too new to parse — end the call with the reason attached.

**The UI is told, always.** New `goodvoice://state` event carrying live /
reconnecting(attempt) / ended(reason). The participant id rides on the same
event because a reconnect changes both at once, and a UI that learned them
separately would spend a frame unable to find itself in the roster.

**DR-8's two open cases are covered.** The handshake that fails after three
tries is now a reconnect rather than an ending, and the roommate holding a
session id that a peer's retry has already replaced is cleared by the
two-second reconcile. Both were seen happening in the drill's output.

**Proof, and what it does not prove.** `bin/reconnect-drill.rs` runs anywhere:
two clients converse, one throws its seat away, and the drill fails unless the
reconnecting client comes back with a *new* id, is audible again, and is heard
by the roommate who did nothing. What it cannot check is the half before that —
that a client *notices* a link that died — because the drill declares the loss
itself. That needs the network taken away for real; docs/testing/reconnect.md
has the `pfctl` and `netsh` runs, and **neither has been run yet**, on either
host.

**Consequences.**

- `Call::drop_session` is public so the drill can kill a session. It is the
  same code path a real drop takes, from the point the session is declared
  lost.
- `AudioSink` grew `level` and `set_gain` with defaults, so test doubles that
  play nothing stay as short as they were.
- A redeploy ends every call in progress and always did (DR-5). It now looks
  like a reconnect into an empty room, which is worth knowing before it looks
  like a bug.

### DR-14: one unreachable STUN URL hangs the whole join (2026-08-21)

> **The 41.4 ms mouth-to-ear in this record is retired by DR-23.** The
> 21.4 ms of wire was measured and stands; the 20 ms added to it for the
> devices was DR-12's engine period, and the devices actually cost 85 ms.
> The real figure is about 106 ms, against an 80 ms budget. Nothing about
> the ICE finding below is affected.

**Context.** DR-13 left every live proof blocked: `Call::join` failed all three
attempts with "ICE gathering never completed", on Windows and under WSL alike,
and the gathering-state callback appeared never to fire.

**What it actually is.** Reading webrtc-rs 0.20.3 rather than watching it:

- `RTCIceGatheringState::Gathering` is *never* published. The only place a
  state change is emitted is `add_local_candidate` in `rtc`'s core, on the
  empty end-of-candidates entry — so the callback is expected to fire exactly
  once, with `Complete`. "Nothing is ever printed" was half a red herring.
- That entry is added by `finish_gathering_if_ready`, which needs both the STUN
  gatherer and the TURN relayer to report done.
- The TURN relayer handles its own timeouts: `TurnEvent::TransactionTimeout`
  sets `gather_finished` and the relayer completes.
- The STUN gatherer does not. It completes only when its `stun_clients` map
  empties, and a client is removed on a candidate or on a socket write failure.
  `StunEvent::TransactionTimeOut` falls into a `_ => error!("STUN error: …")`
  arm that removes nothing (`transports/stun_gatherer.rs`). **A STUN server
  that never answers keeps gathering open forever.**

Cloudflare hands out `stun:stun.cloudflare.com:53` alongside `:3478`, and
DR-13 already measured that this network answers on 3478 and drops 53. One
unreachable URL out of eight was the whole outage. Nothing about the host was
special; the previous runs (DR-7, DR-8) passed from a network that let UDP/53
out.

**Options considered.**

1. *Filter the ICE list* — drop the `:53` URLs before handing them over.
   Rejected: it guesses which port this network dislikes, and a network that
   allows only 53 exists too.
2. *Vendor a patched `webrtc` and remove the timed-out client* — the upstream
   fix, and the same maintenance burden DR-11 is already parked on. Worth
   opening upstream; too heavy to depend on for the measurement.
3. *Stop treating `Complete` as the only way out.* Taken.

**Decision.** `wait_for_gathering` (`rtc/session.rs`) accepts either `Complete`
or **quiet**: no new local candidate for `GATHER_QUIET` (2 s) with at least one
already in hand. `Events` counts candidates through `on_ice_candidate` for no
other purpose. Nothing gathered at all is still a failure at `CONNECT_TIMEOUT`,
because an SDP with no candidates is not a connection anyone can answer.

**Why leaving early is safe.** Candidates are re-read out of the ICE agent
every time the local description is asked for; the only thing `Complete` adds
to the SDP is the `a=end-of-candidates` attribute. Cloudflare is ice-lite and
runs no connectivity checks of its own (DR-7), so the candidates that decide
the call are the ones this client sends *from*, not a list the far end picks
through.

**Measurements — task 2.5, at last.** `cargo run --bin latency -- --pings 30`,
native Windows on the DR-12 machine, against the live deploy:

```
--- 30 bursts heard, 0 lost ---

  first burst heard 3.1 s after both clients were in the room
  (14 went out before the pull was carrying anything)

  wire path (encode → SFU → decode)
    min       20.9 ms
    median    21.4 ms
    p95       22.2 ms
    max       76.5 ms

  mouth to ear, median
    total     41.4 ms  against a 80 ms budget
```

**41.4 ms against the 80 ms budget**, adding DR-12's 20 ms of device period to
a 21.4 ms median wire path. The same run under WSL reports 41.9 ms, so the
number is the network's, not the host's. Repeat it with any `--pings`; a fresh
room is used each time.

Three things the number does not say:

- **No jitter buffer exists yet.** This is the raw network, and the p95/max
  spread (22.2 ms against 76.5 ms) is exactly what a jitter buffer is for. It
  will spend some of the 38 ms of headroom.
- **The analog path is not in it.** Converters either side are the PRD's
  problem too and are not measurable from here.
- **Loss is measured only once the pull is live.** The first heard burst is
  what proves a subscription carries sound; bursts before it were counted as
  lost until this task, which made a clean run look like a lossy one. They are
  reported separately now.

**Consequences.**

- The Windows automated proofs run for the first time: `bin/rtc-spike.rs`
  (task 2.3) and `bin/reconnect-drill.rs` (task 3.5) both **PASS** on this
  host. DR-7 and DR-8 recorded them from macOS only.
- Joining is up to 2 s slower on a network where every ICE URL answers, since
  `Complete` still arrives on its own and the quiet window only ever runs when
  it does not. Measured cost on this network: none — the wait ends on quiet.
- `tokio`'s `test-util` feature is a dev-dependency now, so the three new tests
  in `rtc::session` drive a paused clock instead of waiting out 25 real
  seconds.
- Worth reporting upstream: the STUN gatherer's timeout arm is a one-line fix
  in webrtc-rs, and until it lands every client of that crate hangs on any ICE
  server it cannot reach.
- **Teardown noise.** Both drills end with `microphone frame not sent:
  SendError(SenderRtp(…))` — a frame handed to a sender whose session is
  already closing on the way out of `leave()`. Cosmetic, and unrelated to this;
  worth silencing when a task next touches that path.

### DR-15: cpal stays; the case for the `wasapi` crate has to be made elsewhere (2026-08-21)

> **DR-23 measured what this record left owed, and the verdict holds for a
> different reason.** cpal still stays — but not because there is headroom.
> There is none: the devices cost 85 ms of an 80 ms budget. The parking
> condition below is half met, since the onboard endpoints do report a
> 128-frame minimum; the other half fails, because a control run showed the
> render leg — the one a `wasapi` backend would shorten — is not where the
> time goes. Read the last paragraph's "38 ms of headroom" as withdrawn.

**Context.** Task 2.1 and prd.md open question 4: `cpal` or the `wasapi` crate
on Windows. The seam (`audio::device`) has always meant this decision costs
nothing above it (DR-8), so it comes down to one question — does cpal's WASAPI
path fit the ≤80 ms budget, or does control over shared-mode buffers buy enough
to justify a second backend?

**What the machine says.** DR-12 asked the driver directly:
`IAudioClient3::GetSharedModeEnginePeriod` reports **minimum = default =
maximum = 480 frames (10.0 ms)** on both endpoints. There is no low-latency
mode to unlock here.

**What cpal does with that.** `cpal-0.18.2`, `host/wasapi/device.rs`, in its own
words:

> The callback period is always `GetDevicePeriod()` regardless of what is
> requested here; the value only affects ring-buffer latency.

cpal initialises shared mode event-driven at the engine period and never calls
`IAudioClient3`. So on this machine cpal and a hand-written `wasapi` backend
would land on exactly the same 10 ms, and the second backend would buy nothing
but a second thing to maintain.

**What the call measures.** DR-14 timed the whole path at **41.4 ms mouth to
ear** against the 80 ms budget, of which 21.4 ms is the wire. The devices are
inside that number, and there is 38 ms of headroom.

**Decision.** cpal stays, and it is now the Windows backend rather than a
placeholder for one. `wasapi` is not rejected on principle: it is parked until
a device shows up where `GetSharedModeEnginePeriod` reports something smaller
than the default — an interface with a low-latency driver can report 128 frames
(2.67 ms) — *and* the budget needs it. Both halves have to be true; today
neither is. `audio/hardware.rs` keeps its place behind the seam either way.

**Correcting DR-12 on one detail.** DR-12 read the endpoints' 32-bit float mix
format and concluded `pick_config` would take its `f32` path. It does not:
cpal enumerates several formats per endpoint and `pick_config` prefers `i16`,
which is what both endpoints are actually opened as. `bin/audio-spike` prints
the negotiated configuration on every run, so the next reader does not have to
infer it:

```
  capture  Microphone (fifine Microphone) — 48000 Hz, 2 ch, i16, buffer device default
  render   Headset Earphone (HyperX Virtual Surround Sound) — 48000 Hz, 2 ch, i16, buffer device default
```

That is one fewer conversion on the capture path than DR-12 assumed, not one
more.

**The harness.** `bin/audio-spike` has the two halves of the task's DoD:

- `cargo run --bin audio-spike` — the microphone played straight back out of
  the speakers, through the same `hardware::open()` the app uses, with a level
  meter once a second so a run in a log still shows the microphone was alive.
  **Wear headphones.** On the DR-12 machine: 2250 frames in 45 s, which is
  45.0 s of audio — the loop keeps the device's clock exactly.
- `cargo run --bin audio-spike -- --roundtrip` — a 5 ms burst out of the
  speakers, timed until the microphone hears it. **The earcup has to be held
  against the microphone**; there is no coupling otherwise and the run says so
  rather than reporting a number.

**Still owed by this task, and both need a person.** Whether the loopback is
*audible*, and the round-trip measurement itself. The harness refuses to report
a spread when fewer than half the bursts came back — the first attempt without
coupling heard 1 of 33 and would otherwise have published a confident 238.9 ms
of room noise, which is exactly the number that ends up quoted in a later DR.

**Observed and not chased: one glitch per run.** Both a 45-second monitor and
the round-trip runs print `audio device error: A buffer underrun or overrun
occurred.` exactly once, mid-run rather than at startup, in a debug build. The
frame count stays exact, so nothing is drifting. The likely cause is that the
monitor prefills nothing: the render ring holds only what capture just produced,
so one scheduling hiccup starves the render callback. It is the same shape of
problem as the jitter buffer DR-14 says the call still lacks, and it belongs to
whichever task builds that.

**Consequences.**

- `audio::burst` is a new library module: the burst, the leading-edge rule and
  the one-in-the-air pairing that `bin/latency` and `bin/audio-spike` both need.
  It was `bin/latency`'s private code and had no tests; it now has nine.
- `hardware::describe()` reports the two default endpoints and the
  configuration `open` would pick for them, without opening anything.
- prd.md open question 4 is answered for this class of hardware. It stays open
  for hardware with a low-latency driver, and the probe (`bin/probe`) is what
  answers it there — one run, no listening required.

### DR-16: two ways to measure minimize-to-tray and get the wrong answer (2026-08-21)

**Context.** Task 4.1's DoD is "manual flow works", which usually means nothing
is checked until someone remembers to check it. Most of it can be scripted from
outside the process: post the window messages Windows itself would post, then
ask whether the process is alive and whether the window is visible.

**Trap 1: `MainWindowHandle` is not the app's window.** A debug build is a
console application, so the process owns a console window as well as the Tauri
one, and .NET's `Process.MainWindowHandle` returned the console. `WM_CLOSE` to
that handle kills the process outright — no `CloseRequested`, no handler, no
tray. The reading is "close-to-tray is broken", and it is entirely an artefact.
Enumerating the process' top-level windows shows what is actually there:

```
class='tray_icon_app'            visible=False
class='Tauri Window'             visible=True   title='goodvoice'
class='Tao Thread Event Target'  visible=True
class='PseudoConsoleWindow'      visible=True
```

The scripts match on the class `Tauri Window`.

**Trap 2: closing during startup is handled by Windows, not by Tauri.** Even
against the right window, a `WM_CLOSE` posted a second or two after launch
exits the process without the handler running. Waiting eight seconds first
makes it pass every time. Nobody can click the close button on a window they
have not seen yet, so this is a property of the test — but for an hour it looked
like a heisenbug, because adding an `eprintln!` to the handler "fixed" it: the
extra work happened to move the close past the window that was still opening.
**A test that does not wait measured the wrong thing twice and reported it
confidently both times.**

**What is settled.** Both flows, scripted (docs/testing/tray.md):

```
VISIBLE_BEFORE=True   ALIVE_AFTER_CLOSE=True     VISIBLE_AFTER_CLOSE=False
                      ALIVE_AFTER_MINIMISE=True  VISIBLE_AFTER_MINIMISE=False
ICONIC_WHILE_HIDDEN=False   VISIBLE_AFTER_RESTORE=True
```

`ICONIC_WHILE_HIDDEN=False` is the one worth keeping: `hide()` leaves the
window hidden but *not* minimised, so bringing it back is a `show`, not a
restore animation. That is what "no flicker" rests on.

**Design notes worth keeping.**

- **Minimise has no event to intercept.** What arrives is the `Resized` the
  minimise already performed, so the window is hidden after the animation
  rather than instead of it. The arm is guarded on *visible and minimised*
  together, because `show` un-minimises while still hidden and that resize must
  not read as a fresh minimise — the window would put itself straight back in
  the tray, and the tray icon would look dead.
- **`Window::is_minimized()` from inside the window-event handler is safe.**
  It dispatches to the event loop, and the dispatcher runs it inline when it is
  already on the main thread. That was the first suspect for the startup
  failure above and it was innocent.
- **Quit is the only tidy exit now.** `crate::end_call` runs behind a
  three-second timeout: a stranded seat is reclaimed by the room, a quit that
  hangs on a dead network is not (DR-5).

**Consequences.**

- `tauri`'s `tray-icon` feature is on. It builds on Linux too, so the client's
  test suite still runs off Windows.
- `lib.rs` grew `end_call`, and `leave_room` is now one call into it.
- Task 4.2's menu grows from `Action`/`action_of` in `tray/mod.rs`, which is
  the tested part; adding mute, deafen and leave is a variant each.

### DR-17: `cargo test` stopped starting on Windows, and it was the menu (2026-08-21)

**Context.** Task 4.2 gave the tray menu items that change — a tick against
Mute, greying out Leave when there is no room to leave. The moment the library
mutated a menu item, `cargo test` on Windows stopped running at all:

```
error: test failed, to rerun pass `--lib`
  process didn't exit successfully: goodvoice_client_lib-<hash>.exe
  (exit code: 0xc0000139, STATUS_ENTRYPOINT_NOT_FOUND)
```

No test ran. Nothing was printed. `cargo build` and the app itself were fine.

**What it is.** `dumpbin /imports` on the test executable:

```
comctl32.dll
    DefSubclassProc
    SetWindowSubclass
    RemoveWindowSubclass
    TaskDialogIndirect
```

All four are exported by **comctl32 version 6**, and a process gets version 6
only by asking for it in its manifest. `tauri_build` embeds a manifest that
asks — through `embed-resource`, which emits `cargo:rustc-link-arg-bins`. Bins
and *their* test harnesses get it. The library's own unit-test executable is
not a binary target, so it gets nothing, binds to the version 5 in `system32`,
and dies in the loader before `main`.

**Why the obvious fix does not work.** Handing the same manifest to every
target with `cargo:rustc-link-arg=/MANIFEST:EMBED` plus a
`/MANIFESTDEPENDENCY` is the first thing to try, and the linker refuses it:

```
CVTRES : fatal error CVT1100: duplicate resource. type:MANIFEST, name:1
LINK : fatal error LNK1123: failure during conversion to COFF
```

— because the binaries already have one. `cargo:rustc-link-arg-tests` is the
narrow version of the same idea and does not apply either: it means `tests/`
integration targets, and this crate's tests live inside the library
("the package does not have a test target"). Cargo has no way to aim a link
argument at the one target that is short.

**Decision.** `build.rs` delay-loads it: `/DELAYLOAD:comctl32.dll` plus
`delayimp`, on MSVC only. The test executable never calls into comctl32 and now
never loads it. The app resolves it on the first call, by which time its
manifest has long since asked for version 6 — verified by both tray drills
passing afterwards (docs/testing/tray.md), which exercise exactly the
window-subclassing path that needs it.

**Options considered.**

1. *Drop the menu features that pull it in.* `PredefinedMenuItem` (the
   separators) drags in `TaskDialogIndirect`, so it was the first suspect —
   removing it changed nothing. It is any item **mutation** that does it:
   `set_checked`, `set_enabled`, `set_text` all go through Tauri's
   `run_item_main_thread!`. Task 4.2 is a menu that changes; there is nothing
   to give up here.
2. *Move the Tauri shell out of the library into `main.rs`.* Would work, and
   costs the crate its shape for a linker detail.
3. *`#[cfg(not(test))]` around the mutation.* Product code that vanishes under
   test, so that the tests can pass. No.

**Consequences.**

- One more thing that only bites on Windows and only in a test build. The
  symptom is memorable enough to grep for: **`0xc0000139` means a missing DLL
  export, not a failing test.**
- Delay-loading is per-DLL and comctl32 only. If a future dependency needs
  another version-6-only DLL bound at startup, the same fix extends by one
  line.

### DR-18: what a push-to-talk key is allowed to do (2026-08-21)

**Context.** prd.md open question 3, and task 4.3's other deliverable: a voice
client for gamers has to hear a key while a game has focus, and games are
watched by anti-cheat software that is paid to be suspicious of exactly that
kind of software. What is the safe shape?

**What goodvoice does.** One `WH_KEYBOARD_LL` hook, installed only while a call
is in push-to-talk mode, watching for one key.

**What that is, precisely** — this matters, because the name "hook" covers two
completely different things:

- `WH_KEYBOARD` and `WH_GETMESSAGE` are *injected* hooks: Windows loads your DLL
  into every process on the desktop. That is the thing anti-cheats exist to
  notice, and goodvoice does not do it.
- `WH_KEYBOARD_LL` is not injected. The callback stays in this process; the
  system marshals the event to it and waits for it to return. Nothing of
  goodvoice's is ever mapped into the game.

**What it does not do, on purpose.**

- **It never consumes the key.** Every event is passed on with
  `CallNextHookEx`. The game sees the keystroke exactly as it would have.
- **It never synthesises input.** goodvoice has no `SendInput` and no
  `keybd_event` anywhere in it. (`docs/testing/hotkey.ps1` does, to drive the
  drill — that is a test script, it presses F13, and it should not be run while
  a game is open.)
- **It reads no other process.** No `OpenProcess`, no memory reads, no overlay,
  no graphics hooking.
- **It is not resident.** Out of a call, or in any mode but push-to-talk, there
  is no hook on the desktop at all.

**The stance on the three anti-cheats the PRD names.** This is a *design*
argument, not a compatibility guarantee, and it is worth saying plainly which
is which:

- The behaviour above is what every mainstream voice client does for push to
  talk. A rule that blocked it would break the entire category, not goodvoice.
- What these systems are built to catch — injection, memory access, synthesised
  input, driver-level input — is a list goodvoice is absent from, and that is
  the argument.
- **Vanguard is the one to watch**, because it is a kernel driver loaded at
  boot with the broadest view of the machine. Nothing here is known to conflict
  with it, and "known" is doing real work in that sentence: policies change,
  this has not been tested against every title, and no promise is made.

**If one of them ever does object.** Three fallbacks, in the order they cost:

1. **Raw Input** — `RegisterRawInputDevices` with `RIDEV_INPUTSINK` on a
   message-only window delivers keyboard input regardless of focus with no hook
   anywhere. Same two edges, same behaviour, a smaller footprint. This is the
   first thing to try, and `tray::hotkey`'s surface (`listen`, returning a
   `Listener` that stops on drop) is deliberately narrow enough that swapping
   the implementation touches nothing else.
2. **`RegisterHotKey`** — no hook at all, but it reports only the press. Push to
   talk becomes toggle-to-talk, which is a different feature.
3. **Focus-only** — the in-window handler from task 3.3, which is still there
   and takes over on its own if the hook cannot be installed.

**Consequences.**

- A failed hook is not a failed join. `Hotkey::bind` reports it and the window
  says "heard only while this window has focus" instead of pretending.
- The hook callback runs on the input path of every process on the machine, so
  it stays cheap: one atomic load and a comparison for a key that is not ours.
  Windows drops hooks that take too long, and a slow one is felt as a laggy
  keyboard everywhere, not just here.
- Key repeat is filtered to the two edges. Holding a key would otherwise say
  "start talking" fifty times a second.

### DR-19: the second and a half of ICE nobody was using (2026-08-22)

**Context.** Task 4.4: launch to audible in under three seconds (prd.md §4).
The first measurement was **9.3 s** in a debug build and **7.24 s** in release,
so this was never going to be a matter of shaving.

**The harness.** `bin/coldstart` launches the real app and stops the clock when
a second client — already in the room, listening — decodes the app's first
frame. One clock, one process boundary, and a real far end, so the number
covers everything a person waits through: process start, WebView2, the audio
devices, the room join, ICE, DTLS, the SFU, and the other client's subscribe.
Nobody clicks: the app joins the room named in `GOODVOICE_AUTOJOIN`, which is
also the shape task 6.2's `goodvoice://join/<room>` link will need.

`GOODVOICE_TRACE_JOIN=1` makes a join print where its own time went. That is
what turned this from a number into a list:

```
setup at 320 ms          process, Tauri, WebView2 — the window is already up
join room at 530 ms      HTTPS to the Worker: the room, the SFU session, the ICE servers
join gather at 1520 ms   ICE
join published at 430 ms the SFU accepting the track
join connection at 480 ms DTLS
```

**The finding.** Gathering was the single biggest item, and almost all of it was
waiting for candidates that would never be used. Cloudflare hands out six TURN
URLs — one relay on six ports, so a firewall that blocks one lets another
through — and webrtc-rs allocates on all of them. The quiet-window rule from
DR-14 then waited for the last of them to finish. **What ICE actually needs is
one direct path and one fallback**; the other five relays are more fallbacks of
a kind already in hand, and only one of them is ever used.

**Four changes.**

1. **Gathering stops at `enough`** — one server-reflexive candidate and one
   relay. The quiet window is still there for networks where one of those never
   arrives. **1520 ms → 41 ms.**
2. **The quiet window is 750 ms, not 2 s** (DR-14's value). A server-reflexive
   candidate is one round trip and a relay is an allocation and an
   authentication; a straggler at three quarters of a second is not coming.
3. **A gathering that has settled once is not waited on again.** Nothing here
   restarts ICE, so every renegotiation — one per track subscribed — was paying
   a whole window for an answer that could not have changed.
4. **The roster socket and the peer connection are opened together.** Both hang
   off the join response and neither waits on the other; serially they were two
   TLS handshakes end to end.

**Measurement, five runs, release, on the DR-12 machine against the live
deploy:**

```
  launch → heard in the room      min 2498   median 2692   max 2762 ms
  of which this client's own half min 1899   median 1981   max 2124 ms
  and 711 ms waiting for the other client to subscribe

WITHIN BUDGET, with 308 ms to spare.
```

Five runs of five were heard. Before these changes, one run in three produced
nothing at all within 45 s — the pull side kept being told *the publisher never
started sending* while the publisher was still gathering. Making the publisher
fast appears to have taken that with it, which is a fix that was not aimed at
anything.

**Two optimisations the task suggested, measured and refused.**

- *Parallel audio-init.* Opening the devices takes **72 ms**
  (`bin/audio-spike` prints it). There is nothing there to overlap.
- *Lazy UI.* `setup` runs **320 ms** in, with the window already created. The
  autojoin does not wait for the webview and never did, so building the window
  later would be racing something that is not on the path. Worth at most a
  fraction of that 320 ms, and it costs the app its shape.

**What is left, if the budget ever tightens.** Three server round trips —
the room join (530 ms), the track publish (430 ms) and DTLS (480 ms) — and the
far end's subscribe (711 ms). None is client-side work; they are the network
and Cloudflare. The subscribe is the one with room in it: a client learns about
a new publisher from the roster and then asks for the track, and DR-14 measured
that path taking seconds when it goes wrong.

**Consequences.**

- The gathering rule decides what goes into **every** SDP, so it is regression
  tested by everything: `rtc-spike`, `reconnect-drill` and `latency` were all
  re-run on the same build and all pass (the mouth-to-ear median came out at
  40.7 ms, against DR-14's 41.4 ms).
- `GOODVOICE_AUTOJOIN` joins without a window. Task 4.5's soak needs it too.
- A build script that fails leaves the previous binary in place, and a timing
  run that greps for `BUILD=` rather than `BUILD=0` will measure it and report
  the old numbers with a straight face. That happened once here; see the
  toolchain note about `tauri-winres` losing `RC.EXE`.

### DR-20: the voice client fits in a quarter of the budget; the window does not (2026-08-22)

**Context.** Task 4.5: half an hour idle in a room, under 2% CPU and at or
under 120 MB (prd.md §4). CPU turned out not to be the story.

**The harness.** `bin/soak` launches the release app, joins it to a real room
with a second client already in it, minimises it into the tray, and reads the
whole process tree every two seconds: kernel+user time differenced against the
wall clock and the machine's twelve processors, and the sum of the tree's
working sets and, separately, its private bytes.

Three decisions in that sentence are the measurement:

- **The tree, not the process.** A Tauri app on Windows is
  `goodvoice-client.exe` plus six `msedgewebview2.exe` processes. Reading only
  ours would have reported 34 MB and passed.
- **Somebody in the room.** An app alone in a room subscribes to nothing and
  decodes nothing. That is a cheaper client than anybody runs.
- **Liveness.** The second client counts frames arriving from the app, so a
  soak where the app quietly fell out of the room is reported as such rather
  than as a very good result. 897 of 897 samples carried audio.

`docs/perf/idle-soak.ps1` reads the same tree through CIM and .NET while the
soak runs — no shared code, no shared API. Two implementations agreeing is
evidence about the app; two disagreeing would have been evidence about the
arithmetic.

**Measurement, 30 minutes, minimised in a room, DR-12 machine, live deploy:**

```
  CPU, share of the machine     median 0.39 %   p95 0.65 %   max  1.04 %
  CPU, share of one core        median 4.66 %   p95 7.79 %   max 12.48 %
  memory, tree working sets     median  361 MB              max   404 MB
  memory, tree private bytes    median  157 MB              max   185 MB

  CPU  WITHIN BUDGET   0.39 % against 2 %
  RAM  OVER BUDGET       361 MB against 120 MB
```

The second opinion: CPU median 0.39%, memory median 361.1 MB. The peaks differ
(404 against 367) because two-second sampling catches a transient eighth
process — a WebView2 utility worth about 40 MB for a few seconds — that
five-second sampling steps over.

**Nothing leaks.** Working set by five-minute bucket: 361.0, 363.9, 363.4,
361.1, 361.0, 361.0 MB. Private bytes: 157.4, 157.4, 156.6, 156.4, 156.2,
156.2. Per process, start against end after 26 minutes, the largest change
anywhere in the tree is half a megabyte. Half an hour is not a leak test, but a
slope worth chasing before ship would have shown here, and there is none.

**Where the 361 MB is.** `--type=` is what each WebView2 process was started as:

```
  goodvoice-client    main               34.2 MB ws     7.2 MB private
  msedgewebview2      main              130.6 MB ws    39.8 MB private
  msedgewebview2      gpu-process        63.0 MB ws    59.8 MB private
  msedgewebview2      renderer           61.3 MB ws    26.7 MB private
  msedgewebview2      utility            39.6 MB ws    12.8 MB private
  msedgewebview2      utility            20.1 MB ws     8.7 MB private
  msedgewebview2      crashpad-handler   12.8 MB ws     2.9 MB private
```

**Everything the budget is about is in the first line, and the first line is a
quarter of the ceiling.** The other six are the WebView2 runtime, and they are
all resident *with the window hidden*: a GPU process compositing nothing, a
renderer holding a 420-pixel roster nobody is looking at, two utilities and a
crash handler. It is not an artefact of counting shared pages twice either —
private bytes, which count nothing shared, come to 157 MB across the tree, and
7 MB of that is ours.

**Decision.** Report the gap rather than move the budget, and fix it in
**task 4.6**. prd.md §4 says 120 MB and the app ships at 361; the number that
should change first is the app's, and there are three untried levers before
anybody argues about the PRD:

| | what it should buy | what it costs |
|---|---|---|
| `additionalBrowserArgs`: no GPU process, one renderer, features off | the 63 MB GPU process, some of the 130 MB browser | a config line, and a UI that must not need GPU compositing (it does not) |
| `ICoreWebView2_3::TrySuspend` while hidden, `Resume` on show | the renderer's 61 MB, and Chromium frees more under suspension | COM through `with_webview`, and a resume on every restore |
| close the webview in the tray, rebuild it on show | everything above the 34 MB floor | 4.1's "coming back is instant" — a rebuild is the ~320 ms setup DR-19 measured |

Two traps for whoever takes 4.6. **`webview2-com` is pinned to tauri's `windows`
0.61 and this crate depends on 0.62** — different crates, and their COM types do
not interconvert, so the suspend path has to be written against tauri's. And a
webview that is closed and rebuilt has to be handed the call's current state on
the way back: roster, health and controls are pushed *on change*
(`push_roster`, `push_state`, `push_controls`), and a window that arrives after
the last change learns nothing until the next one.

**Consequences.**

- prd.md §4's 120 MB was written against a client, not against a client plus a
  browser. If 4.6's three levers land short, that row needs restating with what
  is actually being bounded — and F2's acceptance box goes with it.
- CPU has margin to spend: 0.39% of twelve processors, with open-mic
  transmitting continuously. Screen share (Phase 5) is the next thing that will
  want it, and its own budget is FPS rather than this one.
- `bin/soak` is the regression test for 4.6 and for anything else that changes
  what the app keeps resident. It exits non-zero when a budget is missed.

### DR-21: the window goes away, not to sleep (2026-08-22)

**Context.** Task 4.6, from DR-20: idle in a room the client costs 361 MB
against a 120 MB budget, of which 34 MB is goodvoice and 327 MB is a WebView2
runtime with nothing on screen. DR-20 listed three levers, cheapest first.

**Options.**

| | ceiling on what it can buy | what it costs |
|---|---|---|
| (a) `additionalBrowserArgs`: no GPU process, one renderer | the GPU process' 63 MB, part of the browser's 130 MB | a config line |
| (b) `TrySuspend` while hidden, `Resume` on show | the renderer's 61 MB, plus whatever Chromium frees under suspension | COM through `with_webview`, against tauri's `windows` 0.61 rather than this crate's 0.62 |
| (c) destroy the window in the tray, rebuild it on show | all 327 MB | a rebuild on the way back, and a window that has to be told what it missed |

(a) and (b) share a ceiling neither can pass: they make the *idle* browser
cheaper, and the browser is still running. Adding both together and being
generous about what suspension frees leaves something like 200 MB. The budget is
120 and the floor is 34. Only (c) clears it, so (c) is what was built — a
cheaper lever that does not reach the budget is not a cheaper way to pass it.

**Decision.** In the tray, goodvoice has no window at all.

- `tray::window_event` stops intercepting the close: it lets the window be
  destroyed, and `run()`'s `RunEvent::ExitRequested { code: None, .. }` refuses
  the exit that the last window closing would otherwise cause. `code: None` is
  what separates the two exits that reach it — the tray's Quit goes through
  `app.exit(0)` and arrives carrying a code, and that one is meant. Minimise
  still needs the `Resized`-while-minimised trick from task 4.1, because there
  is no "minimise requested" to answer; it now calls `destroy` instead of
  `hide`.
- `tray::show` rebuilds from the app config —
  `WebviewWindowBuilder::from_config(app, app.config().app.windows.first())` —
  so the new window is the declared window rather than an approximation of it,
  and its label is still `main` (which is what keeps `capabilities/default.json`
  applying to it, DR-22).
- A window built in the middle of a call has missed every event that ever
  described it, because `push_roster`, `push_state`, `push_speaking` and
  `push_controls` all emit *changes*. So the window asks: `current_status`
  returns a `Snapshot` — call, controls, health, who is speaking — and App.tsx
  applies it on mount, after the listeners are registered and only if no event
  has already said something newer.
- One more hole the same shape, found on the way: a call joined without a
  window asking for it (`GOODVOICE_AUTOJOIN`, and task 6.2's invite links) was
  never announced to the window at all, and no existing event carries the room
  name. `CALL_EVENT` does now.
- `show` takes an `opening` flag and drops a second Open that arrives while the
  first is still in flight. Building a webview pumps the message loop and Tauri
  registers the window *before* the build returns, so a nested Open finds a
  half-built window and asks it to un-minimise — a dispatch to the main thread,
  from the main thread, while it is inside the build. What comes back is a
  window on screen and an event loop answering nothing: no close, no quit, no
  tray, `IsHungAppWindow` true, and the call still running underneath.
  Honestly about the evidence: a real double click on the icon does **not** do
  this here, because the build finishes in ~130 ms and the second click lands
  after it. What does it every time is UI Automation's `Invoke` twice in a row,
  which is how it was found and why `tray-roundtrip.ps1` clicks once. A slower
  machine closes that gap on its own, and dropping the second Open costs
  nothing — it wanted a window, and one is on its way.

**Measurements.** DR-12 machine, live deploy, release build with
`--features custom-protocol` (DR-22).

`docs/testing/tray-roundtrip.ps1`, joined to a room, closed to the tray and
brought back by invoking the notification-area icon:

```
                       window up      in the tray     back
  processes              7                 1            7
  tree working set     333.2 MB         33.7 MB      338.6 MB
  window handle        alive            gone         new
  click to window                                    146 ms
```

`bin/soak`, 30 minutes minimised in a room with a second client in it, 896
samples, all 896 carrying audio:

```
                      median      peak     budget
  BEFORE (DR-20)      361 MB     404 MB    OVER    120 MB
  AFTER                34.0 MB    34.1 MB  WITHIN  120 MB

  CPU                0.39 %      0.97 %    WITHIN    2 %
  processes in the tree                     1
  drift, first sample to last              +0.0 MB
```

Working set by five-minute bucket: 33.8, 34.0, 34.0, 34.0, 34.0, 34.0 MB. The
PowerShell second opinion, over the same half hour through CIM and .NET rather
than Toolhelp: CPU median 0.39%, memory median 34.0 MB, peak 34.1 MB — the same
numbers from an implementation that shares no code with the first.

**Nothing accumulates across rebuilds.** Twelve close-and-reopen cycles, tree
working set measured in the tray each time: 33.7, 34.1, 34.2, 34.3, 34.4, 34.5,
34.8, 34.7, 34.7, 34.9, 34.9, 34.9 MB. It settles at 34.9 and stays there, so
what the first few cycles cost is a one-off — pages the allocator keeps rather
than a webview that never quite goes away. The rebuild itself is flat across
all twelve: 124 to 153 ms.

**Consequences.**

- 4.1's "coming back is instant, and the webview is still in the room it was
  in" is no longer true. Coming back costs ~130 ms and gives you a *new* window:
  same size, same title, scrolled to the top, with anything typed into the join
  form gone. That is the price of the 327 MB and it is worth saying out loud
  rather than in a commit message. `docs/testing/tray.md` says it too.
- The window is now the app's peak rather than its cost: ~330 MB while it is on
  screen. Nothing budgets that, and levers (a) and (b) are still available to
  whoever wants to.
- prd.md F2 reads "Minimize **hides** the window; tray icon remains; voice keeps
  running". Two of the three are unchanged and the first is now stronger than
  what it asks for, so the box is met rather than renegotiated — but the word is
  wrong and the next person to read it will assume the webview is still there.
  Left as written; the PRD is the ask, not the log.
- `Snapshot` is a second description of the call, alongside the four push
  events, and the two can drift. Both are covered by serialisation tests
  against the field names App.tsx destructures, which is the drift that would
  actually hurt.
- Screen share (Phase 5) will publish from Rust and be watched in a window that
  can now vanish mid-share. Whatever it keeps has to live where the call lives,
  not where the webview does.
- Driving the Windows 11 tray from a script turned out to be most of the work
  in this task. What it costs is written down in `docs/testing/tray.md` rather
  than here: the notification area is a XAML island with no `ToolbarWindow32`,
  the chevron renames itself when open, `goodvoice` matches two different
  buttons, and the right-click menu is a `TrackPopupMenu` that UI Automation
  reports as a pane with no children — reachable by keyboard and not otherwise.

### DR-22: the release build was a different app than the one being measured (2026-08-22)

**Context.** Task 4.6 needed to see the window come back from the tray with the
call still on it. The first screenshot of a rebuilt window showed the join form
during a live call. Chasing that turned up two defects, neither of them 4.6's,
both of which had been true since Phase 0 and neither of which any test could
have caught, because every test either drove the Rust side or ran under
`tauri dev`.

**One: `cargo build --release` produced a client that loads `localhost:1420`.**

`generate_context!` embeds `../dist` only when the `custom-protocol` feature is
on; without it, it bakes in `build.devUrl` and the window points at the Vite dev
server. The Tauri CLI passes the feature for `tauri build`, so a bundled
installer would have been fine — but `Cargo.toml` never declared the feature at
all, and everything in this repo that builds the client builds it by hand:
`bin/soak`, `bin/coldstart`, the tray drills. What they launched was a
goodvoice-titled window containing Edge's "localhost refused to connect".

That is not a cosmetic difference. DR-20 broke 361 MB down by process and
concluded the WebView2 runtime was carrying 327 MB with the window hidden. It
was carrying 327 MB *with an error page in it*. The conclusion survives — the
runtime is the runtime, and it is nearly all of the cost either way — but the
number was not measured against the app.

**Two: no capability file, so the window heard nothing.**

`src-tauri/capabilities/` did not exist. Tauri v2's ACL denies every plugin
command that is not granted by one, and `listen` is a plugin command
(`plugin:event|listen`) even though `invoke` of this crate's own commands is
not. So every `listen` in App.tsx was refused, in dev as well as in release:

```
SNAPDEBUG Command plugin:event|listen not allowed by ACL
```

Which means the window had never received the roster (`push_roster`), the call's
health (`push_state`), the talking dots (`push_speaking`) or mute and deafen
(`push_controls`). It looked right because everything it displays *on a join it
performed itself* comes from `join_room`'s return value. What it could not do
was change afterwards: nobody arriving, nobody leaving, no talking dots, no
reconnect banner, and no tray→window sync at all — the whole point of task 4.2.
`docs/testing/tray.md` had a table of tray→window rows marked as checked. They
cannot have been.

The failure is silent by construction: `listen()` returns a rejected promise,
and the four `listen` calls in App.tsx were never awaited for anything except
cleanup.

**Decision.** Fix both, in this task, because 4.6 cannot be verified without
them.

- `Cargo.toml` gains `custom-protocol = ["tauri/custom-protocol"]`, off by
  default so `tauri dev` keeps its hot reload. Every documented by-hand release
  build now passes `--features custom-protocol`, and the two harnesses that
  launch the app say so in the error they print when the exe is missing.
- `capabilities/default.json` grants `core:event:default` to the `main` window
  and nothing else. Minimal on purpose: the UI's only other Tauri call is
  `invoke` into this crate's commands, which the ACL does not gate. A rebuilt
  window keeps the label `main`, so it is covered by the same capability.

**Consequences.**

- DR-20's per-process table is about a WebView2 hosting an error page. Task 4.6
  makes the distinction moot for the budget — the tray-idle client has no
  WebView2 at all — but nothing else should be quoted from it as "the app".
- Task 4.4's cold start (DR-19) was measured against the same build. What it
  times is launch → first audio frame heard by another client, which is entirely
  Rust and does not wait on the webview, so the number stands; the webview it
  was racing against was loading an error page rather than 23 KB of JS.
- The tray→window half of task 4.2 has never been exercised. It is checked now,
  in `tray-roundtrip.ps1`'s screenshots (a rebuilt window with no `listen` shows
  an empty room) and by hand against the table in `docs/testing/tray.md`.
- Anything added to the UI beyond `invoke` and `listen` needs a line in
  `capabilities/default.json`, and will fail at runtime rather than at build
  time if it does not get one.
- **CI never builds what ships.** The rust job runs fmt, clippy and test with
  default features, which is exactly the configuration that hid both of these:
  the ACL is a runtime refusal, and `custom-protocol` is a feature CI does not
  turn on (it cannot, without building `../dist` first). Task 6.3 puts
  `npm run tauri build` in CI for the installer; that is the job that would
  have caught the first of these, and the second still needs the app to be
  *run*. `tray-roundtrip.ps1` is the closest thing to that in the repo.

### DR-23: the devices cost four times what the plan assumed (2026-08-22)

**Context.** Task 2.1's crate question was settled in DR-15 from the driver's
own numbers; what it still owed was the half that needs a person — whether the
loopback is audible, and the round trip itself. Both were run on the DR-12
machine, by hand, with the earcup held against the microphone.

**The loopback is audible.** `cargo run --bin audio-spike -- --seconds 15`,
speaking into the microphone: the meter peaked at 14 837 of 32 767 and every
run kept the device's clock exactly (749–750 frames in 15 s). A person wearing
the headset confirmed hearing their own voice. That is the first half of the
DoD, and it is the half no log can settle.

**The round trip is 85 ms.** Four runs, all with 20 bursts timed:

| Render endpoint | Bus | Bursts | Median |
|---|---|---|---|
| Headset Earphone (HyperX Virtual Surround Sound) | USB | 20 of 30 | 86.1 ms |
| Headset Earphone (HyperX Virtual Surround Sound) | USB | 20 of 30 | 85.1 ms |
| Headset Earphone (HyperX Virtual Surround Sound) | USB | 20 of 20 | 85.2 ms |
| Speakers (High Definition Audio Device) | onboard analog | 20 of 20 | 84.7 ms |

The spread inside a run is tiny — min 74.2, p95 85.7, max 85.9 on the last —
so this is a stable property of the path and not a sampling accident.

**It is not our buffering.** The obvious suspect was the render ring: the burst
is timed from the moment it is handed over, so anything already queued is
inside the number. `Speakers::queued` and `Microphone::queued` were added to
read both rings at the instant each burst departs and arrives, and the answer
is that they hold **0.0 ms at the median**, 10.0 ms at worst. All 85 ms is
below cpal.

**It is not the USB sound card either.** The fourth run is the control: same
headphone, same microphone, the P2 plug moved off the HyperX USB DAC and into
the motherboard's analog jack. The number did not move — 84.7 against 85.2.
Swapping the entire render device changed nothing, which means the render leg
is not where the time is going.

**What that leaves.** Two candidates, and this run cannot separate them: the
capture leg (the fifine USB microphone, the one constant across all four runs)
and the WASAPI shared-mode stack itself. **The experiment that settles it is
the same round trip with a capture device that is not the fifine**, and it did
not happen because the machine has no second microphone. Until it does, the
85 ms belongs to "the platform and this microphone", jointly.

**DR-14's 41.4 ms is wrong, and so is DR-12's 20 ms.** DR-14 measured 21.4 ms
of wire honestly and then *added DR-12's 20 ms as an assumption* about what the
devices cost. The devices cost 85 ms. A call's mouth-to-ear contains one
capture and one render, which is exactly what this round trip contains, so:

```
  21.4 ms wire (DR-14, measured)  +  84.7 ms devices (measured)  ≈  106 ms
```

against prd.md §4's **80 ms**. The budget is blown by about 26 ms, and it has
been blown since the first call — DR-14 simply never measured this half. The
38 ms of headroom DR-15 spends in its last paragraph does not exist.

Reading DR-12 again shows where the assumption came from: it quoted
`IAudioClient3`'s **engine period**, which is what the callback cadence is, and
called it "what the devices cost". The engine period is one term of the device
latency and not the sum of it — converters, the USB or codec transport and the
endpoint's own processing are the rest, and here they are four times larger.

**DR-15's parking condition is now half met.** It parked the `wasapi` crate
until a device reported a shared-mode period below the default *and* the budget
needed it. Both onboard endpoints do report one:

```
- Speakers (High Definition Audio Device) — IAudioClient3
  default 480 frames (10.0 ms), minimum 128 (2.7 ms), maximum 480 (10.0 ms)
```

and the budget now needs something. But this does **not** make the case for a
second backend, and the control run is why: `wasapi` would buy a shorter
*render* period, and moving the whole render device changed the total by
0.5 ms. Spending a backend to save 7 ms of a 26 ms overrun, on the leg already
shown to be innocent, is the wrong trade. **cpal stays.**

**Decision: the budget is the thing to revisit, not the backend.** What is
owed before anyone writes code against this:

1. The capture-leg experiment above, on any machine with a second microphone.
   It is one run and it decides whether this is a hardware property or ours.
2. If the stack is the floor, prd.md §4's 80 ms is not reachable on Windows
   shared mode with consumer USB audio, and the number in the PRD should say
   what it is measuring. 106 ms mouth-to-ear is still inside what a voice call
   tolerates; it is the *claim* that is wrong, not the product.
3. Task 2.5's harness should stop adding DR-12's 20 ms and report the measured
   device cost instead, or it will keep publishing 41.4 ms.

**Consequences.**

- `bin/audio-spike --roundtrip` now prints what each ring was holding, so the
  next reader does not have to trust that the number is not self-inflicted.
- Its over-budget message used to blame the `wasapi` crate outright. It names
  the control run instead: the render buffer is the one thing it accused, and
  the rings had already cleared it.
- `Speakers::queued` / `Microphone::queued` are measurement apparatus on the
  concrete types, deliberately not on the `AudioSink`/`AudioSource` seam — a
  synthetic source has no ring and should not have to pretend to.
- The single mid-run underrun DR-15 observed still happens, once per run, on
  both endpoints. Still unchased, still belonging to whoever builds the jitter
  buffer.
- One measurement hazard worth writing down: the HyperX's virtual surround was
  on for the first attempts and the burst detector heard 35–43 % of its bursts,
  just under the harness's half rule. Turning it off took the same run to 20 of
  20. A DSP that smears a 5 ms burst does not only add latency — it hides the
  burst from the thing timing it.

### DR-24: the echo canceller builds on Windows; it needed six patches, not five (2026-08-22)

**Context.** DR-11 wrote task 3.4 off with a working implementation and a build
that could not reach Windows, and left two ways back: vendor the `-sys` crate
with the five fixes, or wait for upstream. Vendoring was chosen. This is what
that actually took.

**The five DR-11 named, all confirmed exactly where it said.**

| # | Fix | How |
|---|---|---|
| 1 | `Command::new("cp")` | `fs_extra::dir::copy` with `content_only` — the crate already depends on it, and `content_only` is the trailing-dot trick spelled properly |
| 2 | `Command::new("nm")` | A candidate list, tried in order: rustup's `rust-nm`, then `llvm-nm` under `LIBCLANG_PATH`, then `llvm-nm` on `PATH`, then `nm`. Each is *run* rather than stat'd, because a bare name has to resolve through `PATH` and a Windows path may or may not want `.exe` |
| 3 | meson's `cpp_std=c++17` | `-Dcpp_std=c++20` passed at `meson setup` when the target is MSVC, rather than editing the C++ |
| 4 | `.flag("-std=c++17")` on the wrapper | `/std:c++20` on MSVC, GNU flags elsewhere |
| 5 | `libwebrtc_audio_processing_wrapper.a` | `.lib` on MSVC |

Two of these are better than the workaround DR-11 described. Fix 2 needs no
`nm.exe` copied by hand: **`llvm-tools` turned out not to be installed** on the
target machine — `lib/rustlib/<triple>/bin` holds only `rust-lld` and
`rust-objcopy` — so `rust-nm` was never there to find. What is always there on
a Windows build is LLVM itself, because bindgen cannot run without libclang.
Fix 3 goes through `meson setup` instead of `CXXFLAGS`, so the vendored C++
tree is untouched by four of the five.

**The sixth, which DR-11 could not have seen.** With all five in, 440 objects
compiled, abseil built, `rust-objcopy` prefixed 60 037 symbols, and the link
failed with **156 unresolved externals — every one of them attributed to
`goodvoice_client_lib.dll.exp`**, the export table.

`webrtc/rtc_base/system/rtc_export.h` turns `RTC_EXPORT` into
`__declspec(dllexport)` when `WEBRTC_ENABLE_SYMBOL_EXPORT` and
`WEBRTC_LIBRARY_IMPL` are both set with `WEBRTC_WIN`, and `meson.build` sets
all three unconditionally — in a build configured `default_library=static`.
Every object therefore carries `/EXPORT:` directives in its `.drectve` section,
and `link.exe` obeys them when the archive is absorbed into any DLL.

That alone would only be untidy. What makes it fatal is the interaction with
the crate's own symbol prefixing: `objcopy --redefine-sym` rewrites the symbol
*table* and does not touch `.drectve`, which is text. So the directives are
left asking to export `?ssrc@RtpPacketInfo@webrtc@@QEBAIXZ` while the symbol
itself is now `v2_?ssrc@…`. 156 requests to export things that no longer exist.

The check that found it: `llvm-nm` on the wrapper showed the prefixing had
renamed compiler-local labels (`v2_$LN3`) and nothing the linker was asking
for, and the wrapper turned out not to reference `RtpPacketInfo` at all — which
ruled out the wrapper and pointed at the archive.

The fix is one conditional in the vendored `meson.build`: a static library on
Windows does not ask for symbol exports. Nothing outside the archive wanted
them.

**A seventh thing, not a patch but a trap.** The first attempt at fix 6 changed
nothing, because `build.rs` states its `rerun-if-changed` explicitly — for
`src/wrapper.hpp` and `src/wrapper.cpp` — and that switches off cargo's default
of watching every file in the package. The bundled tree is copied into `OUT_DIR`
and built there, so editing it is invisible. It is now declared too.

**Measured.** `cargo test --lib processing -- --nocapture`, on both hosts:

```
echo cancelled by 31.8 dB (residual 125 of 4872 played)
```

A perfect echo — deterministic noise into the reference ring, handed straight
back as the capture frame, no near-end speech to hide behind — held 31.8 dB
down after the first second. That is DR-11's Linux figure, unchanged, now also
on Windows. The test asserts 20 dB and prints the real number, so the next
reader does not have to re-derive it.

**Consequences.**

- `vendor/webrtc-audio-processing-sys` is 2.1.0 plus six patches, each marked
  `goodvoice vendor patch N of 6` in place. `[patch.crates-io]` in the
  workspace manifest points at it, and `exclude = ["vendor"]` keeps it out of
  the workspace's own builds. 5.2 MB, 675 files.
- All six are obviously-correct against
  `tonarino/webrtc-audio-processing`, and DR-11's second way back is still
  open: a release carrying them turns this into a version bump and a deleted
  directory.
- Windows builds now need meson, ninja and LLVM. CI installs the first two and
  points at the runner's LLVM; the 440-object build happens once and lives in
  the rust-cache after that. `docs/self-hosting.md` (task 6.1) still owes a
  contributor section covering all of it.
- A call has echo cancellation, noise suppression and gain control. DR-11's
  "makes speakers unusable" is retired.
- `audio/vad.rs`'s detector now sees a denoised signal rather than the raw
  microphone, which is a change to its input that DR-10's `Aggressive` setting
  was chosen without.
- Not verified: the echo test is synthetic and perfect, which is the hard case
  for cancellation but not the real one. A person on speakers, in a room, is
  what the task's DoD asks for and what is still owed.

### DR-25: the same appearance system, without the framework it was written for (2026-08-22)

**Context.** goodvoice's style guide has always described GoodChat's appearance
system — ten palettes on `data-theme`, two skins on `data-skin`, the three
frame tokens, the hook classes. It described something the client did not have:
one hard-coded dark palette and no skin at all. This implements it.

**Ported, not copied.** The two apps do not share a stack. GoodChat is React
with Tailwind 4 and daisyUI 5, and its palettes *are* daisyUI themes
(`@plugin "daisyui/theme"`); its skins target daisyUI's component classes.
goodvoice is SolidJS with 329 lines of hand-written CSS over 23 class names.
So:

| Layer | What happened |
|---|---|
| `palettes.css` | Copied verbatim. It is the shared catalog, and §2.1 exists so a colour cannot drift between the two apps |
| `themes.css` | Rewritten. Ten blocks of plain custom properties, landing on goodvoice's six semantic tokens instead of daisyUI's twenty |
| `skins.css` | Rewritten. Same three frame tokens, same cascade rule, goodvoice's geometry |
| `skin-terminal.css` | Rewritten against goodvoice's own class names — 200 lines where GoodChat needs 745, because there is no thread, composer or tile here |
| `useTheme.ts` | Rewritten as `theme.ts`: a Solid signal, and one source of truth instead of two, because goodvoice has no account to sync against |

**The one deliberate deviation: no daisyUI.** §2.2 and §4 of the style guide
name it. Adding Tailwind and daisyUI to theme 23 class names would be
disproportionate on its own; it is worse than that here, because task 4.6 spent
a whole task getting the idle client's webview down to 35.9 MB, and this window
is destroyed and rebuilt on every trip back from the tray. The contract §3 sets
out — palettes → themes → skins by source order, three frame tokens, hook
classes, no component branching on a skin — is what actually matters, and none
of it needs a framework. Measured cost of the whole feature: the CSS bundle
goes from about 5 kB to 13.9 kB, 2.98 kB gzipped.

**A real accessibility bug, found by the tooling rather than by reading.**
Driving the appearance screen through UI Automation to screenshot it, the
buttons came back named `[LIGHT]`, `[DARK]`, `[SYSTEM]`. The terminal skin
draws its bracket motif with `::before` / `::after`, and generated content is
part of an element's accessible name — so a screen reader would have read the
decoration out loud. The style guide asserts the opposite in as many words
("on the ::before, so the text a screen reader announces is still just the
label"), and it was not true.

CSS has the answer and it is one token per rule: `content: "[" / ""` — the
half after the slash is the alternative text. Every `content` in
`skin-terminal.css` carries it now, and the names come back clean. **Worth
knowing for GoodChat, which has the same motif and, going by the same style
guide sentence, most likely the same bug.**

Two layout defects the screenshots caught, both only under the terminal skin,
because it is the skin that makes text wider: the appearance button sat on top
of the wordmark (absolute positioning, and no padding is right for both skins —
the masthead is a three-column grid now, with an empty first column buying the
symmetry back), and `[ PUSH TO TALK ]` wrapped with its closing bracket
stranded on the second line.

**Consequences.**

- Ten palettes × two skins, and a mode that can follow the operating system.
  The catalogs are `ui/appearance.ts`; the state is `ui/theme.ts`, which paints
  before the app mounts, from `main.tsx`, so a rebuilt window never flashes the
  default first.
- The default is `goodvoice-crimson` / `goodvoice-rose` — GoodChat's, because
  the ask was to look like it. goodvoice's old void-and-green is closest to
  `neon matrix`; changing the default back is one constant in `appearance.ts`.
- The client's identity is no longer a colour. Anything that hard-codes one —
  the tray icon, the installer artwork (task 6.3), the README's screenshots
  (6.4) — now has ten backgrounds to look right on.
- Not verified: the nine palette-and-skin combinations nobody looked at. Four
  were checked by eye — retro and terminal, each on a light and a dark palette
  — plus the room panel under terminal. The rest share every rule with those.

### DR-26: the checklists that needed a second person did not need one (2026-08-22)

**Context.** Four tasks carried the same rider: built, tested, and waiting on
someone to confirm the last step by ear or by eye. 3.2's mute, 3.3's talk key,
4.1's "voice continues" and 4.3's global hook. The machine has one person on
it, and half of what was owed was not really about a person at all — it was
about there being a *second participant*.

**`bin/listener` is that participant.** It joins a room, publishes silence or a
steady tone, and once a second reports how many frames it received and how loud
they were. A 20 ms frame path delivers fifty a second, so "am I being heard"
stops being a judgement: it is 50 or it is 0, and the transitions land in the
second the thing happened.

With it, and with UI Automation driving the shipping client, all four rows came
back as numbers:

| Task | The claim | What was measured |
|---|---|---|
| 3.2 | mute stops the packets | 0 frames while muted, 50 after unmute, real microphone through the live SFU. `MUTED` seen in the roster |
| 3.3 | a held key gates transmission | key up 0, key held 50, with the keyboard focus on another application |
| 4.1 | the call survives the window | window closed, **0 WebView2 processes left**, 50 frames a second unbroken across the whole trip, and the rebuilt window inside the same call |
| 4.3 | the key is heard from anywhere, and not taken | the drill hears it from a windowless process; the *voice* follows it while another app has focus; `hotkey.rs` calls `CallNextHookEx` unconditionally |

Three rows are genuinely better than the by-ear version they replace. "You are
heard" measured at the far end catches a gate that half-opens, which an ear
would call working. And 4.1's is the one that mattered: "voice continues" was
the DoD's own sentence and had never been checked at all.

**What is left is what an eye or a game does, and nothing else.** Whether the
rebuilt window flickers (4.1). Whether the key still reaches a fullscreen game
(4.3, steps 4–5). Whether a room hears itself on loudspeakers (3.4). Three
rows, each needing something this session could not synthesise.

**Two traps, both costing several attempts.**

**WebView2 builds its accessibility tree lazily.** A targeted
`FindFirst(Descendants, Name)` on a cold tree returns nothing, which is
indistinguishable from a button that is not there — and the first query is what
wakes it, so the *second* attempt succeeds and the first looks like a bug in the
app. Anything driving this client through UIA must walk the whole tree once
first. It is also why a click script must re-warm after the window is rebuilt
from the tray: that is a new tree.

**A skin renames every button.** The terminal skin's `text-transform:
uppercase` reaches the accessible name, so `push to talk` is `PUSH TO TALK`
and a script written against one skin silently finds nothing under the other.
The brackets used to leak in too until DR-25 gave them empty alternative text.
Any future UIA-driven test should match case-insensitively rather than assume a
skin.

**One correction worth keeping.** Mid-session the listener reported 0 frames for
four minutes and the obvious reading was that the call had died in the tray —
a serious bug in 4.1, if true. It had not: the person had muted, exactly as
asked, between one listener run and the next. The measurement was right and the
window it covered was wrong. A frame counter says what arrived, not why.

### DR-27: nothing said which of twelve binaries was the app (2026-08-22)

**Context.** Task 6.3's first `npm run tauri build` succeeded, printed two
bundle paths, and produced a 1.4 MB MSI. The app on its own is 7.9 MB. What it
had packaged, under the name `goodvoice`, was `audio-spike.exe` — a 386 KB
capture spike. An installer that installs and runs and is the wrong program is
exactly the shape of DR-22's release build, and it announced itself the same
way: one line in a log nobody reads, `Built application at: ...audio-spike.exe`.

**Cause.** The package has twelve `bin` targets — the app plus eleven harnesses
— and no `default-run`. Cargo does not care, because every harness is reached
by name. The bundler has to pick one, and picked the first alphabetically.

**Decision: `default-run = "goodvoice-client"` in `[package]`.** It is Cargo's
own field for this question, it fixes `cargo run` with no `--bin` at the same
time, and it renames nothing — the harness scripts, `docs/perf/idle-soak.ps1`
and `docs/testing/tray-roundtrip.ps1` all still look for `goodvoice-client.exe`.

**Two things that looked like fixes and are not.** `mainBinaryName` in
`tauri.conf.json` *renames* the output binary; it does not choose one, so it
would have shipped the spike under a better name. And an explicit `[[bin]]`
block does not reorder anything: `cargo metadata` sorts targets alphabetically
regardless of what the manifest declares.

**A second binary rides along, and `default-run` does not stop it.** The
finished MSI still carries `audio-spike.exe` as an "external binary". It is not
cargo's doing: passing `--bin goodvoice-client` through to cargo, so the ten
harnesses are never compiled, changes nothing. **The bundler reads
`src-tauri/src/bin/` off the filesystem** and takes the first entry it finds.
Proven by moving the directory aside and re-running `tauri bundle` alone —
`audio-spike.exe` disappears, and only the app and its cdylib remain.

**Not fixed, and why.** The only way out is for `src/bin/` not to exist: the
eleven harnesses move to `src/harness/` with eleven explicit `[[bin]]` paths.
`cargo run --bin <name>` would be unchanged, which is what every document in
this repo actually types, but the prose in this plan and in `docs/` points at
`src/bin/*.rs` in about a dozen places. That is a refactor with a paper trail,
not a build fix, and it is worth doing before 6.4 tags a release — 1.1 MB of a
3.1 MB installer is the spike.

**Measurements.** MSI 5.1 MB, NSIS 3.1 MB. Installed per-user into
`%LOCALAPPDATA%\goodvoice`, no admin prompt. The installed binary joined the
live deploy and was heard at 50 frames a second for 8 seconds by `bin/listener`.

### DR-28: the indicator was a light switch, and the meter under it never reached zero (2026-08-23)

**Context.** The roster's talking dot lagged and read wrong: a hard on at
`SPEAKING_LEVEL` and a hard off, arriving up to 100 ms late. What was wanted
was grey at silence and the theme's accent when the microphone hears something,
with the shades in between.

**Three separate causes, and only one of them was the obvious one.**

**The push was a set of names.** `Vec<String>` cannot say *how loud*, so no
amount of CSS was going to fade anything. It carries `{id, level}` now, plus
the pre-gate input level the sensitivity meter needs, quantised to 1/255 —
without which a still microphone's last floating-point bits read as a change
and push twenty times a second forever. A room where nobody is talking still
pushes nothing: every level in it falls under the floor and collapses to the
same empty reading.

**One meter was answering two questions.** `Meter`'s release is slow on
purpose — the gap between two words must not put the light out. That is right
for the decision and wrong for a drawn level, which then takes seconds to come
down and shows the loudest recent thing rather than the voice. It keeps two
decays now: `held` for `is_speaking`, `shown` for `level`.

**And the release never finished.** `previous - previous / decay` is integer
division: below `decay` the subtrahend is zero and the level stops falling. The
old meter came to rest at 63/32767 and stayed there for the rest of the call.
Invisible under a threshold, and not at all invisible under a dot whose
brightness *is* the number. `saturating_sub((previous / decay).max(1))` — at
least one step, and safe at zero.

**Measured on the way, and worth keeping.** libfvad in `Aggressive` mode at
48 kHz admits a 220 Hz tone at **one percent of full scale**. The detector asks
whether a sound is *shaped* like speech, not whether it is loud, so a room's
hum, a fan and a keyboard pass it at any volume. That is the whole case for a
manual threshold, and it is now the premise of a test rather than an assumption
— the test asserts the detector is permissive, and fails loudly if a future
libfvad turns fussy and quietly makes the setting redundant.

**A threshold has to sit downstream of the gain controller, and that was
checked rather than assumed.** AGC2's adaptive digital gain ramps over seconds
on a quiet input, and a fixed threshold behind it would drift open on its own.
It does not: 26 seconds at a threshold nothing in the room reaches gave exactly
zero frames at a listener in the same room. An earlier six-second run had shown
a 39-frame burst in its last second, which looked exactly like that drift and
was real room sound.

**Two traps for the next script that drives this window through UI Automation.**
DR-26 said to walk the tree once before querying it. That is not enough: the
walk that wakes WebView2's accessibility tree *returns the cold one*, so the
first `FindAll` has to be thrown away and a second one made. And a search by
name alone is not safe here — the wordmark's accent span is a `Text` element
literally named "voice", so a script looking for the voice-activity button
clicks the logo. Match on `ControlType.Button` as well as on the name.

### DR-29: the harnesses are a package, because neither road out was config (2026-08-23)

**Context.** DR-27 left the installer carrying `audio-spike.exe` — a 1.1 MB
measurement tool in a 3.1 MB bundle — and named the fix: move the harnesses out
of `src/bin/` and declare them with explicit `[[bin]]` paths. That was done, and
**the installer got worse**: all eleven harnesses shipped instead of one.

**So the bundler reads a tauri crate for binaries two ways.** Every `[[bin]]`
its manifest declares, *and* whatever files sit in its `src/bin/`. Declaring
them explicitly does not move them out of reach; it hands the bundler a list.
The old behaviour — exactly one stray binary — was the second road with nothing
on the first.

**`bundle.externalBin: []` does not close either road.** Tested against a
`tauri bundle` with no recompile: the wxs came back with all thirteen sources
regardless. Those entries reach the NSIS template's "external binaries" section
but they do not come from that config key.

**Decision: `client/src-tauri/harness`, its own package in the workspace.** A
package the tauri crate does not own is on neither road. The tauri crate is
back to one binary, `src/bin/` does not exist there, and the manifest declares
no `[[bin]]` at all.

**What it cost, which is what DR-27 got wrong.** DR-27 said `cargo run --bin
<name>` would be unchanged. It is not: the root of `client/src-tauri` is a
package, so `--bin` resolves inside it and the harnesses are no longer there.
Every command grew `-p goodvoice-harness` — 37 of them across the plan's task
list, `docs/`, and the harnesses' own usage comments. Cargo's error names the
package for you, which is the only reason this is a nuisance rather than a
trap. The gates grew a flag each for the same reason: `cargo fmt --all`,
`cargo clippy --workspace`, `cargo test --workspace`. Without them the new
package is unlinted and `bin/soak`'s six tests stop running.

`[workspace.lints.clippy]` holds the pedantic setting now, and both packages
inherit it, so the harnesses are held to the bar they were already meeting.

**Measured.** NSIS 3.10 MB → 2.99 MB, MSI 5.09 → 4.52. The wxs lists
`goodvoice-client.exe` and `goodvoice_client_lib.dll` and nothing else; the
NSIS script's "Copy external binaries" section is empty. Installed and heard at
50 frames a second by a listener in the same room. `cargo test --workspace` is
142 — the 136 the tauri crate had, plus the 6 in `bin/soak` that moved with it.

**One upgrade wart, for exactly one machine.** NSIS does not remove files a
newer version stopped shipping, so an install made from the previous bundle
keeps its `audio-spike.exe` until someone deletes it. Only this machine ever
had one.

### DR-30: four ways a Windows runner builds the wrong thing (2026-08-23)

**Context.** The first push in twenty-seven commits, so the first CI run since
task 3.4 vendored webrtc-audio-processing. Four failures, arriving together
because nothing had been pushed in between. Each one is a thing the
development machine gets right by accident.

**1. `prettier --check .` walked the vendored C++ tree.** Four files, one of
them the upstream project's own `.gitlab-ci.yml`. Not ours to reformat, and a
`--write` in there would have buried DR-24's six patches in whitespace.
`src-tauri/vendor` in `.prettierignore`.

**2. meson found gcc.** windows-latest puts Strawberry Perl's gcc and a MinGW
toolchain ahead of MSVC, and meson picks its compiler by looking. It got as far
as abseil's cctz, where MinGW's own `windows.foundation.h` redefines
`IReference<boolean>` over `IReference<BYTE>` and loses the
`WindowsCreateStringReference` family. The fix is the development machine's own
`env.ps1`: enter the dev shell, and carry `INCLUDE`/`LIB`/`LIBPATH` into
`GITHUB_ENV` because an environment does not survive into the next step. `CC`
and `CXX` are named outright as well, so PATH order is not load-bearing.

**3. Then meson found Git's `link.exe`.** *"Found GNU link.exe instead of MSVC
link.exe … This link.exe is not a linker."* It is coreutils' `link`, which
makes a hard link, and Git for Windows puts its `usr/bin` ahead of the dev
shell. Filtered out of PATH alongside Strawberry and MinGW.

**4. Then MAX_PATH, reported as an empty filename.** `C1083: Cannot open
compiler generated file: '': Invalid argument`, on the same file MinGW had
failed to write a dependency file for — which is the tell that both runs hit
the same 260 characters and only the wording differed. The default target
directory spends 44 of them on
`D:\a\goodvoice\goodvoice\client\src-tauri\target` before meson nests
abseil under a `libabsl_synchronization.a.p` object name.
`CARGO_TARGET_DIR: D:\gv`. The development machine has written to
`C:\Users\<you>\gv\target` since task 4.5, for the unrelated reason that
OneDrive was syncing 13 GB of build output, and has been quietly short enough
ever since.

**5. And then bindgen read the wrong clang's headers.** Every AVX-512 intrinsic
an undeclared identifier — `__builtin_elementwise_fshl` and friends — out of
`VC\Tools\Llvm\x64\lib\clang\22\include`. Visual Studio ships its own
clang, the dev shell puts it on PATH, and whichever LLVM answers first is the
one bindgen loads. **The LLVM step was before the dev shell and had to be
after it**, which is the order `env.ps1` has always used. The step now prints
the version it loaded and fails outright if `libclang.dll` is not where it
expects, rather than letting bindgen quietly find another: run 32617030554
says `clang version 20.1.8`, not 22.

**What this is really about.** Every one of the five is the development machine
being right for a reason nobody wrote down — a short path chosen for OneDrive,
a PATH order chosen for no stated reason at all. CI is the only thing that asks
those questions out loud, and it cannot ask them while nobody pushes. 142 tests
on the runner now, which is what `--workspace` is worth after DR-29.

### DR-31: what Windows.Graphics.Capture gives, and what it withholds (2026-08-23)

**Context.** Task 5.1 asks three questions on real silicon: what is there to
capture, what format do frames arrive in, and how does the frame pool behave.
`bin/capture-spike` asks the machine rather than the documentation. Same
machine as DR-12 — Windows 11 (10.0.26200), RTX 2060 — with a 1920×1080
primary and a rotated 768×1366 second display.

**What there is.** Two monitors and, at that moment, exactly **one** window:

```
## monitors
- **\\.\DISPLAY1 (1920×1080, primary)** — 1920×1080, handle 0x1007f
- **\\.\DISPLAY2 (768×1366)** — 768×1366, handle 0x20001

## windows
- **◑ Plan.md completion** — 1280×800, handle 0x40304
```

One window is the interesting number. `EnumWindows` on this desktop returns
well over a hundred handles; all but one of them is invisible, minimised, a
tool window, or **cloaked** — the DWM state that backs every suspended UWP app
and every virtual desktop you are not looking at. Cloaked windows are visible
by `IsWindowVisible`, have titles, and cannot be captured. A picker that
skipped that filter (task 5.3) would offer a list mostly made of things that
fail when clicked.

**The format, verbatim:**

```
- texture 1920×1080, content 1920×1080
- DXGI format 87 (DXGI_FORMAT_B8G8R8A8_UNORM)
```

BGRA8, and — on a monitor — texture size equal to content size. The two are
separate fields because they diverge on a window: a pool sized for a window
keeps its old textures when the window shrinks, and the remainder is undefined.
Anything that scales or encodes reads `ContentSize`, not the texture's.

That format is also the answer 5.2 wanted: BGRA8 is what Media Foundation's
H.264 encoders take as input, so the capture and the encoder agree without a
conversion pass between them.

**The frame pool does not tick.** This is the finding, and it is the one that
would have been assumed wrong:

| target | frames | wall clock | 500 ms waits that timed out |
|---|---|---|---|
| primary monitor, terminal scrolling | 141 | 8.0 s | 0 |
| the terminal window itself | 105 | 6.0 s | 0 |
| second monitor, nothing moving | **1** | 6.1 s | **12** |

An idle display produces exactly one frame — the initial one — and then
nothing at all. Not a slow frame, not a duplicate: silence. WGC delivers on
content change, so **frames per second is a property of what is on the screen,
not of the capture**, and a timeout means "nothing happened" rather than "the
capture broke". `Capturer::next_frame` returns `Ok(None)` for it and says so.

Two consequences reach later tasks. Task 5.3's publish path cannot treat a gap
as a stall, and must not wait for a frame that has no reason to arrive. And
task 5.5's benchmark gets its target for free: a still screen costs nothing
because nothing is captured.

**Intervals, while the content was moving:**

```
- interval min 13.9 ms, median 27.8 ms, p95 205.5 ms, max 305.5 ms   (monitor)
- interval min 13.9 ms, median 22.3 ms, p95 145.8 ms, max 208.3 ms   (window)
```

The 13.9 ms floor is the display's 72 Hz refresh, which is the ceiling WGC can
deliver at. The long tail is not jitter — it is the pauses between bursts of
scrolling. The median is the honest number for a moving screen, and both are
comfortably above the 30 fps a share needs.

**One thing that worked and was not expected to.**
`SetIsBorderRequired(false)` succeeded from an unpackaged process: the dumped
window frame has no yellow capture border on it. It is still called
best-effort in the code, because the call is documented as privileged and a
border is a cosmetic complaint where a failed share is not.

### DR-32: three ways to lose the zero-copy, and one to lose the frames (2026-08-23)

**Context.** Task 5.2 wants H.264 in silicon, fed the texture the capture
already produced. Media Foundation will do exactly that, and it will also do
something that looks identical and is twenty times slower, without saying which
it is doing.

**1. `MF_SA_D3D11_AWARE` is not on the activate.** The natural place to read it
is the `IMFActivate` you already have from `MFTEnumEx` — the same object that
carries the friendly name and the hardware URL. Read there it is **absent for
every encoder on this machine**, including NVENC:

```
- NVIDIA H.264 Encoder MFT — hardware, memory buffers only
- Microsoft AVC DX12 Encoder — hardware, memory buffers only
- H264 Encoder MFT — software, memory buffers only
```

The attribute lives on the **transform's** store, which means activating the
MFT before you can ask. Read there, the same three encoders answer:

```
- NVIDIA H.264 Encoder MFT — hardware, takes D3D11 textures
- Microsoft AVC DX12 Encoder — hardware, takes D3D11 textures
- H264 Encoder MFT — software, memory buffers only
```

**What it cost, measured.** With the attribute read from the activate, the
`MFT_MESSAGE_SET_D3D_MANAGER` was skipped — and everything still worked.
`MFCreateDXGISurfaceBuffer` hands a texture to an encoder that has not been
given the device, and Media Foundation quietly copies it out of GPU memory.
**8.13 ms a frame. With the message sent: 0.42 ms.** Nothing failed, nothing
warned, and the difference is the entire point of the task.

That is why `is_zero_copy()` exists on the encoder rather than staying an
implementation detail: it is the only thing that distinguishes the two.

**2. A synchronous MFT will not allocate your output for you, and says so
several calls later.** The software encoder has neither
`MFT_OUTPUT_STREAM_PROVIDES_SAMPLES` nor `..._CAN_PROVIDE_SAMPLES`, so a
`ProcessOutput` with a null sample fails. Swallow that — it looks exactly like
the ordinary "nothing to give yet" — and the *next* `ProcessInput` returns
`MF_E_NOTACCEPTING` (0xC00D36B5), "the callee is currently not accepting
further input", from a call that has nothing wrong with it. The fix is to check
`GetOutputStreamInfo` and supply a buffer of `cbSize`; the lesson is that
`ProcessOutput`'s errors have to be told apart rather than treated as one.

**3. An asynchronous MFT offers exactly one sample per event.** Having stopped
swallowing errors for (2), the hardware path immediately broke:
`E_UNEXPECTED` (0x8000FFFF). A synchronous transform is drained in a loop until
`MF_E_TRANSFORM_NEED_MORE_INPUT`; an asynchronous one gives one sample per
`METransformHaveOutput` and treats a second ask as a protocol error, not as an
empty answer. `collect` branches on which kind it holds. The old code had both
bugs and neither was visible, because bug (2)'s swallowed error was hiding bug
(3)'s.

**What the three have in common.** Every one of them is Media Foundation
answering a question correctly and the *wrong question* having been asked. None
of the three failed loudly at the point of the mistake: one was 20× slower, one
surfaced as an error on an unrelated call, and one only appeared once another
bug stopped covering for it.

**Also settled here.** NV12 is not optional — no hardware encoder on this
machine offers a BGRA input type — so the `D3D11` video processor between
capture and encode is part of the path rather than an optimisation to
reconsider. It runs on the capture's own device, which is why `Capturer`
exposes `device()` and `context()` and why the encoder takes them rather than
making its own.

### DR-33: only the first viewer ever got a picture (2026-08-24)

**Context.** Task 5.4 was written, compiled and left unticked because nobody
had opened and closed the window during a live share. Somebody — a script —
finally did: `docs/testing/viewer.ps1`, driving the shipping client through UI
Automation while `bin/viewer-drill` shared a screen from the other end.

The first viewer showed the screen. The second, third and fourth sat on
**"nobody is sharing"** while a share was live, every time.

**Two faults, and the second one hid behind the first.**

**1. The playback loop was feeding a window that no longer existed.**
`screen_playback_loop` was spawned with the sink it had at subscribe time and
kept it forever. Opening a second viewer installed a second sink and changed
nothing: the frames kept arriving at the first one's IPC channel, which
belonged to a destroyed webview and swallowed them. The loop now reads
`Shared::watch_sink()` per access unit — a lookup behind a mutex, once per
frame at 30 fps, against a decode that costs milliseconds.

**2. Nothing gave the subscription up, because the window never got to.**
`Viewer.tsx` sent `stop_watching_screen` from Solid's `onCleanup`, which is an
*unmount*. Closing a window is a **destroy**: the webview goes with it and no
JavaScript in it runs again. So the sink stayed installed, the call stayed
subscribed, and `reconcile_watch` — which correctly does nothing when the
sharer and the session have not changed — had no reason to re-wire anything.

The command is gone. `tray::window_event` already sees every window's events
for task 4.6's sake, so the viewer's `Destroyed` is what unsubscribes now.
That also makes prd.md §3 F3's "viewers opt in" a property of the window's
lifetime rather than of a message the window has to remember to send, which is
what task 5.4 always claimed it was.

**The race that fix introduced, and the generation that closes it.** A window's
destruction is noticed asynchronously — the handler spawns onto the runtime —
and a person can open the next viewer before that lands. Unconditionally
clearing the sink would then take the *new* window's subscription away, and
nothing in the window would notice or recover. So `watch_screen` returns a
generation, `unwatch_screen` takes one, and a close only clears the sink it was
handed out for. The generation is read synchronously in the window event, where
it is still the closing window's.

**Why no gate caught it.** `cargo test`, clippy, `tsc` and `prettier` were all
green over the broken code, and stayed green. The transport was not even wrong:
`bin/rewatch` — join, watch, unwatch, watch again, headless — **passed against
the same build**, because a drill that unsubscribes properly exercises the path
that works. What was broken was reachable only through a window being closed by
a person, which is exactly the step the task had been left unticked for.

Two tests now fail without the fix: `a_second_viewer_takes_the_frames_over` and
`a_closing_window_cannot_unsubscribe_the_one_after_it`.

### DR-34: a screen that is not moving was a screen that published nothing (2026-08-24)

**Context.** `viewer.ps1` puts a plain grey sheet over the monitor while it
runs, so that the aspect-ratio measurement has a picture whose edges it can
find (a nearly-black wallpaper inside a nearly-black letterbox measures
nothing). The sheet made the shared screen perfectly still — and cycles 2, 3
and 4 went black again, after DR-33 was fixed and verified.

**Measured, with `bin/share-drill` under the sheet: 0 access units, 0 bytes, 20
seconds.** Not a viewer problem at all. The sharer was publishing nothing.

**Why.** WGC produces a frame when the content changes and nothing when it does
not (DR-31), and the client only ever read the pool when the arrival callback
said something had changed. Two consequences nobody had separated:

- **A share that starts on a still screen never sends a first frame**, so
  there is no keyframe, so no viewer can decode anything, ever, until somebody
  moves a window.
- **A share whose screen goes still ends mid-GOP.** A viewer opening later gets
  P-frames at best and cannot start on one. `request_keyframe` exists and is
  called exactly once, when the share starts, because the sharer has no way to
  know a viewer arrived: the H.264 codec is registered with `rtcp_feedback:
  vec![]`, so no PLI is negotiated and Cloudflare has nothing to ask with.

**Two fixes, both in the capture thread, neither touching negotiation.**

**Ask the pool anyway.** On the poll timeout, `Capturer::next_frame` now calls
`TryGetNextFrame` rather than assuming a timeout means an empty pool. The
session's first frame is the screen as it already is and arrives without ever
being a *change*; asking gets it. It answers once and then has nothing until
something moves, so this is one frame on a still screen rather than ten a
second — the same 20 seconds went from 0 access units to 11.

**Repeat the last keyframe every two seconds while nothing is happening.** The
picture has not changed, so the IDR already in hand *is* the current picture:
re-sending it costs no encode and gives a viewer that opens mid-stillness
something to start on within two seconds. `share.rs` keeps the last keyframe
packet and drops it when a resize makes it describe a different shape.

**What it costs.** A perfectly still 720p share is **69 kB over 20 seconds —
3.5 kB/s**, all of it keyframes of a screen doing nothing. Against zero before,
and against a share that was useless before. On a moving screen it costs
nothing at all: the repeat only fires when the capture has been silent for two
seconds.

**What was considered and not done.** Negotiating `nack pli` / `ccm fir` on
H.264 and requesting a keyframe when Cloudflare asks for one is the version of
this that costs nothing at all and serves a viewer in milliseconds rather than
in up to two seconds. It needs SDP feedback lines, an RTCP read loop on the
screen sender, and its own verification against the live SFU — a task, not a
footnote, and one that composes with both fixes above rather than replacing
them.

### DR-35: sharing costs a GPU-bound game 5.6%, and the budget said ~0 (2026-08-24)

**Context.** Task 5.5 asked for the FPS cost of sharing, against prd.md §4's
"~0 FPS impact sharing". Nobody had measured it; the number was an assumption
carried since the PRD.

**Measured.** *Far Far West*, a steady scene at 57 fps with the GPU 95% busy —
the case the budget is about — and three 30-second PresentMon captures a run so
that drift has somewhere to show:

| share | fps alone → sharing | delta | GPU ms a frame | idle-to-idle noise |
|---|---|---|---|---|
| 1080p30 | 57.0 → 53.8 | **−5.6%** | 16.64 → 17.57 | −0.5 fps |
| 720p30 | 56.9 → 54.4 | **−4.4%** | 16.66 → 17.32 | −0.4 fps |
| 1080p15 | 56.5 → 54.7 | −3.2% | 16.77 → 17.25 | −2.0 fps |

**Where it goes.** 0.93 ms × 57 frames ≈ 53 ms of GPU a second, for 30 shared
frames: **1.8 ms a shared frame**, of which DR-32's NVENC is 0.42. The other
1.4 is WGC handing over a 1920×1080 BGRA texture and the video processor making
NV12 of it — which no encoder here can skip (DR-31), and which is why 720p
saves a third rather than a half: both qualities read the same source.

**Options.**

1. **Fewer frames a second while a game is on screen.** The one lever already
   measured: 0.93 ms → 0.48 when the rate halves, because everything in the
   path is per-frame. 15 fps is choppy for a game and unremarkable for a
   document, so the rate wants to follow what is being shared rather than be a
   constant.
2. **Cheapen the conversion.** 1.4 ms for a capture and a colour convert, on a
   card that encodes in 0.42, is not obviously right. `VideoProcessorBlt` runs
   per frame at source resolution; scale-and-convert in one shader pass at the
   *output* resolution is the alternative, and it is the only option that
   improves the 30 fps case without changing what a viewer sees.
3. **Change the budget.** "~0" was never measured. 5.6% on the worst case, for
   a feature somebody turned on deliberately and can turn off in a click, may
   be an honest price.

**Decision: none yet, and that is deliberate.** The task's DoD is "budget met
or DR with fix plan", and the fix plan is (1) and (2) with (1) already
quantified. What is not defensible is the number being unknown, which it was
until today. Task 6.4's README carries measured numbers, and this one has to be
in it whichever way the choice goes.

**Two notes for whoever runs it next.** PresentMon needs an elevated shell —
an ETW session is not something a normal user opens — and the run is three
captures rather than two: the third caught a two-fps drift in the 15 fps run
that would otherwise have been read as the share costing nothing.

---

**Decision, 2026-08-25 (§7.1): option 3. The budget moves to the measurement.**

prd.md §4 now reads **≤ 6% FPS impact on a running game while sharing 1080p30**,
and F3's acceptance criterion with it; the README carries the same number until
6.4 rewrites it. "~0" was written before anything measured it and was wrong by
the whole of the cost, and a budget that is wrong is worse than a budget that is
large: it is the line every later decision gets checked against.

**Why not the two that make it smaller.** Neither is refuted — both are real,
and (2) is the better engineering — but neither is a v0.1.0 change. Option 1
buys 0.45 ms a frame by halving the rate, and the rule it needs is "is this a
game", which nothing here can answer; a share that quietly went choppy on the
content somebody most wants smooth would be a worse feature than a share that
costs three frames a second. Option 2 replaces `VideoProcessorBlt` with a
scale-and-convert shader pass at output resolution, which is where 1.4 of the
1.8 ms a shared frame is, and its payoff is unmeasured — it is GPU work of a
kind that wants its own measurement task, not a change made against a release.

**What makes 5.6% payable.** It is one deliberate click, reversible in one
more, on the worst case this hardware has: a GPU already 95% busy, at the
highest of the two qualities offered. 720p30 is 4.4% on the same scene, and
that choice is in the picker. What was never defensible was not knowing.

**Left for after the release:** option 2, as its own task with its own
before-and-after (`docs/perf/screenshare-bench.ps1` re-run against the same
scene). If it lands, the budget comes down to what it then measures.

### DR-36: the client could not be pointed anywhere (2026-08-24)

**Context.** Task 6.1 is a document, and the document has four steps: make a
Realtime app, deploy the Worker, check it, point the client at it. Writing the
fourth one is where it stopped. prd.md §9 says "paste the Worker URL into the
client's settings"; the client had no such setting.

What it had was `DEFAULT_SERVER`, an `option_env!("GOODVOICE_SERVER")` read at
**compile time**. So self-hosting meant every person in the squad installing
Rust, MSVC, LLVM and Node, and building a client of their own — for a project
whose task 6.3 ships a 3 MB installer precisely so that nobody has to.

**Where the setting lives, and why not `localStorage`.** The window is not the
only thing that joins. `GOODVOICE_AUTOJOIN` joins before a webview exists (task
4.4), and task 6.2's `goodvoice://join/<room>` will join from a link — neither
can ask a window what server it had in mind. So `home.rs` keeps it in the app's
config directory as `settings.json`, read once at startup, written whole on
change, and the window is a client of that rather than the owner of it.

**Three decisions inside it, each because of what a person will actually type.**
An empty box means "back to the bundled server" rather than an error, because
that is the only way back. A trailing slash is stripped, because every path the
client builds starts with one and `https://x//rooms/y` is a 404 that reads like
a bug in the room code. And a URL with a *path* is refused outright rather than
trimmed to its origin — the thing somebody pastes is the dashboard URL of the
Worker, and telling them beats silently pointing them somewhere they did not
ask for.

**The test that matters is the negative one.** Setting the working URL and
watching the client join proves almost nothing: a client that ignored the
setting entirely would pass it, because the bundled address is the working one.
So `docs/testing/server-setting.ps1` sets an origin that cannot exist, restarts
the app, and requires the join to **fail there**. Then it sets the real one and
requires `bin/listener` to hear fifty frames a second in that room.

**And the guide's own tools were broken for the person it is written for.**
`server/scripts/smoke.sh` announced `all checks passed` with a failing
WebSocket check underneath it — `cmd && pass` puts the command on the left of
`&&`, where `set -e` does not reach, so only a host where that check *could*
fail would ever notice. It also used `curl -o /dev/stdout`, which Windows curl
cannot open, and Git for Windows' default `core.autocrlf` rewrote the script's
line endings so that bash refused line 1. A self-hoster on Windows — this
project's whole audience — hit all three before reaching Cloudflare. Fixed,
and `.gitattributes` pins `*.sh` to LF so the third cannot come back.

### DR-37: the client answered a link before anybody could hear it (2026-08-25)

**Context.** Task 6.2 owed two things: a link whose join *fails* should say so
rather than land on the join form, and `docs/testing/invite.ps1` should pass
twice running. The first was three lines — emit the `INVITE_EVENT` that already
carries the other two refusals. Then the drill failed on its **first** check,
which had nothing to do with any of that, and the window it printed was a plain
join form: no room, and no reason either.

**The thing that was actually broken.** A `goodvoice://` link is the only path
in this client that runs *before a person is looking*. Windows starts the
process for the link, `handle_cli_arguments` feeds it to `open_invite` inside
`setup`, and by the time the webview has been built and `App.tsx` has called
`listen()`, the client has usually already decided: refused the link, or tried
to join and failed on a microphone another program was holding. `app.emit` to a
webview that is not listening yet reaches nobody. The window then draws the
join form, which is indistinguishable from a link that did nothing at all.

Measured both ways on the installed build. Before: clicking
`goodvoice://join/cold-refuse-31?s=https://goodvoice-elsewhere.invalid` cold
left the window saying `SETTINGS | ROOM | ROOM | NAME | NAME | JOIN` — the
refusal was decided, printed to a console a GUI build does not have, and lost.
After: the same click, the same binary, `an invite to cold-refuse-31 — that
invite is for https://goodvoice-elsewhere.invalid, and this client is on
https://goodvoice.goodvoice-server.workers.dev`.

**Decision.** An offer is *kept* as well as emitted (`offer_invite`), and
`Snapshot` carries it, so whichever window mounts first picks it up — the same
mechanism task 4.6 already needed for a call that began while the webview was
away. The window says when it is done with it (`dismiss_invite`), from both the
accept and the dismiss, because a webview is thrown away and rebuilt on every
trip through the tray and an offer that outlived its answer would greet
somebody with an invite from an hour ago.

**Consequences.** The three refusals — another deploy, already in a call, a
join that failed — are now equally durable, and the failed join is new. The
window's button reads *try `<room>` again* when there is no call to leave.

**Two of the drill's three failures were the drill.** It waited for the window
to *say something* and then read what it said: a join form is twelve accessible
names within a second of launch, while a cold start plus a join is thirty
(measured — the app was in the room at t+11s on a warm run and t+38s on the
first run after an install, with `bin/listener` hearing 50 frames a second in
that room throughout). So it waited for a window, found one, read the form and
called a working link broken. It now waits for an *answer* — the room, or the
client saying why not — with the failure branch printing everything the window
says. Its `Explain` step went too: it handed the URL to the binary with the
output redirected and printed nothing, twice, because a release Tauri build is
a GUI-subsystem process with nowhere to write. The client explains itself in
its own window now, which is where a person reads it anyway.

**Measurements.** `docs\testing\invite.ps1` against the installed
`goodvoice_0.1.0_x64-setup.exe`: **PASS three times consecutively**, with the
room heard by `bin/listener` at up to 51 frames a second on each run.

### DR-38: the window came back white, for a quarter of a second (2026-08-25)

**Context.** Task 7.3, and the last row of `docs/testing/tray.md`: does the
rebuilt window flicker? Task 4.6 destroys the window and its webview on the way
to the tray and builds a new one on the way back (DR-21), and every counter in
`tray-roundtrip.ps1` says that goes well — the handle is gone, the tree drops
to 34 MB, a new window exists, it is visible, it shows the call. None of them
can see a flash, because a flash is a thing about pixels over milliseconds and
the drill's finest instrument is a screenshot taken a second afterwards.

**The instrument.** `docs/testing/tray-flicker.ps1`. It clicks the tray icon
and photographs the region the window arrives in at **141 frames a second**.
That was written as "against a screen that changes 60 times a second, so
anything a person could have seen is in at least two frames"; the screen is
144 Hz (DR-40), so it is about one frame per refresh and the margin is a flash
lasting longer than ~7 ms rather than one visible at all. The capture loop is C# rather than
PowerShell for exactly that reason: a rebuild is ~400 ms and PowerShell's
per-iteration cost spends the chances. Each frame is scored on three numbers
over the window's own area: its mean luma, how much of it differs from that
mean (a flat fill scores ~0, a painted window scores high), and its distance
from the bare desktop and from the window as it ends up.

Where to point it was the awkward part, and the answer is a finding of its own
— see *the walk* below.

**The finding: yes, and it is white.** Measured on the release build with
`custom-protocol`, in a room, on a dark palette:

```
i=0    7.3 ms   luma 23.0  busy 0.154   the desktop; the handle exists, nothing on screen
i=2   32.5 ms   luma 248.1 busy 0.031   white
 ...            luma 248.1 busy 0.031   ...fifty-three more frames of exactly this
i=56 427.1 ms   luma 248.1 busy 0.031   white
i=57 434.6 ms   luma 33.3  busy 0.070   the finished window, in one frame
```

**394 ms of a flat white rectangle**, then the whole app at once. Fifty-seven
screen refreshes at this display's 144 Hz (DR-40; this said twenty-four, from
a 60 Hz that was never measured). `busy 0.031` is what says *flat*: 3% of its
pixels
differ from its own mean, which is the border and nothing else. It is not a
half-drawn page — the page never partly draws, it arrives complete — it is
WebView2's own background, before the document exists. The settled window is
luma 33 and the desktop behind is luma 23, so the flash is brighter than
anything on either side of it, which is why it reads as a flash rather than as
a window opening.

**Three ways out, and why the third.**

1. **`backgroundColor` in the window config.** One line, and wrong: goodvoice
   ships ten palettes, four light and six dark, chosen at runtime and stored in
   the webview. A static colour turns a white flash into a wrong-coloured flash
   for everyone not on the default.
2. **Persist the palette's `--bg` and hand it to the builder.** Right colour,
   always, but the window is still a solid rectangle for 394 ms. Better, and it
   costs a stored value that can go stale against the CSS.
3. **Do not show the window until it has painted.** Nothing to flash. Costs a
   fallback, because a window that is never shown is an app nobody can reach.

**Decision: the third.** The window is declared `"visible": false`; `main.tsx`
calls a `window_painted` command after two `requestAnimationFrame` ticks — two,
because a rAF callback runs *before* the paint it was queued for — and
`lib.rs`'s `reveal` shows it and focuses it. `tray::open` no longer asks for
focus itself; focus arrives with the window, rather than 400 ms before there is
anything in it.

The fallback is `reveal_after_grace`: 1500 ms after the window is built, if it
is still hidden, show it anyway and say so on stderr. Everything that could
break the promise lives outside this crate — a webview that fails to load, a
bundle missing its assets, a WebView2 runtime that is not installed — and none
of them may cost a person their window. `is_visible` is the interlock rather
than a flag of ours: `show` twice is harmless, but a late `set_focus` snatching
focus back from whatever somebody has moved on to is not.

A command rather than `getCurrentWindow().show()` on purpose: this crate's own
commands reach `invoke` without a permission, so nothing is added to
`capabilities/default.json` — and DR-22 is the reason to care, where every
`listen` was silently refused by an ACL nobody had written.

**Measured after, same drill, same build flags:**

```
BETWEEN_FRAMES=0        no frame is neither the desktop nor the finished window
FLAT_FILL_FRAMES=0
i=55 411.0 ms  luma 22.1  the desktop
i=56 421.7 ms  luma 33.3  the finished window
GEOM_VISIBLE_AT_MS=427  and the webview did it, not the grace timer
```

Desktop to finished window in **one frame**, twice over. The window is now
visible at 427 ms instead of 20 ms, which is the trade: nothing happens for
four tenths of a second after the click, and then the window is there and
complete. `HANDLE_AT_MS` is unchanged at ~6 ms — the handle was never what
anybody was waiting for.

Nothing else moved. `bin/coldstart`: **2681 ms median of five, all five heard**,
against 2692 ms recorded in task 4.4 and a 3000 ms budget — the window was
never on the audio path (DR-19 measured it up at 320 ms and idle). 170 tests,
clippy and both format gates clean. `tray-roundtrip.ps1` still reports the
window destroyed, 35 MB in the tray, a new window, visible, showing the room.

**The walk.** The rebuilt window does not come back where it was. The config
names no position, so Windows names one, and it is a different one every time:
`104,104 → 208,208 → 52,52 → 130,130` over four rebuilds in one run, and
`130,130 → 234,234 → 78,78 → 156,156` in another. It is Windows' cascade, it is
not new, and nobody had noticed because no drill had ever compared two
rebuilds' rectangles. It is why `tray-flicker.ps1` cannot be pointed at a fixed
region and instead waits for the handle — a few milliseconds, long before the
first paint — reads the rectangle off it and starts there. Left as a finding
rather than fixed: **§7.12**.

**Two things about the old drill this cost.** `tray-roundtrip.ps1`'s
`REBUILT_IN_MS=146` was never the rebuild. The stopwatch was wrapped around
`InvokePattern.Invoke()` on the notification-area icon, and that call does not
return for **2008 ms** on this desktop — measured directly, with the window
already back before it returns. The number is now printed as two, `HANDLE_IN_MS`
and `REBUILT_IN_MS` (to *visible*, which is what DR-38 makes the meaningful
one), both labelled as upper bounds that include the instrument; their
**difference**, ~400 ms, is the real figure and agrees with `tray-flicker`'s
427 ms.

And `QUIT_CLICKED=False` for two runs, with the app blamed twice, was
`SetCursorPos` **returning false**: Windows refuses injected pointer movement
while somebody is at the machine, and the right-click menu is the one step that
needs a real mouse (UI Automation has no "invoke with the other button", and
the popup is a `#32768` pane with no children — `tray.md`). The drill now says
`QUIT_CLICKED=False (no-pointer (the desktop is in use))` and the answer is to
leave the desktop alone and run it again, which is also what §7.2 needs.

### DR-39: the drill was refused input by a program it was not testing (2026-08-25)

**Context.** §7.2 is the last of the checklists that block the release, and the
"Start here" note said to try automating it before walking it, because
everything in its table except the right-click menu is reachable from UI
Automation. The drill was written (`docs/testing/tray-menu.ps1`) and would not
run: every step that needs a real mouse or a real key was refused by Windows.

**What DR-38 and `tray.md` said was wrong.** Both recorded the cause as
`SetCursorPos` failing *while somebody is at the machine*. Measured on this
desktop with `GetLastInputInfo` reporting **31 minutes** since the last human
input: still refused. Nobody was at the machine and it made no difference.

**What it actually is: UIPI.** A medium-integrity process may not inject input
into a desktop whose *foreground* window belongs to a higher-integrity one. The
foreground window is a property of the desktop rather than of goodvoice, so one
elevated program anywhere on screen blocks every drill in `docs/testing/` that
clicks. Four measurements, all from a medium-integrity PowerShell:

```text
SetCursorPos          False, and GetLastError untouched (2 in one run, 203 in
                      the next — stale values, not a reason, which is why two
                      earlier sessions could not read anything out of it)
SendInput             returns 1 — the event was accepted — and the cursor does
                      not move
AttachThreadInput     False, to the foreground window's thread
OpenProcessToken      ERROR_ACCESS_DENIED on the foreground process, which a
                      medium-integrity caller only gets from something above
                      medium. Same call on explorer, firefox and the terminal
                      succeeds.
```

Here the foreground was held by an **elevated Discord**, with a window that
`IsWindowVisible` reported as `False`. So the desktop looked idle, looked
normal in a screenshot, and refused everything.

**Two things that do not fix it, both measured.** Starting goodvoice does not:
its window comes up and the elevated window keeps the foreground, so
`SetCursorPos` is refused with goodvoice on screen and focused as far as the
app is concerned. And `Shell.Application.MinimizeAll()` does not: the
foreground stayed where it was.

**What does, confirmed.** A click on any ordinary window. The desktop was
refusing injection for the whole session; somebody came back to the machine,
clicked something, and the next `SetCursorPos` returned true — after which
`tray-menu.ps1` ran end to end and passed. Closing the elevated program does
the same. Running the drill elevated also works, and measures a slightly
different desktop than the one a person uses, so it is the fallback rather than
the default.

**What still works with no pointer at all: UI Automation.** In exactly these
conditions `tray-roundtrip.ps1` completed its whole round trip —
`TRAY_CLICKED_1=True`, `REBUILT_IN_MS_1=2425`, `TREE_MB_IN_TRAY_1=34.7` — and
only its final `QUIT_CLICKED` step failed, because that one step needs a real
mouse. Reading the window, clicking the tray icon and measuring the rebuild are
all available on a desktop that refuses injection; the right-click menu and
every click into the webview are not.

**Recorded where the next session will hit it.** `tray-menu.ps1` prints
`POINTER=` before it starts anything and stops there if the answer is no,
naming the program that holds the foreground and whether it is elevated:

```text
POINTER=refused (UIPI: Discord holds the foreground and is elevated)
RESULT=BLOCKED
```

**And the trap this leaves behind.** A drill that reports `no-pointer` is
reporting on the *desktop*, not on goodvoice. Nothing about the tray menu was
learned in any of the three sessions that hit this, and two of them wrote the
app down as the suspect.

**The other half of the same session, kept here because it is what the block
was hiding.** `tray.md` had recorded that the popup is a `TrackPopupMenu` that
UIA reports as a `#32768` pane with no children — true, and taken to mean the
ticks could only ever be photographed. They can be read: the pane answers
`MN_GETHMENU` (`0x01E1`) with the `HMENU` it is drawing, and a menu handle is a
USER object rather than a pointer, so `GetMenuItemCount` and `GetMenuItemInfo`
walk it from another process and give back text, `MFS_CHECKED`, `MFS_GRAYED`
and separators. That is what turned §7.2 from a checklist into a drill:

```text
MENU_ROW5_AFTER=Open goodvoice | -- | [x] Mute | [x] Deafen | Leave room | -- | Quit goodvoice
MENU_ROW6_AFTER=Open goodvoice | -- | Mute (grey) | Deafen (grey) | Leave room (grey) | -- | Quit goodvoice
```

The photograph is still taken beside each one, because a menu right in its own
handle and wrong on the screen is a thing that could happen and nothing else
would catch it. Both agree.

### DR-40: the game a drill can be (2026-08-25)

**Context.** §7.5, and the last two rows of `docs/testing/hotkey.md`. Push to
talk exists for one case — prd.md §3 F2, a key heard while a game has the
screen — and that case had been "a person and a game" since task 4.3. The
scripted half (`hotkey.ps1`) proves the `WH_KEYBOARD_LL` hook hears a key from
a process with no window; what it cannot show is the same thing holding when
something else owns the display, or that the thing owning it still gets the key.

**The move.** A game in fullscreen exclusive is not a genre, it is a call:
a D3D11 swap chain with `SetFullscreenState(TRUE)` on it. So
`harness/src/bin/fullscreen-drill.rs` is one — a `WS_POPUP` window the size of
the display, a swap chain that is allowed to change the mode, a colour that
follows the key, and a `wndproc` that counts the edges of one virtual key. Run
beside the real client and `bin/listener`, it answers all three halves of the
definition of done at once, and `docs/testing/hotkey-fullscreen.ps1` is that
run without a person in it.

```
GLOBAL=heard from anywhere, including over a game
FOREGROUND=fullscreen-drill
  game MODE=exclusive     DRIVER=hardware     DOWNS=2 UPS=2
  game DISPLAY_REFRESHES=4810 (144 Hz)        PRESENTS=46453
  room 0 0 ... 0 33 50 50 46 0 0 0 0 0 4 50 50 50 47 0 0 ... 0
HEARD_BURSTS=2
RESULT=PASS
```

**What each line is evidence of.** `MODE=exclusive` is DXGI's own
`GetFullscreenState`, which is necessary and not sufficient — a swap chain can
accept the call and drop straight back out. `DISPLAY_REFRESHES` is the
sufficient half: `GetFrameStatistics` answers for a chain that owns an output
and refuses one that does not, and `SyncRefreshCount` is the *display's* vblank
counter, so 144 Hz there is the monitor saying who has it. The `room` line is
the roommate's frames-a-second column — two bursts of fifty, four seconds each,
where the key was held, and holes either side, because a gated client sends no
packets rather than silent ones (task 3.2). And `DOWNS=2 UPS=2` is the
fullscreen window's own `WM_KEYDOWN`: the hook passes every key on
(`tray/hotkey.rs`), and that is the count that would be zero if it did not.

**What it is not.** No game is loaded, so no anti-cheat is. DR-18 is where that
argument lives and this does not move it. What is measured is Windows' input
path and DXGI's display ownership, which is what "over a fullscreen game" means
for every other claim in this repo.

**A drill that measures a room needs a control in it.** One run had every
other assertion green — the hook installed, the display taken, the fullscreen
window counting both edges — and a room that heard nothing at all. Read
literally that is a talk key which stops working over a game; it is not. A
capture device that will not open produces the identical column of zeroes, and
the drill had no way to tell the two apart. It holds the key once before the
display is ever touched now, and a run that fails *that* hold prints
`INCONCLUSIVE` with the app's stderr beneath it instead of a verdict on a
feature it did not reach.

**Two traps, and the second one hid the first.**

*A synthesised key needs a scan code.* `hotkey.ps1` passes zero for it and
works, because the Rust hook reads `vkCode` out of a `KBDLLHOOKSTRUCT`. A
webview does not: a DOM `KeyboardEvent.code` is derived from the **physical
scan code**, so `keybd_event(vk, 0, ...)` arrives in the window as an empty
string. The rebind stores it, `vk_for_code` cannot name it, the hook never
installs, and the window then says *heard only while this window has focus* —
which is true, and reads exactly like a broken feature. `MapVirtualKey(vk,
MAPVK_VK_TO_VSC)` is the missing byte. Two of the drill's assertions failed on
it, in two different places, and neither pointed at the cause.

*And that state survives a restart.* A talk key the window cannot name is
stored anyway, so the settings button comes back reading `key:` with nothing
after it. The next run of the drill matched `key: ` — with the space — found no
key button at all, and reported an app that had lost its settings screen. The
app is not wrong here, but it is not defensive either: nothing refuses to bind
a key that `vk_for_code` will not accept, and the only symptom is a notice
saying push to talk is window-only.

**And one number about this machine that corrects DR-38.** The display is
**144 Hz**, measured two ways: `DISPLAY_REFRESHES` above, and
`Win32_VideoController.CurrentRefreshRate`. DR-38's instrument argument says
"141 frames a second, against a screen that changes 60 times a second — so
anything a person could have seen is in at least two frames". The margin is not
there: at 141 fps against 144 Hz the camera takes about one frame per refresh.
Its *findings* are untouched — 394 ms of white is 394 ms whatever the refresh,
and fifty-five consecutive frames is 390 ms of it — but what "zero frames of
neither" licenses afterwards is narrower than it was written: no flash longer
than about 7 ms, rather than no flash at all. The two places that say 60 Hz now
say this.

**The frame rate is the GPU's, not the screen's.** `Present(1, ...)` asks to be
held to the refresh and is not: 46 453 presents against 4 810 refreshes. Some
driver setting on this machine is overriding the sync interval. It changes
nothing here — the question is input — but a drill that reported 1 363 fps
without saying why would be reporting a screen nobody was watching, so it says
`VSYNC=off` when the two counts disagree by more than double.

### DR-41: the room has no loudspeaker in it (2026-08-25)

**Context.** §7.6, the last release-blocking row that had never been reached by
an instrument. Every other row on §7's list that looked like it needed a person
turned out not to — the tray menu, the flicker, the `retro` shot, the talk key
over a fullscreen game — and this file's "Start here" said to ask the same
question of this one before scheduling anybody. What it is short of is a
loudspeaker in front of a microphone, which is the one thing on that list that
is a fact about a room rather than about Windows.

**The instrument.** `harness/src/bin/echo-room.rs`, and it reaches everything
except that fact. Two clients in one process the way `bin/latency` does it: the
room holds the real devices through the same `hardware::open` the app uses, the
far end publishes a 1 200 Hz tone and keeps every frame that comes back. The
tone makes the whole trip — SFU, transducer, air, microphone, SFU — and the
walk is four twelve-second segments: the room silent, the room with the
suppressor on, the tone playing with the canceller **off**, and the tone
playing with it on. `docs/testing/echo.md` is how to read it.

**The answer: no, this machine cannot host the test.** Two runs, and the
coupling came out at **0.4 dB** and **2.0 dB** against the 6 dB the drill asks
for before it will say anything about a canceller. The reason is in the
endpoint list rather than in the numbers:

```
## render endpoints
- **Headset Earphone (HyperX Virtual Surround Sound)** — 48000 Hz, 2 ch, 32-bit; …
Present and not active:
- Speakers (High Definition Audio Device) — not present (nothing in the jack)
- Speakers (fifine Microphone) — disabled
- Headphones (High Definition Audio Device) — not present (nothing in the jack)
- … ten more, all `not present` or `disabled`
## capture endpoints
- **Microphone (fifine Microphone)** — 48000 Hz, 2 ch, 32-bit; …
```

That listing is `bin/probe`, which grew the second half for this: it had only
ever printed `DEVICE_STATE_ACTIVE`, and a machine that had a loudspeaker last
week and has none today is indistinguishable from one that never had one unless
the inactive endpoints are printed with their state.

The only thing on this machine that makes sound is a headset earcup. DR-23 got
its acoustic round trip out of this same pair of devices and said how: *with
the earcup held against the microphone*, by hand. An earcup that nobody is
holding is not a room, and the onboard analog jack — which DR-23 also measured
through, at 84.7 ms — now reports `not present`, so the speaker that was in it
on 2026-08-22 has been unplugged since.

**The first metric was wrong, and it said yes.** The obvious measurement is the
tone's bin in the canceller-off segment against the same bin with the
loudspeaker silent. On its first run that reported

```
COUPLING=17.2 dB over the silent room
```

and there was no coupling. The room got louder between the two segments — its
median level went 121.0 → 453.3 — and a bin that rises with everything else
rose with it. Fifteen seconds is long enough for a room to change, and the gain
controller, which sits after both switches and is not one of them, moves the
rest. **Reading the tone against its own neighbours in the same 200 ms** —
960 Hz and 1 500 Hz, out of the same window — cannot be fooled that way: a
chair or a gain change lifts all three bins together and the ratio does not
move. The same run measured that way is 0.4 dB. Both columns are printed, the
absolute one because it is what `bin/listener --tone` reports and the one
comparable between machines; only the self-normalising one decides anything.

**Two independent readings agree**, which is why this is recorded as a fact
about the room rather than as a drill that needs more work. The drill's own
per-window standout says 2.0 dB, and a whole-file spectrum of the recordings
computed outside this repository puts the room's energy at 200–450 Hz and finds
the 1 200 Hz bin *below* its neighbours in the very segment where the tone was
playing.

**The windows are 200 ms because the room is loud.** A tone adds to itself
across consecutive frames and a room does not, so ten frames read as one window
buy about ten decibels over reading one — the difference between seeing a quiet
echo and reporting an empty room. The ceiling is the two device clocks: both
nominally 48 kHz, not the same crystal, and once their drift reaches half a
cycle the window stops adding up. At 200 ms and a hundred parts per million
that is a fortieth of a cycle. `tone::bin_energy` takes a slice rather than a
`Frame` for this, and a test pins the ten-decibel gain.

**Consequences.** §7.6 stays open and stays release-blocking (prd.md §3 F4).
What it needs is no longer "a person in a room" but one specific thing: a
loudspeaker the fifine can hear — an earcup laid against it, a speaker back in
the analog jack, or headphones in the fifine's *own* jack, which Windows
carries as the disabled `Speakers (fifine Microphone)` and which would put a
transducer a centimetre from the capsule — after which the whole row is one
command. Task 4.7's other
half is closer than that: `--record` writes a WAV per segment **even when the
run refuses a verdict**, so `quiet.wav` against `suppressed.wav` is a listening
test that can be done today. The level cannot answer it — `NOISE_SUPPRESSED`
came out at −1.3, 0.1 and −0.4 dB across three runs of the same room, which is
the sound of a measurement that is not measuring the thing, because the gain
controller answers a quieter frame by turning it up.

**Left unmeasured:** whether AEC3 handles a *real* delay at all. The synthetic
test in `processing.rs` hands the reference back with no delay, which is the
easy case for the estimator; DR-23 measured this hardware's acoustic round trip
at 84.7 ms, which is not. Nothing here says which side of AEC3's search window
that falls on, and nothing will until there is a loudspeaker.
