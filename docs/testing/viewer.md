# Testing the screen viewer

goodvoice's viewer is a second window (plan.md task 5.4). Opening it is the
whole of subscribing to somebody's screen, closing it is the whole of giving
that up, and the call must not notice either — prd.md §3 F3 asks for opt-in
viewing and for audio that is never gated on video.

`viewer.ps1` checks that in about two minutes, with `bin/viewer-drill` at the
other end of the call.

| | scripted | by hand |
|---|---|---|
| The viewer opens onto a live share and shows it | ✅ `viewer.ps1` | |
| It shows it again on the second, third and fourth opening | ✅ | |
| The picture keeps its shape in a window of the wrong shape | ✅ *(aspect column)* | |
| Voice never dips while the window comes and goes | ✅ `bin/viewer-drill` | |
| Closing the viewer does not take the app to the tray | ✅ *(the app survives every cycle)* | |
| Both skins | ✅ `-Skin terminal`, and the default is `retro` | |
| A share of a screen that never changes still arrives | ✅ `bin/share-drill` under the sheet | |
| It looks right to a person | | ✅ *(the PNGs)* |

## Running it

```powershell
# Build the app the drill drives. --features custom-protocol is not optional:
# see tray.md's "The build that is not the app".
cd client\src-tauri
cargo build --release --features custom-protocol --bin goodvoice-client
cargo build --release -p goodvoice-harness --bin viewer-drill

powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\viewer.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\viewer.ps1 -Skin terminal
```

What one run says, four cycles by default:

```
  t+  5s  the sharer is live at 1280x720 on hardware after 0.9 s
  t+  7s  the app offers "WATCH SHARER'S SCREEN"     the roster reached the app
  t+  7s  cycle 1: the viewer is open
  t+ 14s  cycle 1: window 960x540, picture 954x534, aspect 1.787, 98% of it lit
  t+ 15s  cycle 1: the viewer is closed
   ... three more, two of them resized ...

  cycle  shape           window       picture      aspect  lit
  1      as opened       960x540      954x534      1,787   0,983
  2      as opened       960x540      954x534      1,787   0,983
  2      stretched wide  1084x381     675x375      1,8     0,612
  3      as opened       960x540      954x534      1,787   0,983
  3      squashed tall   504x661      498x282      1,766   0,42
  4      as opened       960x540      954x534      1,787   0,983

- the app was first heard at 5 s
- 101 seconds measured, 0 of them below 45 frames
- lowest second: 49 frames
```

**The aspect column is the claim.** 16:9 is 1.778. A window stretched to 2.85:1
and one squashed to 0.76:1 both show a picture at 1.77–1.80 — the letterbox
grows and the picture keeps its shape. `675 × 375` is what `object-fit:
contain` gives a 1280×720 source in a 1084×381 client area (677 wide,
arithmetically); `498 × 282` is the same in a 504×661 one (283 high).

`docs/ui/viewer-letterbox.png` is that middle row as a person sees it: the
viewer stretched to 1084×381, the sheet letterboxed inside it, and — because
the sheet is the whole monitor — the viewer showing itself showing itself.

**"lit" is how much of the window the picture covers.** 98% with no letterbox
to speak of, 61% stretched wide, 42% squashed tall. Zero means a viewer that
never got a picture, which is the failure DR-33 was about.

**The frames-a-second table is the definition of done.** A 20 ms frame path
delivers fifty a second; the drill fails the run if any second between the app
being first heard and the room emptying carries fewer than `--floor` (45).
Every run since DR-33 has been 0 dips with a lowest second of 48–50.

## Three things it is shaped around

**A screenshot of coordinates is not a screenshot of a window.** Anything that
takes the foreground while the script runs — a person using the machine — puts
its own pixels where the viewer's were. The script hit-tests the middle of the
viewer's client area before every capture and reports `not measured` rather
than measuring somebody's browser. Two runs during this task's own verification
did exactly that.

**The grey sheet is not decoration.** The aspect measurement finds where the
picture ends and the letterbox begins, which it can only do if they are
different colours. Sharing a desktop whose wallpaper is nearly black into a
window whose theme background is nearly black measures nothing — a fact about
that desktop rather than about the viewer. The sheet is also what keeps
whatever the person at the machine had on screen out of the PNGs. `-NoBackdrop`
turns it off.

