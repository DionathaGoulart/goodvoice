# plan.md 7.2: the seven rows of tray.md's "The menu", walked by a script.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\tray-menu.ps1
#   ... -Exe path\to\goodvoice-client.exe
#   ... -Room squad-42          # any room nobody else is in
#
# The table checks every tray item against the *other* half of the app and
# against a roommate, because state synced in one direction only looks fine
# until you use the other one. Three columns, three instruments:
#
#   Tray shows          the real HMENU behind the popup — see "Reading a menu
#                       nobody can see" below. Text, ticks and greying, as the
#                       menu itself holds them.
#   Window shows        UI Automation on the webview. The buttons are named for
#                       what they will do next ("mute" / "unmute"), so their
#                       names *are* the window's state.
#   Someone else sees   `bin/listener` in the same room, whose roster lines say
#                       who it can see and what they are.
#
# Needs a RELEASE build with the `custom-protocol` feature — see tray.md. And
# it needs a desktop that will accept injected input: read POINTER= in the
# output before believing anything else, and "When the pointer is refused"
# below when it says no.
[CmdletBinding()]
param(
  [string] $Exe = "$env:CARGO_TARGET_DIR\release\goodvoice-client.exe",
  [string] $ListenerExe = "$env:CARGO_TARGET_DIR\release\listener.exe",
  [string] $Room = ('tray-' + (Get-Random -Minimum 10000 -Maximum 99999)),
  [string] $Out = "$env:TEMP\goodvoice-tray-menu"
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing

# ---------------------------------------------------------------------------
# Reading a menu nobody can see
#
# The tray's right-click menu is a `TrackPopupMenu`, which UI Automation
# reports as a `#32768` pane **with no children at all** (tray.md). The items
# are on the screen and not in the tree, so for a long time the only evidence
# anyone could take of a tick was a photograph.
#
# There is a better way, and it is what makes this drill possible: the popup
# window answers `MN_GETHMENU` with the `HMENU` it is drawing. A menu handle is
# a USER object rather than a pointer, so `GetMenuItemCount` and
# `GetMenuItemInfo` read it from here even though goodvoice owns it — text,
# `MFS_CHECKED`, `MFS_GRAYED`, separators and all. That is every column tray.md
# asks about, measured instead of looked at.
#
# The screenshot is still taken (`MENU_SHOT_*`), because a menu that is right
# in its own handle and wrong on the screen is a thing that could happen and
# nothing here would catch it.
# ---------------------------------------------------------------------------
Add-Type -Namespace Gv -Name Menu -MemberDefinition @'
[DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr hWnd, uint msg, IntPtr wp, IntPtr lp);
[DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr menu);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool GetMenuItemInfo(IntPtr menu, uint item, bool byPosition, ref MENUITEMINFO info);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool IsWindow(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int cmd);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
[DllImport("user32.dll")] public static extern int GetClassName(IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr hWnd, out int pid);
[DllImport("user32.dll", SetLastError=true)] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
[DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
[DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
[DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
[DllImport("advapi32.dll", SetLastError=true)] public static extern bool OpenProcessToken(IntPtr proc, uint access, out IntPtr token);
[DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr hWnd, int attr, out RECT r, int size);
public struct RECT { public int L, T, R, B; }
public struct POINT { public int X, Y; }
[StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
public struct MENUITEMINFO {
  public uint cbSize, fMask, fType, fState;
  public uint wID;
  public IntPtr hSubMenu, hbmpChecked, hbmpUnchecked;
  public IntPtr dwItemData;
  public IntPtr dwTypeData;
  public uint cch;
  public IntPtr hbmpItem;
}
public delegate bool EnumProc(IntPtr hWnd, IntPtr lp);
'@

if (-not (Test-Path $Exe)) { $Exe = 'client\src-tauri\target\release\goodvoice-client.exe' }
if (-not (Test-Path $Exe)) { Write-Output "NO_EXE=$Exe"; exit 2 }
if (-not (Test-Path $ListenerExe)) { $ListenerExe = 'client\src-tauri\target\release\listener.exe' }
if (-not (Test-Path $ListenerExe)) { Write-Output "NO_LISTENER=$ListenerExe"; exit 2 }
New-Item -ItemType Directory -Force -Path $Out | Out-Null

$script:ok = $true
$script:t0 = Get-Date
function Say([string] $line) {
  Write-Output ("  t+{0,3}s  {1}" -f [int]((Get-Date) - $script:t0).TotalSeconds, $line)
}
function Check([string] $label, [bool] $pass, [string] $value) {
  Write-Output ("{0}={1}{2}" -f $label, $value, $(if ($pass) { '' } else { '   <-- NOT WHAT THE TABLE SAYS' }))
  if (-not $pass) { $script:ok = $false }
}

# ---------------------------------------------------------------------------
# When the pointer is refused
#
# Every step here that opens the tray menu, and every click on a webview
# button, is a synthesised mouse — UI Automation has no "invoke with the other
# button", and a WebView2 acts on input rather than on automation patterns
# (DR-26, and 7.4's note in tray.md). So the whole drill rests on Windows
# accepting injected input from this process, and there is one common reason it
# will not: **UIPI**. A process at medium integrity may not inject into a
# desktop whose foreground window belongs to an *elevated* process — and since
# the foreground window is a property of the desktop rather than of goodvoice,
# one elevated app anywhere on screen turns every step below into a failure
# that looks exactly like a broken tray menu.
#
# It is worth naming the culprit rather than reporting "the desktop is in use",
# which is what this said before and is not the rule: the machine can be idle
# for half an hour and still refuse. `OpenProcessToken` on the foreground
# process is the test — access denied from a medium-integrity caller means the
# owner is higher than medium.
#
# The fix is a click: put any ordinary window in front, or close the elevated
# one. Running this drill elevated works too, and measures a slightly different
# desktop than the one a person uses.
function Get-PointerVerdict {
  $here = New-Object Gv.Menu+POINT
  [Gv.Menu]::GetCursorPos([ref] $here) | Out-Null
  if ([Gv.Menu]::SetCursorPos($here.X, $here.Y)) { return 'ok' }

  $fg = [Gv.Menu]::GetForegroundWindow()
  $owner = 0
  [Gv.Menu]::GetWindowThreadProcessId($fg, [ref] $owner) | Out-Null
  $name = try { (Get-Process -Id $owner -EA Stop).Name } catch { "pid $owner" }

  # PROCESS_QUERY_LIMITED_INFORMATION is granted across integrity levels;
  # TOKEN_QUERY on the handle it returns is not.
  $proc = [Gv.Menu]::OpenProcess(0x1000, $false, $owner)
  if ($proc -eq [IntPtr]::Zero) { return "refused (foreground is $name, which will not open)" }
  $token = [IntPtr]::Zero
  $opened = [Gv.Menu]::OpenProcessToken($proc, 0x0008, [ref] $token)
  $why = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
  if ($token -ne [IntPtr]::Zero) { [Gv.Menu]::CloseHandle($token) | Out-Null }
  [Gv.Menu]::CloseHandle($proc) | Out-Null
  if (-not $opened -and $why -eq 5) {
    return "refused (UIPI: $name holds the foreground and is elevated)"
  }
  return "refused (foreground is $name; not an integrity level this can explain)"
}

# --- the window ------------------------------------------------------------

# By window class, not by MainWindowHandle: a debug build owns a console window
# too and .NET will hand you that one (tray.md).
function Find-Window([int] $Owner) {
  $script:hit = [IntPtr]::Zero
  $cb = [Gv.Menu+EnumProc] {
    param($h, $l)
    $who = 0
    [Gv.Menu]::GetWindowThreadProcessId($h, [ref] $who) | Out-Null
    if ($who -eq $Owner) {
      $c = New-Object System.Text.StringBuilder 256
      [Gv.Menu]::GetClassName($h, $c, 256) | Out-Null
      if ($c.ToString() -eq 'Tauri Window') { $script:hit = $h; return $false }
    }
    return $true
  }
  [Gv.Menu]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
  return $script:hit
}

$uiaAny = [System.Windows.Automation.Condition]::TrueCondition

# Every name the window is currently showing. Walked twice, because WebView2
# builds its accessibility tree lazily and the first walk is what wakes it
# (DR-26).
function Window-Names {
  $h = Find-Window $script:app.Id
  if ($h -eq [IntPtr]::Zero) { return @() }
  try {
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($h)
    $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny) | Out-Null
    Start-Sleep -Milliseconds 700
    return @($root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny) |
      ForEach-Object { $_.Current.Name } | Where-Object { $_ })
  } catch { return @() }
}

function Has([string[]] $names, [string] $pattern) {
  return @($names | Where-Object { $_ -imatch $pattern }).Count -gt 0
}

# Waits for the window to say something, rather than for a fixed number of
# seconds. An assertion taken too early reads the screen before the event that
# changes it has arrived, and blames the app for the drill's impatience — the
# mistake that made invite.ps1 call a working link broken (task 6.2).
function Window-Until([scriptblock] $Answered, [int] $Seconds = 15) {
  $deadline = (Get-Date).AddSeconds($Seconds)
  $said = @()
  while ((Get-Date) -lt $deadline) {
    $said = Window-Names
    if ($said.Count -gt 0 -and (& $Answered $said)) { return $said }
    Start-Sleep -Milliseconds 500
  }
  return $said
}

# Clicks the middle of the first element named this, with a real mouse.
#
# **Not `InvokePattern`.** It is offered on every button in this window and
# calling it returns success and does nothing — the window is a WebView2 and it
# acts on input, not on patterns. And UI Automation reports a bounding
# rectangle for elements *below the fold*, so the point is checked against the
# window's own frame before any button goes down (7.4's two traps).
function Click-Named([string] $pattern, [string] $ControlType = '') {
  $h = Find-Window $script:app.Id
  if ($h -eq [IntPtr]::Zero) { return $false }
  $root = [System.Windows.Automation.AutomationElement]::FromHandle($h)
  $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny) | Out-Null
  Start-Sleep -Milliseconds 500
  foreach ($e in $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny)) {
    if ($e.Current.Name -inotmatch $pattern) { continue }
    # A field and its label share a name, and only one of them can be typed
    # into. The caller says which it wants.
    if ($ControlType -and $e.Current.ControlType.ProgrammaticName -ne $ControlType) { continue }
    try {
      $e.GetCurrentPattern([System.Windows.Automation.ScrollItemPattern]::Pattern).ScrollIntoView()
      Start-Sleep -Milliseconds 300
    } catch { }
    $box = $e.Current.BoundingRectangle
    if ($box.Width -le 0 -or $box.Height -le 0) { continue }
    $x = [int]($box.X + $box.Width / 2)
    $y = [int]($box.Y + $box.Height / 2)
    $frame = New-Object Gv.Menu+RECT
    [Gv.Menu]::GetWindowRect($h, [ref] $frame) | Out-Null
    if ($x -lt $frame.L -or $x -gt $frame.R -or $y -lt $frame.T -or $y -gt $frame.B) { continue }
    [Gv.Menu]::ShowWindow($h, 9) | Out-Null   # SW_RESTORE, in case of the tray
    [Gv.Menu]::SetForegroundWindow($h) | Out-Null
    Start-Sleep -Milliseconds 400
    if (-not [Gv.Menu]::SetCursorPos($x, $y)) { return $false }
    Start-Sleep -Milliseconds 200
    [Gv.Menu]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)   # left down
    Start-Sleep -Milliseconds 80
    [Gv.Menu]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)   # left up
    Start-Sleep -Milliseconds 700
    return $true
  }
  return $false
}

