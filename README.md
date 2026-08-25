# goodvoice

Lightweight, open-source voice chat for Windows gamers. **Mumble-simple,
Discord-quality, zero performance cost.**

Three features. Nothing more:

1. **Voice chat** — rooms of 1–8, lowest possible latency, high audio quality
2. **System tray** — minimize and forget it while you game
3. **Screen share** — 720p/1080p, hardware-encoded, 5.6% FPS cost to a
   GPU-bound game

## Performance budgets (hard requirements)

| Metric | Budget |
|---|---|
| End-to-end voice latency | ≤ 80 ms |
| Idle CPU in a room | < 2% |
| RAM | ≤ 120 MB |
| FPS impact while sharing 1080p30 | ≤ 6% (measured: 5.6%) |
| Cold start → talking | < 3 s |

## Stack

Rust + Tauri v2 client (WASAPI, Opus, Windows.Graphics.Capture, hardware H.264,
webrtc-rs) · Cloudflare Workers + Durable Objects signaling · Cloudflare Realtime
SFU media plane. No database. No accounts. Rooms are ephemeral.

## Self-hosting

Bring your own free-tier Cloudflare account: create a Realtime app, set
`CALLS_APP_ID` and `CALLS_APP_SECRET`, `wrangler deploy`. Full guide:
[docs/self-hosting.md](docs/self-hosting.md).

## Status

Pre-alpha. Planning docs live in [.harness/](.harness/) — start with
[prd.md](.harness/prd.md) and [plan.md](.harness/plan.md).

## License

[MIT](LICENSE)
