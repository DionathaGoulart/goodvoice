# plan.md 7.4: what the share picker looks like under the `retro` skin.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\share-picker-shot.ps1
#
# Task 5.3 shipped `docs/ui/share-picker.png` and `docs/ui/share-live.png` and
# both are the `terminal` skin. The picker's CSS is the shared base plus a
# `terminal` block, which makes `retro` the *unmodified base* — the one nobody
# had looked at. This drives the real app into the picker with `retro` selected
# and saves the shot, so that looking at it is a thing somebody can repeat
# rather than a thing that happened once.
#
# Driven the way the other drills drive this window: UI Automation, on the
# installed app, with the tree walked twice because WebView2 builds it lazily
# (DR-26). The skin is chosen by *clicking it in the settings screen* rather
# than by writing the webview's `localStorage` from outside — the point is what
# a person sees after doing what a person does.
[CmdletBinding()]
param(
  [string] $Installed = "$env:LOCALAPPDATA\goodvoice\goodvoice-client.exe",
  [string] $Room = 'pickershot',
  [string] $Skin = 'neobrutal',
  [string] $Shot = 'docs\ui\share-picker-retro.png'
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type -Namespace Gv -Name Shot -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int cmd);
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr hWnd, int attr, out RECT r, int size);
public struct RECT { public int L, T, R, B; }
'@

if (-not (Test-Path $Installed)) { Write-Output "NOT_INSTALLED=$Installed"; exit 2 }

$uiaAny = [System.Windows.Automation.Condition]::TrueCondition
$script:t0 = Get-Date
function Say([string] $line) {
  Write-Output ("  t+{0,3}s  {1}" -f [int]((Get-Date) - $script:t0).TotalSeconds, $line)
}

function App { Get-Process goodvoice-client -EA SilentlyContinue | Select-Object -First 1 }

# Every element the window is currently showing. Walked twice for DR-26's
# reason: the first walk is what wakes the accessibility tree up.
function Elements {
  $app = App
  if (-not $app -or $app.MainWindowHandle -eq 0) { return @() }
  try {
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($app.MainWindowHandle)
    $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny) | Out-Null
    Start-Sleep -Milliseconds 700
    return @($root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny))
  } catch { return @() }
}

function Names([object[]] $elements) {
  return @($elements | ForEach-Object { $_.Current.Name } | Where-Object { $_ })
}

# Clicks the middle of the first thing named this, with a real mouse.
#
# **Not `InvokePattern`.** UI Automation offers one on every button in this
# window and calling it returns success and does nothing: the window is a
# WebView2, and what it acts on is input, not automation patterns. A drill that
# trusted the return value walked away believing it had opened the settings
# screen. A synthesized click is also nearer to the thing being documented —
# these screenshots are of what a person sees after doing what a person does.
function Press([string] $name, [int] $Seconds = 20) {
  $deadline = (Get-Date).AddSeconds($Seconds)
  while ((Get-Date) -lt $deadline) {
    foreach ($e in Elements) {
      if ($e.Current.Name -inotmatch "^\s*$name\s*$") { continue }
      if (Click-Element $e) { return $true }
    }
    Start-Sleep -Milliseconds 500
  }
  return $false
}

# Clicks, then waits for the window to show it landed, and tries again if it
# did not. Every click here is on something whose own name disappears when it
# works, so "did it land" is a question about the *next* screen.
function Press-Until([string] $name, [scriptblock] $Landed, [int] $Tries = 3) {
  for ($try = 1; $try -le $Tries; $try++) {
    if (-not (Press $name)) { return $false }
    $said = Waiting $Landed 12
    if ($said.Count -gt 0 -and (& $Landed $said)) { return $true }
  }
  return $false
}

# The middle of whatever this element occupies on screen.
#
# **Scrolled to first, and never clicked outside the window.** The settings
# screen is taller than the window it is in, and UI Automation reports a
# bounding rectangle for things below the fold as happily as for things on
# screen — so a drill that clicked what it was told clicked *the desktop*, or
# whatever else happened to be at that spot, and then read a settings screen
# that had not changed and blamed the app. Both halves are needed: `ScrollItem`
# to bring it into view, and the check that the point really is inside this
# window before any button goes down.
function Click-Element($element) {
  try {
    try {
      $element.GetCurrentPattern(
        [System.Windows.Automation.ScrollItemPattern]::Pattern).ScrollIntoView()
      Start-Sleep -Milliseconds 500
    } catch { }
    $box = $element.Current.BoundingRectangle
    if ($box.Width -le 0 -or $box.Height -le 0) { return $false }
    $x = [int]($box.X + $box.Width / 2)
    $y = [int]($box.Y + $box.Height / 2)
    $window = (App).MainWindowHandle
    $frame = New-Object Gv.Shot+RECT
    [Gv.Shot]::GetWindowRect($window, [ref] $frame) | Out-Null
    if ($x -lt $frame.L -or $x -gt $frame.R -or $y -lt $frame.T -or $y -gt $frame.B) {
      return $false
    }
    [Gv.Shot]::ShowWindow($window, 9) | Out-Null   # SW_RESTORE, in case of the tray
    [Gv.Shot]::SetForegroundWindow($window) | Out-Null
    Start-Sleep -Milliseconds 400
    [Gv.Shot]::SetCursorPos($x, $y) | Out-Null
    Start-Sleep -Milliseconds 200
    [Gv.Shot]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)   # left down
    Start-Sleep -Milliseconds 80
    [Gv.Shot]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)   # left up
    Start-Sleep -Milliseconds 800
    return $true
  } catch { return $false }
}

