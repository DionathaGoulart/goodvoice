# plan.md 7.10: can a viewer ask for a picture, and what does a still share
# cost a room with nobody in it?
#
# The answer to the first is **no, not on this SFU** — see DR-44, and see the
# `PLI` line this prints. What is left is the second, and a drill that would
# notice if the first ever became true.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\keyframe.ps1
#   ... -Rounds 4 -Seconds 8 -StillSeconds 20
#
# Both halves need the same thing and no drill provided it: **a screen that is
# not moving.** WGC produces a frame when the content changes and nothing when
# it does not (DR-31), so every number here is about a still screen or it is
# about nothing. Run `rewatch` on a desktop with a console printing on it and
# what you measure is the console.
#
# So this puts `viewer.ps1`'s grey sheet over the monitor, runs both drills
# with their output redirected to files, and takes it down.
#
# ## What each half proves
#
# **rewatch, under the sheet.** A viewer opens, closes and opens again, four
# times. Every open is a fresh `tracks/new` to Cloudflare, and on a still
# screen the only thing that can give it a picture is the sharer's two-second
# repeat — because nothing ever asks. `a=rtcp-fb:102 nack pli` has been in this
# client's offer since before anybody added it deliberately, and Cloudflare has
# never used it: the publisher's `pli_count` sat at **zero** across four
# viewers opening, with video flowing the whole time (DR-44).
#
# So what the seconds here measure is a subscription round trip plus wherever
# in the two-second cycle the viewer happened to arrive: 0.96 to 2.26 s over
# four rounds, which is the shape of a wait for a clock rather than an answer
# to a question.
#
# **share-drill --no-viewer, under the sheet.** Nobody watches, so nobody asks.
# Counted at ana's own end — between the encoder and the transport — because
# bruno cannot tell "she sent nothing" from "nobody was listening".
#
# What this half is *not* is a check that a still share sends nothing, which is
# what §7.10 asked for before this drill existed and which Cloudflare refuses:
# a `tracks/new` for a track that has never carried a packet comes back *the
# publisher never started sending*. So a share that went silent would be a
# share nobody could open, and the two-second repeat stays as the heartbeat
# that keeps it subscribable (DR-44). What this measures is that it costs that
# and nothing more.
#
# Needs release builds of both drills. Nothing here needs the app.
[CmdletBinding()]
param(
  [int] $Rounds = 4,
  [int] $Seconds = 8,
  [int] $StillSeconds = 20,
  [string] $Bin = "$env:CARGO_TARGET_DIR\release",
  [string] $OutDir = "$env:TEMP\goodvoice-keyframe",
  [switch] $NoBackdrop
)