**A skin renames every button, and some of them are toggles.** The terminal
skin's `text-transform: uppercase` reaches the accessible name (DR-26), so
every match here is case-insensitive. And `aria-pressed` turns a `<button>`
into a UIA *toggle* button, which does not support Invoke at all: settings, the
skin buttons, mute and deafen all need `TogglePattern` instead. A script that
only invokes finds them and then does nothing.

## The other drills

`bin/rewatch` asks the same question as cycles 2–4 without a window, a webview
or an automation tree: join, watch, unwatch, watch again, and report what each
viewer received.

```powershell
cargo run -p goodvoice-harness --bin rewatch -- --rounds 3 --seconds 6
```

It passed while the app was failing, which is what located DR-33: the transport
re-subscribes correctly, and what was broken was the window's end of it.

`bin/share-drill` (task 5.3) is worth running on a screen that genuinely never
changes — start it and then touch nothing, mouse included:

```powershell
cargo run -p goodvoice-harness --bin share-drill -- --seconds 20
```

Before DR-34 that was **0 access units in 20 seconds** — a share that published
nothing at all because WGC announces changes and there were none. It is now 11
keyframes and 69 kB, which is 3.5 kB/s for a screen that is doing nothing.

**Touching nothing is harder than it reads**, and `docs\testing\keyframe.ps1`
is what stopped relying on it: it puts this file's grey sheet over the monitor,
runs the drills hidden with their output redirected to files, and takes the
sheet down. It also runs `share-drill --no-viewer`, which counts what the
*sharer* put on the wire rather than what reached anybody — the two are the
same number at the receiving end and different questions at the sending one.

That 3.5 kB/s is what a still share costs a room with nobody in it, and DR-44
is why it is not zero: Cloudflare refuses a subscription to a track that has
never carried a packet, so the repeat is the heartbeat that keeps the share
openable, not merely a courtesy to whoever opens late.

## What a closed viewer costs the room

`bin/watch-cost` is §7.9, and it is the only instrument here that counts
*below* the socket. Every other one counts what this client read — and closing
the viewer is exactly what stops it reading (`rtc::session::reconcile_watch`),
so a sink reporting zero says nothing about whether the packets stopped or are
being thrown away. It counts through `Call::wire` (`rtc::wire`) instead:
webrtc's own transport counters, which see every datagram before anything
decides what it is, and its per-SSRC inbound counters, which see every RTP
packet the endpoint can name a track for whether or not a receiver is draining
it.

```powershell
cargo run -p goodvoice-harness --bin watch-cost -- --seconds 15
```

Three phases, and the question is whether the third looks like the first or the
second:

```
  phase             in    video    audio      out
  never opened      5.7      0.0      4.0      5.6
  open             76.4     68.9      4.1      5.6
  closed            5.6      0.0      4.0      5.6
```

**Before `tracks/close`, the third row was the second row.** Measured
2026-08-27: 62.7 of the 62.9 kB/s an open viewer was receiving still arrived
after it closed — 100% of it, for as long as the share lasted, on every client
that had ever opened one. Nothing had told Cloudflare, because giving up the
viewer aborted a playback task and did not cross the wire.

**Windows' per-process IO counters cannot see any of this**, which is what
§7.9 was waiting on an instrument for. They read 5.2 / 5.4 / 5.6 kB/s across
the same three phases — a 69 kB/s share is invisible inside them. The numbers
above are the same three phases measured where the packets actually arrive.

`close_pull` is the fix: the transceiver goes `Inactive`, this side offers, and
`tracks/close` names the mid. The Worker already proxied the operation
(`server/src/sfu.ts`) and nothing had ever called it.

**Two things the table is also watching.** `audio` is flat across all three
phases, so the close does not disturb what is being *heard*; `out` is flat too,
which is the DR-8 check — closing a pull is a renegotiation, and a
renegotiation is what once rebuilt the microphone's sender underneath the
publish loop and left the room quiet. A run where phase 3 stops sending fails
on that alone. `bin/rewatch` covers the other side of it: four
watch-close-watch cycles, every viewer got a picture, first picture 0.90–1.06 s
— an `Inactive` m-section is re-subscribable, which is why the transceiver is
not `stop()`ped.
