# plan.md 4.6: does the window go away *entirely* when goodvoice goes to the
# tray, and does clicking the icon bring it back with the call still on screen?
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\tray-roundtrip.ps1
#   ... -Via minimise          # the other way in; default is the close button
#   ... -Cycles 5               # away and back five times, watching for a leak
#   ... -Exe path\to\goodvoice-client.exe
#
# Needs a RELEASE build with the `custom-protocol` feature — see tray.md. A
# build without it points the webview at the Vite dev server, and every number
# here is then about Edge's error page.
[CmdletBinding()]
param(
  [ValidateSet('close', 'minimise')] [string] $Via = 'close',
  [int] $Cycles = 3,
  [string] $Exe = "$env:CARGO_TARGET_DIR\release\goodvoice-client.exe",
  [string] $ShotDir = "$env:TEMP\goodvoice-tray-roundtrip"
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type -Namespace Gv -Name Tray -MemberDefinition @'
[DllImport("user32.dll")] public static extern int PostMessage(IntPtr hWnd, uint msg, IntPtr wp, IntPtr lp);
[DllImport("user32.dll")] public static extern bool IsWindow(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int cmd);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
[DllImport("user32.dll")] public static extern int GetClassName(IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr hWnd, out int pid);
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
[DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
public struct RECT { public int L, T, R, B; }
public delegate bool EnumProc(IntPtr hWnd, IntPtr lp);
'@

if (-not (Test-Path $Exe)) { $Exe = 'client\src-tauri\target\release\goodvoice-client.exe' }
if (-not (Test-Path $Exe)) { Write-Output "NO_EXE=$Exe"; exit 2 }
New-Item -ItemType Directory -Force -Path $ShotDir | Out-Null

# By window class, not by MainWindowHandle: a debug build also owns a console
# window, and .NET will hand you that one.
function Find-Window([int] $Owner) {
  $script:hit = [IntPtr]::Zero
  $cb = [Gv.Tray+EnumProc] {
    param($h, $l)
    $who = 0
    [Gv.Tray]::GetWindowThreadProcessId($h, [ref]$who) | Out-Null
    if ($who -eq $Owner) {
      $c = New-Object System.Text.StringBuilder 256
      [Gv.Tray]::GetClassName($h, $c, 256) | Out-Null
      if ($c.ToString() -eq 'Tauri Window') { $script:hit = $h; return $false }
    }
    return $true
  }
  [Gv.Tray]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
  return $script:hit
}

# The whole tree, because the 327 MB this task is about is WebView2's, and
# WebView2's renderers are grandchildren rather than children.
function Get-Tree([int] $Root) {
  $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, WorkingSetSize
  $keep = @($Root); $seen = 0
  while ($keep.Count -ne $seen) {
    $seen = $keep.Count
    foreach ($q in $all) {
      if ($keep -contains $q.ParentProcessId -and $keep -notcontains $q.ProcessId) { $keep += $q.ProcessId }
    }
  }
  $bytes = ($all | Where-Object { $keep -contains $_.ProcessId } | Measure-Object WorkingSetSize -Sum).Sum
  return [pscustomobject]@{ Count = $keep.Count; MB = [math]::Round($bytes / 1MB, 1) }
}

function Save-Shot([IntPtr] $h, [string] $name) {
  [Gv.Tray]::SetForegroundWindow($h) | Out-Null
  Start-Sleep -Milliseconds 800
  $r = New-Object Gv.Tray+RECT
  if (-not [Gv.Tray]::GetWindowRect($h, [ref] $r)) { return $null }
  $bmp = New-Object System.Drawing.Bitmap (($r.R - $r.L), ($r.B - $r.T))
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size)
  $path = Join-Path $ShotDir "$name.png"
  $bmp.Save($path)
  $g.Dispose(); $bmp.Dispose()
  return $path
}

# The notification-area icon, through UI Automation. Windows 11's tray is a
# XAML island, so there is no ToolbarWindow32 to hit-test any more — but the
# icons are still automation buttons with an Invoke pattern, and Invoke on one
# is the left click `tray::show` is wired to. New icons start behind the
# chevron, which is a button under the same root.
$uiaRoot = [System.Windows.Automation.AutomationElement]::RootElement
$uiaAny = [System.Windows.Automation.Condition]::TrueCondition
# Matched as a prefix, because the shell appends state to these names: the
# chevron is "Show Hidden Icons Hide" while the flyout is open, and matching it
# exactly is a click that silently lands on nothing.
function Find-TrayButton([string] $name) {
  foreach ($top in $uiaRoot.FindAll([System.Windows.Automation.TreeScope]::Children, $uiaAny)) {
    if ($top.Current.ClassName -notmatch 'Shell_TrayWnd|Overflow') { continue }
    foreach ($d in $top.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny)) {
      if ($d.Current.ControlType.ProgrammaticName -ne 'ControlType.Button') { continue }
      # ...and "goodvoice" must not match the taskbar's "goodvoice - 1 running
      # window", which is a different button that does a different thing.
      if ($d.Current.Name -eq $name -or $d.Current.Name -like "$name ?*") {
        if ($d.Current.Name -notlike '* running window*') { return $d }
      }
    }
  }
  return $null
}

