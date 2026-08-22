# Testing the tray

goodvoice's window is not the app (plan.md task 4.1), and since task 4.6 it is
not even *there*: closing or minimising destroys the window and its webview,
and the tray icon builds a new one on the way back. The call carries on
throughout — audio lives in Rust and never depended on a webview existing.

`tray-roundtrip.ps1` checks that in about a minute.

| | scripted | by hand |
|---|---|---|
| The window goes away and the app survives | ✅ `tray-roundtrip.ps1` | |
| WebView2 goes with it, and the tree drops under 120 MB | ✅ | |
| Clicking the icon builds a new window, quickly | ✅ | |
| The new window comes back showing the call | ✅ *(screenshot)* | |
| Nothing accumulates over repeated round trips | ✅ `-Cycles 12` | |
| The tray can still quit an app with no window | ✅ | |
| The icon is where a person can find it, and no flicker | | ✅ |

## Running it

```powershell
# Build the app the drill drives. --features custom-protocol is not optional:
# see "The build that is not the app" below.
cd client\src-tauri
cargo build --release --features custom-protocol --bin goodvoice-client

powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\tray-roundtrip.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\tray-roundtrip.ps1 -Via minimise
```

Expected, and what each line means (three cycles by default; `-Cycles 12` to
lean on it harder):

```
ROOM=drill-195614            the drill joins a real room, so there is a call to lose
VISIBLE_BEFORE=True          the window came up
PROCESSES_BEFORE=7           goodvoice plus WebView2's six
TREE_MB_BEFORE=333.2         what an app with a window on screen costs
ALIVE_IN_TRAY_1=True         closing it did not end the process
WINDOW_IN_TRAY_1=False       the window handle is gone — destroyed, not hidden
PROCESSES_IN_TRAY_1=1        ...and WebView2 went with it
TREE_MB_IN_TRAY_1=33.7       which is the 120 MB budget, met with room (task 4.6)
TRAY_CLICKED_1=True          the notification-area icon was found and invoked
WINDOW_AFTER_TRAY_1=True     a new window exists
REBUILT_IN_MS_1=146          what "coming back is instant" costs now
VISIBLE_AFTER_TRAY_1=True    and it is on screen
   ... the same again for cycles 2 and 3 ...
TREE_MB_BACK=338.6           back to a normal window's cost
QUIT_CLICKED=True            right-click → the last menu item
QUIT_ENDED_IT=True           ...and the process ended, which is the whole trap
LEFTOVER=0                   nothing was left running
RESULT=PASS
```

`SHOT_BEFORE` and `SHOT_AFTER` name two PNGs. **Open `after.png`** — it is the
only check no assertion here can make: the rebuilt window has to show the room
code and the roster, not the join form. A window that comes back empty during a
live call is the failure mode task 4.6 introduced and `current_status` exists to
prevent.

The in-tray figure creeps for the first few cycles and then stops — 33.7, 34.1,
34.2, 34.3, 34.4, 34.5, 34.8, 34.7, 34.7, 34.9, 34.9, 34.9 over twelve. A cost
paid once, not per cycle. A run where it climbs to the last cycle is the
regression to chase.

## Three traps this drill is shaped around

**The build that is not the app.** `cargo build --release` on its own produces a
binary whose webview points at the Vite dev server, because `generate_context!`
only embeds `../dist` when the `custom-protocol` feature is on — the Tauri CLI
passes it, a bare cargo build does not. What you get instead is Edge's
"localhost refused to connect" inside a goodvoice window: it launches, it joins,
it minimises, and every measurement taken from it is about an error page. See
DR-22.

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

## Clicking a tray icon from a script

Windows 11's notification area is a XAML island: there is no `ToolbarWindow32`
to hit-test any more, and the old trick of finding the icon's rectangle and
synthesising a click at those coordinates finds nothing. UI Automation still
sees it. The icons are automation buttons under `Shell_TrayWnd`, and `Invoke` on
one is the left click `tray::show` is wired to.

