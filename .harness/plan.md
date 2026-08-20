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
- [ ] **0.2 Worker hello world** — `server/src/index.ts`, `server/wrangler.toml`,
  `server/package.json`, `server/tsconfig.json`. Minimal Hono-less router (plain
  `fetch` handler) returning `{"ok":true}` on `GET /health`; Durable Object class
  `Room` declared and bound (empty shell).
  DoD: local dev server answers health check.
  Verify: `cd server && npm install && npx wrangler dev` + `curl localhost:8787/health`.
- [ ] **0.3 Lint/format gates** — `client/src-tauri` (clippy pedantic per
  styleguide), Prettier config for `server/` and `client/ui`.
  DoD: all four commands pass clean:
  `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `npx prettier --check .` (server and ui), `npx tsc --noEmit` (server and ui).
- [ ] **0.4 CI** — `.github/workflows/ci.yml`. On push/PR: rust fmt+clippy+test
  (windows-latest runner), server/ui prettier+tsc, `wrangler deploy --dry-run`.
  DoD: workflow file is valid and passes on push.
  Verify: `gh run watch` on the push, or `act` locally if available.
- [ ] **0.5 [WIN] Hardware risk probe (spike)** — `client/src-tauri/src/bin/probe.rs`.
  Tiny binary that (a) enumerates WASAPI render/capture devices + their shared-mode
  min buffer sizes, (b) enumerates Media Foundation H.264 encoders and flags which
  are hardware (NVENC/AMF/QuickSync). Front-loads the two hardware unknowns.
  DoD: probe output from a real Windows gaming machine pasted into a Decision
  Record in this file (devices found, min buffer ms, hw encoders found).
  Verify: `cargo run --bin probe` on Windows host.
- [ ] **0.6 README + LICENSE + .gitignore checked in** — root. README states the
  three features, perf budgets, self-host one-liner; MIT LICENSE.
  DoD: files exist, README renders. Verify: `git ls-files | grep -E 'README|LICENSE'`.

## Phase 1 — Signaling core

Goal: rooms exist server-side; a mock client can join/leave and get SFU credentials.
No Rust in this phase — Worker + DO only, integration-tested with vitest.

- [ ] **1.1 Room Durable Object: roster + cap** — `server/src/room.ts`. In-memory
  only (no storage API): participants map (id → {name, joinedAt}), join rejects at
  8 with a typed error, leave removes, last-leave resets state. WebSocket per
  participant for roster pushes. Zod-validate every inbound message.
  DoD: unit tests cover join/leave/cap/last-out-reset.
  Verify: `cd server && npx vitest run` (uses `@cloudflare/vitest-pool-workers`).
- [ ] **1.2 Worker routes** — `server/src/index.ts`. `POST /rooms/:code/join` →
  routes to DO by room code (idFromName), `GET /rooms/:code/ws` → WebSocket upgrade
  proxied to DO, `GET /health`. No other routes. CORS: `origin: *`.
  DoD: route tests pass. Verify: `npx vitest run`.
- [ ] **1.3 Realtime SFU credential exchange** — `server/src/room.ts` + new
  `server/src/sfu.ts`. On join, DO calls Cloudflare Realtime API
  (`CALLS_APP_ID`/`CALLS_APP_SECRET` secrets) to create a session; returns session
  credentials + TURN credentials to the client. Bitrate caps read from
  `MAX_VIDEO_BITRATE`/`MAX_AUDIO_BITRATE` env vars (cloudflare/meet pattern).
  DoD: mocked-API tests pass; a real `wrangler dev --remote` join returns live
  credentials. Verify: `npx vitest run` + manual curl against dev deploy.
- [ ] **1.4 Timeout cleanup + room death** — `server/src/room.ts`. Heartbeat over
  WS; missed heartbeats (e.g. 30 s) evict the participant; DO alarm as backstop;
  empty room leaves zero state behind.
  DoD: tests simulate silent disconnect → eviction → empty-room reset.
  Verify: `npx vitest run`.
- [ ] **1.5 Deploy to real Cloudflare** — `server/`. `wrangler deploy` to a test
  account; document required secrets in `wrangler.toml` comments.
  DoD: deployed URL passes health + join flow with curl/websocat script committed
  as `server/scripts/smoke.sh`. Verify: `bash server/scripts/smoke.sh <url>`.

## Phase 2 — Voice MVP (2 people)

Goal: two clients talking through the SFU. Riskiest phase — spikes first.

- [ ] **2.1 [WIN] WASAPI capture/playback spike** — `client/src-tauri/src/audio/`.
  Event-driven shared-mode capture → ring buffer → playback (loopback monitor).
  Decides `cpal` vs `wasapi` crate (PRD open question 4) — write the Decision Record.
  DoD: mic loopback audible; round-trip device latency measured and recorded in DR.
  Verify: `cargo test -p goodvoice-client audio` + manual loopback run
  (`cargo run --bin audio-spike`).
- [ ] **2.2 Opus encode/decode pipeline** — `client/src-tauri/src/audio/opus.rs`.
  20 ms frames, 48 kHz, 32 kbps start; encode→decode round-trip preserves audio.
  DoD: unit tests with synthetic tones; no allocation on the frame path (assert
  with a counting allocator in test builds).
  Verify: `cargo test -p goodvoice-client opus`.
- [ ] **2.3 [WIN] webrtc-rs ↔ Cloudflare SFU spike** — `client/src-tauri/src/rtc/`.
  Prove webrtc-rs can complete ICE/DTLS with Realtime SFU and push an Opus track
  (PRD open question 2). This is the project's biggest unknown — if blocked,
  Decision Record + libwebrtc FFI evaluation, and STOP for user input.
  DoD: a published track from client A is pulled by a throwaway web page or second
  client. Verify: `cargo run --bin rtc-spike -- --room test` against Phase 1 deploy.
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

(none yet)