# The chevron is a toggle and the flyout closes itself, so "open it if the icon
# is missing" is wrong half the time: on the cycle after one that left it open,
# invoking the chevron is what *hides* the icon. Ask, act, ask again.
function Show-TrayIcon([string] $name) {
  for ($try = 0; $try -lt 3; $try++) {
    $found = Find-TrayButton $name
    if ($found) { return $found }
    Invoke-TrayButton 'Show Hidden Icons' | Out-Null
    Start-Sleep -Seconds 2
  }
  return $null
}

# Finding and clicking are separate on purpose. Rolled into one, "look for the
# icon, and open the chevron if it is not there" clicks the icon twice whenever
# it *is* there — and two Opens in a row is a different test than the one this
# is trying to run.
function Invoke-TrayButton([string] $name) {
  $button = Find-TrayButton $name
  if (-not $button) { return $false }
  $pat = $null
  if (-not $button.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref] $pat)) {
    return $false
  }
  $pat.Invoke()
  return $true
}

# The last item of the right-click menu — which is Quit goodvoice, and the only
# item this drill has any business clicking.
#
# Two things do not work here and both look like the app is broken.
# UI Automation has no "invoke with the other button", so the right-click is a
# real synthesised mouse at the icon's own bounding rectangle. And the popup it
# opens is a `TrackPopupMenu`, which UIA reports as a `#32768` pane with **no
# children at all** — the items are on screen and not in the tree, so there is
# nothing to `Invoke`. The keyboard is the way in: Up with nothing selected
# highlights the last item, and Return picks it.
function Invoke-TrayMenuLast([string] $icon) {
  $button = Show-TrayIcon $icon
  if (-not $button) { return $false }
  $rect = $button.Current.BoundingRectangle

  [Gv.Tray]::SetCursorPos([int]($rect.X + $rect.Width / 2), [int]($rect.Y + $rect.Height / 2)) | Out-Null
  Start-Sleep -Milliseconds 300
  [Gv.Tray]::mouse_event(0x0008, 0, 0, 0, [UIntPtr]::Zero)   # RIGHTDOWN
  [Gv.Tray]::mouse_event(0x0010, 0, 0, 0, [UIntPtr]::Zero)   # RIGHTUP
  Start-Sleep -Seconds 1

  $menu = $uiaRoot.FindAll([System.Windows.Automation.TreeScope]::Children, $uiaAny) |
    Where-Object { $_.Current.ClassName -eq '#32768' }
  if (-not $menu) { return $false }

  [Gv.Tray]::keybd_event(0x26, 0, 0, [UIntPtr]::Zero)        # VK_UP down
  [Gv.Tray]::keybd_event(0x26, 0, 2, [UIntPtr]::Zero)        # VK_UP up
  Start-Sleep -Milliseconds 400
  [Gv.Tray]::keybd_event(0x0D, 0, 0, [UIntPtr]::Zero)        # VK_RETURN down
  [Gv.Tray]::keybd_event(0x0D, 0, 2, [UIntPtr]::Zero)        # VK_RETURN up
  return $true
}

# In a room, because a window that comes back has to come back showing the
# call — which is the half of 4.6 that is not about memory.
$room = "drill-" + (Get-Random -Minimum 100000 -Maximum 999999)
$env:GOODVOICE_AUTOJOIN = $room
$p = Start-Process -FilePath $Exe -PassThru
$ok = $true
function Check([string] $label, [bool] $pass, [string] $value) {
  Write-Output ("{0}={1}" -f $label, $value)
  if (-not $pass) { $script:ok = $false }
}

