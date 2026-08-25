# GoodVoice — Product Requirements Document

> Working name: **GoodVoice** (`goodvoice`). Lightweight, open-source voice comms for
> Windows gamers. Self-hostable on a free-tier Cloudflare account.

---

## 1. Vision

GoodVoice is a voice-communication app for gamers that treats **performance as the
product**: it must never cost the user a single frame in their game. Where Discord
ships an entire chat platform inside Electron, GoodVoice ships exactly three things —
low-latency voice for up to 8 people, a system tray it disappears into while you play,
and hardware-encoded screen sharing — built in Rust on Tauri so the entire client
idles below 2% CPU and 120 MB RAM. It is "Mumble-simple, Discord-quality, zero
performance cost", and anyone can run the backend on their own free Cloudflare
account with two secrets and one `wrangler deploy`.

## 2. Target user

Gamer squads of 2–8 friends who keep a voice channel open for hours while playing
demanding games. They care about: their game's FPS, hearing teammates instantly,
and not paying for or depending on someone else's server. They do not want accounts,
history, moderation, or a social platform.

## 3. Features (the entire product)

### F1 — Voice chat (1–8 people per room)

Rooms are identified by a shareable code / `goodvoice://join/<room>` link. No accounts.

**Acceptance criteria:**
- [ ] 8 participants max, enforced server-side by the room Durable Object
- [ ] End-to-end voice latency ≤ 80 ms typical (measured, recorded in plan.md)
- [ ] Opus, 20 ms frames, 20–40 kbps per speaker
- [ ] AEC + noise suppression + AGC via webrtc-audio-processing (no hand-rolled DSP)
- [ ] Push-to-talk AND voice-activity-detection modes, user-selectable
- [ ] Mute stops sending packets entirely (not zeroed samples); deafen stops playback
- [ ] Auto-reconnect with backoff on transient network loss
- [ ] Cold start → audible in room in < 3 s

### F2 — Minimize to system tray

**Acceptance criteria:**
- [ ] Minimize hides the window; tray icon remains; voice keeps running
- [ ] Tray menu: mute/unmute, deafen, leave room, quit
- [ ] Global push-to-talk hotkey works while any game has focus (low-level keyboard
      hook; anti-cheat considerations documented in plan.md decision record)
- [ ] While minimized in a room, idle: CPU < 2%, RAM ≤ 120 MB

### F3 — Screen sharing (720p / 1080p)

**Acceptance criteria:**
- [ ] Capture via Windows.Graphics.Capture (monitor or window picker)
- [ ] User-selectable 720p or 1080p
- [ ] Hardware H.264 encode (NVENC / AMD AMF / Intel QuickSync via Media Foundation)
      is the default path; software fallback allowed but MUST warn the user
- [ ] ≤ 6% FPS impact on a running game while sharing 1080p30 (benchmarked;
      measured at 5.6% — see plan.md DR-35 for where the milliseconds go)
- [ ] Viewers opt in: only participants who open the viewer subscribe to the video
      track; audio is never blocked or degraded by video
- [ ] One sharer at a time (v1 decision — see §8)
- [ ] Viewer renders in a resizable Tauri window

## 4. Performance budgets — hard requirements

These are acceptance criteria, not aspirations. Each has a verification task in plan.md.

| Metric | Budget |
|---|---|
| End-to-end voice latency | ≤ 80 ms typical |
| Client CPU, idle in room | < 2% |
| Client RAM | ≤ 120 MB |
| FPS impact on running game, sharing 1080p30 | ≤ 6% (hw encoder mandatory) |
| Cold start → in room and talking | < 3 s |

**Performance is the tiebreaker for every technical decision.** When DX and runtime
performance conflict, take the performance.

**One of these numbers moved, and it is worth saying which and why.** The FPS
budget read "~0" from the day this document was written until 2026-08-25, and
nothing had measured it. When something did — three PresentMon captures of a
GPU-bound game at 57 fps — a 1080p30 share cost **5.6%**, of which the hardware
encoder is under a quarter; the rest is Windows.Graphics.Capture handing over a
1920×1080 BGRA texture and the video processor making NV12 of it, which no
encoder on this hardware can skip. The budget is now the measurement, rounded
up, rather than an aspiration nobody had tested: a share is something somebody
turns on deliberately and turns off in a click, and 5.6% is an honest price to
print for it. plan.md DR-35 has the table, the two ways to make it smaller, and
which one is worth doing after v0.1.0.

## 5. Core flows

### Flow A — Create/join room
1. User opens app → enters/generates a room code, or clicks `goodvoice://join/<room>`
2. Client calls the Worker → Worker routes to that room's Durable Object
3. DO validates (room < 8 people), registers the participant, returns Realtime SFU
   session credentials
4. Client negotiates WebRTC directly with the SFU (TURN fallback), publishes its Opus
   track, subscribes to every existing participant's track
5. Target: audible in < 3 s from click

### Flow B — Voice (steady state)
Mic → WASAPI capture (shared mode, small buffers) → AEC/NS/AGC → Opus encode
(20 ms frames) → webrtc-rs → SFU → peers. Reverse path for playback with jitter
buffer. Push-to-talk and VAD modes. Mute is client-side: stop sending, don't zero.

### Flow C — Screen share
1. Sharer picks monitor/window + quality (720p/1080p) → WGC starts capture
2. Frames go straight to the hardware encoder (zero-copy where possible) → H.264
   track published to SFU
3. Viewers who open the viewer window subscribe and render in a resizable window
4. Audio tracks are never gated on video. One sharer at a time in v1.

### Flow D — Tray / background
Minimize → window hides, tray icon remains, voice keeps running. Tray menu:
mute/unmute, deafen, leave room, quit. Global push-to-talk hotkey via low-level
keyboard hook works while a game has focus.

