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
- [ ] **0.5 [WIN] Hardware risk probe (spike)** — `client/src-tauri/src/bin/probe.rs`.
  Tiny binary that (a) enumerates WASAPI render/capture devices + their shared-mode
  min buffer sizes, (b) enumerates Media Foundation H.264 encoders and flags which
  are hardware (NVENC/AMF/QuickSync). Front-loads the two hardware unknowns.
  DoD: probe output from a real Windows gaming machine pasted into a Decision
  Record in this file (devices found, min buffer ms, hw encoders found).
  Verify: `cargo run --bin probe` on Windows host.
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

- [ ] **2.1 [WIN] WASAPI capture/playback spike** — `client/src-tauri/src/audio/`.
  Event-driven shared-mode capture → ring buffer → playback (loopback monitor).
  Decides `cpal` vs `wasapi` crate (PRD open question 4) — write the Decision Record.
  DoD: mic loopback audible; round-trip device latency measured and recorded in DR.
  Verify: `cargo test -p goodvoice-client audio` + manual loopback run
  (`cargo run --bin audio-spike`).
  **Narrowed by 2.4.** The seam is in (`audio/device.rs`) with a working cpal
  backend behind it (`audio/hardware.rs`), so this task is no longer "build the
  audio layer" — it is the measurement it was always about: does cpal's WASAPI
  backend hit the 80 ms budget, or does the `wasapi` crate's control over
  shared-mode buffer sizes buy enough to justify a second backend? Whatever wins
  lands beside `hardware.rs` and nothing above the seam moves. See DR-8.
- [x] **2.2 Opus encode/decode pipeline** — `client/src-tauri/src/audio/opus.rs`.
  20 ms frames, 48 kHz, 32 kbps start; encode→decode round-trip preserves audio.
  DoD: unit tests with synthetic tones; no allocation on the frame path (assert
  with a counting allocator in test builds).
  Verify: `cargo test -p goodvoice-client opus`.
  Done out of order — it is not [WIN] and does not depend on 2.1's outcome.
  See DR-3 (crate choice) and DR-4 (CMake pin).
- [x] **2.3 webrtc-rs ↔ Cloudflare SFU spike** — `client/src-tauri/src/bin/rtc-spike.rs`.
  Prove webrtc-rs can complete ICE/DTLS with Realtime SFU and push an Opus track
  (PRD open question 2). This is the project's biggest unknown — if blocked,
  Decision Record + libwebrtc FFI evaluation, and STOP for user input.
  DoD: a published track from client A is pulled by a throwaway web page or second
  client. Verify: `cargo run --bin rtc-spike -- --room test` against Phase 1 deploy.
  **Server half:** the Worker proxy DR-2 called for is in —
  `POST /rooms/:code/sfu/tracks/new`, `PUT …/renegotiate`, `PUT …/tracks/close`,
  signed with the app secret, scoped to the caller's own session, and refusing
  any track that pulls from a session outside the room (58 tests green).
  **Client half:** the spike runs both ends in one process — a speaker publishes
  a 440 Hz tone as `mic`, a listener finds it on the roster, pulls it, and
  decodes it back. It passed against the live deploy on the first run, and the
  `[WIN]` tag it used to carry was wrong: nothing on this path is
  Windows-specific. See DR-7.
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
- [ ] **2.5 [WIN] Latency measurement harness** — `client/src-tauri/src/bin/latency.rs`
  or in-app debug overlay. Measure mouth-to-ear latency (loopback tone timestamp
  method) across the real SFU path.
  DoD: measured number recorded in a Decision Record vs the 80 ms budget; if over
  budget, the DR lists the suspects (buffer sizes, jitter buffer depth) and next steps.
  Verify: committed measurement notes + reproducible run instructions.

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
- [ ] **3.2 Mute / deafen** — `audio/`, `ui`. Mute halts encoding+sending (packets
  stop, not zeroed — assert in test via packet counter); deafen halts playback;
  state visible in roster for everyone (signaling message).
  DoD: tests + two-client visual/audio confirmation. Verify: `cargo test` + `npx vitest run`.
  Task 2.4 shipped the mechanism; what was missing was the proof. `cargo test`
  covers the client half (packets stop rather than going silent, and start
  again on unmute), and `server/test/presence.test.ts` covers the room's half:
  both flags reach everyone else's roster, they stack, they survive a late
  joiner, and a message the room cannot parse moves nothing.
  **Left for this task:** the two-client audio confirmation, by ear.
