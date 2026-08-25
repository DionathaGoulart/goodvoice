# plan.md 6.2: does clicking a `goodvoice://join/<room>` link put this machine
# in that room — and does a link stop where it should?
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\invite.ps1
#   ... -Server https://goodvoice.<subdomain>.workers.dev
#
# **Run this against the INSTALLED app**, not a build tree. The scheme is
# registered by the installer (`plugins.deep-link.desktop.schemes` in
# tauri.conf.json), so a link only ever reaches a client that was installed:
#
#   cd client && npm run tauri build
#   & "$env:CARGO_TARGET_DIR\release\bundle\nsis\goodvoice_0.1.0_x64-setup.exe" /S
#
# Three questions, and the last two are the ones a link gets wrong:
#
#   1. a link joins the room it names;
#   2. a link arriving while a call is running does NOT take that call — it is
#      offered, and the offer names the room;
#   3. a link for another deploy is not followed at all. A link that could
#      repoint somebody's client at a server of the sender's choosing is a link
#      that can put them in a stranger's room with their microphone open.
#
# **What is asked, and why it is not the microphone.** The room the client is
# in is read from its own window, through UI Automation: the masthead shows the
# room code. An earlier version of this drill measured `bin/listener` frames
# instead and was flaky for a reason that has nothing to do with links — a
# microphone another application had taken. Frames are still reported when
# there are any, as evidence rather than as the verdict.
#
# **Everything here waits for an answer, never for a window.** The three
# reasons this drill has failed against a working client were all its own, and
# the third was the worst: it waited for the window to *say something*, and a
# window says twelve things — the join form — within a second of appearing,
# while a join takes several. It then read the join form it had just waited for
# and reported a link that did not work. Both other reasons are written up
# where they were fixed: a room matcher that also matched the invite banner's
# own text (`In-Room`), and a force-killed app whose WASAPI endpoint was still
# busy (`Stop-App`).
[CmdletBinding()]
param(
  [string] $Server = 'https://goodvoice.goodvoice-server.workers.dev',
  [string] $Elsewhere = 'https://goodvoice-elsewhere.invalid',
  [string] $Installed = "$env:LOCALAPPDATA\goodvoice\goodvoice-client.exe",
  [string] $ListenerExe = "$env:CARGO_TARGET_DIR\release\listener.exe",
  [string] $Out = "$env:TEMP\goodvoice-invite"
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

if (-not (Test-Path $Installed)) { Write-Output "NOT_INSTALLED=$Installed"; exit 2 }
New-Item -ItemType Directory -Force -Path $Out | Out-Null

$script:t0 = Get-Date
function Say([string] $line) {
  Write-Output ("  t+{0,3}s  {1}" -f [int]((Get-Date) - $script:t0).TotalSeconds, $line)
}

$uiaAny = [System.Windows.Automation.Condition]::TrueCondition

# Everything the window is currently saying, as text.
#
# DR-26: WebView2 builds its accessibility tree lazily and the first walk is
# what wakes it, so this walks twice — without that, a window that is perfectly
# fine reads as empty.
function Read-Window {
  $app = Get-Process goodvoice-client -EA SilentlyContinue | Select-Object -First 1
  if (-not $app -or $app.MainWindowHandle -eq 0) { return @() }
  try {
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($app.MainWindowHandle)
    $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny) | Out-Null
    Start-Sleep -Milliseconds 700
    $said = @()
    foreach ($e in $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny)) {
      if ($e.Current.Name) { $said += $e.Current.Name }
    }
    return $said
  } catch { return @() }
}

# Waits for the window to say the thing being waited for, and returns
# everything it says — whether or not it ever said it.
#
# `$Answered` is asked of every read. Waiting for a window to merely *exist*
# is what made this drill lie: the join form is twelve accessible names and it
# is up within a second, while a join is a microphone, an ICE round and a
# Cloudflare session — several seconds later. A drill that stopped at "the
# window is saying something" read the join form and called a working link
# broken, twice.
function Wait-For([scriptblock] $Answered, [int] $Seconds = 40) {
  $deadline = (Get-Date).AddSeconds($Seconds)
  $said = @()
  while ((Get-Date) -lt $deadline) {
    $said = Read-Window
    if ($said.Count -gt 0 -and (& $Answered $said)) { return $said }
    Start-Sleep -Milliseconds 500
  }
  return $said
}

