# Testing the tray

goodvoice's window is not the app (plan.md task 4.1), and since task 4.6 it is
not even *there*: closing or minimising destroys the window and its webview,
and the tray icon builds a new one on the way back. The call carries on
throughout — audio lives in Rust and never depended on a webview existing.

`tray-roundtrip.ps1` checks that in about a minute, `tray-flicker.ps1` checks
what it looks like while it happens, and `tray-menu.ps1` walks what the menu
offers while the window is gone.

| | scripted | by hand |
|---|---|---|
| The window goes away and the app survives | ✅ `tray-roundtrip.ps1` | |
| WebView2 goes with it, and the tree drops under 120 MB | ✅ | |
| Clicking the icon builds a new window, quickly | ✅ | |
| The new window comes back showing the call | ✅ *(screenshot)* | |
| Nothing accumulates over repeated round trips | ✅ `-Cycles 12` | |
| The tray can still quit an app with no window | ✅ | |
| It does not flicker on the way back | ✅ `tray-flicker.ps1` | |
| The menu's seven rows, against the window and a roommate | ✅ `tray-menu.ps1` | |
| The icon is where a person can find it | | ✅ |

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
HANDLE_IN_MS_1=2025          a window handle exists...
REBUILT_IN_MS_1=2434         ...and it is on screen. Both include the instrument
VISIBLE_AFTER_TRAY_1=True    still there three seconds later
   ... the same again for cycles 2 and 3 ...
TREE_MB_BACK=338.6           back to a normal window's cost
QUIT_CLICKED=True            right-click → the last menu item
QUIT_ENDED_IT=True           ...and the process ended, which is the whole trap
LEFTOVER=0                   nothing was left running
RESULT=PASS
```

**Neither of those two milliseconds figures is the rebuild.** The stopwatch
goes round `InvokePattern.Invoke()` on the notification-area icon, and that
call does not return for about **two seconds** on this desktop — the window is
already back when it does. They are upper bounds that include the instrument,
kept because a wild one still means something went wrong. Their *difference*,
~400 ms, is real, and it is the handle existing versus somebody seeing the
window: since DR-38 the window is built hidden and shows itself once the
webview has painted. The rebuild's own figure is `tray-flicker.ps1`'s
`GEOM_VISIBLE_AT_MS`, which clicks with a real mouse and reads **427 ms**.

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

And two more, both learned by chasing the app for something Windows was doing:

- **`Invoke` does not come back.** On this icon `InvokePattern.Invoke()` holds
  for **2008 ms**, measured, while the shell finishes with it — and the window
  is already rebuilt when it returns. Nothing is wrong; it is just not
  something to put a stopwatch around. `tray-flicker.ps1` uses a real mouse for
  that reason, and `tray-roundtrip.ps1` labels its two figures as bounds.
- **`SetCursorPos` returns false when an *elevated* window holds the
  foreground.** Windows refuses injected pointer movement then, so every step
  that needs a real mouse fails — and it fails looking exactly like a tray menu
  that does not work. Two runs blamed the app before the return value was
  checked; it is checked now, and `tray-menu.ps1`'s `POINTER=` line names the
  program rather than guessing at it.
  **This was diagnosed wrong twice, as "somebody is at the machine".** It is
  not that: measured on a desktop idle for 31 minutes, injection was still
  refused, and the reason was an elevated Discord holding the foreground with
  an invisible window. The rule is UIPI — a medium-integrity process may not
  inject into a desktop whose foreground window belongs to something higher —
  and the foreground window is a property of the *desktop*, so one elevated app
  anywhere on screen blocks every drill here. DR-39 has the measurements and
  the two things that do **not** fix it. What does: click any ordinary window,
  or close the elevated one.

## Watching the rebuild, frame by frame

`tray-flicker.ps1` answers plan.md §7.3, the one row here no counter could
reach: **does the window flash on the way back?** It did — 394 ms of flat
white — and since DR-38 it does not.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File docs	esting	ray-flicker.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File docs	esting	ray-flicker.ps1 -Cycles 5
```

It clicks the tray icon and photographs the region the window arrives in at
about **140 frames a second**, against a screen that changes 60 times a second,
so anything a person could have seen is in at least two frames. That rate is
why the capture loop is C# and not PowerShell: a rebuild is ~400 ms, and
PowerShell's per-iteration cost spends the chances. `FRAME_RATE` is printed
first for exactly that reason — below 60 the rest of the run is inadmissible.

Each frame is scored over the window's own area, inset by twelve so the drop
shadow and the invisible resize border are nobody's evidence:

```
DESKTOP_FRAMES_1=56       frames that match the bare desktop
SETTLED_FRAMES_1=226      frames that match the window as it ends up
BETWEEN_FRAMES_1=0        frames that are neither — the interesting ones
FLAT_FILL_FRAMES_1=0      ...of which these are a solid colour, i.e. a flash
MONOTONIC_1=last_desktop=55 first_settled=56
```

`BETWEEN_FRAMES=0` is the answer: the window goes from not-there to finished in
one frame. Before DR-38 it was 55 frames, every one of them luma 248 with 3% of
its pixels differing from that — a white rectangle — for 394 ms.

