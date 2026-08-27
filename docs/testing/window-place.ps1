# plan.md 7.12: does the window come back where it was left?
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\window-place.ps1
#   ... -Exe path\to\goodvoice-client.exe
#
# `tray-flicker.ps1`'s WALK line answers half of this — five windows in one run
# at one position rather than four cascading down the screen — but it never
# *moves* the window, so what it proves is that goodvoice stopped walking, not
# that it remembers. This is the other half, and it is three questions:
#
#   MOVED     a window dragged somewhere and closed writes that rectangle to
#             `settings.json`, in logical pixels, position and size both.
#   REOPEN    the window the tray builds next comes up there.
#   RESTART   so does the first window of the *next process*, which is a
#             different path: that one is built from the config before `setup`
#             runs and has to be moved rather than born in place (`place.rs`).
#
# And a fourth that is about the failure this could cause rather than the one
# it fixes:
#
#   OFFSCREEN a remembered position that is no longer on any screen — the
#             second monitor that went away — is refused, and the window comes
#             up somewhere a person can reach it. A window that exists, has
#             focus and is nowhere is worse than one that forgot.
#
# Needs a RELEASE build with the `custom-protocol` feature: without it the
# window is Edge's error page, which is a window like any other for the
# purposes of a rectangle but not a build anybody should be measuring.
#
# It writes `settings.json` and puts it back. If this drill is killed part-way,
# `settings.json.window-place.bak` beside it is the original.
[CmdletBinding()]
param(
  [string] $Exe = "$env:CARGO_TARGET_DIR\release\goodvoice-client.exe",
  # Somewhere no cascade would ever land, so a pass cannot be a coincidence:
  # Windows starts at 104,104 and steps by 104.
  [int] $TargetX = 690,
  [int] $TargetY = 337,
  [int] $TargetW = 520,
  [int] $TargetH = 700
)

[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Windows.Forms

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

namespace GvPlace {
  public class Win {
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr h, out int pid);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int t, bool repaint);
    [DllImport("user32.dll")] public static extern int PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, UIntPtr e);
    // Per-window rather than per-system: the answer is the scale of the screen
    // this window is on, which is the one `place.rs` divided by.
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);

    public struct RECT { public int L, T, R, B; }
    public delegate bool EnumProc(IntPtr h, IntPtr lp);

    // By class, not by MainWindowHandle: a debug build owns a console window
    // too and .NET hands you that one (tray.md).
    [ThreadStatic] static IntPtr hit;
    [ThreadStatic] static int want;
    static bool Visit(IntPtr h, IntPtr lp) {
      int who;
      GetWindowThreadProcessId(h, out who);
      if (who == want) {
        StringBuilder c = new StringBuilder(256);
        GetClassName(h, c, 256);
        if (c.ToString() == "Tauri Window") { hit = h; return false; }
      }
      return true;
    }
    public static IntPtr FindTauri(int pid) {
      hit = IntPtr.Zero; want = pid;
      EnumWindows(new EnumProc(Visit), IntPtr.Zero);
      return hit;
    }

    public static void LeftClick(int x, int y) {
      SetCursorPos(x, y);
      mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero);   // LEFTDOWN
      mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);   // LEFTUP
    }
  }
}
'@

$script:ok = $true
function Check([string] $label, [bool] $pass, $value) {
  Write-Output ("{0}={1}" -f $label, $value)
  if (-not $pass) { $script:ok = $false }
}

# ---- the tray icon, as tray-flicker.ps1 finds it ----------------------------
# Same walk, same four traps: names with state appended, `goodvoice` being two
# different buttons, ask-then-act, and never twice in a row. Copied rather than
# shared because a drill that cannot be run on its own is a drill nobody runs.
$uiaRoot = [System.Windows.Automation.AutomationElement]::RootElement
$uiaAny = [System.Windows.Automation.Condition]::TrueCondition
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
function Get-TrayPoint {
  for ($try = 0; $try -lt 3; $try++) {
    $icon = Find-TrayButton 'goodvoice'
    if ($icon) {
      $r = $icon.Current.BoundingRectangle
      return @([int]($r.X + $r.Width / 2), [int]($r.Y + $r.Height / 2))
    }
    $chevron = Find-TrayButton 'Show Hidden Icons'
    if ($chevron) {
      $pat = $null
      if ($chevron.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref] $pat)) { $pat.Invoke() }
    }
    Start-Sleep -Seconds 2
  }
  return $null
}