function Waiting([scriptblock] $Answered, [int] $Seconds = 40) {
  $deadline = (Get-Date).AddSeconds($Seconds)
  $said = @()
  while ((Get-Date) -lt $deadline) {
    $said = Names (Elements)
    if ($said.Count -gt 0 -and (& $Answered $said)) { return $said }
    Start-Sleep -Milliseconds 500
  }
  return $said
}

function Stop-App {
  Get-Process goodvoice-client -EA SilentlyContinue | Stop-Process -Force
  # A force-killed WASAPI stream keeps its endpoint busy for a moment, and the
  # next launch joins with a microphone (invite.ps1, DR-37).
  Start-Sleep -Seconds 5
}

Write-Output 'goodvoice share-picker shot, retro skin (plan.md 7.4)'
Write-Output "  installed  $Installed"
Write-Output ''

Stop-App

# In a room, because the picker only exists inside a call. The same room and
# the same joined-without-a-window path as `docs/ui/share-picker.png`.
$env:GOODVOICE_AUTOJOIN = $Room
Start-Process -FilePath $Installed
$said = Waiting { param($it) @($it | Where-Object { $_ -imatch "^\s*room\s+$Room\s*$" }).Count -gt 0 } 60
if ($said.Count -eq 0 -or -not @($said | Where-Object { $_ -imatch "^\s*room\s+$Room\s*$" })) {
  Write-Output "NEVER_JOINED: [$($said -join ' | ')]"; Stop-App; exit 3
}
Say "in $Room"

$onSkins = { param($it) @($it | Where-Object { $_ -imatch '^\s*skin\s*$' }).Count -gt 0 }
if (-not (Press-Until 'settings' $onSkins)) { Write-Output 'NO_SETTINGS_SCREEN'; Stop-App; exit 4 }
Say 'the settings screen is open'

# The terminal skin uppercases every label, including the accessible names
# (DR-26); retro leaves them alone. So "did the skin change" is answered by the
# window's own casing, which is the one thing about it that is machine-readable.
$asWritten = { param($it) @($it | Where-Object { $_ -cmatch '^share a screen$' }).Count -gt 0 }
# The hint under the two skins belongs to whichever is selected, so it is the
# window saying which one it is now wearing.
$onRetro = { param($it) @($it | Where-Object { $_ -imatch 'thick frame, hard shadow' }).Count -gt 0 }
if (-not (Press-Until $Skin $onRetro)) { Write-Output "SKIN_WOULD_NOT_CHANGE: $Skin"; Stop-App; exit 5 }
Say "skin: $Skin"
if (-not (Press-Until 'done' $asWritten)) {
  Write-Output "SETTINGS_WOULD_NOT_CLOSE: [$((Names (Elements)) -join ' | ')]"; Stop-App; exit 6
}
Say 'back in the room, and the window is no longer shouting — this is retro'

$onPicker = { param($it) @($it | Where-Object { $_ -imatch 'what to share' }).Count -gt 0 }
if (-not (Press-Until 'share a screen' $onPicker)) {
  Write-Output 'PICKER_DID_NOT_OPEN'; Stop-App; exit 8
}
Say 'the picker is open'

$window = (App).MainWindowHandle
[Gv.Shot]::SetForegroundWindow($window) | Out-Null
Start-Sleep -Seconds 2
# The frame as it is *drawn*, not as it is sized. `GetWindowRect` includes the
# invisible resize border a composited window carries — eight pixels a side of
# whatever happens to be behind it, which in a screenshot is somebody else's
# window down the left edge.
$box = New-Object Gv.Shot+RECT
$DWMWA_EXTENDED_FRAME_BOUNDS = 9
$size = [System.Runtime.InteropServices.Marshal]::SizeOf([type][Gv.Shot+RECT])
if ([Gv.Shot]::DwmGetWindowAttribute($window, $DWMWA_EXTENDED_FRAME_BOUNDS, [ref] $box, $size) -ne 0) {
  [Gv.Shot]::GetWindowRect($window, [ref] $box) | Out-Null
}
$w = $box.R - $box.L
$h = $box.B - $box.T
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($box.L, $box.T, 0, 0, $bmp.Size)
$g.Dispose()
$bmp.Save((Resolve-Path -LiteralPath (Split-Path $Shot)).Path + '\' + (Split-Path $Shot -Leaf))
$bmp.Dispose()
Say "saved $Shot ($w x $h)"

Stop-App
Write-Output ''
Write-Output 'RESULT=SHOT'