# Types into one of the join form's fields.
#
# **Typed, not `ValuePattern.SetValue`.** The form is SolidJS and its state
# comes from `onInput`; a value written straight into the DOM node is a value
# the app has never heard of, and `join` would stay disabled with the room code
# visible on screen — the most convincing wrong answer this drill could give.
#
# It has to be typed at all because a client that arrived by `GOODVOICE_AUTOJOIN`
# never filled this form in: `room` is a signal that starts empty and autojoin
# does not touch it, so after a tray → Leave the field is blank and `join` is
# disabled. A drill that only clicked the button read that as the app refusing
# to re-join.
function Fill-Field([string] $field, [string] $text) {
  if (-not (Click-Named "^\s*$field\s*$" 'ControlType.Edit')) { return $false }
  $keys = New-Object -ComObject WScript.Shell
  $keys.SendKeys('^a')
  Start-Sleep -Milliseconds 150
  $keys.SendKeys($text)
  Start-Sleep -Milliseconds 400
  return $true
}

# --- the tray icon and its menu --------------------------------------------

$uiaRoot = [System.Windows.Automation.AutomationElement]::RootElement

# Matched as a prefix, because the shell appends state to these names, and
# "goodvoice" must not match the taskbar's "goodvoice - 1 running window"
# (tray.md).
function Find-TrayButton([string] $name) {
  foreach ($top in $uiaRoot.FindAll([System.Windows.Automation.TreeScope]::Children, $uiaAny)) {
    if ($top.Current.ClassName -notmatch 'Shell_TrayWnd|Overflow') { continue }
    foreach ($d in $top.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny)) {
      if ($d.Current.ControlType.ProgrammaticName -ne 'ControlType.Button') { continue }
      if ($d.Current.Name -eq $name -or $d.Current.Name -like "$name ?*") {
        if ($d.Current.Name -notlike '* running window*') { return $d }
      }
    }
  }
  return $null
}

