# plan.md 4.5: what does goodvoice cost while nobody is talking?
#
# The second opinion. `bin/soak` launches the app and measures it through
# Toolhelp and GetProcessTimes; this attaches to an app that is already running
# and measures it through CIM and .NET, so the two share no code and no API. A
# budget met by one implementation of the arithmetic and not the other is a bug
# in the arithmetic, and that is exactly what this is here to catch.
#
#   powershell -ExecutionPolicy Bypass -File docs\perf\idle-soak.ps1 -Minutes 30
#
# It measures the whole process tree -- goodvoice-client.exe plus WebView2's
# browser, GPU, network and renderer processes -- because that is what the app
# costs the machine. Measuring only the process with our name on it reports
# about a third of the memory and passes a budget it has not met.
param(
  [int]$ProcessId = 0,
  [string]$Name = 'goodvoice-client',
  [int]$Minutes = 30,
  [int]$IntervalSeconds = 5,
  [string]$Csv = 'docs\perf\idle-soak-powershell.csv'
)

$ErrorActionPreference = 'Stop'
# A CSV written on a machine whose decimal separator is a comma is not a CSV.
# Every number below is formatted for a reader, not for this desktop's locale.
[System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::InvariantCulture
# prd.md section 4, idle in a room.
$cpuBudget = 2.0
$ramBudget = 120.0

if ($ProcessId -eq 0) {
  $app = Get-Process $Name -ErrorAction SilentlyContinue | Sort-Object StartTime | Select-Object -Last 1
  if (-not $app) { Write-Output "NO_APP -- start $Name (or bin/soak) first"; exit 2 }
  $ProcessId = $app.Id
}

# WebView2's renderers are children of its browser process, not of ours, so one
# generation is not enough: this keeps expanding until a pass adds nothing.
function Get-Tree([int]$root) {
  $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId
  $tree = @($root)
  do {
    $before = $tree.Count
    foreach ($p in $all) {
      if (($tree -contains $p.ParentProcessId) -and -not ($tree -contains $p.ProcessId)) {
        $tree += $p.ProcessId
      }
    }
  } while ($tree.Count -ne $before)
  return $tree
}

$cores = [Environment]::ProcessorCount
Write-Output "goodvoice idle soak, second opinion (plan.md task 4.5)"
Write-Output "  pid       $ProcessId"
Write-Output "  soak      $Minutes minutes, sampled every $IntervalSeconds s"
Write-Output "  machine   $cores logical processors"
Write-Output "  csv       $Csv"

$dir = Split-Path -Parent $Csv
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
'seconds,processes,cpu_machine_percent,working_set_mb,private_mb' | Set-Content -Path $Csv

# One reading of the tree: how much CPU time it has burned since its processes
# started, and how much memory it is holding right now.
function Read-Tree([int]$root) {
  $cpu = 0.0; $ws = 0.0; $priv = 0.0; $n = 0
  foreach ($id in Get-Tree $root) {
    $p = Get-Process -Id $id -ErrorAction SilentlyContinue
    if (-not $p) { continue }   # a renderer that exited between the list and here
    $cpu += $p.TotalProcessorTime.TotalSeconds
    $ws += $p.WorkingSet64
    $priv += $p.PrivateMemorySize64
    $n++
  }
  return [pscustomobject]@{ Cpu = $cpu; WorkingSet = $ws; Private = $priv; Count = $n }
}

# Who is holding the memory, not just how much. The answer decides whether a
# gap against the budget is something this app can fix or something WebView2
# charges for existing, so it is printed rather than left to be guessed at.
function Show-Breakdown([int]$root, [string]$when) {
  Write-Output ''
  Write-Output "  --- $when ---"
  foreach ($id in Get-Tree $root) {
    $p = Get-Process -Id $id -ErrorAction SilentlyContinue
    if (-not $p) { continue }
    $line = '    {0,-24} pid {1,-7} {2,8:N1} MB working set  {3,8:N1} MB private'
    Write-Output ($line -f $p.ProcessName, $p.Id, ($p.WorkingSet64 / 1MB), ($p.PrivateMemorySize64 / 1MB))
  }
}

Show-Breakdown $ProcessId 'at the start'

$started = Get-Date
$previous = Read-Tree $ProcessId
$previousAt = Get-Date
$cpuSamples = @(); $ramSamples = @()

while (((Get-Date) - $started).TotalMinutes -lt $Minutes) {
  Start-Sleep -Seconds $IntervalSeconds
  if (-not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) {
    Write-Output 'THE_APP_EXITED -- a soak of a process that is not running is not a measurement'
    break
  }

  $now = Read-Tree $ProcessId
  $at = Get-Date
  $wall = ($at - $previousAt).TotalSeconds
  # A tree whose total went down lost a process; that sample is skipped rather
  # than reported as a negative one.
  $delta = $now.Cpu - $previous.Cpu
  $percent = if ($wall -gt 0 -and $delta -ge 0) { 100.0 * $delta / ($wall * $cores) } else { 0.0 }
  $ws = $now.WorkingSet / 1MB
  $priv = $now.Private / 1MB

  # F, not N: N groups thousands, and a seconds column that reads 1,559.1 puts
  # a comma inside a field of a comma-separated file.
  '{0:F1},{1},{2:F3},{3:F2},{4:F2}' -f ($at - $started).TotalSeconds, $now.Count, $percent, $ws, $priv |
    Add-Content -Path $Csv
  $cpuSamples += $percent
  $ramSamples += $ws
  $previous = $now
  $previousAt = $at
}

Show-Breakdown $ProcessId 'at the end'

if ($cpuSamples.Count -eq 0) { Write-Output 'NO_SAMPLES'; exit 1 }
function Median($values) { $s = @($values | Sort-Object); return $s[[int](($s.Count - 1) / 2)] }

$cpuMedian = Median $cpuSamples
$ramPeak = ($ramSamples | Measure-Object -Maximum).Maximum
Write-Output ''
Write-Output ("SAMPLES=" + $cpuSamples.Count)
Write-Output ("CPU_MEDIAN_PERCENT={0:N2}" -f $cpuMedian)
Write-Output ("CPU_MAX_PERCENT={0:N2}" -f ($cpuSamples | Measure-Object -Maximum).Maximum)
Write-Output ("RAM_MEDIAN_MB={0:N1}" -f (Median $ramSamples))
Write-Output ("RAM_PEAK_MB={0:N1}" -f $ramPeak)
Write-Output ("CPU_WITHIN_BUDGET=" + ($cpuMedian -lt $cpuBudget))
Write-Output ("RAM_WITHIN_BUDGET=" + ($ramPeak -le $ramBudget))