try {
  $h = [IntPtr]::Zero
  for ($i = 0; $i -lt 40 -and $h -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 500; $h = Find-Window $p.Id }
  if ($h -eq [IntPtr]::Zero) { Write-Output 'NO_WINDOW'; exit 1 }
  # The app has to be past its own startup before a window message means
  # anything to it: a close in the first second is handled by Windows, not by
  # Tauri. It is also how long the autojoin takes to land.
  Start-Sleep -Seconds 8

  $before = Get-Tree $p.Id
  Check 'ROOM' $true $room
  Check 'VISIBLE_BEFORE' ([Gv.Tray]::IsWindowVisible($h)) ([Gv.Tray]::IsWindowVisible($h))
  Check 'PROCESSES_BEFORE' ($before.Count -gt 1) $before.Count
  Check 'TREE_MB_BEFORE' $true $before.MB
  Write-Output ("SHOT_BEFORE=" + (Save-Shot $h 'before'))

  # More than once, because a webview that is torn down and stood back up is
  # exactly the shape of thing that gives a little back each time. Three cycles
  # will not prove there is no leak; a tray figure that climbs every cycle
  # would prove there is one.
  $back = $h
  for ($cycle = 1; $cycle -le $Cycles; $cycle++) {
    switch ($Via) {
      'close' { [Gv.Tray]::PostMessage($back, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null }  # WM_CLOSE
      'minimise' { [Gv.Tray]::ShowWindow($back, 6) | Out-Null }                                    # SW_MINIMIZE
    }
    Start-Sleep -Seconds 6
    $p.Refresh()
    $away = Get-Tree $p.Id
    Check "ALIVE_IN_TRAY_$cycle" (-not $p.HasExited) (-not $p.HasExited)
    # Not "hidden": the handle itself is gone. That is the 327 MB.
    Check "WINDOW_IN_TRAY_$cycle" (-not [Gv.Tray]::IsWindow($back)) ([Gv.Tray]::IsWindow($back))
    Check "PROCESSES_IN_TRAY_$cycle" ($away.Count -eq 1) $away.Count
    Check "TREE_MB_IN_TRAY_$cycle" ($away.MB -le 120) $away.MB

    $icon = Show-TrayIcon 'goodvoice'
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $clicked = $null -ne $icon -and (Invoke-TrayButton 'goodvoice')
    Check "TRAY_CLICKED_$cycle" $clicked $clicked
    $back = [IntPtr]::Zero
    for ($i = 0; $i -lt 60 -and $back -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 100; $back = Find-Window $p.Id }
    $clock.Stop()
    Check "WINDOW_AFTER_TRAY_$cycle" ($back -ne [IntPtr]::Zero) ($back -ne [IntPtr]::Zero)
    Check "REBUILT_IN_MS_$cycle" ($clock.ElapsedMilliseconds -lt 3000) $clock.ElapsedMilliseconds
    Start-Sleep -Seconds 3
    Check "VISIBLE_AFTER_TRAY_$cycle" ([Gv.Tray]::IsWindowVisible($back)) ([Gv.Tray]::IsWindowVisible($back))
  }
  $again = Get-Tree $p.Id
  Check 'TREE_MB_BACK' $true $again.MB
  Write-Output ("SHOT_AFTER=" + (Save-Shot $back 'after'))

  # And out. The trap the whole design is shaped around: an app whose window
  # closes into the tray and whose tray cannot quit it is an app nobody can
  # stop. Worth checking here rather than by hand, because `run()` refuses the
  # exit that a closing window asks for and has to keep letting this one
  # through.
  [Gv.Tray]::PostMessage($back, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
  Start-Sleep -Seconds 4
  $quit = Invoke-TrayMenuLast 'goodvoice'
  Check 'QUIT_CLICKED' $quit $quit
  for ($i = 0; $i -lt 60 -and -not $p.HasExited; $i++) { Start-Sleep -Milliseconds 250; $p.Refresh() }
  Check 'QUIT_ENDED_IT' $p.HasExited $p.HasExited
}
finally {
  Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 2
  $left = @(Get-Process goodvoice-client -ErrorAction SilentlyContinue).Count
  Check 'LEFTOVER' ($left -eq 0) $left
}

# The one thing no assertion here covers: whether `after.png` shows the room
# rather than the join form. Open it.
Write-Output ("RESULT=" + $(if ($ok) { 'PASS' } else { 'FAIL' }))
exit $(if ($ok) { 0 } else { 1 })
