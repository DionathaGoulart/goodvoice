# plan.md 5.5: what does sharing a screen cost the game that is on it?
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\perf\screenshare-bench.ps1
#   ... -Process FarFarWest-Win64-Shipping.exe   # whatever is running
#   ... -Seconds 45 -Quality 720
#
# **Run it elevated.** PresentMon opens an ETW trace session, which needs
# administrative privilege or membership of "Performance Log Users"; without it
# the first capture fails with "access denied" and nothing else runs.
#
# Three captures, in this order: the game alone, the game with a 1080p hardware
# share of the same screen, and the game alone again. The third is not a
# formality — a GPU that has warmed up or a scene that has drifted shows up as
# a difference between the two baselines, and a benchmark that took one
# baseline would have blamed the share for it.
[CmdletBinding()]
param(
  [string] $Process = 'FarFarWest-Win64-Shipping.exe',
  [int] $Seconds = 30,
  [ValidateSet('1080', '720')] [string] $Quality = '1080',
  [string] $Room = "bench-$(Get-Random -Maximum 999999)",
  [string] $PresentMon = "$env:USERPROFILE\gv\tools\PresentMon.exe",
  [string] $DrillExe = "$env:CARGO_TARGET_DIR\release\share-drill.exe",
  [string] $Out = "$env:TEMP\goodvoice-bench"
)

if (-not (Test-Path $PresentMon)) { Write-Output "NO_PRESENTMON=$PresentMon"; exit 2 }
if (-not (Test-Path $DrillExe)) { $DrillExe = 'client\src-tauri\target\release\share-drill.exe' }
if (-not (Test-Path $DrillExe)) { Write-Output "NO_DRILL=$DrillExe"; exit 2 }
if (-not (Get-Process -Name ($Process -replace '\.exe$', '') -EA SilentlyContinue)) {
  Write-Output "NOT_RUNNING=$Process"; exit 3
}
New-Item -ItemType Directory -Force -Path $Out | Out-Null

$script:t0 = Get-Date
function Say([string] $line) {
  Write-Output ("  t+{0,3}s  {1}" -f [int]((Get-Date) - $script:t0).TotalSeconds, $line)
}

