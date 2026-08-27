# plan.md 6.5 / DR-47: the window changes language, and the tray menu with it.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\language.ps1
#   ... -Installed path\to\goodvoice-client.exe
#
# The dictionary half of two languages needs no drill: a missing string does
# not compile (strings.ts is an interface each language satisfies). What needs
# one is the half the type checker cannot reach — **the tray menu is written in
# Rust, built before any webview exists, and told what language to be in by a
# window that mounts later** (DR-47). Nothing about that is checked by a build,
# and its failure mode is quiet: a menu in English hanging off a window in
# Portuguese.
#
# So this drives the real app through the picker and then reads the *real
# HMENU* behind the tray popup, which is the only way to see menu text from
# outside the process — the popup is a `#32768` pane that UI Automation reports
# with no children at all (tray.md). Both directions, because a relabel that
# works once and not back is a relabel that has hard-coded something.
#
# Needs an INSTALLED build, or any release build with `custom-protocol`
# (DR-22), and a desktop that will accept injected input — read POINTER= before
# believing a failure, and see tray.md's "When the pointer is refused" (DR-39).
[CmdletBinding()]
param(
  [string] $Installed = "$env:LOCALAPPDATA\goodvoice\goodvoice-client.exe",
  [string] $Shot = 'docs\ui\settings-language-ptbr.png'
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type -Namespace Gv -Name Lang -MemberDefinition @'
[DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr hWnd, uint msg, IntPtr wp, IntPtr lp);
[DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr menu);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool GetMenuItemInfo(IntPtr menu, uint item, bool byPosition, ref MENUITEMINFO info);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int cmd);
[DllImport("user32.dll", SetLastError=true)] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, int data, UIntPtr extra);
[DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr hWnd, int attr, out RECT r, int size);
public struct RECT { public int L, T, R, B; }
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
'@

if (-not (Test-Path $Installed)) { Write-Output "NOT_INSTALLED=$Installed"; exit 2 }

$uiaAny = [System.Windows.Automation.Condition]::TrueCondition
$uiaRoot = [System.Windows.Automation.AutomationElement]::RootElement
$script:t0 = Get-Date
$script:failures = 0

function Say([string] $line) {
  Write-Output ("  t+{0,3}s  {1}" -f [int]((Get-Date) - $script:t0).TotalSeconds, $line)
}

function Check([string] $name, [bool] $ok, [string] $detail) {
  if (-not $ok) { $script:failures++ }
  Write-Output ("  {0,-22} {1}  {2}" -f $name, $(if ($ok) { 'PASS' } else { 'FAIL' }), $detail)
}

function App { Get-Process goodvoice-client -EA SilentlyContinue | Select-Object -First 1 }

function Elements {
  $app = App
  if (-not $app -or $app.MainWindowHandle -eq 0) { return @() }
  try {
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($app.MainWindowHandle)
    # Walked twice: the first walk is what wakes WebView2's tree up (DR-26).
    $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny) | Out-Null
    Start-Sleep -Milliseconds 700
    return @($root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny))
  } catch { return @() }
}

function Names([object[]] $elements) {
  return @($elements | ForEach-Object { $_.Current.Name } | Where-Object { $_ })
}

# A real mouse, never `InvokePattern`: UI Automation offers one on every button
# in this window and calling it returns success and does nothing, because the
# window is a WebView2 and what it acts on is input (DR-26).
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
    $frame = New-Object Gv.Lang+RECT
    [Gv.Lang]::GetWindowRect($window, [ref] $frame) | Out-Null
    # Kept even though `Reach` has already scrolled: UI Automation reports a
    # rectangle for things below the fold as happily as for things on screen,
    # and a click on one of those lands on the desktop. This is the check that
    # caught `ScrollIntoView` doing nothing — see `Wheel`.
    if ($x -lt $frame.L -or $x -gt $frame.R -or $y -lt $frame.T -or $y -gt $frame.B) {
      return $false
    }
    [Gv.Lang]::ShowWindow($window, 9) | Out-Null
    [Gv.Lang]::SetForegroundWindow($window) | Out-Null
    Start-Sleep -Milliseconds 400
    [Gv.Lang]::SetCursorPos($x, $y) | Out-Null
    Start-Sleep -Milliseconds 200
    [Gv.Lang]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 80
    [Gv.Lang]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 800
    return $true
  } catch { return $false }
}