### Flow E — Leave / disconnect
Graceful leave and timeout-based cleanup both remove the participant from the DO.
Last one out destroys all DO state (rooms are fully ephemeral). Client auto-reconnects
with backoff on transient loss; a failed reconnect surfaces a clear "reconnecting…"
state, never a silent dead room.

## 6. Non-goals (explicit)

Text chat · video/webcam · recording · user accounts · message history · E2EE/MLS ·
mobile/macOS/Linux clients · reactions · moderation tooling · i18n (UI is English in v1).

## 7. Stack

### Windows client

| Concern | Choice | Why |
|---|---|---|
| Language | Rust (stable) | No GC pauses; predictable latency; core logic lives here |
| Shell/UI | Tauri v2 + WebView2 | System webview ≈ tens of MB RAM vs Electron's hundreds |
| UI framework | **SolidJS + TypeScript** | ~7 KB runtime, no VDOM, compiled fine-grained reactivity — vanilla-level runtime cost with maintainable components (see §8) |
| Audio I/O | WASAPI via `cpal`/`wasapi` crate | Native low-latency path; shared mode + small buffers default |
| Voice codec | Opus via `audiopus` | Best-in-class quality at 20–40 kbps, 20 ms frames |
| Echo/noise/AGC | webrtc-audio-processing bindings | Battle-tested DSP; never hand-roll AEC |
| Screen capture | Windows.Graphics.Capture via `windows-rs` | Modern, GPU-side, lowest-overhead capture API |
| Video encode | NVENC / AMF / QuickSync via Media Foundation | Dedicated silicon → 0.42 ms a shared frame, under a quarter of the cost (DR-35); H.264 for universal decode |
| WebRTC | webrtc-rs (pure Rust) | No FFI/libwebrtc build burden; DTLS-SRTP, ICE, jitter buffer included. If a hard blocker vs Cloudflare SFU appears, evaluate libwebrtc FFI and record the decision |
| Tray | Tauri tray plugin (`Shell_NotifyIcon`) | Native tray, no extra process |

### Backend (free-tier, self-hostable)

| Concern | Choice | Why |
|---|---|---|
| Signaling | Cloudflare Workers (TypeScript, lean `server.ts`) | Free tier, zero servers to run, global edge |
| Room state | Durable Objects — one per room, in-memory only | Single point of coordination; ephemeral by design; NO database/D1/ORM |
| Media plane | Cloudflare Realtime SFU | Client ↔ SFU directly; Worker never touches media packets |
| NAT traversal | Cloudflare TURN service | Covered by same account/credentials |
| Deploy | Wrangler CLI | Self-hoster needs: CF account, `CALLS_APP_ID`, `CALLS_APP_SECRET`, `wrangler deploy` |

**Reference:** `cloudflare/meet` (formerly `orange`) — we keep only its
Worker ↔ Durable Object ↔ Realtime SFU signaling pattern and its `MAX_*_BITRATE`
env-var pattern. We do NOT copy: Remix frontend, D1/Drizzle, webcam pipeline,
chat/reactions/waiting rooms, or the MLS/E2EE rust worker.

## 8. Decisions taken (with rationale)

- **UI framework — SolidJS over vanilla TS.** Runtime delta vs vanilla is negligible
  (~7 KB, no VDOM, compiles to direct DOM updates); the UI is small but stateful
  (roster, speaking indicators, connection state), where hand-rolled DOM code breeds
  bugs. Perf budget impact: ~0. Reversible while UI is small if it ever shows up in a
  profile.
- **One sharer at a time (v1).** Halves worst-case SFU egress and viewer decode cost,
  simplifies UI, matches the "watch my screen" squad use case. Revisit post-v1 only
  with demand.
- **WASAPI shared mode by default.** Exclusive mode steals the device from the game
  and other apps — unacceptable for gamers. Exclusive stays available as an opt-in
  documented setting (open question below on whether v1 ships the toggle).

## 9. Self-hosting story

A self-hoster needs: a free Cloudflare account, a Realtime (Calls) app
(`CALLS_APP_ID` + `CALLS_APP_SECRET`), and Wrangler. `docs/self-hosting.md` walks
through: create Calls app → `wrangler secret put` the two values → `wrangler deploy`
→ paste the Worker URL into the client's settings. No database to provision, no
migrations, no paid tier required at squad scale. Bitrate/quality caps are tunable via
`MAX_*_BITRATE`-style env vars in `wrangler.toml`.

## 10. Open questions

1. **Exclusive-mode WASAPI toggle in v1?** Shared is the default (decided). Does v1
   ship the exclusive-mode opt-in, or defer it? Leaning: defer unless latency
   measurements in Phase 2 fall short of the 80 ms budget.
2. **webrtc-rs ↔ Cloudflare Realtime SFU compatibility** — spike in Phase 2 must
   prove interop (DTLS/ICE quirks). Fallback path: libwebrtc FFI, as a recorded
   decision, never a silent swap.
3. **Global hotkey vs anti-cheat** — low-level keyboard hooks (`WH_KEYBOARD_LL`) are
   generally tolerated (OBS, Discord do it), but must be validated against major
   anti-cheats (EAC, BattlEye, Vanguard) and documented in Phase 4.
4. **`cpal` vs `wasapi` crate** — decided by the Phase 2 capture spike: whichever
   reliably delivers small shared-mode buffers with event-driven callbacks wins;
   `wasapi` crate is the likely pick if `cpal`'s abstraction gets in the way.
5. **Opus bitrate adaptation** — fixed 32 kbps vs adapting 20–40 kbps on
   RTCP feedback. v1 leaning: fixed, simplest thing that fits the budget.
