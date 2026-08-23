# Testing global push to talk

The talk key from task 3.3 is handled by the webview, which hears it only while
the webview is what is being typed into — which is exactly never, if someone is
playing a game. Task 4.3 watches the whole desktop instead
(`tray/hotkey.rs`, `WH_KEYBOARD_LL`).

| | scripted | by hand |
|---|---|---|
| The key arrives from another process | ✅ `hotkey.ps1` | |
| Both edges arrive, and only the edges | ✅ | |
| The key still reaches what had focus | | ✅ |
| It works over a fullscreen game | | ✅ |

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

## By hand

The scripted half proves the hook hears the desktop. What it cannot show is
that the desktop still hears the key, and that both hold inside a game.

1. **Join a room, pick push to talk.** The window says *heard from anywhere,
   including over a game*. If it says *only while this window has focus*, the
   hook did not install — the key is still yours, but only here.
2. **Click on another window and hold the key.** You are heard. Let go: you are
   not.
3. **Type the key into a text field somewhere.** It types. goodvoice listens to
   that key; it does not take it, because a talk key a game stops seeing is a
   key nobody would bind to anything.
4. **Start a game, fullscreen, and hold the key.** Heard, and still doing
   whatever the game binds it to. This is the DoD, and it is the one step no
   script can stand in for.
5. **Leave the room.** The hook comes off with the call: out of a room,
   goodvoice is not in the keyboard's way at all. There is no way to see this
   from the outside, which is why it is worth knowing it is true.

See DR-18 for why a keyboard hook is the shape this takes, and what to do if an
anti-cheat ever objects to it.