# ---------------------------------------------------------------------------
# Reaching a control below the fold
#
# `.shell` is the scrolling element and it holds the whole screen, masthead
# included — and the settings screen is a little over twice the height of the
# window it is in, so `language` and its two buttons are ~570 px below the
# bottom edge on a default-sized window.
#
# **`ScrollItemPattern` does not do it.** WebView2 offers the pattern on these
# elements and `ScrollIntoView()` returns without throwing and without
# scrolling, which is the same trap `InvokePattern` sets on the buttons
# themselves (DR-26): an API that answers and does nothing. The first version
# of this drill trusted it, and `Click-Element`'s bounds check is what caught
# the lie — every language click was silently refused for being 570 px outside
# the window, and the drill reported the app had not changed language.
#
# So: a real wheel, over the window, re-reading the rectangle after every
# notch because it has moved.
function Wheel([int] $notches) {
  $window = (App).MainWindowHandle
  $frame = New-Object Gv.Lang+RECT
  [Gv.Lang]::GetWindowRect($window, [ref] $frame) | Out-Null
  [Gv.Lang]::ShowWindow($window, 9) | Out-Null
  [Gv.Lang]::SetForegroundWindow($window) | Out-Null
  Start-Sleep -Milliseconds 250
  [Gv.Lang]::SetCursorPos([int](($frame.L + $frame.R) / 2), [int](($frame.T + $frame.B) / 2)) | Out-Null
  Start-Sleep -Milliseconds 150
  # WHEEL. `dwData` is a DWORD in the header and a *signed* wheel delta in
  # fact, so it is declared `int` above: widening it to `uint` here means
  # writing the two's complement by hand, and `-band 0xFFFFFFFF` does not do
  # that in PowerShell — `0xFFFFFFFF` parses as the int -1, so -360 stays -360
  # and the cast throws.
  [Gv.Lang]::mouse_event(0x0800, 0, 0, 120 * $notches, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 450
}

# The named element, scrolled until its middle is inside the window, or $null.
function Reach([string] $name, [int] $Tries = 16) {
  for ($try = 0; $try -lt $Tries; $try++) {
    $found = $null
    foreach ($e in Elements) {
      if ($e.Current.Name -imatch "^\s*$name\s*$") { $found = $e; break }
    }
    if (-not $found) { return $null }
    $box = $found.Current.BoundingRectangle
    if ($box.Width -le 0 -or $box.Height -le 0) { return $null }
    $window = (App).MainWindowHandle
    $frame = New-Object Gv.Lang+RECT
    [Gv.Lang]::GetWindowRect($window, [ref] $frame) | Out-Null
    $cy = [int]($box.Y + $box.Height / 2)
    # A margin off each edge, so the click lands on the control rather than on
    # the pixel of it that happens to be showing.
    #
    # **The band and the direction are the same two numbers**, and getting that
    # wrong is a hang rather than a miss: with the band `B - 40` and the
    # direction `> B`, anything landing in the 40 px between them is "not there
    # yet" *and* "scroll up", so the drill wheeled back to where it came from
    # and then down again forever. It cost sixteen tries and two Press-Until
    # rounds before it gave up and reported the app had not changed language.
    $top = $frame.T + 40
    $bottom = $frame.B - 40
    if ($cy -ge $top -and $cy -le $bottom) { return $found }
    Wheel $(if ($cy -gt $bottom) { -3 } else { 3 })
  }
  return $null
}

function Press([string] $name, [int] $Seconds = 30) {
  $deadline = (Get-Date).AddSeconds($Seconds)
  while ((Get-Date) -lt $deadline) {
    $target = Reach $name
    if ($target -and (Click-Element $target)) { return $true }
    Start-Sleep -Milliseconds 500
  }
  return $false
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

function Press-Until([string] $name, [scriptblock] $Landed, [int] $Tries = 3) {
  for ($try = 1; $try -le $Tries; $try++) {
    if (-not (Press $name)) { continue }
    $said = Waiting $Landed 12
    if ($said.Count -gt 0 -and (& $Landed $said)) { return $true }
  }
  return $false
}

# ---------------------------------------------------------------------------
# The tray menu, read out of the HMENU the popup is drawing (tray-menu.ps1).
# ---------------------------------------------------------------------------
$MN_GETHMENU = 0x01E1
$MIIM_STATE = 0x00000001
$MIIM_STRING = 0x00000040
$MIIM_FTYPE = 0x00000100
$MFT_SEPARATOR = 0x00000800

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

# The five item texts, in order, or @() with the reason in $script:menuStep.
function Read-TrayMenu {
  $button = Show-TrayIcon 'goodvoice'
  if (-not $button) { $script:menuStep = 'no-icon'; return @() }
  $rect = $button.Current.BoundingRectangle
  if (-not [Gv.Lang]::SetCursorPos([int]($rect.X + $rect.Width / 2), [int]($rect.Y + $rect.Height / 2))) {
    $script:menuStep = 'no-pointer'; return @()
  }
  Start-Sleep -Milliseconds 300
  [Gv.Lang]::mouse_event(0x0008, 0, 0, 0, [UIntPtr]::Zero)
  [Gv.Lang]::mouse_event(0x0010, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Seconds 1
  $popup = $uiaRoot.FindAll([System.Windows.Automation.TreeScope]::Children, $uiaAny) |
    Where-Object { $_.Current.ClassName -eq '#32768' } | Select-Object -First 1
  if (-not $popup) { $script:menuStep = 'no-menu'; return @() }

  $hwnd = [IntPtr] $popup.Current.NativeWindowHandle
  $menu = [Gv.Lang]::SendMessage($hwnd, $MN_GETHMENU, [IntPtr]::Zero, [IntPtr]::Zero)
  $texts = @()
  if ($menu -ne [IntPtr]::Zero) {
    $count = [Gv.Lang]::GetMenuItemCount($menu)
    for ($i = 0; $i -lt $count; $i++) {
      $info = New-Object Gv.Lang+MENUITEMINFO
      $info.cbSize = [Runtime.InteropServices.Marshal]::SizeOf([type][Gv.Lang+MENUITEMINFO])
      $info.fMask = $MIIM_STATE -bor $MIIM_STRING -bor $MIIM_FTYPE
      $info.dwTypeData = [IntPtr]::Zero
      [Gv.Lang]::GetMenuItemInfo($menu, $i, $true, [ref] $info) | Out-Null
      if (($info.fType -band $MFT_SEPARATOR) -ne 0) { continue }
      if ($info.cch -le 0) { continue }
      $len = [int]$info.cch + 1
      $buffer = [Runtime.InteropServices.Marshal]::AllocHGlobal($len * 2)
      $info.cbSize = [Runtime.InteropServices.Marshal]::SizeOf([type][Gv.Lang+MENUITEMINFO])
      $info.fMask = $MIIM_STATE -bor $MIIM_STRING -bor $MIIM_FTYPE
      $info.dwTypeData = $buffer
      $info.cch = $len
      if ([Gv.Lang]::GetMenuItemInfo($menu, $i, $true, [ref] $info)) {
        $texts += [Runtime.InteropServices.Marshal]::PtrToStringUni($buffer)
      }
      [Runtime.InteropServices.Marshal]::FreeHGlobal($buffer)
    }
  }
  # Escape, or the popup keeps the pointer and every later click is eaten.
  [Gv.Lang]::keybd_event(0x1B, 0, 0, [UIntPtr]::Zero)
  [Gv.Lang]::keybd_event(0x1B, 0, 2, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 500
  $script:menuStep = 'ok'
  return $texts
}

function Save-Shot([string] $path) {
  $window = (App).MainWindowHandle
  [Gv.Lang]::SetForegroundWindow($window) | Out-Null
  Start-Sleep -Seconds 2
  # The frame as drawn, not as sized: `GetWindowRect` includes the invisible
  # resize border, which in a screenshot is somebody else's window.
  $box = New-Object Gv.Lang+RECT
  $size = [System.Runtime.InteropServices.Marshal]::SizeOf([type][Gv.Lang+RECT])
  if ([Gv.Lang]::DwmGetWindowAttribute($window, 9, [ref] $box, $size) -ne 0) {
    [Gv.Lang]::GetWindowRect($window, [ref] $box) | Out-Null
  }
  $w = $box.R - $box.L
  $h = $box.B - $box.T
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($box.L, $box.T, 0, 0, $bmp.Size)
  $g.Dispose()
  $bmp.Save((Resolve-Path -LiteralPath (Split-Path $path)).Path + '\' + (Split-Path $path -Leaf))
  $bmp.Dispose()
  return "$w x $h"
}

function Stop-App {
  Get-Process goodvoice-client -EA SilentlyContinue | Stop-Process -Force
  # A force-killed WASAPI stream keeps its endpoint busy for a moment (DR-37).
  Start-Sleep -Seconds 5
}

Write-Output 'goodvoice language drill (plan.md 6.5, DR-47)'
Write-Output "  installed  $Installed"
Write-Output ''

Stop-App
Start-Process -FilePath $Installed
$said = Waiting { param($it) @($it | Where-Object { $_ -imatch '^\s*(settings|ajustes)\s*$' }).Count -gt 0 } 60
if ($said.Count -eq 0) { Write-Output 'WINDOW_NEVER_CAME_UP'; Stop-App; exit 3 }
Say 'the window is up'

# Which language it started in. Either is a pass — the machine's own locale is
# the input on a fresh install — and it is what the drill switches *away* from.
$startedPt = @($said | Where-Object { $_ -imatch '^\s*ajustes\s*$' }).Count -gt 0
$startedIn = if ($startedPt) { 'pt-BR' } else { 'en' }
Say "it started in $startedIn"

$onLanguageRow = { param($it) @($it | Where-Object { $_ -imatch '^\s*(language|idioma)\s*$' }).Count -gt 0 }
if (-not (Press-Until $(if ($startedPt) { 'ajustes' } else { 'settings' }) $onLanguageRow)) {
  Write-Output "NO_SETTINGS_SCREEN: [$((Names (Elements)) -join ' | ')]"; Stop-App; exit 4
}
Say 'the settings screen is open, and it has a language row'

# ---- to Portuguese --------------------------------------------------------
# `português` is the endonym and is not translated, so the button is named the
# same thing in either language — which is the point of an endonym.
#
# **Matched case-insensitively, and that is not laziness.** `.section` and
# `.field-label` are `text-transform: uppercase`, and a CSS transform reaches
# the accessible name: the tree says `APPEARANCE`, never `appearance`. A
# case-sensitive check here failed against a window that had changed language
# perfectly well, which is a drill lying about the app.
$inPt = { param($it) @($it | Where-Object { $_ -imatch '^aparência$' }).Count -gt 0 }
Check 'PT_WINDOW' (Press-Until 'português' $inPt) 'the window is in Portuguese'
$names = Names (Elements)
$ptWords = @($names | Where-Object { $_ -imatch '^(idioma|aparência|áudio|servidor|paleta|pele|modo|transmissão|pronto|claro|escuro|sistema)$' })
Check 'PT_WORDS' ($ptWords.Count -ge 5) ("saw: " + ($ptWords -join ', '))

$shot = Save-Shot $Shot
Say "saved $Shot ($shot)"

$ptMenu = Read-TrayMenu
Write-Output "  POINTER=$script:menuStep"
Check 'PT_TRAY' (
  $ptMenu.Count -eq 5 -and ($ptMenu -join '|') -cmatch 'Abrir o goodvoice' -and
  ($ptMenu -join '|') -cmatch 'Silenciar' -and ($ptMenu -join '|') -cmatch 'Sair da sala'
) ("menu: " + ($ptMenu -join ' | '))

# ---- and back to English --------------------------------------------------
# Both directions, because a relabel that only works once has hard-coded
# something — and because the fresh-install path (DR-47) only ever exercises
# the way *in*.
$inEn = { param($it) @($it | Where-Object { $_ -imatch '^appearance$' }).Count -gt 0 }
Check 'EN_WINDOW' (Press-Until 'english' $inEn) 'the window is back in English'

$enMenu = Read-TrayMenu
Check 'EN_TRAY' (
  $enMenu.Count -eq 5 -and ($enMenu -join '|') -cmatch 'Open goodvoice' -and
  ($enMenu -join '|') -cmatch 'Leave room'
) ("menu: " + ($enMenu -join ' | '))

Stop-App

# ---- and it is remembered -------------------------------------------------
# The last thing chosen above was English, so this run should come up English
# whatever the machine's locale is — which is the only way to tell "it was
# stored" apart from "it was detected again".
Start-Process -FilePath $Installed
$again = Waiting { param($it) @($it | Where-Object { $_ -imatch '^\s*(settings|ajustes)\s*$' }).Count -gt 0 } 60
Check 'REMEMBERED' (
  @($again | Where-Object { $_ -imatch '^settings$' }).Count -gt 0
) ("second run came up in " + $(if (@($again | Where-Object { $_ -imatch '^ajustes$' }).Count -gt 0) { 'pt-BR' } else { 'en' }))

Stop-App
Write-Output ''
Write-Output ("RESULT=" + $(if ($script:failures -eq 0) { 'PASS' } else { "FAIL ($script:failures)" }))
exit $(if ($script:failures -eq 0) { 0 } else { 1 })