[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture

$rewatch = Join-Path $Bin 'rewatch.exe'
$shareDrill = Join-Path $Bin 'share-drill.exe'
foreach ($exe in @($rewatch, $shareDrill)) {
  if (-not (Test-Path $exe)) { Write-Output "NO_EXE=$exe"; exit 2 }
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$script:ok = $true
function Check([string] $label, [bool] $pass, $value) {
  Write-Output ("{0}={1}" -f $label, $value)
  if (-not $pass) { $script:ok = $false }
}

# The grey sheet, lifted from viewer.ps1 — same form, same reason for TopMost:
# Windows refuses the foreground to a process that did not have it, so a sheet
# started from a script sits wherever the z-order left it.
function Start-Backdrop {
  $script = @'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$form = New-Object System.Windows.Forms.Form
$form.FormBorderStyle = 'None'
$form.BackColor = [System.Drawing.Color]::FromArgb(176, 176, 176)
$form.Bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$form.ShowInTaskbar = $false
$form.TopMost = $true
$label = New-Object System.Windows.Forms.Label
$label.Text = "goodvoice is running the 7.10 keyframe drill. This sheet is the screen being shared, and it has to stay still: please do not move the mouse. It goes away on its own."
$label.AutoSize = $true
$label.Font = New-Object System.Drawing.Font('Segoe UI', 14)
$label.ForeColor = [System.Drawing.Color]::FromArgb(60, 60, 60)
$label.Location = New-Object System.Drawing.Point(40, 40)
$form.Controls.Add($label)
[System.Windows.Forms.Application]::Run($form)
'@
  $path = Join-Path $OutDir 'backdrop.ps1'
  Set-Content -Path $path -Value $script -Encoding UTF8
  return Start-Process powershell -PassThru -WindowStyle Hidden `
    -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $path)
}

# Hidden and redirected, so that running the drill is not itself something
# moving on the screen the drill is sharing.
function Run-Drill([string] $exe, [string[]] $drillArgs, [string] $name) {
  $log = Join-Path $OutDir "$name.txt"
  $err = Join-Path $OutDir "$name.err.txt"
  # `-Wait`, not `WaitForExit()` on the returned object: a process started
  # with `-PassThru` alone hands back a null `ExitCode` however long you wait
  # on it, and a null that is not zero is a drill that fails on its own
  # plumbing.
  $proc = Start-Process -FilePath $exe -PassThru -Wait -WindowStyle Hidden `
    -ArgumentList $drillArgs -RedirectStandardOutput $log -RedirectStandardError $err
  $code = $proc.ExitCode
  Check "${name}_EXIT" ($code -eq 0) $code
  return Get-Content $log
}

$backdrop = if ($NoBackdrop) { $null } else { Start-Backdrop }
try {
  if ($backdrop) { Start-Sleep -Seconds 2 }

  # ---- half one: a viewer opening onto a still screen ------------------------
  Write-Output '--- rewatch, under the sheet'
  $out = Run-Drill $rewatch @('--rounds', "$Rounds", '--seconds', "$Seconds") 'rewatch'
  $out | ForEach-Object { Write-Output "  $_" }

  # `  1   53   2   1.05 s` — or `**never**` where the seconds should be.
  $rows = $out | Select-String '^\s+\d+\s+\d+\s+\d+\s+'
  Check 'ROUNDS' ($rows.Count -eq $Rounds) $rows.Count
  $never = @($rows | Where-Object { $_ -match 'never' })
  # The whole of it. On a still screen nothing is encoded, so a viewer that got
  # a picture got it because somebody asked and the sharer answered.
  Check 'BLIND_ROUNDS' ($never.Count -eq 0) $never.Count
  $times = @($rows | ForEach-Object {
      if ($_ -match '([\d.]+)\s*s\s*$') { [double]$Matches[1] }
    })
  if ($times.Count) {
    # Spread, not speed, is what these say. A subscription is most of a second
    # on its own (§7.9), so a client whose ask worked would land just above
    # that every time; one that is waiting for a two-second clock lands
    # anywhere in a two-second band, which is what these do.
    Check 'FIRST_PICTURE_WORST_S' $true ([math]::Round(($times | Measure-Object -Maximum).Maximum, 2))
    Check 'FIRST_PICTURE_BEST_S' $true ([math]::Round(($times | Measure-Object -Minimum).Minimum, 2))
    Check 'FIRST_PICTURE_MEDIAN_S' $true ([math]::Round(($times | Sort-Object)[[int]($times.Count / 2)], 2))
  }

  # ---- half two: what a still share costs a room with nobody in it ----------
  Write-Output ''
  Write-Output '--- share-drill --no-viewer, under the sheet'
  $out = Run-Drill $shareDrill @('--no-viewer', '--seconds', "$StillSeconds") 'share-drill'
  $out | ForEach-Object { Write-Output "  $_" }

  $wire = $out | Select-String '^- (\d+) access units, (\d+) bytes'
  if (-not $wire) { Check 'SENT' $false '(no line)' }
  else {
    # The first match is ana's own count; bruno's block prints the same shape
    # and is nothing here, because nobody watched.
    $units = [int]$wire[0].Matches[0].Groups[1].Value
    $bytes = [int]$wire[0].Matches[0].Groups[2].Value
    Check 'SENT_UNITS' $true $units
    Check 'SENT_BYTES' $true $bytes
    Check 'SENT_KB_PER_S' $true ([math]::Round($bytes / 1024.0 / $StillSeconds, 2))
    # The heartbeat and nothing else: one keyframe every two seconds, plus the
    # picture every share opens with. More than that on a screen that did not
    # move would mean something is encoding when there is nothing to encode.
    $budget = [int]($StillSeconds / 2) + 3
    Check 'SENT_IS_THE_HEARTBEAT' ($units -le $budget) "$units units in $StillSeconds s (at most $budget)"
  }
}
finally {
  if ($backdrop) { Stop-Process -Id $backdrop.Id -Force -ErrorAction SilentlyContinue }
  Get-Process rewatch, share-drill -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}

Write-Output ''
Write-Output "LOGS=$OutDir"
Write-Output ("RESULT=" + $(if ($script:ok) { 'PASS' } else { 'FAIL' }))
exit $(if ($script:ok) { 0 } else { 1 })
