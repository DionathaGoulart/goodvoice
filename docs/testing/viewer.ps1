# plan.md 5.4: does opening and closing the viewer, over and over, during a
# live share, cost the voice anything — and is what it shows a picture, the
# right shape, in whatever shape the window is?
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\viewer.ps1
#   ... -Cycles 6                 # six trips instead of four
#   ... -Skin terminal            # switch the skin first, then watch
#   ... -Exe path\to\goodvoice-client.exe
#
# Needs a RELEASE build with the `custom-protocol` feature (see tray.md): a
# build without it points the webview at the Vite dev server, and the viewer is
# then Edge's error page.
#
# The other end of the call is `bin/viewer-drill`, started from here. It shares
# a monitor into the room and counts the frames of voice arriving back from the
# app, once a second, for as long as this script is clicking. The verdict on
# "audio unaffected throughout" is its table, not this script's.
[CmdletBinding()]
param(
  [string] $Room = "view-$(Get-Random -Maximum 999999)",
  [int] $Cycles = 4,
  [int] $OpenSeconds = 6,
  [int] $ClosedSeconds = 4,
  [ValidateSet('', 'retro', 'terminal')] [string] $Skin = '',
  [switch] $NoBackdrop,
  [string] $Exe = "$env:CARGO_TARGET_DIR\release\goodvoice-client.exe",
  [string] $DrillExe = "$env:CARGO_TARGET_DIR\release\viewer-drill.exe",
  [string] $ShotDir = "$env:TEMP\goodvoice-viewer"
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type -Namespace Gv -Name View -MemberDefinition @'
[DllImport("user32.dll")] public static extern int PostMessage(IntPtr hWnd, uint msg, IntPtr wp, IntPtr lp);
[DllImport("user32.dll")] public static extern bool IsWindow(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hWnd, ref POINT p);
[DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr hWnd, int x, int y, int w, int h, bool repaint);
[DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr after, int x, int y, int w, int h, uint flags);
[DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
[DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr hWnd, uint flags);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
[DllImport("user32.dll")] public static extern int GetClassName(IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr hWnd, out int pid);
public struct RECT { public int L, T, R, B; }
public struct POINT { public int X, Y; }
public delegate bool EnumProc(IntPtr hWnd, IntPtr lp);
'@

if (-not (Test-Path $Exe)) { $Exe = 'client\src-tauri\target\release\goodvoice-client.exe' }
if (-not (Test-Path $DrillExe)) { $DrillExe = 'client\src-tauri\target\release\viewer-drill.exe' }
if (-not (Test-Path $Exe)) { Write-Output "NO_EXE=$Exe"; exit 2 }
if (-not (Test-Path $DrillExe)) { Write-Output "NO_DRILL=$DrillExe"; exit 2 }
New-Item -ItemType Directory -Force -Path $ShotDir | Out-Null

$script:t0 = Get-Date
function Say([string] $line) {
  $at = [int]((Get-Date) - $script:t0).TotalSeconds
  Write-Output ("  t+{0,3}s  {1}" -f $at, $line)
}

# --- windows ---------------------------------------------------------------

# By window class and title, not by MainWindowHandle: this process owns two
# Tauri windows once the viewer is open, and .NET will hand you whichever it
# feels like.
function Find-Windows([int] $Owner) {
  $script:found = @()
  $cb = [Gv.View+EnumProc] {
    param($h, $l)
    $who = 0
    [Gv.View]::GetWindowThreadProcessId($h, [ref]$who) | Out-Null
    if ($who -eq $Owner) {
      $c = New-Object System.Text.StringBuilder 256
      [Gv.View]::GetClassName($h, $c, 256) | Out-Null
      if ($c.ToString() -eq 'Tauri Window') {
        $t = New-Object System.Text.StringBuilder 256
        [Gv.View]::GetWindowText($h, $t, 256) | Out-Null
        $script:found += [pscustomobject]@{ Handle = $h; Title = $t.ToString() }
      }
    }
    return $true
  }
  [Gv.View]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
  return $script:found
}

# The backdrop's own window, which is not a Tauri one: whatever visible window
# that process owns.
function Find-Sheet([int] $Owner) {
  $script:sheet = [IntPtr]::Zero
  $cb = [Gv.View+EnumProc] {
    param($h, $l)
    $who = 0
    [Gv.View]::GetWindowThreadProcessId($h, [ref]$who) | Out-Null
    if ($who -eq $Owner -and [Gv.View]::IsWindowVisible($h)) { $script:sheet = $h; return $false }
    return $true
  }
  [Gv.View]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
  if ($script:sheet -eq [IntPtr]::Zero) { return $null }
  return $script:sheet
}

function Find-Main([int] $Owner) {
  return (Find-Windows $Owner | Where-Object { $_.Title -notlike '*screen*' } | Select-Object -First 1)
}
function Find-Viewer([int] $Owner) {
  return (Find-Windows $Owner | Where-Object { $_.Title -like '*screen*' } | Select-Object -First 1)
}

# The sharer still has this file open, and Get-Content's default share mode
# refuses it. Read it the way another writer allows.
function Read-Log([string] $path) {
  if (-not (Test-Path $path)) { return @() }
  $stream = [System.IO.FileStream]::new($path, [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
  $reader = [System.IO.StreamReader]::new($stream)
  $text = $reader.ReadToEnd()
  $reader.Dispose(); $stream.Dispose()
  return $text -split "`r?`n"
}

function Wait-For([scriptblock] $Probe, [int] $Seconds) {
  $deadline = (Get-Date).AddSeconds($Seconds)
  while ((Get-Date) -lt $deadline) {
    $hit = & $Probe
    if ($hit) { return $hit }
    Start-Sleep -Milliseconds 250
  }
  return $null
}

# --- the buttons -----------------------------------------------------------

$uiaAny = [System.Windows.Automation.Condition]::TrueCondition

# DR-26: WebView2 builds its accessibility tree lazily, and a targeted query on
# a cold tree finds nothing — which is indistinguishable from a button that is
# not there. Walking it once is what wakes it.
#
# The window is found again on every call rather than held: task 4.6 destroys
# and rebuilds the main window, so a handle is only good until it is not, and a
# tree from a dead one throws `ElementNotAvailable` rather than returning
# nothing.
function Get-Root([int] $Owner) {
  $window = Find-Main $Owner
  if (-not $window) { return $null }
  try {
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($window.Handle)
    $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny) | Out-Null
    return $root
  } catch {
    return $null
  }
}

# Case-insensitively, and by pattern: the terminal skin uppercases every
# accessible name (DR-26), and the sharer's name is in the middle of this one.
function Find-Button([System.Windows.Automation.AutomationElement] $root, [string] $pattern) {
  if (-not $root) { return $null }
  foreach ($e in $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny)) {
    if ($e.Current.ControlType.ProgrammaticName -ne 'ControlType.Button') { continue }
    if ($e.Current.Name -imatch $pattern) { return $e }
  }
  return $null
}

# Invoke, or toggle. `aria-pressed` turns a <button> into a UIA *toggle*
# button, and a toggle button does not support Invoke — which is every button
# in this window that shows a state: settings, the skins, mute, deafen.
function Invoke-Button([System.Windows.Automation.AutomationElement] $e) {
  try {
    $e.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
    return $true
  } catch {
    try {
      $e.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern).Toggle()
      return $true
    } catch {
      return $false
    }
  }
}

# --- what is on the screen -------------------------------------------------

# The client area only: a title bar is not the picture, and counting it would
# make every window "have a picture in it".
function Get-ClientBox([IntPtr] $h) {
  $r = New-Object Gv.View+RECT
  if (-not [Gv.View]::GetClientRect($h, [ref] $r)) { return $null }
  $p = New-Object Gv.View+POINT
  $p.X = 0; $p.Y = 0
  if (-not [Gv.View]::ClientToScreen($h, [ref] $p)) { return $null }
  return [pscustomobject]@{ X = $p.X; Y = $p.Y; W = ($r.R - $r.L); H = ($r.B - $r.T) }
}

# Is there a picture in there, and what shape is it?
#
# By variation, not by colour. The letterbox is a flat fill — `.viewer`'s
# background is the palette's own `--bg` (app.css) — so a row of the client
# area that is all one value is letterbox and a row that is not is picture.
# Matching an expected colour instead would be a test of which palette the app
# happens to be in, and a dark palette's near-black bar and a terminal's
# near-black wallpaper are the same colour anyway.
#
# The box those rows and columns make is what `object-fit: contain` produced,
# and its aspect ratio is the claim task 5.4 makes about a resize.
function Measure-Picture([IntPtr] $h, [string] $name) {
  # Topmost for the length of the shot, and back afterwards. The sheet under it
  # is topmost too (`Start-Backdrop`), and a viewer left permanently topmost
  # would not be the window a person opens.
  [Gv.View]::SetWindowPos($h, [IntPtr](-1), 0, 0, 0, 0, 0x13) | Out-Null
  [Gv.View]::SetForegroundWindow($h) | Out-Null
  Start-Sleep -Milliseconds 700

  $box = Get-ClientBox $h
  if (-not $box -or $box.W -lt 16 -or $box.H -lt 16) {
    [Gv.View]::SetWindowPos($h, [IntPtr](-2), 0, 0, 0, 0, 0x13) | Out-Null
    return $null
  }

  # A screenshot of coordinates is not a screenshot of a window. Whatever is
  # on top at the middle of the viewer's client area is what the shot will
  # contain, and on a machine somebody is using that is not always the viewer —
  # so it is asked rather than assumed, and a shot that would have been of
  # something else is reported as not taken rather than taken wrongly.
  $middle = New-Object Gv.View+POINT
  $middle.X = $box.X + [int]($box.W / 2)
  $middle.Y = $box.Y + [int]($box.H / 2)
  $ours = $false
  foreach ($try in 1..4) {
    $top = [Gv.View]::GetAncestor([Gv.View]::WindowFromPoint($middle), 2)
    if ($top -eq $h) { $ours = $true; break }
    [Gv.View]::SetForegroundWindow($h) | Out-Null
    Start-Sleep -Milliseconds 700
  }
  if (-not $ours) {
    [Gv.View]::SetWindowPos($h, [IntPtr](-2), 0, 0, 0, 0, 0x13) | Out-Null
    return [pscustomobject]@{
      Path     = $null
      Window   = 'not measured'
      Picture  = 'something was in front of the viewer'
      Aspect   = 0
      Fraction = 0
    }
  }

  $bmp = New-Object System.Drawing.Bitmap $box.W, $box.H
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($box.X, $box.Y, 0, 0, $bmp.Size)
  $g.Dispose()
  [Gv.View]::SetWindowPos($h, [IntPtr](-2), 0, 0, 0, 0, 0x13) | Out-Null
  $path = Join-Path $ShotDir "$name.png"
  $bmp.Save($path)

  $rect = New-Object System.Drawing.Rectangle 0, 0, $box.W, $box.H
  $data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly,
    [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $bytes = New-Object byte[] ($data.Stride * $box.H)
  [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $bytes, 0, $bytes.Length)
  $stride = $data.Stride
  $bmp.UnlockBits($data)
  $bmp.Dispose()

  # Against the fill, which is letterbox wherever there is any — and by how
  # *much* of the row differs rather than whether any of it does. The retro
  # skin paints a dot grid on that background: a handful of pixels a row sit a
  # few values off, and a rule that called any variation "picture" would call
  # the letterbox picture too.
  $far = 40
  $most = 0.2
  $step = 3
  $cols = [int][math]::Ceiling($box.W / $step)
  $colFar = New-Object int[] $cols
  $colSeen = New-Object int[] $cols

  # The letterbox's own colour — the value that fills the border of the client
  # area — read as the most common luminance along all four edges rather than
  # from the single corner pixel.
  #
  # The corner is only letterbox when there *is* letterbox, and `object-fit:
  # contain` leaves none at all in a window that is already the source's shape.
  # The "as opened" window is exactly that: a 16:9 viewer onto a 16:9 monitor,
  # picture edge to edge, corner included. Reading the reference there measures
  # the picture against its own top-left pixel, which is why a share of a flat
  # sheet came back as no picture at all (§7.14). Along the whole border the
  # letterbox still wins the count whenever there is any, and when there is
  # none the mode is one of the backdrop's two greys — which the other one is
  # far from, so the picture is still found.
  $tally = New-Object int[] 32
  for ($x = 0; $x -lt $box.W; $x += $step) {
    foreach ($y in @(0, ($box.H - 1))) {
      $i = $y * $stride + $x * 4
      $tally[[int]((($bytes[$i] + 5 * $bytes[$i + 1] + 2 * $bytes[$i + 2]) / 8) / 8)]++
    }
  }
  for ($y = 0; $y -lt $box.H; $y += $step) {
    foreach ($x in @(0, ($box.W - 1))) {
      $i = $y * $stride + $x * 4
      $tally[[int]((($bytes[$i] + 5 * $bytes[$i + 1] + 2 * $bytes[$i + 2]) / 8) / 8)]++
    }
  }
  $mode = 0
  for ($b = 1; $b -lt 32; $b++) { if ($tally[$b] -gt $tally[$mode]) { $mode = $b } }
  $fill = $mode * 8 + 4

  $minY = -1; $maxY = -1; $rowsLit = 0; $rowsSeen = 0
  for ($y = 0; $y -lt $box.H; $y += $step) {
    $row = $y * $stride
    $rowFar = 0; $rowSeen = 0; $ci = 0
    for ($x = 0; $x -lt $box.W; $x += $step) {
      $i = $row + $x * 4
      # Luminance, integer: blue, green, red as Windows stores them.
      $lum = [int](($bytes[$i] + 5 * $bytes[$i + 1] + 2 * $bytes[$i + 2]) / 8)
      $rowSeen++
      if ([math]::Abs($lum - $fill) -gt $far) {
        $rowFar++
        if ($ci -lt $cols) { $colFar[$ci]++ }
      }
      if ($ci -lt $cols) { $colSeen[$ci]++ }
      $ci++
    }
    $rowsSeen++
    if ($rowFar -gt ($rowSeen * $most)) {
      $rowsLit++
      if ($minY -lt 0) { $minY = $y }
      $maxY = $y
    }
  }

  $minX = -1; $maxX = -1; $colsLit = 0
  for ($i = 0; $i -lt $cols; $i++) {
    if ($colFar[$i] -gt ($colSeen[$i] * $most)) {
      $colsLit++
      if ($minX -lt 0) { $minX = $i * $step }
      $maxX = $i * $step
    }
  }

  $pw = if ($maxX -ge 0) { $maxX - $minX + $step } else { 0 }
  $ph = if ($maxY -ge 0) { $maxY - $minY + $step } else { 0 }
  return [pscustomobject]@{
    Path     = $path
    Window   = "$($box.W)x$($box.H)"
    Picture  = "$pw" + 'x' + "$ph"
    Aspect   = if ($ph -gt 0) { [math]::Round($pw / $ph, 3) } else { 0 }
    Fraction = [math]::Round(($rowsLit / [math]::Max($rowsSeen, 1)) * ($colsLit / [math]::Max($cols, 1)), 3)
  }
}

# --- something worth sharing ------------------------------------------------

# A grey chequered sheet over the monitor, for the length of the run.
#
# Not decoration, and the chequer is not decoration either.
#
# The aspect check below measures where the picture ends and the letterbox
# begins, and it can only do that if the two are different colours: share a
# desktop whose wallpaper is nearly black into a window whose theme background
# is nearly black and the boundary is unmeasurable, which is a fact about that
# desktop rather than about the viewer. Both greys here are far from any
# palette's `--bg`, light or dark.
#
# The squares answer the other half (§7.14). A window that is already the
# source's shape has no letterbox at all, so there is no boundary to find and
# the only honest question left is whether the whole client area is picture —
# which a *flat* sheet cannot answer, because a picture of one colour edge to
# edge and an empty window painted one colour are the same pixels. Two greys
# 80 apart put variation in every row and every column of the picture, so the
# picture is found by what it contains rather than by where it stops. 64-pixel
# squares survive the 1920 -> 1280 encode and the scale down into a
# 500-pixel-wide viewer with room to spare.
#
# The sheet also keeps whatever the person at the machine had on screen out of
# the screenshots.
function Start-Backdrop {
  $script = @'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$form = New-Object System.Windows.Forms.Form
$form.FormBorderStyle = 'None'
$form.BackColor = [System.Drawing.Color]::FromArgb(176, 176, 176)
$form.Bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$form.ShowInTaskbar = $false
$cell = 64
$dark = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(96, 96, 96))
$form.Add_Paint({
  param($sender, $e)
  for ($y = 0; $y -lt $sender.Height; $y += $cell) {
    for ($x = 0; $x -lt $sender.Width; $x += $cell) {
      if ((([int]($x / $cell)) + ([int]($y / $cell))) % 2 -eq 0) { continue }
      $e.Graphics.FillRectangle($dark, $x, $y, $cell, $cell)
    }
  }
})
# Topmost, because nothing else will raise it: Windows refuses the foreground
# to a process that did not have it, so a sheet started from a script sits
# wherever the z-order left it — which, on a machine with a maximised window
# open, is underneath.
$form.TopMost = $true
$label = New-Object System.Windows.Forms.Label
$label.Text = "goodvoice is running the task 5.4 viewer drill. This sheet is the screen being shared; it goes away on its own."
$label.AutoSize = $true
$label.Font = New-Object System.Drawing.Font('Segoe UI', 14)
$label.ForeColor = [System.Drawing.Color]::FromArgb(60, 60, 60)
$label.Location = New-Object System.Drawing.Point(40, 40)
$form.Controls.Add($label)
[System.Windows.Forms.Application]::Run($form)
'@
  $path = Join-Path $ShotDir 'backdrop.ps1'
  Set-Content -Path $path -Value $script -Encoding UTF8
  return Start-Process powershell -PassThru -WindowStyle Hidden `
    -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $path)
}

# --- the run ---------------------------------------------------------------

Write-Output "goodvoice viewer drill (plan.md 5.4)"
Write-Output "  room     $Room"
Write-Output "  exe      $Exe"
Write-Output "  shots    $ShotDir"
Write-Output ''

$backdrop = if ($NoBackdrop) { $null } else { Start-Backdrop }
$script:sheetWindow = $null
if ($backdrop) {
  Start-Sleep -Seconds 1
  $script:sheetWindow = Wait-For { Find-Sheet $backdrop.Id } 10
}

# The share first: a viewer that opens before there is anything to see is a
# window waiting for a keyframe, and this is not measuring that.
$drillLog = Join-Path $ShotDir 'drill.txt'
$drillSeconds = ($Cycles * ($OpenSeconds + $ClosedSeconds + 6)) + 40
$drill = Start-Process -FilePath $DrillExe -PassThru -WindowStyle Hidden `
  -ArgumentList @('--room', $Room, '--seconds', "$drillSeconds") `
  -RedirectStandardOutput $drillLog -RedirectStandardError (Join-Path $ShotDir 'drill.err.txt')
Say "the sharer is starting (pid $($drill.Id), $drillSeconds s)"

$live = Wait-For { if ((Read-Log $drillLog) -match 'live at') { $true } } 60
if (-not $live) {
  Write-Output 'SHARE_NEVER_WENT_LIVE'
  Read-Log $drillLog
  exit 3
}
$shape = ((Read-Log $drillLog) -match 'live at')[0].Trim()
Say "the sharer is $shape"

# Its output is worth keeping: an autojoin that failed, or a microphone the
# machine would not open, is invisible from the outside and looks exactly like
# a client that is in the room and silent.
$appLog = Join-Path $ShotDir 'app.txt'
$env:GOODVOICE_AUTOJOIN = $Room
$app = Start-Process -FilePath $Exe -PassThru `
  -RedirectStandardOutput $appLog -RedirectStandardError (Join-Path $ShotDir 'app.err.txt')
Remove-Item Env:\GOODVOICE_AUTOJOIN
Say "the app is starting (pid $($app.Id))"

$root = Wait-For { Get-Root $app.Id } 40
if (-not $root) { Write-Output 'NO_MAIN_WINDOW'; exit 4 }
Say 'the main window is up, its tree is warm'

if ($Skin) {
  $settings = Wait-For { Find-Button $root '^\s*settings\s*$' } 15
  if (-not $settings) { Write-Output 'NO_SETTINGS_BUTTON'; exit 5 }
  Invoke-Button $settings | Out-Null
  Start-Sleep -Seconds 1
  $root = Get-Root $app.Id
  $label = if ($Skin -eq 'retro') { 'neobrutal' } else { 'terminal' }
  $pick = Wait-For { Find-Button $root "^\s*$label\s*$" } 15
  if (-not $pick) { Write-Output "NO_SKIN_BUTTON=$label"; exit 5 }
  Invoke-Button $pick | Out-Null
  Start-Sleep -Seconds 1
  $root = Get-Root $app.Id
  $back = Find-Button $root '^\s*back\s*$'
  if ($back) { Invoke-Button $back | Out-Null }
  Start-Sleep -Seconds 1
  $root = Get-Root $app.Id
  Say "the skin is $Skin ($label)"
}

# The button only exists while somebody else is sharing, so its arrival is also
# the roster saying the share reached the app.
$watch = Wait-For { Find-Button (Get-Root $app.Id) 'watch .*screen' } 60
if (-not $watch) { Write-Output 'NO_WATCH_BUTTON'; exit 6 }
Say "the app offers `"$($watch.Current.Name)`""

$results = @()
for ($cycle = 1; $cycle -le $Cycles; $cycle++) {
  $watch = Wait-For { Find-Button (Get-Root $app.Id) 'watch .*screen' } 20
  if (-not $watch) { Write-Output "WATCH_BUTTON_GONE cycle=$cycle"; exit 6 }
  Invoke-Button $watch | Out-Null

  $viewer = Wait-For { Find-Viewer $app.Id } 20
  if (-not $viewer) { Write-Output "NO_VIEWER cycle=$cycle"; exit 7 }
  Say "cycle ${cycle}: the viewer is open"
  Start-Sleep -Seconds $OpenSeconds

  $shot = Measure-Picture $viewer.Handle "cycle$cycle-open"
  Say ("cycle ${cycle}: window $($shot.Window), picture $($shot.Picture), aspect $($shot.Aspect), $([int]($shot.Fraction * 100))% of it lit")
  $results += [pscustomobject]@{ Cycle = $cycle; Shape = 'as opened'; Shot = $shot }

  # One cycle gets stretched into a shape the picture is not, and one gets
  # squashed into the other one. `object-fit: contain` is what has to hold.
  if ($cycle -eq 2 -or $cycle -eq 3) {
    $r = New-Object Gv.View+RECT
    [Gv.View]::GetWindowRect($viewer.Handle, [ref] $r) | Out-Null
    $wide = ($cycle -eq 2)
    $w = if ($wide) { 1100 } else { 520 }
    $h = if ($wide) { 420 } else { 700 }
    [Gv.View]::MoveWindow($viewer.Handle, $r.L, $r.T, $w, $h, $true) | Out-Null
    Start-Sleep -Seconds 3
    $shape2 = if ($wide) { 'stretched wide' } else { 'squashed tall' }
    $shot2 = Measure-Picture $viewer.Handle "cycle$cycle-$(if ($wide) { 'wide' } else { 'tall' })"
    Say ("cycle ${cycle}: $shape2 to $($shot2.Window), picture $($shot2.Picture), aspect $($shot2.Aspect)")
    $results += [pscustomobject]@{ Cycle = $cycle; Shape = $shape2; Shot = $shot2 }
  }

  # WM_CLOSE, which is what the window's own close button sends. The viewer is
  # meant to be destroyed by it — `tray::window_event` ignores every window but
  # `main`, so this one does not go to the tray (task 4.6).
  [Gv.View]::PostMessage($viewer.Handle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
  $gone = Wait-For { if (-not (Find-Viewer $app.Id)) { $true } } 15
  if (-not $gone) { Write-Output "VIEWER_WOULD_NOT_CLOSE cycle=$cycle"; exit 8 }
  Say "cycle ${cycle}: the viewer is closed"
  Start-Sleep -Seconds $ClosedSeconds
}

Say 'done clicking; waiting for the sharer to finish counting'
# The app stays in the room until the counting stops. Killing it first would
# stop the frames a heartbeat before the roster noticed, and the drill would
# read its own teardown as a glitch.
$drill | Wait-Process -Timeout ($drillSeconds + 60)
$app | Stop-Process -Force
if ($backdrop) { $backdrop | Stop-Process -Force }

Write-Output ''
Write-Output '### what the viewer showed'
Write-Output ''
Write-Output ('  {0,-6} {1,-15} {2,-12} {3,-12} {4,-7} {5}' -f 'cycle', 'shape', 'window', 'picture', 'aspect', 'lit')
foreach ($r in $results) {
  Write-Output ('  {0,-6} {1,-15} {2,-12} {3,-12} {4,-7} {5}' -f `
      $r.Cycle, $r.Shape, $r.Shot.Window, $r.Shot.Picture, $r.Shot.Aspect, $r.Shot.Fraction)
}
Write-Output ''
Write-Output '### what the far end heard'
Write-Output ''
Read-Log $drillLog
$err = Read-Log (Join-Path $ShotDir 'drill.err.txt')
if ($err) { Write-Output ''; Write-Output '### the sharer complained'; $err }
Write-Output ''
Write-Output '### what the app said'
Write-Output ''
Read-Log $appLog
$appErr = Read-Log (Join-Path $ShotDir 'app.err.txt')
if ($appErr) { $appErr }
Write-Output ''
Write-Output "shots in $ShotDir"
