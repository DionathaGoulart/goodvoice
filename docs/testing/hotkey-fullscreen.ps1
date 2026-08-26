# plan.md §7.5: the talk key over a fullscreen game, without a game or a person.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\hotkey-fullscreen.ps1
#   ... -Key F13 -Room squad-42
#   ... -Windowed        # the same walk with the display left alone (hotkey.md row 3)
#
# hotkey.md has had two rows waiting on "a person and a game" since task 4.3:
# the key still reaching whatever had focus, and both of those holding inside a
# fullscreen game. Neither needs a game. A game in fullscreen exclusive is a
# D3D11 swap chain with `SetFullscreenState(TRUE)` on it, and `bin/fullscreen-drill`
# is one — so the three things §7.5's definition of done asks about are all
# readable from here:
#
#   the display is taken    fullscreen-drill's MODE=exclusive, which is DXGI's
#                           own answer, plus the foreground window being its
#   the key opens the mic   `bin/listener` in the same room, whose frames/s
#                           column goes from nothing to a stream and back. A
#                           gated client sends *no packets* rather than silent
#                           ones (bin/mute-drill), so this is unambiguous
#   the game still gets it  the drill's own DOWNS/UPS: goodvoice's hook passes
#                           every key on (tray/hotkey.rs), and this is the
#                           window that would stop seeing them if it did not
#
# What it is not: an anti-cheat. No game is loaded, and DR-18 is where that
# argument lives. What is measured is Windows' input path and DXGI's display
# ownership, which is what "over a fullscreen game" means for this feature.
#
# Needs a RELEASE build with the `custom-protocol` feature — a debug build
# points the webview at a dev server and every name read below is Edge's error
# page (DR-22). And it needs a desktop that will accept injected input: read
# POINTER= before believing anything else.
[CmdletBinding()]
param(
  [string] $Exe = "$env:CARGO_TARGET_DIR\release\goodvoice-client.exe",
  [string] $ListenerExe = "$env:CARGO_TARGET_DIR\release\listener.exe",
  [string] $DrillExe = "$env:CARGO_TARGET_DIR\release\fullscreen-drill.exe",
  [string] $Room = ('ptt-' + (Get-Random -Minimum 10000 -Maximum 99999)),
  # F13 by default for the reason hotkey.ps1 picks it: no keyboard here has
  # one, so nothing else on the desktop is listening for it.
  [string] $Key = 'F13',
  [switch] $Windowed,
  [string] $Out = "$env:TEMP\goodvoice-hotkey-fullscreen"
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

Add-Type -Namespace Gv -Name Ptt -MemberDefinition @'
[DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
[DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code, uint mapType);
[DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
[DllImport("user32.dll", SetLastError=true)] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int cmd);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
[DllImport("user32.dll")] public static extern int GetClassName(IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr hWnd, out int pid);
[DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
[DllImport("advapi32.dll", SetLastError=true)] public static extern bool OpenProcessToken(IntPtr proc, uint access, out IntPtr token);
[DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
public struct RECT { public int L, T, R, B; }
public struct POINT { public int X, Y; }
public delegate bool EnumProc(IntPtr hWnd, IntPtr lp);
'@

# --- what to run -----------------------------------------------------------

function Fallback([string] $path, [string] $name) {
  if (Test-Path $path) { return $path }
  $debug = "$env:CARGO_TARGET_DIR\debug\$name"
  if (Test-Path $debug) { return $debug }
  $local = "client\src-tauri\target\release\$name"
  if (Test-Path $local) { return $local }
  return $path
}
$Exe = Fallback $Exe 'goodvoice-client.exe'
$ListenerExe = Fallback $ListenerExe 'listener.exe'
$DrillExe = Fallback $DrillExe 'fullscreen-drill.exe'
foreach ($pair in @(@('APP', $Exe), @('LISTENER', $ListenerExe), @('DRILL', $DrillExe))) {
  if (-not (Test-Path $pair[1])) { Write-Output ("NO_" + $pair[0] + "=" + $pair[1]); exit 2 }
}
New-Item -ItemType Directory -Force -Path $Out | Out-Null

$script:ok = $true
$script:t0 = Get-Date
function Say([string] $line) {
  Write-Output ("  t+{0,3}s  {1}" -f [int]((Get-Date) - $script:t0).TotalSeconds, $line)
}
function Check([string] $label, [bool] $pass, [string] $value) {
  Write-Output ("{0}={1}{2}" -f $label, $value, $(if ($pass) { '' } else { '   <-- NOT WHAT 7.5 ASKS FOR' }))
  if (-not $pass) { $script:ok = $false }
}

# The same table `vk_for_code` holds in tray/hotkey.rs, cut down to the keys
# worth binding from a script. A key this cannot name is a key the drill would
# press as something else, so it refuses rather than guesses.
function Get-Vk([string] $code) {
  if ($code -match '^F([1-9]|1[0-9]|2[0-4])$') { return 0x70 + [int]$Matches[1] - 1 }
  if ($code -match '^Key([A-Z])$') { return [byte][char]$Matches[1] }
  if ($code -eq 'Space') { return 0x20 }
  return 0
}
$vk = Get-Vk $Key
if ($vk -eq 0) { Write-Output "UNKNOWN_KEY=$Key"; exit 2 }
# **The scan code is not optional, and leaving it zero costs the whole drill.**
# `hotkey.ps1` passes 0 and works, because the Rust hook reads `vkCode` out of
# a KBDLLHOOKSTRUCT. The webview does not: a DOM `KeyboardEvent.code` is
# derived from the *physical* scan code, so a key injected without one arrives
# in the window as "Unidentified" -- and the rebind below stores that as the
# talk key, `vk_for_code` cannot name it, the desktop hook never installs, and
# the window truthfully reports "heard only while this window has focus". Two
# failures, one missing byte. MAPVK_VK_TO_VSC is where the byte comes from.
# Only non-extended keys are bound here (F-keys, letters, space), so no
# KEYEVENTF_EXTENDEDKEY is needed with them.
$scan = [byte]([Gv.Ptt]::MapVirtualKey([uint32]$vk, 0))

# --- when the pointer is refused (DR-39) -----------------------------------
#
# Every click below, and every synthesised key, needs Windows to accept
# injected input from a medium-integrity process. It will not while an
# *elevated* program holds the foreground — UIPI — and since the foreground
# window is a property of the desktop rather than of goodvoice, one elevated
# app anywhere on screen turns this whole drill into a failure that looks
# exactly like a broken talk key. Measured with the desktop idle for 31
# minutes, so "somebody is at the machine" is not the rule.
#
# The fix is a click: put any ordinary window in front, or close the elevated
# one.
function Get-PointerVerdict {
  $here = New-Object Gv.Ptt+POINT
  [Gv.Ptt]::GetCursorPos([ref] $here) | Out-Null
  if ([Gv.Ptt]::SetCursorPos($here.X, $here.Y)) { return 'ok' }

  $fg = [Gv.Ptt]::GetForegroundWindow()
  $owner = 0
  [Gv.Ptt]::GetWindowThreadProcessId($fg, [ref] $owner) | Out-Null
  $name = try { (Get-Process -Id $owner -EA Stop).Name } catch { "pid $owner" }
  $proc = [Gv.Ptt]::OpenProcess(0x1000, $false, $owner)
  if ($proc -eq [IntPtr]::Zero) { return "refused (foreground is $name, which will not open)" }
  $token = [IntPtr]::Zero
  $opened = [Gv.Ptt]::OpenProcessToken($proc, 0x0008, [ref] $token)
  $why = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
  if ($token -ne [IntPtr]::Zero) { [Gv.Ptt]::CloseHandle($token) | Out-Null }
  [Gv.Ptt]::CloseHandle($proc) | Out-Null
  if (-not $opened -and $why -eq 5) { return "refused (UIPI: $name holds the foreground and is elevated)" }
  return "refused (foreground is $name; not an integrity level this can explain)"
}

# --- the window ------------------------------------------------------------

$uiaAny = [System.Windows.Automation.Condition]::TrueCondition

# By window class, not by MainWindowHandle: a debug build owns a console window
# too and .NET hands you that one (tray.md).
function Find-Window([int] $Owner) {
  $script:hit = [IntPtr]::Zero
  $cb = [Gv.Ptt+EnumProc] {
    param($h, $l)
    $who = 0
    [Gv.Ptt]::GetWindowThreadProcessId($h, [ref] $who) | Out-Null
    if ($who -eq $Owner) {
      $c = New-Object System.Text.StringBuilder 256
      [Gv.Ptt]::GetClassName($h, $c, 256) | Out-Null
      if ($c.ToString() -eq 'Tauri Window') { $script:hit = $h; return $false }
    }
    return $true
  }
  [Gv.Ptt]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
  return $script:hit
}

# Walked twice: WebView2 builds its accessibility tree lazily and the first
# walk is what wakes it (DR-26).
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

function Window-Until([scriptblock] $Answered, [int] $Seconds = 20) {
  $deadline = (Get-Date).AddSeconds($Seconds)
  $said = @()
  while ((Get-Date) -lt $deadline) {
    $said = Window-Names
    if ($said.Count -gt 0 -and (& $Answered $said)) { return $said }
    Start-Sleep -Milliseconds 500
  }
  return $said
}

# **Not `InvokePattern`.** It is offered on every button in this window,
# returns success, and does nothing: a WebView2 acts on input, not on patterns
# (DR-26, and 7.4's note in tray.md). And UI Automation gives bounding
# rectangles for elements below the fold as readily as for visible ones, so the
# point is scrolled into view and checked against the window's own frame before
# any button goes down.
function Click-Named([string] $pattern, [string] $ControlType = '') {
  $h = Find-Window $script:app.Id
  if ($h -eq [IntPtr]::Zero) { return $false }
  $root = [System.Windows.Automation.AutomationElement]::FromHandle($h)
  $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny) | Out-Null
  Start-Sleep -Milliseconds 500
  foreach ($e in $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny)) {
    if ($e.Current.Name -inotmatch $pattern) { continue }
    if ($ControlType -and $e.Current.ControlType.ProgrammaticName -ne $ControlType) { continue }
    try {
      $e.GetCurrentPattern([System.Windows.Automation.ScrollItemPattern]::Pattern).ScrollIntoView()
      Start-Sleep -Milliseconds 300
    } catch { }
    $box = $e.Current.BoundingRectangle
    if ($box.Width -le 0 -or $box.Height -le 0) { continue }
    $x = [int]($box.X + $box.Width / 2)
    $y = [int]($box.Y + $box.Height / 2)
    $frame = New-Object Gv.Ptt+RECT
    [Gv.Ptt]::GetWindowRect($h, [ref] $frame) | Out-Null
    if ($x -lt $frame.L -or $x -gt $frame.R -or $y -lt $frame.T -or $y -gt $frame.B) { continue }
    [Gv.Ptt]::ShowWindow($h, 9) | Out-Null    # SW_RESTORE, in case of the tray
    [Gv.Ptt]::SetForegroundWindow($h) | Out-Null
    Start-Sleep -Milliseconds 400
    if (-not [Gv.Ptt]::SetCursorPos($x, $y)) { return $false }
    Start-Sleep -Milliseconds 200
    [Gv.Ptt]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 80
    [Gv.Ptt]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 700
    return $true
  }
  return $false
}

# **Typed, not `ValuePattern.SetValue`.** The form is SolidJS and its state
# comes from `onInput`; a value written into the DOM node is one the app has
# never heard of, and `join` stays disabled with the room code on screen —
# the most convincing wrong answer this drill could give (tray-menu.ps1).
function Fill-Field([string] $field, [string] $text) {
  if (-not (Click-Named "^\s*$field\s*$" 'ControlType.Edit')) { return $false }
  $keys = New-Object -ComObject WScript.Shell
  $keys.SendKeys('^a')
  Start-Sleep -Milliseconds 150
  $keys.SendKeys($text)
  Start-Sleep -Milliseconds 400
  return $true
}

function Hold-Key([int] $ms) {
  [Gv.Ptt]::keybd_event([byte]$vk, $scan, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds $ms
  [Gv.Ptt]::keybd_event([byte]$vk, $scan, 2, [UIntPtr]::Zero)   # 2 = KEYEVENTF_KEYUP
}

# `Get-Content`'s default share mode is refused while the writer still has the
# file open, which a running listener does (invite.ps1).
function Read-Log([string] $path) {
  if (-not (Test-Path $path)) { return @() }
  $stream = [System.IO.FileStream]::new($path, [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
  $reader = [System.IO.StreamReader]::new($stream)
  $text = $reader.ReadToEnd()
  $reader.Dispose(); $stream.Dispose()
  return $text -split "`r?`n"
}

# --- the run ---------------------------------------------------------------

Write-Output 'goodvoice fullscreen talk-key drill (plan.md 7.5, hotkey.md rows 3 and 4)'
Write-Output "  app        $Exe"
Write-Output "  listener   $ListenerExe"
Write-Output "  game       $DrillExe$(if ($Windowed) { '  (windowed: the display is left alone)' })"
Write-Output ("  key        {0} (VK 0x{1:X2}, scan 0x{2:X2})" -f $Key, $vk, $scan)
Write-Output "  room       $Room"
Write-Output "  output     $Out"
Write-Output ''

$pointer = Get-PointerVerdict
Check 'POINTER' ($pointer -eq 'ok') $pointer
if ($pointer -ne 'ok') {
  Write-Output ''
  Write-Output 'Nothing below this line can run: every step needs a real key or a real'
  Write-Output 'mouse, and Windows is refusing both. See "when the pointer is refused"'
  Write-Output 'in this script, and DR-39.'
  Write-Output 'RESULT=BLOCKED'
  exit 4
}

Get-Process goodvoice-client, fullscreen-drill, listener -EA SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 3

$heardPath = Join-Path $Out 'listener.txt'
$drillPath = Join-Path $Out 'fullscreen.txt'

# The roommate goes in first: it is the only thing here that can say whether
# the microphone actually opened, and it has to be listening before it does.
$listener = Start-Process -FilePath $ListenerExe -PassThru -WindowStyle Hidden `
  -ArgumentList '--room', $Room, '--name', 'roommate', '--seconds', '150' `
  -RedirectStandardOutput $heardPath -RedirectStandardError (Join-Path $Out 'listener.err')
Start-Sleep -Seconds 8
Say 'the roommate is in the room'

# Started without GOODVOICE_AUTOJOIN, on purpose: an autojoined call is opened
# in `TransmitMode::Open` (lib.rs), and the mode this drill is about is one the
# window chooses before it joins.
Remove-Item Env:\GOODVOICE_AUTOJOIN -EA SilentlyContinue
# Its output is kept: an app that could not open the microphone says so on
# stderr, and that is the difference between "the key did not work" and "there
# was nothing for it to open" -- see the baseline hold below.
$script:app = Start-Process -FilePath $Exe -PassThru `
  -RedirectStandardOutput (Join-Path $Out 'app.txt') `
  -RedirectStandardError (Join-Path $Out 'app.err')
Start-Sleep -Seconds 6
$said = Window-Until { param($n) Has $n 'join|room' }
Check 'WINDOW' ($said.Count -gt 0) ($said.Count.ToString() + ' names')

# --- push to talk, on this key ---------------------------------------------

Say 'choosing push to talk'
if (-not (Click-Named '^\s*settings\s*$')) { Check 'SETTINGS' $false 'no settings button' }
if (-not (Click-Named '^\s*push to talk\s*$')) { Check 'MODE' $false 'no push-to-talk button' }
$said = Window-Until { param($n) Has $n 'heard only while the key below is held' }
Check 'MODE' (Has $said 'heard only while the key below is held') 'push to talk'

Say "binding $Key"
# Matched without the space after the colon: a talk key the window cannot name
# is stored as an empty string and the button reads "key:" with nothing after
# it, which is exactly the state a previous failed rebind leaves behind.
if (-not (Click-Named '^\s*key:')) { Check 'REBIND' $false 'no key button' }
$said = Window-Until { param($n) Has $n 'press a key, or escape' } 8
Check 'REBIND' (Has $said 'press a key, or escape') 'listening for a key'
# The window is what has focus here, so this one goes in through its own
# keydown handler — which is also the only way to set the binding.
Hold-Key 120
Start-Sleep -Milliseconds 800
# `keyName` in App.tsx lowercases what it shows, so F13 reads as "key: f13".
$expected = 'key: ' + ($Key -replace '^(Key|Digit)', '' -replace '(Left|Right)$', ' $1').ToLower()
$said = Window-Until { param($n) Has $n ([regex]::Escape($expected)) } 8
# What the button actually says, not what it was hoped to say: a rebind that
# stored something else is the one failure here that would otherwise print as
# though it had worked.
$bound = @($said | Where-Object { $_ -imatch '^\s*key:' })[0]
Check 'KEY_BOUND' (Has $said ([regex]::Escape($expected))) ("$bound" + $(if ($bound -imatch [regex]::Escape($expected)) { '' } else { " (wanted $expected)" }))

if (-not (Click-Named '^\s*(back|done)\s*$')) { Say 'the settings screen would not close' }

# --- into the room ---------------------------------------------------------

Say "joining $Room"
if (-not (Fill-Field 'room' $Room)) { Check 'JOIN' $false 'no room field' }
if (-not (Click-Named '^\s*join\s*$')) { Check 'JOIN' $false 'no join button' }
$said = Window-Until { param($n) Has $n 'leave' } 30
Check 'JOIN' (Has $said 'leave') $Room

# The window's own answer to "is this key heard everywhere, or only here". It
# is the only place the difference is visible without a game in the way, and
# it is `talk_key_is_global` — the hook being on the desktop, not a guess.
Say 'asking the window whether the hook is on the desktop'
if (-not (Click-Named '^\s*settings\s*$')) { Say 'the settings button moved' }
$said = Window-Until { param($n) Has $n 'heard (from anywhere|only while this window)' } 10
$global = @($said | Where-Object { $_ -imatch 'heard (from anywhere|only while this window)' })[0]
Check 'GLOBAL' ($global -imatch 'from anywhere') ("$global")
if (-not (Click-Named '^\s*(back|done)\s*$')) { Say 'the settings screen would not close' }

# --- a hold before the game, which is the control --------------------------
#
# The room going quiet is only evidence about the talk key if the room was ever
# going to hear anything. A microphone another program is holding, or a capture
# device that would not open, produces exactly the same column of zeroes as a
# key that never arrived -- and blames the feature for the machine. So one hold
# happens here, with the window still on screen and the display untouched: if
# the room does not hear this one, nothing after it is admissible.
Say 'holding the key once with nothing in the way'
Hold-Key 3000
Start-Sleep -Seconds 5

# --- the game takes the screen ---------------------------------------------

$seconds = 34
Say 'taking the display'
$drillArgs = @('--key', $Key, '--seconds', "$seconds")
if ($Windowed) { $drillArgs += '--windowed' }
$drill = Start-Process -FilePath $DrillExe -PassThru `
  -ArgumentList $drillArgs -RedirectStandardOutput $drillPath `
  -RedirectStandardError (Join-Path $Out 'fullscreen.err')
Start-Sleep -Seconds 6

$fg = [Gv.Ptt]::GetForegroundWindow()
$fgOwner = 0
[Gv.Ptt]::GetWindowThreadProcessId($fg, [ref] $fgOwner) | Out-Null
$fgName = try { (Get-Process -Id $fgOwner -EA Stop).Name } catch { "pid $fgOwner" }
Check 'FOREGROUND' ($fgOwner -eq $drill.Id) $fgName

# Two holds with a gap, because one is a level and two are a switch: a client
# that had simply been transmitting all along would give the same first burst
# and no silence between.
Say 'holding the key (1 of 2)'
Hold-Key 4000
Start-Sleep -Seconds 6
Say 'holding the key (2 of 2)'
Hold-Key 4000
Start-Sleep -Seconds 6

$drill.WaitForExit(30000) | Out-Null
Say 'the display is back'

# --- what each of the three saw --------------------------------------------

$transcript = Read-Log $drillPath
foreach ($line in $transcript | Where-Object { $_ -match '^(MODE|EXCLUSIVE_AT_END|FOREGROUND_AT_END|DRIVER|FRAMES|FPS|DISPLAY_REFRESHES|PRESENTS|VSYNC|DOWNS)=' }) {
  Write-Output ('  game ' + $line)
}
$mode = @($transcript | Where-Object { $_ -match '^MODE=' })[0] -replace '^MODE=', ''
$edges = @($transcript | Where-Object { $_ -match '^DOWNS=' })[0]
$downs = 0; $ups = 0
if ($edges -match '^DOWNS=(\d+) UPS=(\d+)') { $downs = [int]$Matches[1]; $ups = [int]$Matches[2] }

Check 'DISPLAY' ($mode -eq $(if ($Windowed) { 'windowed' } else { 'exclusive' })) $mode
# The half nobody could script before: the window in front still got the key,
# which is what "and the game still receives it" means.
Check 'GAME_HEARD' ($downs -ge 2 -and $ups -ge 2) "$downs down, $ups up"

# The roommate's frames/s column. A gated client sends nothing at all rather
# than silence (bin/mute-drill), so a row is either a stream or a hole.
$rows = @(Read-Log $heardPath | Where-Object { $_ -match '^\s+\d+s\s+\d+\s+' })
$heard = @()
foreach ($row in $rows) {
  if ($row -match '^\s+(\d+)s\s+(\d+)\s') { $heard += [int]$Matches[2] }
}
$bursts = 0
$live = 0
$wasLive = $false
foreach ($frames in $heard) {
  $now = $frames -ge 25          # half of the 50 a second a live 20 ms path sends
  if ($now) { $live++ }
  if ($now -and -not $wasLive) { $bursts++ }
  $wasLive = $now
}
$silent = @($heard | Where-Object { $_ -eq 0 }).Count
Write-Output ("  room " + ($heard -join ' '))

# Three holds: the control, then two with the display taken.
if ($bursts -eq 0) {
  # Nothing at all was heard, including the control, so this run says nothing
  # about the talk key. The app's own stderr usually names the reason.
  Write-Output 'HEARD_BURSTS=0'
  foreach ($line in (Read-Log (Join-Path $Out 'app.err')) | Where-Object { $_ }) {
    Write-Output ("  app  " + $line)
  }
  Write-Output ''
  Write-Output 'RESULT=INCONCLUSIVE -- the room heard nothing even with nothing in the'
  Write-Output 'way, so the microphone was not open before the display was ever taken.'
  Write-Output 'A capture device another program is holding is the usual reason; the'
  Write-Output 'app lines above, if any, say which.'
  Get-Process goodvoice-client, fullscreen-drill -EA SilentlyContinue | Stop-Process -Force
  if (-not $listener.HasExited) { Stop-Process -Id $listener.Id -Force -EA SilentlyContinue }
  exit 3
}
Check 'HEARD_BURSTS' ($bursts -eq 3) "$bursts (one control, two over the game)"
Check 'HEARD_SECONDS' ($live -ge 6) "$live of $($heard.Count) rows carried audio, $silent carried none"

Get-Process goodvoice-client, fullscreen-drill -EA SilentlyContinue | Stop-Process -Force
if (-not $listener.HasExited) { Stop-Process -Id $listener.Id -Force -EA SilentlyContinue }

Write-Output ''
if ($script:ok) {
  $took = if ($Windowed) { 'the display was left alone' } else { 'the display was taken' }
  Write-Output ("RESULT=PASS -- $took, the key still opened the microphone, and the window in front still got it.")
  exit 0
}
Write-Output ('RESULT=FAIL -- read the lines marked above; the transcripts are in ' + $Out)
exit 1