Four things about that cost an evening between them:

- **Names have state appended.** The chevron is `Show Hidden Icons` when the
  flyout is shut and **`Show Hidden Icons Hide`** when it is open. Matched
  exactly, every click after the first lands on nothing and the drill reports a
  tray icon that has gone missing.
- **`goodvoice` is two buttons.** The tray icon, and the taskbar's `goodvoice -
  1 running window`. Clicking the second one does something else entirely.
- **Ask, then act — never both at once.** "Find the icon, and open the chevron
  if it is not there" clicks the icon *twice* whenever it is there, and see the
  next point.
- **Two `Invoke`s in a row wedge the app.** Not a bug in goodvoice: a real
  mouse double click on the same icon is fine, and `tray::show` drops a second
  Open that arrives during the first anyway. But UI Automation's programmatic
  activation, twice with nothing in between, reliably leaves the app inside
  `WebviewWindowBuilder::build` with `IsHungAppWindow` true. Click once.

The right-click menu needs a real mouse — UI Automation has no "invoke with the
other button" — and then the keyboard, because the popup it opens is a
`TrackPopupMenu` that UIA reports as a `#32768` pane **with no children at
all**. The items are on the screen and not in the tree, so there is nothing to
`Invoke`. Up with nothing selected highlights the last item, which is Quit.

## By hand, the part no script can sign off

Build and run it, then:

1. **The icon is in the notification area.** Windows hides new icons behind the
   chevron by default — drag it out before deciding it is missing.
2. **Close the window.** It vanishes; the icon stays. Task Manager's memory
   column for goodvoice drops by about 300 MB within a second or two.
3. **Left-click the icon.** The window comes back, focused, showing the room it
   was in. It is a *new* window — same size and title, scrolled to the top,
   whatever you had typed in the join form gone. That is the trade task 4.6
   made: an eighth of a second and a fresh window, for 327 MB.
4. **Watch for flicker.** The rebuild paints once, at its final size.
5. **With a call running**, close the window and keep talking. Audio does not
   pause, and the roster is still right when the window comes back.
6. **Right-click → Quit goodvoice.** The process ends *and* the room stops
   showing you in it — Quit hands the seat back before exiting (DR-5), which
   the close button no longer does because it no longer ends anything.

## The menu (task 4.2)

Right-click the icon. Out of a call it reads: **Open goodvoice**, then a greyed
**Mute**, **Deafen** and **Leave room**, then **Quit goodvoice**. Greyed is the
point — a menu offering to leave a room you are not in is a menu that lies.

Join a room from the window and go through it. Every item is checked against
the *other* half of the app, because state that is synced in one direction only
looks fine until you use the other one:

| Do this | Tray shows | Window shows | Someone else sees |
|---|---|---|---|
| Join a room | all three items live | the room panel | you arrive |
| Tray → Mute | ✔ next to Mute | the mute button lit | you go **muted** |
| Window → unmute | tick gone | button unlit | your tag clears |
| Tray → Deafen | ✔ next to Deafen | deafen button lit | you go **deafened** |
| Window → mute *and* deafen | both ticked | both lit | both tags |
| Tray → Leave room | all three greyed again | back to the join panel | you leave |
| Join again after that | live again | the room panel | you arrive |

Three of those rows exist because they used to be broken:

- **The last one.** A call that ended on its own — dropped, refused, or left
  from the tray — used to be kept in memory as if it were still running, so the
  next join was refused with "already in a call" and the only way back into a
  room was to restart the app. `push_state` lets go of it now.
- **The unmute row.** The window sets its own button on the click *and* is told
  by the event that follows. Both, so the button answers instantly and still
  ends up saying what the call actually says.
- **Every row where the window is told something.** Until DR-22 the crate had
  no capability file at all, so every `listen` was refused by the ACL — in dev
  as well as in release — and the window heard nothing: not the roster, not the
  talking dots, not a reconnect, and not the tray's own mute. Run this table
  again on any build that touches `capabilities/`.