- [ ] **3.3 Push-to-talk + VAD modes** — `audio/vad.rs`, `ui` settings. PTT
  (in-window key first; global hotkey is Phase 4), VAD via webrtc-audio-processing's
  voice detection with hangover time; mode persisted locally.
  DoD: both modes demonstrably gate transmission. Verify: `cargo test audio::vad` + manual.
- [ ] **3.4 AEC/NS/AGC integration** — `audio/processing.rs`. webrtc-audio-processing
  between capture and encode; loudspeaker echo cancelled (needs render-stream
  reference feed).
  DoD: speaker-echo test call shows no self-echo; DR records config chosen.
  Verify: manual echo test + `cargo test`.
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
  covered, and both were seen happening in the drill's output. See DR-9.
  **Not verified:** the netdown run itself, on either host. The drill kills the
  session; only pulling the network checks that the client *notices* — see
  docs/testing/reconnect.md.

## Phase 4 — Tray & polish

- [ ] **4.1 Minimize-to-tray** — `tray/`, Tauri config. Close/minimize hides window,
  tray icon persists, voice continues; restore on click.
  DoD: manual flow works; no window flicker. Verify: manual + `cargo clippy` clean.
- [ ] **4.2 Tray menu** — `tray/menu.rs`. Mute/unmute, deafen, leave room, quit —
  all functional and state-synced with UI.
  DoD: each item verified against in-room state. Verify: manual checklist in PR.
- [ ] **4.3 [WIN] Global push-to-talk hotkey** — `tray/hotkey.rs`. Low-level
  keyboard hook (`WH_KEYBOARD_LL`); works while a fullscreen game has focus.
  Write the anti-cheat Decision Record (EAC/BattlEye/Vanguard stance, PRD open q3).
  DoD: PTT works over a running game; DR committed.
  Verify: manual in-game test.
- [ ] **4.4 [WIN] Cold-start budget** — measure app-launch → audible-in-room; must
  be <3 s. Optimize (lazy UI, parallel join+audio-init) until it is.
  DoD: measurement in DR, budget met. Verify: scripted timing run, 5-run median.
- [ ] **4.5 [WIN] Idle CPU/RAM budget verification** — 30-min idle-in-room soak:
  CPU <2%, RAM ≤120 MB.
  DoD: numbers in DR, budgets met (or DR explains the gap + fix tasks added).
  Verify: soak script + Task Manager/ETW capture committed to `docs/perf/`.

## Phase 5 — Screen share

- [ ] **5.1 [WIN] WGC capture spike** — `capture/wgc.rs`. Enumerate
  monitors/windows, capture frames via Windows.Graphics.Capture, report fps/format.
  DoD: spike bin dumps N frames + timing stats; DR records surface format and
  frame-pool behavior. Verify: `cargo run --bin capture-spike`.
- [ ] **5.2 [WIN] Hardware encode paths** — `capture/encoder.rs`. Media Foundation
  H.264: NVENC, AMF, QuickSync; pick first available hw MFT; zero-copy
  (GPU texture → encoder) where possible; software fallback flagged to caller.
  DoD: encoded bitstream plays in a standard player from at least one hw path
  (per 0.5 probe results); fallback path warns.
  Verify: `cargo run --bin capture-spike -- --encode` + committed sample analysis.
- [ ] **5.3 720p/1080p selection + publish** — `capture/`, `rtc/`, `ui`. Picker UI
  (monitor/window + quality), scale in encoder, publish H.264 track to SFU;
  server enforces one-sharer-at-a-time (DO rejects second share).
  DoD: share visible to a second client; `npx vitest run` covers the DO rule.
  Verify: manual two-client share + tests.
- [ ] **5.4 Viewer window** — `ui`, new Tauri window. Opt-in subscribe on open,
  unsubscribe on close, resizable, aspect-correct; audio unaffected throughout.
  DoD: open/close viewer repeatedly during live share, voice never glitches.
  Verify: manual + `npx tsc --noEmit`.
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