# ---- the window -------------------------------------------------------------
function Wait-Window([int] $owner, [int] $seconds = 25) {
  for ($i = 0; $i -lt $seconds * 4; $i++) {
    $h = [GvPlace.Win]::FindTauri($owner)
    if ($h -ne [IntPtr]::Zero -and [GvPlace.Win]::IsWindowVisible($h)) { return $h }
    Start-Sleep -Milliseconds 250
  }
  return [IntPtr]::Zero
}
function Get-Frame([IntPtr] $h) {
  $r = New-Object GvPlace.Win+RECT
  [GvPlace.Win]::GetWindowRect($h, [ref] $r) | Out-Null
  $c = New-Object GvPlace.Win+RECT
  [GvPlace.Win]::GetClientRect($h, [ref] $c) | Out-Null
  $dpi = [GvPlace.Win]::GetDpiForWindow($h)
  if ($dpi -le 0) { $dpi = 96 }
  $scale = $dpi / 96.0
  # What `place.rs` stores: the outer position and the *inner* size, both
  # divided by this window's own scale factor.
  return [pscustomobject]@{
    Handle  = $h
    Text    = "{0},{1} {2}x{3}" -f $r.L, $r.T, ($r.R - $r.L), ($r.B - $r.T)
    X       = $r.L; Y = $r.T
    OuterW  = $r.R - $r.L; OuterH = $r.B - $r.T
    Scale   = $scale
    LogX    = [math]::Round($r.L / $scale, 3)
    LogY    = [math]::Round($r.T / $scale, 3)
    LogW    = [math]::Round(($c.R - $c.L) / $scale, 3)
    LogH    = [math]::Round(($c.B - $c.T) / $scale, 3)
  }
}
function Close-Window([int] $owner) {
  $h = [GvPlace.Win]::FindTauri($owner)
  if ($h -eq [IntPtr]::Zero) { return $true }
  [GvPlace.Win]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null   # WM_CLOSE
  for ($i = 0; $i -lt 40; $i++) {
    Start-Sleep -Milliseconds 250
    if ([GvPlace.Win]::FindTauri($owner) -eq [IntPtr]::Zero) { return $true }
  }
  return $false
}
function Stop-App($proc) {
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 2
}

if (-not (Test-Path $Exe)) { $Exe = 'client\src-tauri\target\release\goodvoice-client.exe' }
if (-not (Test-Path $Exe)) { Write-Output "NO_EXE=$Exe"; exit 2 }
Check 'EXE' $true $Exe

$settings = Join-Path $env:APPDATA 'art.good.goodvoice\settings.json'
$backup = "$settings.window-place.bak"
if (Test-Path $settings) { Copy-Item $settings $backup -Force }

