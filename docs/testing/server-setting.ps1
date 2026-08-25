# plan.md 6.1: does "paste the Worker URL into the client's settings" (prd.md
# §9) actually point the client at that Worker — and does it still, after the
# app is closed and opened again?
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\server-setting.ps1
#   ... -Server https://goodvoice.<subdomain>.workers.dev    # the one that works
#
# Needs a RELEASE build with `custom-protocol` (tray.md). Three things are
# checked, in the only order that can tell them apart:
#
#   1. a URL that is not an origin is refused, with a sentence, and changes
#      nothing;
#   2. a valid-but-wrong origin is *kept* — the client restarts pointed at it
#      and its autojoin fails there rather than quietly falling back to the
#      server the build shipped with;
#   3. the real one is kept the same way, and the client joins it.
#
# (2) is the one worth having. A client that ignored the setting would pass a
# test that only ever set the working URL.
[CmdletBinding()]
param(
  [string] $Server = 'https://goodvoice.goodvoice-server.workers.dev',
  [string] $Wrong = 'https://goodvoice-nowhere.invalid',
  [string] $Nonsense = 'dash.cloudflare.com/workers/services/view/goodvoice',
  [string] $Exe = "$env:CARGO_TARGET_DIR\release\goodvoice-client.exe",
  [string] $ListenerExe = "$env:CARGO_TARGET_DIR\release\listener.exe",
  [string] $Out = "$env:TEMP\goodvoice-server-setting"
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -Namespace Gv -Name Set -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
[DllImport("user32.dll")] public static extern int GetClassName(IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr hWnd, out int pid);
public delegate bool EnumProc(IntPtr hWnd, IntPtr lp);
'@

if (-not (Test-Path $Exe)) { Write-Output "NO_EXE=$Exe"; exit 2 }
if (-not (Test-Path $ListenerExe)) { Write-Output "NO_LISTENER=$ListenerExe"; exit 2 }
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$settings = Join-Path $env:APPDATA 'art.good.goodvoice\settings.json'

$script:t0 = Get-Date
function Say([string] $line) {
  Write-Output ("  t+{0,3}s  {1}" -f [int]((Get-Date) - $script:t0).TotalSeconds, $line)
}

function Find-Main([int] $Owner) {
  $script:hit = [IntPtr]::Zero
  $cb = [Gv.Set+EnumProc] {
    param($h, $l)
    $who = 0
    [Gv.Set]::GetWindowThreadProcessId($h, [ref]$who) | Out-Null
    if ($who -eq $Owner) {
      $c = New-Object System.Text.StringBuilder 256
      [Gv.Set]::GetClassName($h, $c, 256) | Out-Null
      if ($c.ToString() -eq 'Tauri Window') { $script:hit = $h; return $false }
    }
    return $true
  }
  [Gv.Set]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
  if ($script:hit -eq [IntPtr]::Zero) { return $null }
  return $script:hit
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

$uiaAny = [System.Windows.Automation.Condition]::TrueCondition

# DR-26: WebView2's tree is built lazily and the first targeted query wakes it.
function Get-Root([int] $Owner) {
  $window = Find-Main $Owner
  if (-not $window) { return $null }
  try {
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($window)
    $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny) | Out-Null
    return $root
  } catch { return $null }
}

function Find-By([System.Windows.Automation.AutomationElement] $root, [string] $type, [string] $pattern) {
  if (-not $root) { return $null }
  foreach ($e in $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny)) {
    if ($e.Current.ControlType.ProgrammaticName -ne "ControlType.$type") { continue }
    if ($e.Current.Name -imatch $pattern) { return $e }
  }
  return $null
}

# `aria-pressed` makes a <button> a UIA toggle button, and a toggle button has
# no Invoke at all (docs/testing/viewer.md).
function Press([System.Windows.Automation.AutomationElement] $e) {
  try { $e.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke(); return $true }
  catch {
    try { $e.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern).Toggle(); return $true }
    catch { return $false }
  }
}

# Types into the box the way a person does, as far as UIA is concerned: the
# value pattern, which fires the input event the window listens on.
function Set-Box([System.Windows.Automation.AutomationElement] $e, [string] $text) {
  $e.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($text)
}

# --- one trip through the settings screen ------------------------------------

