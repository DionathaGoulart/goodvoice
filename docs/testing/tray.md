# Testing minimize-to-tray

goodvoice's window is not the app (plan.md task 4.1). Closing it or minimising
it hides it into the notification area, and the call carries on: audio lives in
Rust and never depended on a webview being visible.

Three of the four things that has to do can be checked without a person, and
the fourth cannot be checked without one.

| | scripted | by hand |
|---|---|---|
| Close hides the window, app survives | ✅ `close-to-tray.ps1` | |
| Minimise hides the window, app survives | ✅ `minimise-to-tray.ps1` | |
| A hidden window can be restored | ✅ (Windows' own restore) | |
| The icon is *there*, a click brings the window back, no flicker | | ✅ |

Both scripts are Windows-only and expect a debug build at
`%CARGO_TARGET_DIR%\debug\goodvoice-client.exe`. Adjust `$exe` if yours is
elsewhere.

## Two traps these scripts are shaped around

**Find the window by class, not by `MainWindowHandle`.** A debug build is a
console application, so the process owns a console window too, and .NET will
happily hand you that one. Closing it kills the process without Tauri seeing
anything — which looks exactly like close-to-tray being broken. The window that
matters has the class `Tauri Window`.

**Give the app eight seconds first.** A `WM_CLOSE` in the first second or two of
the process' life is handled by Windows rather than by Tauri, and the app exits.
Nobody can click a close button on a window they have not seen yet, so this is
a property of the test rather than a bug — but a test that does not wait
measures the wrong thing, twice, and then reports it confidently.

## Close

```powershell
# client/src-tauri, then:
powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\close-to-tray.ps1
```

Expected, and what each line means:

```
VISIBLE_BEFORE=True          the window came up
ALIVE_AFTER_CLOSE=True       closing it did not end the process
VISIBLE_AFTER_CLOSE=False    ...because the window was hidden instead
ICONIC_WHILE_HIDDEN=False    hidden, not minimised — which is what makes the restore cheap
VISIBLE_AFTER_RESTORE=True   un-minimise + show brings it back, the sequence `tray::show` uses
LEFTOVER=0                   nothing was left running
```

## Minimise

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\minimise-to-tray.ps1
```

```
VISIBLE_BEFORE=True
ALIVE_AFTER_MINIMISE=True
VISIBLE_AFTER_MINIMISE=False    it left the taskbar for the tray
LEFTOVER=0
```

## By hand, the part no script can sign off

Build and run it, then:

1. **The icon is in the notification area.** Windows hides new icons behind the
   chevron by default — drag it out before deciding it is missing.
2. **Close the window.** It vanishes; the icon stays.
3. **Left-click the icon.** The window comes back where it was, focused, with
   the same room still on screen — the webview was hidden, not destroyed.
4. **Watch for flicker.** Nothing should flash on the way back: `tray::show`
   un-minimises while the window is still hidden, so the restore animation
   happens where nobody can see it.
5. **With a call running**, close the window and keep talking. Audio does not
   pause, and the roster is still right when the window comes back.
6. **Right-click → Quit goodvoice.** The process ends *and* the room stops
   showing you in it — Quit hands the seat back before exiting (DR-5), which
   the close button no longer does because it no longer ends anything.
