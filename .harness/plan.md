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
>   CPU, ≤120 MB RAM, ~0 FPS impact sharing, <3 s cold start.

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
  it was stuck.

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
  catches between two frames.
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
  game, and the game still receiving it. Steps 4–5.
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
  same person on loudspeakers.

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
  **Not verified:** the picker under the `retro` skin — both screenshots are
  `terminal`. The picker's CSS is the shared base plus a `terminal` block, so
  retro is the unmodified base, but it has not been looked at.
- [ ] **5.4 Viewer window** — `ui`, new Tauri window. Opt-in subscribe on open,
  unsubscribe on close, resizable, aspect-correct; audio unaffected throughout.
  DoD: open/close viewer repeatedly during live share, voice never glitches.
  Verify: manual + `npx tsc --noEmit`.
  **Written, not verified.** Everything below exists, compiles and passes the
  gates; what has not happened is a person opening and closing the window
  during a live share, which is the entire definition of done. Left unticked
  for that reason.
  Built: `ui/Viewer.tsx` is a second Tauri window (label `screen`) rendered
  from the same bundle, routed on `location.hash` in `main.tsx`.
  `open_screen_viewer`, `watch_screen` and `stop_watching_screen` in `lib.rs`
  are the commands behind it, and the main window grows a *watch N's screen*
  button whenever the roster shows somebody else sharing.
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
  **Opt-in is the window's lifetime, not a flag.** `Call::watch_screen` is the
  only thing that pulls the video track, and only this window ever calls it —
  so a client with no viewer open is one Cloudflare is not sending video to at
  all (prd.md §3 F3).
  `tray::window_event` now ignores every window but `main`: without that, task
  4.6's minimise-into-the-tray would destroy the viewer and end a live share
  every time somebody put it out of the way.
  Verified so far: `npx tsc --noEmit`, `prettier --check` and `cargo clippy
  --workspace --all-targets -- -D warnings` green; the client and the harness
  build in release.
  **Still to do:** the manual run — the sharer is `bin/share-drill --room
  <code> --seconds <n>`, and the app joins the same room with
  `GOODVOICE_AUTOJOIN`. Open and close the viewer repeatedly while that runs
  and listen for the voice path glitching. Also unverified: the aspect-correct
  claim through a resize, and the viewer under the `retro` skin.
- [ ] **5.5 [WIN] FPS-impact benchmark** — `docs/perf/screenshare-bench.md`.
  Run a GPU-bound game (e.g. built-in benchmark), record FPS with/without 1080p
  share (hw encode). Target ~0 delta.
  DoD: methodology + numbers committed; budget met or DR with fix plan.
  Verify: committed benchmark doc reproducible by another dev.

## Phase 6 — Ship

- [ ] **6.1 Self-hosting guide** — `docs/self-hosting.md`. Cloudflare account →
  Calls app → secrets → `wrangler deploy` → point client at Worker URL. A
  non-Cloudflare-user must succeed following only this doc.
  DoD: guide tested from scratch on a fresh account. Verify: clean-account walkthrough.
- [ ] **6.2 Invite links** — `client` (deep link `goodvoice://join/<room>`),
  Windows protocol registration via Tauri config; UI "copy invite" button.
  DoD: clicking a link on a machine with the app installed joins the room.
  Verify: manual link test.
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
  **Not done — the two the task also names.** Protocol registration belongs to
  6.2 and 6.2 has not started, so nothing here registers `goodvoice://`. And
  the install was done on this machine, not a clean VM: it proves the bundle
  runs, not that it carries everything a machine without the toolchain needs.
  **The bundle is the app and nothing else, as of DR-29.** It used to drop a
  1.1 MB `audio-spike.exe` beside it. NSIS is 3.0 MB, MSI 4.3 MB, and the
  installed directory is `goodvoice-client.exe` and `uninstall.exe`.
- [ ] **6.4 README final + first release** — README (features, budgets, measured
  numbers, self-host pointer, screenshots), tag `v0.1.0`, GitHub release with
  installer artifact via CI.
  DoD: release page has installer + checksums; CI built it.
  Verify: download from release page, install, join call.

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