function Read-Log([string] $path) {
  if (-not (Test-Path $path)) { return @() }
  $stream = [System.IO.FileStream]::new($path, [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
  $reader = [System.IO.StreamReader]::new($stream)
  $text = $reader.ReadToEnd()
  $reader.Dispose(); $stream.Dispose()
  return $text -split "`r?`n"
}

# One capture, and what came out of it.
function Measure-Frames([string] $name) {
  $csv = Join-Path $Out "$name.csv"
  Remove-Item $csv -EA SilentlyContinue
  & $PresentMon --process_name $Process --timed $Seconds --output_file $csv `
    --terminate_after_timed --no_console_stats 2>&1 | Out-Null
  if (-not (Test-Path $csv)) { return $null }

  $rows = Import-Csv $csv
  # Frame *time* rather than frame rate: an average of rates is not the rate,
  # and the percentile that matters is a percentile of times.
  $times = @($rows | ForEach-Object { $_.MsBetweenPresents -as [double] } | Where-Object { $_ -gt 0 })
  if ($times.Count -lt 10) { return $null }
  $gpu = @($rows | ForEach-Object { $_.MsGPUBusy -as [double] } | Where-Object { $_ -gt 0 })
  $sorted = $times | Sort-Object
  # The slowest one frame in a hundred, which is where a stutter lives. Read as
  # a frame rate so it sits beside the mean.
  $p99 = $sorted[[int][math]::Floor($sorted.Count * 0.99)]

  return [pscustomobject]@{
    Name    = $name
    Frames  = $times.Count
    Fps     = [math]::Round(1000 / (($times | Measure-Object -Average).Average), 1)
    LowFps  = [math]::Round(1000 / $p99, 1)
    GpuMs   = if ($gpu.Count) { [math]::Round(($gpu | Measure-Object -Average).Average, 2) } else { 0 }
    Csv     = $csv
  }
}

Write-Output 'goodvoice screen-share FPS benchmark (plan.md 5.5)'
Write-Output "  game     $Process"
Write-Output "  capture  $Seconds s each, three of them"
Write-Output "  share    ${Quality}p, hardware encode, of the screen the game is on"
Write-Output "  room     $Room"
Write-Output ''

Say 'capturing the game on its own'
$before = Measure-Frames 'before'
if (-not $before) { Write-Output 'NO_FRAMES_BEFORE'; exit 4 }
Say ("{0} frames, {1} fps, {2} fps 1% low, {3} ms GPU" -f $before.Frames, $before.Fps, $before.LowFps, $before.GpuMs)

$log = Join-Path $Out 'share.txt'
$drill = Start-Process -FilePath $DrillExe -PassThru -WindowStyle Hidden `
  -ArgumentList @('--room', $Room, '--seconds', "$($Seconds + 40)", "--$Quality") `
  -RedirectStandardOutput $log -RedirectStandardError (Join-Path $Out 'share.err.txt')

$live = $null
$deadline = (Get-Date).AddSeconds(60)
while ((Get-Date) -lt $deadline -and -not $live) {
  $live = ((Read-Log $log) -match 'is sharing') | Select-Object -First 1
  if (-not $live) { Start-Sleep -Milliseconds 500 }
}
if (-not $live) { Write-Output 'SHARE_NEVER_WENT_LIVE'; $drill | Stop-Process -Force; exit 5 }
Say $live.Trim()
# The encoder's first seconds include opening it; the steady state is what a
# game feels.
Start-Sleep -Seconds 5

Say 'capturing with the share live'
$during = Measure-Frames 'during'
$drill | Stop-Process -Force -EA SilentlyContinue
if (-not $during) { Write-Output 'NO_FRAMES_DURING'; exit 6 }
Say ("{0} frames, {1} fps, {2} fps 1% low, {3} ms GPU" -f $during.Frames, $during.Fps, $during.LowFps, $during.GpuMs)

Start-Sleep -Seconds 5
Say 'capturing the game on its own again'
$after = Measure-Frames 'after'
if (-not $after) { Write-Output 'NO_FRAMES_AFTER'; exit 7 }
Say ("{0} frames, {1} fps, {2} fps 1% low, {3} ms GPU" -f $after.Frames, $after.Fps, $after.LowFps, $after.GpuMs)

$delta = [math]::Round($during.Fps - $before.Fps, 1)
$drift = [math]::Round($after.Fps - $before.Fps, 1)
$percent = if ($before.Fps -gt 0) { [math]::Round(100 * $delta / $before.Fps, 1) } else { 0 }

$report = @()
$report += '### what the share cost the game'
$report += ''
$report += ('| capture | frames | fps | 1% low | GPU ms/frame |')
$report += ('|---|---|---|---|---|')
foreach ($run in @($before, $during, $after)) {
  $report += ('| {0} | {1} | {2} | {3} | {4} |' -f $run.Name, $run.Frames, $run.Fps, $run.LowFps, $run.GpuMs)
}
$report += ''
$report += ("- sharing changed the frame rate by **{0} fps** ({1}%)" -f $delta, $percent)
$report += ("- the two idle captures differ by {0} fps, which is what this measurement's own noise looks like" -f $drift)
$report += ("- GPU time a frame: {0} ms alone, {1} ms while sharing" -f $before.GpuMs, $during.GpuMs)
$report += ''
$version = (Get-Item $PresentMon).VersionInfo.ProductVersion
$report += ("Captured with PresentMon $version, $Seconds s a capture, ${Quality}p hardware encode.")

$report | ForEach-Object { Write-Output $_ }
$report | Set-Content (Join-Path $Out 'report.md') -Encoding UTF8
Write-Output ''
Write-Output "csv and report in $Out"