`GEOM_VISIBLE_AT_MS` from the first pass is the honest "how long until the
window is back" figure: **427 ms**, clicked with a real mouse, from a run that
also confirms the window does not resize or move after somebody could see it.

`FILMSTRIP_n` names a PNG of the transition, frame by frame with the
millisecond on each. It stays in `%TEMP%` rather than in the repo: it is a
photograph of the desktop the drill ran on, and whatever else was on it.

### The window walks

`WALK` prints the position of every window the run was given:

```
WALK=104,104 -> 208,208 -> 52,52 -> 130,130
```

**It is a different place every time.** `tauri.conf.json` gives the window a
size and no position, so Windows picks one and cascades from the last. A person
who puts goodvoice where they want it and closes it to the tray does not get it
back there. Nobody had noticed because no drill had compared two rebuilds'
rectangles. plan.md §7.12 owns it; it is also why this drill cannot be pointed
at a fixed region and instead waits for the handle to exist — a few
milliseconds, long before the first paint — and reads the rectangle off it.

## By hand, the part no script can sign off

Build and run it, then:

1. **The icon is in the notification area.** Windows hides new icons behind the
   chevron by default — drag it out before deciding it is missing.
2. **Close the window.** It vanishes; the icon stays. Task Manager's memory
   column for goodvoice drops by about 300 MB within a second or two.
3. **Left-click the icon.** Nothing happens for about four tenths of a second,
   and then the window is there, focused, complete, showing the room it was in.
   It is a *new* window — same size and title, scrolled to the top, whatever you
   had typed in the join form gone, and **not where you left it** (see *the
   window walks* above). That is the trade task 4.6 made: 427 ms and a fresh
   window, for 327 MB.
4. **Watch for flicker.** There is none, and `tray-flicker.ps1` above is the
   reason to believe that rather than this paragraph. There was: 394 ms of
   white, on every trip back, until DR-38.
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

`tray-menu.ps1` walks this table (plan.md §7.2), and **it passes** — every row,
every column, against the live deploy. It needs a desktop that will accept
injected input; if it will not, the drill stops at `POINTER=` and says which
program is in the way rather than blaming the app (see the `SetCursorPos` note
above, and DR-39). The three columns come from three instruments, none of which
is a pair of eyes:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\tray-menu.ps1
```

- **Tray shows** — the real `HMENU`. The popup answers `MN_GETHMENU` with the
  menu handle it is drawing, and a menu handle is a USER object rather than a
  pointer, so `GetMenuItemInfo` reads text, `MFS_CHECKED` and `MFS_GRAYED` out
  of it from another process. That is the tick and the greying, measured — the
  one thing the `#32768`-with-no-children trap above had made unreadable. Each
  opening is also photographed to `MENU_SHOT_*`, because a menu that is right
  in its own handle and wrong on the screen is a thing that could happen.
- **Window shows** — UI Automation. The buttons are named for what they will do
  next, so `unmute` in the tree *is* "the mute button is lit".
- **Someone else sees** — `bin/listener` in the same room, which since §7.2
  prints a `roster @ Ns` line whenever the room's flags change:
  `roster @ 7s   roommate (you) | coldstart muted`. Flags only, so a level
  meter moving does not print and a mute does.

| Do this | Tray shows | Window shows | Someone else sees |
|---|---|---|---|
| Join a room | all three items live | the room panel | you arrive |
| Tray → Mute | ✔ next to Mute | the mute button lit | you go **muted** |
| Window → unmute | tick gone | button unlit | your tag clears |
| Tray → Deafen | ✔ next to Deafen | deafen button lit | you go **deafened** |
| Window → mute *and* deafen | both ticked | both lit | both tags |
| Tray → Leave room | all three greyed again | back to the join panel | you leave |
| Join again after that | live again | the room panel | you arrive |

What that reads like when it passes — the tray column, straight out of the
menu's own handle:

```text
MENU_ROW1_JOINED=Open goodvoice | -- | Mute | Deafen | Leave room | -- | Quit goodvoice
MENU_ROW2_AFTER =Open goodvoice | -- | [x] Mute | Deafen | Leave room | -- | Quit goodvoice
MENU_ROW5_AFTER =Open goodvoice | -- | [x] Mute | [x] Deafen | Leave room | -- | Quit goodvoice
MENU_ROW6_AFTER =Open goodvoice | -- | Mute (grey) | Deafen (grey) | Leave room (grey) | -- | Quit goodvoice
ROW5_ROOMMATE=coldstart muted deafened | roommate (you)
RESULT=PASS
```

**Two things worth knowing before reading a failure.**

A tray → Leave clears the ticks as well as greying the items: `ROW6_AFTER`
shows three grey items and no `[x]`, from a call that was muted *and* deafened
when it ended. The flags belong to the call, and the call is gone.

And a client that arrived by `GOODVOICE_AUTOJOIN` has **an empty join form**:
`room` is a signal that starts empty and autojoin never touches it, so after a
tray → Leave the field is blank and `join` is disabled. The last row of the
table has to *type* a room code, and a drill that only clicked the button read
a disabled button as the app refusing to re-join — which is the exact shape of
the bug that row exists to catch, arriving from the drill instead.

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