$app = $null
try {
  # ---- MOVED ---------------------------------------------------------------
  # A window put somewhere on purpose, then closed. The close is what writes
  # the file: every move is noted in memory and only the window going away
  # goes to the disk (`place::keep`).
  $app = Start-Process -FilePath $Exe -PassThru
  $h = Wait-Window $app.Id
  if ($h -eq [IntPtr]::Zero) { Write-Output 'NO_WINDOW'; exit 1 }
  Check 'BORN_AT' $true (Get-Frame $h).Text
  Start-Sleep -Seconds 2

  [GvPlace.Win]::MoveWindow($h, $TargetX, $TargetY, $TargetW, $TargetH, $true) | Out-Null
  Start-Sleep -Seconds 1
  $moved = Get-Frame $h
  Check 'MOVED_TO' ($moved.X -eq $TargetX -and $moved.Y -eq $TargetY) $moved.Text
  Check 'SCALE' $true $moved.Scale

  if (-not (Close-Window $app.Id)) { Write-Output 'CLOSE_FAILED'; exit 1 }
  Start-Sleep -Seconds 1

  $stored = (Get-Content $settings -Raw | ConvertFrom-Json).window
  if (-not $stored) { Check 'WROTE_WINDOW' $false '(nothing)'; }
  else {
    $wrote = "{0},{1} {2}x{3} max={4}" -f $stored.x, $stored.y, $stored.width, $stored.height, $stored.maximized
    # Exact, not near: logical is the unit the config is in, so nothing has to
    # round on the way back and a pixel of drift would be a bug rather than
    # arithmetic.
    $same = ($stored.x -eq $moved.LogX) -and ($stored.y -eq $moved.LogY) -and
            ($stored.width -eq $moved.LogW) -and ($stored.height -eq $moved.LogH) -and
            (-not $stored.maximized)
    Check 'WROTE_WINDOW' $same $wrote
    Check 'EXPECTED_WINDOW' $true ("{0},{1} {2}x{3}" -f $moved.LogX, $moved.LogY, $moved.LogW, $moved.LogH)
  }

  # ---- REOPEN --------------------------------------------------------------
  # The tray builds the next one, and it is born in place rather than moved
  # there — which is the whole reason `tray::open` writes the rectangle into
  # the config instead of calling `set_position` afterwards.
  Start-Sleep -Seconds 2
  $point = Get-TrayPoint
  if (-not $point) { Write-Output 'NO_TRAY_ICON'; exit 1 }
  [GvPlace.Win]::LeftClick($point[0], $point[1])
  $h = Wait-Window $app.Id
  if ($h -eq [IntPtr]::Zero) { Write-Output 'NO_WINDOW_AFTER_TRAY'; exit 1 }
  Start-Sleep -Seconds 1
  $again = Get-Frame $h
  Check 'REOPEN_AT' ($again.Text -eq $moved.Text) $again.Text

  # ---- RESTART -------------------------------------------------------------
  # A different code path: this window is built from the config before `setup`
  # runs, so it is moved while hidden rather than born in place. The kill is
  # deliberate — the file was written by the close above, and a process killed
  # outright is the proof that it was.
  if (-not (Close-Window $app.Id)) { Write-Output 'CLOSE_FAILED'; exit 1 }
  Stop-App $app
  $app = Start-Process -FilePath $Exe -PassThru
  $h = Wait-Window $app.Id
  if ($h -eq [IntPtr]::Zero) { Write-Output 'NO_WINDOW_AFTER_RESTART'; exit 1 }
  Start-Sleep -Seconds 1
  $restarted = Get-Frame $h
  Check 'RESTART_AT' ($restarted.Text -eq $moved.Text) $restarted.Text
  Stop-App $app
  $app = $null

  # ---- OFFSCREEN -----------------------------------------------------------
  # The monitor that is not there any more, written by hand because unplugging
  # one is not something a drill can do. Far enough out that no arrangement of
  # screens could contain it.
  $json = Get-Content $settings -Raw | ConvertFrom-Json
  $json.window.x = -9000.0
  $json.window.y = -9000.0
  $json | ConvertTo-Json | Set-Content $settings -Encoding UTF8
  $app = Start-Process -FilePath $Exe -PassThru
  $h = Wait-Window $app.Id
  if ($h -eq [IntPtr]::Zero) { Write-Output 'NO_WINDOW_AFTER_OFFSCREEN'; exit 1 }
  Start-Sleep -Seconds 1
  $rescued = Get-Frame $h
  $virtual = [System.Windows.Forms.SystemInformation]::VirtualScreen
  $onScreen = ($rescued.X -ge $virtual.Left) -and ($rescued.Y -ge $virtual.Top) -and
              ($rescued.X -lt $virtual.Right) -and ($rescued.Y -lt $virtual.Bottom)
  Check 'OFFSCREEN_REFUSED' ($onScreen -and $rescued.X -ne -9000) $rescued.Text
  Check 'VIRTUAL_SCREEN' $true $virtual
}
finally {
  if ($app) { Stop-App $app }
  $left = @(Get-Process goodvoice-client -ErrorAction SilentlyContinue).Count
  Check 'LEFTOVER' ($left -eq 0) $left
  if (Test-Path $backup) { Move-Item $backup $settings -Force }
  Check 'SETTINGS_RESTORED' (-not (Test-Path $backup)) $settings
}

Write-Output ("RESULT=" + $(if ($script:ok) { 'PASS' } else { 'FAIL' }))
exit $(if ($script:ok) { 0 } else { 1 })