# The chevron is a toggle and the flyout closes itself, so ask, act, ask again
# — never "open it if the icon is missing", which hides the icon on the cycle
# after one that left it open (tray.md).
function Show-TrayIcon([string] $name) {
  for ($try = 0; $try -lt 3; $try++) {
    $found = Find-TrayButton $name
    if ($found) { return $found }
    $chevron = Find-TrayButton 'Show Hidden Icons'
    if ($chevron) {
      $pat = $null
      if ($chevron.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref] $pat)) {
        $pat.Invoke()
      }
    }
    Start-Sleep -Seconds 2
  }
  return $null
}

# Right-clicks the tray icon and returns the popup window, or $null with the
# reason in $script:menuStep.
function Open-TrayMenu {
  $button = Show-TrayIcon 'goodvoice'
  if (-not $button) { $script:menuStep = 'no-icon'; return $null }
  $rect = $button.Current.BoundingRectangle
  if (-not [Gv.Menu]::SetCursorPos([int]($rect.X + $rect.Width / 2), [int]($rect.Y + $rect.Height / 2))) {
    $script:menuStep = 'no-pointer'
    return $null
  }
  Start-Sleep -Milliseconds 300
  [Gv.Menu]::mouse_event(0x0008, 0, 0, 0, [UIntPtr]::Zero)   # RIGHTDOWN
  [Gv.Menu]::mouse_event(0x0010, 0, 0, 0, [UIntPtr]::Zero)   # RIGHTUP
  Start-Sleep -Seconds 1
  $popup = $uiaRoot.FindAll([System.Windows.Automation.TreeScope]::Children, $uiaAny) |
    Where-Object { $_.Current.ClassName -eq '#32768' } | Select-Object -First 1
  if (-not $popup) { $script:menuStep = 'no-menu'; return $null }
  $script:menuStep = 'ok'
  return $popup
}

