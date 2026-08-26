# Testing global push to talk

The talk key from task 3.3 is handled by the webview, which hears it only while
the webview is what is being typed into — which is exactly never, if someone is
playing a game. Task 4.3 watches the whole desktop instead
(`tray/hotkey.rs`, `WH_KEYBOARD_LL`).

| | scripted | by hand |
|---|---|---|
| The key arrives from another process | ✅ `hotkey.ps1` | |
| Both edges arrive, and only the edges | ✅ | |
| The key still reaches what had focus | ✅ `hotkey-fullscreen.ps1 -Windowed` | |
| It works over a fullscreen game | ✅ `hotkey-fullscreen.ps1` | |

The bottom two rows waited on "a person and a game" from task 4.3 until
2026-08-25. They did not need one — see *Over a game* below.

## Scripted

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\hotkey.ps1
```

It starts `hotkey-drill` — a process with **no window at all**, so nothing it
hears can have come from having focus — and then synthesises three presses of
F13 with `keybd_event`, which puts them in the same system input queue a real
keyboard does. F13 because no keyboard here has one: the presses cannot disturb
whatever happens to be on screen.

```
    2634 ms  down
    2889 ms  up
    ...
--- 3 presses, 3 releases ---
PASS — the key was heard from outside this process.
EXIT=0
```

`EXIT=0` only when both edges arrived. The drill runs standalone too:

```sh
cargo run -p goodvoice-harness --bin hotkey-drill                        # Space, ten seconds
cargo run -p goodvoice-harness --bin hotkey-drill -- --key KeyV --seconds 30
```

### Two things that make this script look broken

**`SendInput` from PowerShell.** The obvious way to synthesise a keystroke, and
it returns 0 with `ERROR_INVALID_PARAMETER` unless the `INPUT` struct is
marshalled exactly right — 32 bytes on x64, and PowerShell hands you a *copy*
of a nested struct field, so `$evt.ki.wVk = 0x7C` sets nothing and the call is
made with no key in it. `keybd_event` takes four scalars, goes through the same
queue, and cannot be got wrong.

**`Start-Process -PassThru`.** Its `ExitCode` comes back empty however long you
wait on it, which reads as a drill that never finished. The script starts the
process through `System.Diagnostics.Process` instead.

## Over a game

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\hotkey-fullscreen.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\hotkey-fullscreen.ps1 -Windowed
```

`hotkey.ps1` proves the hook hears a key from a process with no window. What it
cannot show is the case the feature exists for (prd.md §3 F2): a *fullscreen
exclusive* game on the screen, the key held, and the game still getting the key
it binds to a weapon.

That does not need a game. A game in fullscreen exclusive is a D3D11 swap chain
with `SetFullscreenState(TRUE)` on it, and `bin/fullscreen-drill` is one. So the
script drives the real client into push to talk on F13, joins a room with
`bin/listener` in it, puts the drill's window on the display, and holds the key
twice — and all three halves of the question are read off instruments:

```
GLOBAL=heard from anywhere, including over a game
FOREGROUND=fullscreen-drill
  game MODE=exclusive
  game DISPLAY_REFRESHES=4810 (144 Hz)
  game DOWNS=2 UPS=2
  room 0 ... 54 50 26 0 0 0 0 0 0 22 50 50 50 28 0 0 0 0 21 50 50 50 29 0 ... 0
HEARD_BURSTS=3 (one control, two over the game)
RESULT=PASS
```

- **The display really was taken.** `MODE=exclusive` is DXGI's own answer, and
  `DISPLAY_REFRESHES` is the *display's* vblank counter — `GetFrameStatistics`
  answers for a swap chain that owns an output and refuses one that does not,
  so 144 Hz there is the monitor saying who has it.
- **The key opened the microphone.** The `room` line is the roommate's
  frames-a-second column. A gated client sends no packets rather than silent
  ones (`bin/mute-drill`), so each row is either a stream or a hole: three
  bursts, exactly where the key was held.
- **And the first burst is the control.** The key is held once *before* the
  display is touched, because a room going quiet is only evidence about the
  talk key if the room was ever going to hear anything: a capture device
  another program is holding gives the same column of zeroes and blames the
  feature for the machine. A run whose control burst is missing prints
  `RESULT=INCONCLUSIVE` with the app's own stderr under it, and says nothing
  about push to talk either way.
- **The game still got the key.** `DOWNS=2 UPS=2` is the fullscreen window's
  own `WM_KEYDOWN`. If the hook swallowed the key, this is the count that would
  go to zero.

`-Windowed` runs the identical walk with the display left alone, which is the
third row of the table: the key reaches whatever has focus.

### Two things that make *this* script look broken

**A synthesised key needs a scan code.** `hotkey.ps1` passes zero and works,
because the Rust hook reads `vkCode` out of a `KBDLLHOOKSTRUCT`. The webview
does not: a DOM `KeyboardEvent.code` is derived from the *physical* scan code,
so a key injected without one arrives in the window as an empty string. The
rebind stores that as the talk key, `vk_for_code` cannot name it, the hook
never installs, and the window truthfully reports *heard only while this window
has focus*. Two failures, one missing byte; `MapVirtualKey(vk, MAPVK_VK_TO_VSC)`
is where the byte comes from.

**A talk key the window cannot name is stored anyway.** The state above
survives a restart, and the settings button then reads `key:` with nothing
after it. Nothing is broken — the window says push to talk is window-only, and
it is — but a drill matching `key: ` with the space walks away reporting that
the app has no key button at all.

## By hand

Three of the five steps below are the script above. What is left for a person
is the two ends of it: that the key still types where it is meant to, and that
the hook comes off with the call.

1. **Join a room, pick push to talk.** The window says *heard from anywhere,
   including over a game*. If it says *only while this window has focus*, the
   hook did not install — the key is still yours, but only here.
2. **Click on another window and hold the key.** You are heard. Let go: you are
   not.
3. **Type the key into a text field somewhere.** It types. goodvoice listens to
   that key; it does not take it, because a talk key a game stops seeing is a
   key nobody would bind to anything.
4. **Start a game, fullscreen, and hold the key.** Heard, and still doing
   whatever the game binds it to. `hotkey-fullscreen.ps1` is this step against
   a swap chain instead of a game; what a real game adds to it is an
   anti-cheat, which is DR-18's argument and not this one's.
5. **Leave the room.** The hook comes off with the call: out of a room,
   goodvoice is not in the keyboard's way at all. There is no way to see this
   from the outside, which is why it is worth knowing it is true.

See DR-18 for why a keyboard hook is the shape this takes, and what to do if an
anti-cheat ever objects to it.