# The window is offering an invite rather than acting on it — which is an
# answer, and one this drill waits for twice.
function Offering([string[]] $said, [string] $room) {
  return @($said | Where-Object { $_ -imatch "^\s*$room\s*$" }).Count -gt 0 -and
         @($said | Where-Object { $_ -imatch 'an invite to' }).Count -gt 0
}

# The room this client is *in*, which is not the only room the window names: an
# invite banner offers another one, and matching a bare room code found that
# instead and called it a hijacked call. The masthead's room carries an
# `aria-label` of "room <code>" precisely so the two can be told apart.
#
# Case-insensitive, because the terminal skin uppercases every accessible name
# (DR-26).
function In-Room([string[]] $said, [string] $room) {
  return @($said | Where-Object { $_ -imatch "^\s*room\s+$room\s*$" }).Count -gt 0
}

# Killed, and then given time to let go.
#
# `Stop-Process -Force` is not how a person closes goodvoice, and a WASAPI
# capture stream that was killed rather than closed keeps its endpoint busy for
# a moment afterwards. Relaunch inside that moment and the join fails on the
# microphone — which looks exactly like a link that did not work.
function Stop-App {
  Get-Process goodvoice-client -EA SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 5
}

# Why there is no "ask the client what went wrong" step here.
#
# There was one: it handed the same URL to the same binary with the output
# redirected, and it printed nothing, twice, because a release build of a Tauri
# app is a GUI-subsystem process — `println!` and `eprintln!` have nowhere to
# go. The client says what went wrong in its own window instead, which is where
# a person would read it anyway, and the failing branches below print
# everything the window says.

function Click([string] $link) { Start-Process $link }