function Set-Server([string] $url, [string] $what) {
  $app = Start-Process -FilePath $Exe -PassThru `
    -RedirectStandardOutput (Join-Path $Out "$what.out") `
    -RedirectStandardError (Join-Path $Out "$what.err")
  $root = Wait-For { Get-Root $app.Id } 40
  if (-not $root) { Write-Output 'NO_MAIN_WINDOW'; exit 4 }

  $settingsButton = Wait-For { Find-By (Get-Root $app.Id) 'Button' '^\s*settings\s*$' } 20
  if (-not $settingsButton) { Write-Output 'NO_SETTINGS_BUTTON'; exit 5 }
  Press $settingsButton | Out-Null
  Start-Sleep -Seconds 1

  $root = Get-Root $app.Id
  $box = Find-By $root 'Edit' 'worker url'
  if (-not $box) { Write-Output 'NO_SERVER_BOX'; exit 6 }
  Set-Box $box $url
  Start-Sleep -Milliseconds 400

  $use = Find-By (Get-Root $app.Id) 'Button' 'use this server'
  if (-not $use) { Write-Output 'NO_USE_BUTTON'; exit 7 }
  Press $use | Out-Null
  Start-Sleep -Seconds 1

  # Whatever the window says now — the notice under the box is either the
  # refusal or the ordinary hint.
  $root = Get-Root $app.Id
  $said = @()
  foreach ($e in $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $uiaAny)) {
    if ($e.Current.ControlType.ProgrammaticName -eq 'ControlType.Text') { $said += $e.Current.Name }
  }
  $app | Stop-Process -Force
  Start-Sleep -Seconds 1
  return $said
}

# --- and what the client does with it ----------------------------------------

# Starts the app pointed at a room and reports what its own log says about the
# join, which names the server it reached.
function Try-Autojoin([string] $room, [string] $what) {
  $log = Join-Path $Out "$what.autojoin.txt"
  $env:GOODVOICE_AUTOJOIN = $room
  $app = Start-Process -FilePath $Exe -PassThru -RedirectStandardOutput $log `
    -RedirectStandardError (Join-Path $Out "$what.autojoin.err")
  Remove-Item Env:\GOODVOICE_AUTOJOIN
  Start-Sleep -Seconds 12
  $app | Stop-Process -Force
  $text = (Get-Content $log -EA SilentlyContinue) -join "`n"
  $text += (Get-Content (Join-Path $Out "$what.autojoin.err") -EA SilentlyContinue) -join "`n"
  return $text
}

Write-Output 'goodvoice server-setting drill (plan.md 6.1)'
Write-Output "  exe       $Exe"
Write-Output "  settings  $settings"
Write-Output ''

Say 'clearing whatever was set before'
Remove-Item $settings -EA SilentlyContinue

Say "typing something that is not an origin: $Nonsense"
$said = Set-Server $Nonsense 'nonsense'
$refused = @($said | Where-Object { $_ -imatch 'origin only|start with https' })
if ($refused.Count -eq 0) { Write-Output "NOT_REFUSED: $($said -join ' | ')"; exit 8 }
Say "refused: $($refused[0])"
if (Test-Path $settings) { Write-Output 'REFUSAL_WAS_SAVED'; exit 9 }
Say 'and nothing was written'

Say "pointing the client at a Worker that does not exist: $Wrong"
Set-Server $Wrong 'wrong' | Out-Null
if (-not (Test-Path $settings)) { Write-Output 'NOTHING_SAVED'; exit 10 }
Say "settings.json says: $((Get-Content $settings -Raw).Trim() -replace '\s+', ' ')"

$room = "setting-$(Get-Random -Maximum 999999)"
$failed = Try-Autojoin $room 'wrong'
if ($failed -notmatch 'autojoin failed') {
  Write-Output "THE_SETTING_WAS_IGNORED: $failed"; exit 11
}
Say 'the restarted client tried that one and failed there — it did not fall back'

Say "pointing it at the real one: $Server"
Set-Server $Server 'right' | Out-Null

# A second person in the room, so "it joined" is fifty frames a second rather
# than a line in a log (DR-26).
$heard = Join-Path $Out 'listener.txt'
# Its own `--seconds`, and waited for rather than killed: a Rust program whose
# stdout is a file writes in blocks, so a listener stopped mid-run is a listener
# whose last few seconds were never flushed. That looked exactly like "the
# client joined and nobody heard it".
$listener = Start-Process -FilePath $ListenerExe -PassThru -WindowStyle Hidden `
  -ArgumentList @('--base', $Server, '--room', $room, '--seconds', '20') `
  -RedirectStandardOutput $heard -RedirectStandardError (Join-Path $Out 'listener.err')
Start-Sleep -Seconds 4

$joined = Try-Autojoin $room 'right'
$listener | Wait-Process -Timeout 40 -EA SilentlyContinue
if ($joined -notmatch 'autojoined') { Write-Output "DID_NOT_JOIN: $joined"; exit 12 }
Say ($joined -split "`n" | Where-Object { $_ -match 'autojoined' } | Select-Object -First 1)

$frames = @(Get-Content $heard -EA SilentlyContinue |
  ForEach-Object { if ($_ -match '^\s+\d+s\s+(\d+)') { [int]$Matches[1] } } |
  Where-Object { $_ -gt 0 })
Write-Output ''
Write-Output '### what the far end heard'
Write-Output ''
if ($frames.Count -eq 0) {
  Write-Output '  **nothing** — the client joined the right server and was not heard'
  exit 13
}
Write-Output ("  {0} seconds with audio, {1} frames a second at the busiest" -f $frames.Count, ($frames | Measure-Object -Maximum).Maximum)
Write-Output ''
Write-Output 'RESULT=PASS'
