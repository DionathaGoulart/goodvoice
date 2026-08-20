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
- [x] **2.2 Opus encode/decode pipeline** — `client/src-tauri/src/audio/opus.rs`.
  20 ms frames, 48 kHz, 32 kbps start; encode→decode round-trip preserves audio.
  DoD: unit tests with synthetic tones; no allocation on the frame path (assert
  with a counting allocator in test builds).
  Verify: `cargo test -p goodvoice-client opus`.
  Done out of order — it is not [WIN] and does not depend on 2.1's outcome.
  See DR-3 (crate choice) and DR-4 (CMake pin).
- [ ] **2.3 [WIN] webrtc-rs ↔ Cloudflare SFU spike** — `client/src-tauri/src/rtc/`.
  Prove webrtc-rs can complete ICE/DTLS with Realtime SFU and push an Opus track
  (PRD open question 2). This is the project's biggest unknown — if blocked,
  Decision Record + libwebrtc FFI evaluation, and STOP for user input.
  DoD: a published track from client A is pulled by a throwaway web page or second
  client. Verify: `cargo run --bin rtc-spike -- --room test` against Phase 1 deploy.
  **Server half done:** the Worker proxy DR-2 called for is in —
  `POST /rooms/:code/sfu/tracks/new`, `PUT …/renegotiate`, `PUT …/tracks/close`,
  signed with the app secret, scoped to the caller's own session, and refusing
  any track that pulls from a session outside the room (58 tests green). The
  Rust spike itself still needs a Windows host.
- [ ] **2.4 Wire join flow end-to-end** — `rtc/session.rs`, `audio/`, minimal UI
  (`client/ui`): room code input → join → publish mic → subscribe/playback peers.
  DoD: two Windows machines (or machine+VM) hold a conversation.
  Verify: manual two-client call + `cargo test` green.
- [ ] **2.5 [WIN] Latency measurement harness** — `client/src-tauri/src/bin/latency.rs`
  or in-app debug overlay. Measure mouth-to-ear latency (loopback tone timestamp
  method) across the real SFU path.
  DoD: measured number recorded in a Decision Record vs the 80 ms budget; if over
  budget, the DR lists the suspects (buffer sizes, jitter buffer depth) and next steps.
  Verify: committed measurement notes + reproducible run instructions.

## Phase 3 — Full rooms

- [ ] **3.1 N-party audio** — `rtc/`, `audio/mixer.rs`. Subscribe to up to 7 remote
  tracks, mix for playback; roster UI shows who's in the room and who's speaking.
  DoD: 4+ clients (mix of machines/VMs) converse; CPU stays in budget.
  Verify: manual multi-client session + `cargo test`.
- [ ] **3.2 Mute / deafen** — `audio/`, `ui`. Mute halts encoding+sending (packets
  stop, not zeroed — assert in test via packet counter); deafen halts playback;
  state visible in roster for everyone (signaling message).
  DoD: tests + two-client visual/audio confirmation. Verify: `cargo test` + `npx vitest run`.
- [ ] **3.3 Push-to-talk + VAD modes** — `audio/vad.rs`, `ui` settings. PTT
  (in-window key first; global hotkey is Phase 4), VAD via webrtc-audio-processing's
  voice detection with hangover time; mode persisted locally.
  DoD: both modes demonstrably gate transmission. Verify: `cargo test audio::vad` + manual.
- [ ] **3.4 AEC/NS/AGC integration** — `audio/processing.rs`. webrtc-audio-processing
  between capture and encode; loudspeaker echo cancelled (needs render-stream
  reference feed).
  DoD: speaker-echo test call shows no self-echo; DR records config chosen.
  Verify: manual echo test + `cargo test`.
- [ ] **3.5 Auto-reconnect** — `rtc/reconnect.rs`. Exponential backoff, rejoin same
  room, resubscribe all tracks; UI shows reconnecting state.
  DoD: kill network 10 s mid-call → call resumes without restart.
  Verify: scripted netdown test documented + manual run.

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