# `Get-Content`'s default share mode is refused while the writer still has the
# file open, which for a listener that is still finishing it does.
function Read-Log([string] $path) {
  if (-not (Test-Path $path)) { return @() }
  $stream = [System.IO.FileStream]::new($path, [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
  $reader = [System.IO.StreamReader]::new($stream)
  $text = $reader.ReadToEnd()
  $reader.Dispose(); $stream.Dispose()
  return $text -split "`r?`n"
}

# Supporting evidence: how many frames a second reached a second person in the
# room, when the machine's microphone was available to give any.
function Heard([string] $name) {
  $rows = @(Read-Log (Join-Path $Out "$name.txt") |
    ForEach-Object { if ($_ -match '^\s+\d+s\s+(\d+)') { [int]$Matches[1] } })
  $loud = @($rows | Where-Object { $_ -ge 40 })
  if ($rows.Count -eq 0) { return 'the listener never reported' }
  if ($loud.Count -eq 0) { return 'nothing (a microphone this machine would not give)' }
  return "$($loud.Count) s of voice, up to $(($rows | Measure-Object -Maximum).Maximum) frames"
}

Write-Output 'goodvoice invite-link drill (plan.md 6.2)'
Write-Output "  installed  $Installed"
Write-Output "  server     $Server"
Write-Output ''

$command = (Get-ItemProperty 'HKCU:\Software\Classes\goodvoice\shell\open\command' -EA SilentlyContinue).'(default)'
if (-not $command) { Write-Output 'SCHEME_NOT_REGISTERED'; exit 3 }
Say "the scheme is registered: $command"
if ($command -notlike "*$Installed*") {
  Say "warning: the scheme points somewhere other than $Installed"
}

Stop-App

# --- 1. a link joins the room it names ---------------------------------------

$first = "invite-$(Get-Random -Maximum 999999)"
$ear = $null
if (Test-Path $ListenerExe) {
  $ear = Start-Process -FilePath $ListenerExe -PassThru -WindowStyle Hidden `
    -ArgumentList @('--base', $Server, '--room', $first, '--seconds', '25', '--name', 'ear') `
    -RedirectStandardOutput (Join-Path $Out 'ear.txt') `
    -RedirectStandardError (Join-Path $Out 'ear.err')
  Start-Sleep -Seconds 3
}

Say "clicking goodvoice://join/$first"
Click "goodvoice://join/$first`?s=$Server"
# Either answer ends the wait: the room, or the client saying why not. The
# second is an answer as much as the first — it is the one 6.2 added — and
# waiting the full deadline for it would only delay a verdict already reached.
$said = Wait-For { param($it) (In-Room $it $first) -or (Offering $it $first) }
if ($said.Count -eq 0) { Write-Output 'NO_WINDOW_AFTER_CLICK'; Stop-App; exit 4 }
if (-not (In-Room $said $first)) {
  Write-Output "LINK_DID_NOT_JOIN: the window says [$($said -join ' | ')]"
  Stop-App; exit 5
}
Say "the app is in $first"
if ($ear) { $ear | Wait-Process -Timeout 60 -EA SilentlyContinue; Say ("the room heard: " + (Heard 'ear')) }

# --- 2. a second link is offered, not taken ----------------------------------

$second = "invite-$(Get-Random -Maximum 999999)"

# The call has to still be running for this test to be about anything. A call
# that ended on its own — a microphone another application took, a transport
# that dropped — would leave the client free to join, and blaming the link for
# that would be the wrong bug written down.
$before = Wait-For { param($it) In-Room $it $first } 10
if (-not (In-Room $before $first)) {
  Write-Output "THE_CALL_ENDED_BEFORE_THE_TEST: [$($before -join ' | ')]"
  Stop-App; exit 12
}
Say "still in $first, so there is a call for the link to interrupt"

Say "clicking goodvoice://join/$second while that call is running"
Click "goodvoice://join/$second`?s=$Server"
# The offer is what is being waited for; the second room appearing in the
# masthead would be the failure, and it is checked for either way below.
$said = Wait-For { param($it) (Offering $it $second) -or (In-Room $it $second) } 25
if (In-Room $said $second) { Write-Output 'THE_LINK_TOOK_THE_CALL'; Stop-App; exit 6 }
if (-not (In-Room $said $first)) {
  Write-Output "THE_CALL_WENT_SOMEWHERE_ELSE: [$($said -join ' | ')]"
  Stop-App; exit 7
}
$offered = @($said | Where-Object { $_ -imatch 'an invite to' -or $_ -imatch "leave and join" }).Count -gt 0
if (-not $offered) {
  Write-Output "THE_LINK_WAS_SWALLOWED: [$($said -join ' | ')]"
  Stop-App; exit 8
}
Say "still in $first, and the window offers the invite to $second"

# --- 3. a link for another deploy is refused ---------------------------------

Stop-App
$third = "invite-$(Get-Random -Maximum 999999)"
Say "clicking a link that claims to be for $Elsewhere"
Click "goodvoice://join/$third`?s=$Elsewhere"
$said = Wait-For { param($it)
  @($it | Where-Object { $_ -imatch 'that invite is for' }).Count -gt 0 -or (In-Room $it $third)
} 30
if ($said.Count -eq 0) { Write-Output 'NO_WINDOW_AFTER_CLICK'; Stop-App; exit 9 }
if (In-Room $said $third) { Write-Output 'A_LINK_FOR_ANOTHER_SERVER_WAS_FOLLOWED'; Stop-App; exit 10 }
$told = @($said | Where-Object { $_ -imatch 'that invite is for' }).Count -gt 0
if (-not $told) {
  Write-Output "NOTHING_WAS_SAID: [$($said -join ' | ')]"
  Stop-App; exit 11
}
Say 'not joined, and the window says which server it was for'

Stop-App
Write-Output ''
Write-Output 'RESULT=PASS'