# What the menu holds, read out of the HMENU the popup is drawing. One object
# per item: Text, Checked, Enabled, Separator.
$MN_GETHMENU = 0x01E1
$MIIM_STATE = 0x00000001
$MIIM_STRING = 0x00000040
$MIIM_FTYPE = 0x00000100
$MFT_SEPARATOR = 0x00000800
$MFS_GRAYED = 0x00000003   # MF_GRAYED | MF_DISABLED — either means unusable
$MFS_CHECKED = 0x00000008

function Read-Menu($popup) {
  $hwnd = [IntPtr] $popup.Current.NativeWindowHandle
  $script:menu = [Gv.Menu]::SendMessage($hwnd, $MN_GETHMENU, [IntPtr]::Zero, [IntPtr]::Zero)
  if ($script:menu -eq [IntPtr]::Zero) { return @() }
  $count = [Gv.Menu]::GetMenuItemCount($menu)
  if ($count -le 0) { return @() }
  $items = @()
  for ($i = 0; $i -lt $count; $i++) {
    $info = New-Object Gv.Menu+MENUITEMINFO
    $info.cbSize = [Runtime.InteropServices.Marshal]::SizeOf([type][Gv.Menu+MENUITEMINFO])
    $info.fMask = $MIIM_STATE -bor $MIIM_STRING -bor $MIIM_FTYPE
    # Two calls: the first for how long the text is, the second for the text.
    # `dwTypeData` null with `MIIM_STRING` is how `cch` gets filled in.
    $info.dwTypeData = [IntPtr]::Zero
    [Gv.Menu]::GetMenuItemInfo($menu, $i, $true, [ref] $info) | Out-Null
    $text = ''
    if ($info.cch -gt 0) {
      $len = [int]$info.cch + 1
      $buffer = [Runtime.InteropServices.Marshal]::AllocHGlobal($len * 2)
      $info.cbSize = [Runtime.InteropServices.Marshal]::SizeOf([type][Gv.Menu+MENUITEMINFO])
      $info.fMask = $MIIM_STATE -bor $MIIM_STRING -bor $MIIM_FTYPE
      $info.dwTypeData = $buffer
      $info.cch = $len
      if ([Gv.Menu]::GetMenuItemInfo($menu, $i, $true, [ref] $info)) {
        $text = [Runtime.InteropServices.Marshal]::PtrToStringUni($buffer)
      }
      [Runtime.InteropServices.Marshal]::FreeHGlobal($buffer)
    }
    $items += [pscustomobject]@{
      Text      = $text
      Separator = (($info.fType -band $MFT_SEPARATOR) -ne 0)
      Checked   = (($info.fState -band $MFS_CHECKED) -ne 0)
      Enabled   = (($info.fState -band $MFS_GRAYED) -eq 0)
    }
  }
  return $items
}

# One line a person can read, e.g.
#   Open goodvoice | -- | [x] Mute | [ ] Deafen (grey) | Leave room | -- | Quit
function Show-Menu($items) {
  return (@($items | ForEach-Object {
        if ($_.Separator) { return '--' }
        $tick = if ($_.Checked) { '[x] ' } else { '' }
        $grey = if ($_.Enabled) { '' } else { ' (grey)' }
        "$tick$($_.Text)$grey"
      }) -join ' | ')
}

function Save-MenuShot($popup, [string] $name) {
  $hwnd = [IntPtr] $popup.Current.NativeWindowHandle
  $box = New-Object Gv.Menu+RECT
  if (-not [Gv.Menu]::GetWindowRect($hwnd, [ref] $box)) { return $null }
  $w = $box.R - $box.L
  $h = $box.B - $box.T
  if ($w -le 0 -or $h -le 0) { return $null }
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($box.L, $box.T, 0, 0, $bmp.Size)
  $g.Dispose()
  $path = Join-Path $Out "$name.png"
  $bmp.Save($path)
  $bmp.Dispose()
  return $path
}

# Escape closes the popup without picking anything, which is how every read-only
# look at the menu ends.
function Close-Menu {
  [Gv.Menu]::keybd_event(0x1B, 0, 0, [UIntPtr]::Zero)
  [Gv.Menu]::keybd_event(0x1B, 0, 2, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 600
}

# Picks the item at `Index` (0-based, separators included), by keyboard.
#
# The items are not in the automation tree, so there is nothing to `Invoke`;
# Down moves the highlight and Return picks it. Down skips separators, which is
# why the caller passes the index it read out of the HMENU rather than a
# hand-counted one — the two differ by however many separators are above it.
function Pick-MenuItem($items, [int] $index) {
  $steps = 0
  for ($i = 0; $i -le $index; $i++) {
    if (-not $items[$i].Separator) { $steps++ }
  }
  for ($i = 0; $i -lt $steps; $i++) {
    [Gv.Menu]::keybd_event(0x28, 0, 0, [UIntPtr]::Zero)   # VK_DOWN
    [Gv.Menu]::keybd_event(0x28, 0, 2, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 120
  }
  Start-Sleep -Milliseconds 300
  [Gv.Menu]::keybd_event(0x0D, 0, 0, [UIntPtr]::Zero)     # VK_RETURN
  [Gv.Menu]::keybd_event(0x0D, 0, 2, [UIntPtr]::Zero)
  Start-Sleep -Seconds 2
}

function Index-Of($items, [string] $pattern) {
  for ($i = 0; $i -lt $items.Count; $i++) {
    if (-not $items[$i].Separator -and $items[$i].Text -imatch $pattern) { return $i }
  }
  return -1
}

# Opens the menu, reads it, and either picks something or closes it again. The
# menu as it was *before* the pick is left in `$script:menu`.
#
# **Left in a variable rather than returned.** Every `Write-Output` inside a
# PowerShell function joins that function's return value, so a version of this
# that both printed `MENU_…=` lines and returned the items returned the lines
# too — and the caller's `(Item $script:menu 'Mute')` then walked an array of strings
# looking for `.Text`. It happened to find the right item anyway, and the
# evidence lines never reached the transcript. One output stream, one meaning.
function Use-Menu([string] $label, [string] $pick) {
  $script:menu = @()
  $popup = Open-TrayMenu
  if (-not $popup) {
    Check "MENU_$label" $false "unreadable ($script:menuStep)"
    return
  }
  $items = Read-Menu $popup
  Write-Output ("MENU_SHOT_$label=" + (Save-MenuShot $popup $label))
  if ($items.Count -eq 0) {
    Check "MENU_$label" $false 'the popup would not give up its HMENU'
    Close-Menu
    return
  }
  Write-Output ("MENU_$label=" + (Show-Menu $items))
  $script:menu = $items
  if ($pick) {
    $at = Index-Of $items $pick
    if ($at -lt 0) { Check "MENU_PICK_$label" $false "no item matching $pick"; Close-Menu; return }
    Pick-MenuItem $items $at
  } else {
    Close-Menu
  }
}

function Item($items, [string] $pattern) {
  foreach ($i in $items) { if (-not $i.Separator -and $i.Text -imatch $pattern) { return $i } }
  return $null
}

# --- the roommate ----------------------------------------------------------

# `Get-Content`'s default share mode is refused while the writer still has the
# file open, which for a listener that is still running it does (invite.ps1).
function Read-Log([string] $path) {
  if (-not (Test-Path $path)) { return @() }
  $stream = [System.IO.FileStream]::new($path, [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
  $reader = [System.IO.StreamReader]::new($stream)
  $text = $reader.ReadToEnd()
  $reader.Dispose(); $stream.Dispose()
  return $text -split "`r?`n"
}

$heardPath = Join-Path $Out 'listener.txt'

# The last thing the roommate said about the room. `bin/listener` prints a
# roster line only when the flags change, so the newest one is the current
# state and there is no need to correlate clocks.
function Roommate-Sees {
  $lines = @(Read-Log $heardPath | Where-Object { $_ -match '^\s+roster @ ' })
  if ($lines.Count -eq 0) { return '' }
  return ($lines[-1] -replace '^\s+roster @ \d+s\s+', '').Trim()
}

# Waits for the roommate to see a change rather than for a fixed pause: the
# flag has to cross the room and come back down a WebSocket.
function Roommate-Until([scriptblock] $Answered, [int] $Seconds = 15) {
  $deadline = (Get-Date).AddSeconds($Seconds)
  $seen = ''
  while ((Get-Date) -lt $deadline) {
    $seen = Roommate-Sees
    if ($seen -and (& $Answered $seen)) { return $seen }
    Start-Sleep -Milliseconds 500
  }
  return $seen
}

# --- the run ---------------------------------------------------------------

Write-Output 'goodvoice tray-menu drill (plan.md 7.2, tray.md "The menu")'
Write-Output "  app        $Exe"
Write-Output "  listener   $ListenerExe"
Write-Output "  room       $Room"
Write-Output "  output     $Out"
Write-Output ''

$pointer = Get-PointerVerdict
Check 'POINTER' ($pointer -eq 'ok') $pointer
if ($pointer -ne 'ok') {
  Write-Output ''
  Write-Output 'Nothing below this line can run: every step needs a real mouse or a'
  Write-Output 'real key, and Windows is refusing both. See "When the pointer is'
  Write-Output 'refused" in this script, and tray.md.'
  Write-Output 'RESULT=BLOCKED'
  exit 4
}

Get-Process goodvoice-client -EA SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 3

# The roommate goes in first, so the window's own arrival is something it can
# report. Long enough to outlast the whole walk.
$listener = Start-Process -FilePath $ListenerExe -PassThru -WindowStyle Hidden `
  -ArgumentList '--room', $Room, '--name', 'roommate', '--seconds', '420' `
  -RedirectStandardOutput $heardPath -RedirectStandardError (Join-Path $Out 'listener.err')
Start-Sleep -Seconds 8
Say 'the roommate is in the room'

$env:GOODVOICE_AUTOJOIN = $Room
$script:app = Start-Process -FilePath $Exe -PassThru

try {
  $h = [IntPtr]::Zero
  for ($i = 0; $i -lt 40 -and $h -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 500; $h = Find-Window $script:app.Id }
  if ($h -eq [IntPtr]::Zero) { Write-Output 'NO_WINDOW'; exit 3 }

  # ---- row 1: join a room ------------------------------------------------
  $said = Window-Until { param($it) Has $it "^\s*room\s+$Room\s*$" } 60
  Check 'ROW1_WINDOW' (Has $said "^\s*room\s+$Room\s*$") ($(if (Has $said "^\s*room\s+$Room\s*$") { "the room panel, $Room" } else { "[$($said -join ' | ')]" }))
  $sees = Roommate-Until { param($s) $s -imatch 'roommate' -and ($s -split '\|').Count -ge 2 } 30
  Check 'ROW1_ROOMMATE' (($sees -split '\|').Count -ge 2) $sees
  Use-Menu 'ROW1_JOINED' ''
  $live = (Item $script:menu 'Mute') -and (Item $script:menu 'Mute').Enabled -and
          (Item $script:menu 'Deafen').Enabled -and (Item $script:menu 'Leave').Enabled
  Check 'ROW1_TRAY' ([bool] $live) $(if ($live) { 'all three items live' } else { 'not all three are live' })

  # ---- row 2: tray -> Mute ------------------------------------------------
  Use-Menu 'ROW2_BEFORE' 'Mute'
  $said = Window-Until { param($it) Has $it '^\s*unmute\s*$' }
  Check 'ROW2_WINDOW' (Has $said '^\s*unmute\s*$') $(if (Has $said '^\s*unmute\s*$') { 'the mute button is lit' } else { "[$($said -join ' | ')]" })
  $sees = Roommate-Until { param($s) $s -imatch 'muted' }
  Check 'ROW2_ROOMMATE' ($sees -imatch 'muted') $sees
  Use-Menu 'ROW2_AFTER' ''
  Check 'ROW2_TRAY' ((Item $script:menu 'Mute').Checked) ("Mute checked=" + (Item $script:menu 'Mute').Checked)

  # ---- row 3: window -> unmute -------------------------------------------
  Check 'ROW3_CLICKED' (Click-Named '^\s*unmute\s*$') 'the window'
  $said = Window-Until { param($it) Has $it '^\s*mute\s*$' }
  Check 'ROW3_WINDOW' (Has $said '^\s*mute\s*$') $(if (Has $said '^\s*mute\s*$') { 'the button is unlit' } else { "[$($said -join ' | ')]" })
  $sees = Roommate-Until { param($s) $s -inotmatch 'muted' }
  Check 'ROW3_ROOMMATE' ($sees -inotmatch 'muted') $sees
  Use-Menu 'ROW3_AFTER' ''
  Check 'ROW3_TRAY' (-not (Item $script:menu 'Mute').Checked) ("Mute checked=" + (Item $script:menu 'Mute').Checked)

  # ---- row 4: tray -> Deafen ---------------------------------------------
  Use-Menu 'ROW4_BEFORE' 'Deafen'
  $said = Window-Until { param($it) Has $it '^\s*undeafen\s*$' }
  Check 'ROW4_WINDOW' (Has $said '^\s*undeafen\s*$') $(if (Has $said '^\s*undeafen\s*$') { 'the deafen button is lit' } else { "[$($said -join ' | ')]" })
  $sees = Roommate-Until { param($s) $s -imatch 'deafened' }
  Check 'ROW4_ROOMMATE' ($sees -imatch 'deafened') $sees
  Use-Menu 'ROW4_AFTER' ''
  Check 'ROW4_TRAY' ((Item $script:menu 'Deafen').Checked) ("Deafen checked=" + (Item $script:menu 'Deafen').Checked)

  # ---- row 5: window -> mute as well --------------------------------------
  Check 'ROW5_CLICKED' (Click-Named '^\s*mute\s*$') 'the window'
  $said = Window-Until { param($it) (Has $it '^\s*unmute\s*$') -and (Has $it '^\s*undeafen\s*$') }
  $both = (Has $said '^\s*unmute\s*$') -and (Has $said '^\s*undeafen\s*$')
  Check 'ROW5_WINDOW' $both $(if ($both) { 'both buttons lit' } else { "[$($said -join ' | ')]" })
  $sees = Roommate-Until { param($s) $s -imatch 'muted' -and $s -imatch 'deafened' }
  Check 'ROW5_ROOMMATE' (($sees -imatch 'muted') -and ($sees -imatch 'deafened')) $sees
  Use-Menu 'ROW5_AFTER' ''
  $ticked = (Item $script:menu 'Mute').Checked -and (Item $script:menu 'Deafen').Checked
  Check 'ROW5_TRAY' $ticked ("Mute=" + (Item $script:menu 'Mute').Checked + " Deafen=" + (Item $script:menu 'Deafen').Checked)

  # ---- row 6: tray -> Leave room ------------------------------------------
  Use-Menu 'ROW6_BEFORE' 'Leave'
  # The join panel, which is the window with no room in it. Its own three
  # fields are the thing to look for; the room label is what has to be gone.
  $said = Window-Until { param($it) -not (Has $it "^\s*room\s+$Room\s*$") } 20
  Check 'ROW6_WINDOW' (-not (Has $said "^\s*room\s+$Room\s*$")) $(if (Has $said "^\s*room\s+$Room\s*$") { 'still in the room' } else { 'back to the join panel' })
  $sees = Roommate-Until { param($s) ($s -split '\|').Count -eq 1 } 30
  Check 'ROW6_ROOMMATE' (($sees -split '\|').Count -eq 1) $sees
  Use-Menu 'ROW6_AFTER' ''
  $grey = -not (Item $script:menu 'Mute').Enabled -and -not (Item $script:menu 'Deafen').Enabled -and
          -not (Item $script:menu 'Leave').Enabled
  Check 'ROW6_TRAY' $grey $(if ($grey) { 'all three greyed again' } else { 'something is still live' })

  # ---- row 7: join again --------------------------------------------------
  # The row that exists because it used to be broken: a call that ended on its
  # own was kept in memory as if it were still running, and the next join was
  # refused with "already in a call" until the app was restarted.
  Check 'ROW7_TYPED' (Fill-Field 'room' $Room) "the room code, into an empty form"
  Check 'ROW7_CLICKED' (Click-Named '^\s*join\s*$') 'the window'
  $said = Window-Until { param($it) Has $it "^\s*room\s+$Room\s*$" } 40
  Check 'ROW7_WINDOW' (Has $said "^\s*room\s+$Room\s*$") $(if (Has $said "^\s*room\s+$Room\s*$") { 'the room panel' } else { "[$($said -join ' | ')]" })
  $sees = Roommate-Until { param($s) ($s -split '\|').Count -ge 2 } 30
  Check 'ROW7_ROOMMATE' (($sees -split '\|').Count -ge 2) $sees
  Use-Menu 'ROW7_AFTER' ''
  $live = (Item $script:menu 'Mute').Enabled -and (Item $script:menu 'Deafen').Enabled -and (Item $script:menu 'Leave').Enabled
  Check 'ROW7_TRAY' ([bool] $live) $(if ($live) { 'live again' } else { 'still greyed' })
}
finally {
  Stop-Process -Id $script:app.Id -Force -EA SilentlyContinue
  Stop-Process -Id $listener.Id -Force -EA SilentlyContinue
  Start-Sleep -Seconds 2
  Get-Process goodvoice-client -EA SilentlyContinue | Stop-Process -Force
  Write-Output ("HEARD=" + $heardPath)
}

Write-Output ''
Write-Output ("RESULT=" + $(if ($script:ok) { 'PASS' } else { 'FAIL' }))
exit $(if ($script:ok) { 0 } else { 1 })
