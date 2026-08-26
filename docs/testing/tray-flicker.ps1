# plan.md 7.3: does the window flicker when the tray rebuilds it?
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\testing\tray-flicker.ps1
#   ... -Cycles 5           # five rebuilds, each one photographed
#   ... -Exe path\to\goodvoice-client.exe
#
# `tray.md`'s last row is the one no counter answers. Closing goodvoice
# destroys the window and its webview (DR-21); clicking the tray icon builds a
# new one in about a seventh of a second; whether that is *a flash* is a thing
# about pixels. The eye it needs is a screen grab running faster than the
# screen refreshes, which is what the C# below is for — PowerShell's
# per-iteration cost spends the ten chances a 150 ms rebuild gives you.
#
# What comes out is a filmstrip — every frame of a rebuild, at ~140 a second
# against a screen that changes 60 times a second — plus the numbers that say
# which frames are the bare desktop, which are the finished window, and which
# are neither. A frame that is neither *and* is a flat fill is the flash. A
# frame that is neither and is busy is a window part-way through its first
# paint, which is a different thing and not a fault.
#
# Two passes, because a flicker has two shapes. `GEOM` is the window arriving
# at the wrong size or place and jumping; `FRAME` is it arriving blank.
#
# Needs a RELEASE build with the `custom-protocol` feature — see tray.md. A
# build without it is Edge's error page in a goodvoice window, and its paint is
# not this app's paint.
[CmdletBinding()]
param(
  [int] $Cycles = 3,
  [string] $Exe = "$env:CARGO_TARGET_DIR\release\goodvoice-client.exe",
  [string] $OutDir = "$env:TEMP\goodvoice-tray-flicker",
  [int] $Frames = 400,
  [int] $WatchMs = 2000,
  [int] $Margin = 48
)

# Decimal commas otherwise, in the output and in the CSV both.
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing

Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

namespace Gv {
  public class Win {
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr h, out int pid);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern int PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, UIntPtr e);

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

    // Not `InvokePattern.Invoke()`, which is what tray-roundtrip.ps1 uses on
    // this same icon. Invoke works there and works here — but it does not
    // return until the shell has finished with it, and every millisecond it
    // holds is a millisecond of the rebuild nobody photographed. `mouse_event`
    // returns while the click is still on its way.
    public static void LeftClick(int x, int y) {
      SetCursorPos(x, y);
      mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero);   // LEFTDOWN
      mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);   // LEFTUP
    }
  }

  // One rebuild, watched as geometry: when a window handle for this process
  // first exists, what rectangle it claims, when it turns visible, and whether
  // the rectangle changes afterwards. A window that arrives at one size and
  // jumps to another is a flicker no screen grab has to catch.
  public class Geometry {
    public List<string> Rows = new List<string>();
    public double FirstMs = -1, VisibleMs = -1;
    public int Changes = 0;
    public string FinalRect = "";

    public void Watch(int pid, double maxMs) {
      Stopwatch sw = Stopwatch.StartNew();
      string last = null;
      while (sw.Elapsed.TotalMilliseconds < maxMs) {
        IntPtr h = Win.FindTauri(pid);
        double t = sw.Elapsed.TotalMilliseconds;
        if (h == IntPtr.Zero) { continue; }
        if (FirstMs < 0) { FirstMs = t; }
        Win.RECT r;
        if (!Win.GetWindowRect(h, out r)) { continue; }
        bool vis = Win.IsWindowVisible(h);
        if (vis && VisibleMs < 0) { VisibleMs = t; }
        FinalRect = r.L + "," + r.T + " " + (r.R - r.L) + "x" + (r.B - r.T);
        string now = FinalRect + " " + (vis ? "vis" : "hid");
        if (now != last) {
          Rows.Add(t.ToString("0.0") + " " + now);
          if (last != null) { Changes++; }
          last = now;
        }
      }
    }
  }

  // One rebuild, watched as pixels. The bitmaps and their Graphics are
  // allocated up front so the loop is a BitBlt and a timestamp and nothing
  // else.
  //
  // Where to point it is the awkward part: the config gives the window no
  // position, so Windows picks, and it picks a *different* one each rebuild
  // (see FLICKER.md — the window walks down the screen). Guessing the region
  // in advance photographs the wrong patch of desktop. So the capture waits
  // for the handle to exist — a few milliseconds, and long before the first
  // paint — reads the rectangle off it, and starts there.
  public class Filmstrip {
    public Bitmap[] Shots;
    public Bitmap Base;
    public double[] Ms;
    public int Count;
    public int X, Y;                     // top-left of the captured region
    public double HandleMs = -1;         // when a window handle first existed
    public string Rect = "";
    Graphics[] pens;
    Graphics basePen;
    int w, h, margin;

    public Filmstrip(int width, int height, int marginPx, int slots) {
      margin = marginPx;
      w = width + 2 * margin; h = height + 2 * margin;
      Shots = new Bitmap[slots];
      pens = new Graphics[slots];
      Ms = new double[slots];
      for (int i = 0; i < slots; i++) {
        Shots[i] = new Bitmap(w, h, PixelFormat.Format32bppRgb);
        pens[i] = Graphics.FromImage(Shots[i]);
      }
      Base = new Bitmap(w, h, PixelFormat.Format32bppRgb);
      basePen = Graphics.FromImage(Base);
    }

    public int Width { get { return w; } }
    public int Height { get { return h; } }

    // The bare desktop at the same coordinates, taken with the window gone.
    // Frame zero cannot serve: by the time the region is known the window
    // already exists.
    public void CaptureBase() {
      basePen.CopyFromScreen(X, Y, 0, 0, new Size(w, h), CopyPixelOperation.SourceCopy);
    }

    public double Run(int pid, double maxMs) {
      Size size = new Size(w, h);
      Stopwatch sw = Stopwatch.StartNew();
      Count = 0; HandleMs = -1;
      IntPtr found = IntPtr.Zero;
      while (sw.Elapsed.TotalMilliseconds < maxMs) {
        found = Win.FindTauri(pid);
        if (found != IntPtr.Zero) { HandleMs = sw.Elapsed.TotalMilliseconds; break; }
      }
      if (found == IntPtr.Zero) { return sw.Elapsed.TotalMilliseconds; }
      Win.RECT r;
      Win.GetWindowRect(found, out r);
      X = r.L - margin; Y = r.T - margin;
      Rect = r.L + "," + r.T + " " + (r.R - r.L) + "x" + (r.B - r.T);
      while (Count < Shots.Length && sw.Elapsed.TotalMilliseconds < maxMs) {
        Ms[Count] = sw.Elapsed.TotalMilliseconds;
        pens[Count].CopyFromScreen(X, Y, 0, 0, size, CopyPixelOperation.SourceCopy);
        Count++;
      }
      return sw.Elapsed.TotalMilliseconds;
    }

    // Luma of every fourth pixel of every fourth row inside the sub-rectangle
    // the window occupies. Subsampled because the question is "what colour is
    // this frame and how busy is it", not "are these two frames identical".
    static byte[] Sample(Bitmap bmp, int rx, int ry, int rw, int rh) {
      Rectangle box = new Rectangle(rx, ry, rw, rh);
      BitmapData d = bmp.LockBits(box, ImageLockMode.ReadOnly, PixelFormat.Format32bppRgb);
      int cols = (rw + 3) / 4, rows = (rh + 3) / 4;
      byte[] outp = new byte[cols * rows];
      // rw*4 rather than the stride: on a locked sub-rectangle GDI+ may hand
      // back a pointer into the parent bitmap, and reading a whole stride from
      // the last row of the sub-rectangle then reads past it.
      byte[] line = new byte[rw * 4];
      int k = 0;
      for (int yy = 0; yy < rh; yy += 4) {
        Marshal.Copy(IntPtr.Add(d.Scan0, yy * d.Stride), line, 0, rw * 4);
        for (int xx = 0; xx < rw; xx += 4) {
          int p = xx * 4;   // BGRA in memory; the usual 30/59/11 split, integer
          outp[k++] = (byte)((line[p + 2] * 77 + line[p + 1] * 151 + line[p] * 28) >> 8);
        }
      }
      bmp.UnlockBits(d);
      return outp;
    }

    // For every frame: mean luma, how much of it differs from its own mean
    // (a flat fill scores ~0, a painted window scores high), and how far it is
    // from the bare desktop and from the settled window.
    public double[] Analyse(int rx, int ry, int rw, int rh, int finalIdx) {
      byte[][] all = new byte[Count][];
      for (int i = 0; i < Count; i++) { all[i] = Sample(Shots[i], rx, ry, rw, rh); }
      byte[] b = Sample(Base, rx, ry, rw, rh), f = all[finalIdx];
      double[] outp = new double[Count * 4];
      for (int i = 0; i < Count; i++) {
        byte[] s = all[i];
        double sum = 0;
        for (int k = 0; k < s.Length; k++) { sum += s[k]; }
        double mean = sum / s.Length;
        double busy = 0, db = 0, df = 0;
        for (int k = 0; k < s.Length; k++) {
          if (Math.Abs(s[k] - mean) > 24) { busy++; }
          db += Math.Abs(s[k] - b[k]);
          df += Math.Abs(s[k] - f[k]);
        }
        outp[i * 4 + 0] = mean;
        outp[i * 4 + 1] = busy / s.Length;
        outp[i * 4 + 2] = db / s.Length;
        outp[i * 4 + 3] = df / s.Length;
      }
      return outp;
    }
  }
}
'@

if (-not (Test-Path $Exe)) { $Exe = 'client\src-tauri\target\release\goodvoice-client.exe' }
if (-not (Test-Path $Exe)) { Write-Output "NO_EXE=$Exe"; exit 2 }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
Get-ChildItem $OutDir -Include *.png, *.csv -Recurse -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue

$script:ok = $true
function Check([string] $label, [bool] $pass, $value) {
  Write-Output ("{0}={1}" -f $label, $value)
  if (-not $pass) { $script:ok = $false }
}

# The filmstrip. Every frame from the last one showing only the desktop to a
# few past the first one showing the finished window, laid out left to right
# with the millisecond it was taken on it — the whole of what a person had a
# chance to see, at better than the rate they could have seen it.
function Save-Sheet($strip, [int] $from, [int] $to, [string] $path) {
  $n = $to - $from + 1
  if ($n -le 0) { return }
  $cols = [math]::Min($n, 8)
  $rows = [math]::Ceiling($n / $cols)
  $scale = 0.42
  $cw = [int]($strip.Width * $scale)
  $ch = [int]($strip.Height * $scale)
  $pad = 6; $label = 18
  $sheet = New-Object System.Drawing.Bitmap ([int]($cols * ($cw + $pad) + $pad)), ([int]($rows * ($ch + $label + $pad) + $pad))
  $g = [System.Drawing.Graphics]::FromImage($sheet)
  $g.Clear([System.Drawing.Color]::FromArgb(24, 24, 28))
  $font = New-Object System.Drawing.Font 'Consolas', 10
  $ink = [System.Drawing.Brushes]::Gainsboro
  for ($k = 0; $k -lt $n; $k++) {
    $c = $k % $cols; $r = [math]::Floor($k / $cols)
    $x = $pad + $c * ($cw + $pad)
    $y = $pad + $r * ($ch + $label + $pad)
    $g.DrawString(("{0}  {1} ms" -f ($from + $k), [math]::Round($strip.Ms[$from + $k], 1)), $font, $ink, $x, $y)
    $g.DrawImage($strip.Shots[$from + $k], $x, ($y + $label), $cw, $ch)
  }
  $g.Dispose(); $font.Dispose()
  $sheet.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $sheet.Dispose()
}

# ---- the notification-area icon, as tray-roundtrip.ps1 finds it -------------
# Same four traps: names with state appended, `goodvoice` being two different
# buttons, ask-then-act, and never twice in a row. Finding it is a two-second
# UI Automation walk, so it happens *before* any clock starts — the first
# version of this drill timed the walk and reported a 2.2-second rebuild.
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
function Show-TrayIcon([string] $name) {
  for ($try = 0; $try -lt 3; $try++) {
    $found = Find-TrayButton $name
    if ($found) { return $found }
    $chevron = Find-TrayButton 'Show Hidden Icons'
    if ($chevron) {
      $pat = $null
      if ($chevron.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref] $pat)) { $pat.Invoke() }
    }
    Start-Sleep -Seconds 2
  }
  return $null
}
# The point on screen to click, worked out while nothing is being measured.
function Get-TrayPoint {
  $icon = Show-TrayIcon 'goodvoice'
  if (-not $icon) { return $null }
  $r = $icon.Current.BoundingRectangle
  return @([int]($r.X + $r.Width / 2), [int]($r.Y + $r.Height / 2))
}

function Away([int] $owner) {
  $h = [Gv.Win]::FindTauri($owner)
  if ($h -eq [IntPtr]::Zero) { return $true }
  [Gv.Win]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null   # WM_CLOSE
  for ($i = 0; $i -lt 40; $i++) {
    Start-Sleep -Milliseconds 250
    if ([Gv.Win]::FindTauri($owner) -eq [IntPtr]::Zero) { return $true }
  }
  return $false
}

# ---- the drill --------------------------------------------------------------
# In a room, because the window that comes back has to repaint a call and not
# an empty join form — the busiest thing it ever paints at once.
$room = "flick-" + (Get-Random -Minimum 100000 -Maximum 999999)
$env:GOODVOICE_AUTOJOIN = $room
$p = Start-Process -FilePath $Exe -PassThru
Check 'ROOM' $true $room
$walk = @()

try {
  $h = [IntPtr]::Zero
  for ($i = 0; $i -lt 40 -and $h -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 500; $h = [Gv.Win]::FindTauri($p.Id) }
  if ($h -eq [IntPtr]::Zero) { Write-Output 'NO_WINDOW'; exit 1 }
  # The eight seconds tray.md argues for: a WM_CLOSE inside the first second or
  # two is handled by Windows rather than by Tauri, and the app just exits.
  Start-Sleep -Seconds 8

  $r0 = New-Object Gv.Win+RECT
  [Gv.Win]::GetWindowRect($h, [ref] $r0) | Out-Null
  $w0 = $r0.R - $r0.L; $h0 = $r0.B - $r0.T
  Check 'RECT_FIRST' $true ("{0},{1} {2}x{3}" -f $r0.L, $r0.T, $w0, $h0)
  $walk += "{0},{1}" -f $r0.L, $r0.T

  # ---- pass A: the rebuild as geometry --------------------------------------
  if (-not (Away $p.Id)) { Write-Output 'CLOSE_FAILED'; exit 1 }
  Start-Sleep -Seconds 3
  $point = Get-TrayPoint
  if (-not $point) { Write-Output 'NO_TRAY_ICON'; exit 1 }
  $geo = New-Object Gv.Geometry
  [Gv.Win]::LeftClick($point[0], $point[1])
  $geo.Watch($p.Id, $WatchMs)
  Check 'GEOM_HANDLE_AT_MS' ($geo.FirstMs -ge 0) ([math]::Round($geo.FirstMs, 1))
  Check 'GEOM_VISIBLE_AT_MS' ($geo.VisibleMs -ge 0) ([math]::Round($geo.VisibleMs, 1))
  # Zero would be a window born visible at its final size. One is the ordinary
  # hidden-then-shown flip. More is a window moving or resizing after somebody
  # could already see it, which is the flicker this pass exists for.
  Check 'GEOM_CHANGES_AFTER_FIRST' ($geo.Changes -le 1) $geo.Changes
  Check 'GEOM_RECT' $true $geo.FinalRect
  foreach ($row in $geo.Rows) { Write-Output "  geom $row" }
  $walk += ($geo.FinalRect -split ' ')[0]
  Start-Sleep -Seconds 3

  # ---- pass B: the rebuild as pixels ----------------------------------------
  $strip = New-Object Gv.Filmstrip @($w0, $h0, $Margin, $Frames)
  for ($cycle = 1; $cycle -le $Cycles; $cycle++) {
    if (-not (Away $p.Id)) { Write-Output 'CLOSE_FAILED'; exit 1 }
    Start-Sleep -Seconds 3
    $point = Get-TrayPoint
    if (-not $point) { Write-Output 'NO_TRAY_ICON'; exit 1 }

    [Gv.Win]::LeftClick($point[0], $point[1])
    $span = $strip.Run($p.Id, $WatchMs)
    Check "HANDLE_AT_MS_$cycle" ($strip.HandleMs -ge 0 -and $strip.HandleMs -lt 1000) ([math]::Round($strip.HandleMs, 1))
    Check "FRAMES_$cycle" ($strip.Count -gt 60) $strip.Count
    $rate = if ($span -gt 0) { [math]::Round($strip.Count / $span * 1000, 1) } else { 0 }
    # 60 is the screen. A filmstrip slower than the screen could have a flash
    # hiding between two of its frames; this is the number that makes the rest
    # of the run admissible.
    Check "FRAME_RATE_$cycle" ($rate -ge 60) $rate
    Check "RECT_$cycle" $true $strip.Rect
    $walk += ($strip.Rect -split ' ')[0]

    # The window has to still be where the capture pointed, or the frames are
    # of the wrong patch of screen and everything below is noise.
    Start-Sleep -Seconds 2
    $now = [Gv.Win]::FindTauri($p.Id)
    $r2 = New-Object Gv.Win+RECT
    [Gv.Win]::GetWindowRect($now, [ref] $r2) | Out-Null
    $settledRect = "{0},{1} {2}x{3}" -f $r2.L, $r2.T, ($r2.R - $r2.L), ($r2.B - $r2.T)
    Check "STILL_THERE_$cycle" ($settledRect -eq $strip.Rect) $settledRect

    # The bare desktop underneath, taken at the same coordinates with the
    # window gone — and the close that takes it is the next cycle's close.
    if (-not (Away $p.Id)) { Write-Output 'CLOSE_FAILED'; exit 1 }
    Start-Sleep -Seconds 2
    $strip.CaptureBase()

    # Measured over the window's own area inset by twelve, so the drop shadow
    # and the invisible resize border are nobody's evidence.
    $ax = $Margin + 12; $ay = $Margin + 12
    $aw = $w0 - 24; $ah = $h0 - 24
    $stats = $strip.Analyse($ax, $ay, $aw, $ah, $strip.Count - 1)
    $rows = @()
    for ($i = 0; $i -lt $strip.Count; $i++) {
      $rows += [pscustomobject]@{
        i         = $i
        ms        = [math]::Round($strip.Ms[$i], 1)
        luma      = [math]::Round($stats[$i * 4], 1)
        busy      = [math]::Round($stats[$i * 4 + 1], 3)
        vs_before = [math]::Round($stats[$i * 4 + 2], 1)
        vs_after  = [math]::Round($stats[$i * 4 + 3], 1)
      }
    }
    $csv = Join-Path $OutDir "frames-$cycle.csv"
    $rows | Export-Csv -NoTypeInformation -Path $csv
    Write-Output "FRAME_CSV_$cycle=$csv"

    # Three classes, and only the third is interesting. `desktop` is the region
    # with no window over it, `settled` is the window as it ends up; a frame
    # near neither is the window mid-arrival, and how many of those there are
    # is the length of the transition in screen refreshes.
    $desktop = @($rows | Where-Object { $_.vs_before -le 4 })
    $settled = @($rows | Where-Object { $_.vs_after -le 4 })
    $between = @($rows | Where-Object { $_.vs_before -gt 4 -and $_.vs_after -gt 4 })
    # A flat fill — a white, black or grey rectangle where the window is — is
    # the flash itself, as against a window that is merely half-drawn.
    $flat = @($between | Where-Object { $_.busy -lt 0.02 })
    Check "DESKTOP_FRAMES_$cycle" $true $desktop.Count
    Check "SETTLED_FRAMES_$cycle" ($settled.Count -gt 5) $settled.Count
    Check "BETWEEN_FRAMES_$cycle" $true $between.Count
    Check "FLAT_FILL_FRAMES_$cycle" ($flat.Count -eq 0) $flat.Count
    if ($between.Count -gt 0) {
      Check "BETWEEN_SPAN_MS_$cycle" $true ([math]::Round($between[-1].ms - $between[0].ms, 1))
      foreach ($b in $between) { Write-Output ("  between i={0} ms={1} luma={2} busy={3}" -f $b.i, $b.ms, $b.luma, $b.busy) }
    }
    # And it must not come back after it has settled: a settled run with a
    # desktop frame after it is the window blinking out again.
    $lastDesktop = if ($desktop.Count) { $desktop[-1].i } else { -1 }
    $firstSettled = if ($settled.Count) { $settled[0].i } else { 9999 }
    Check "MONOTONIC_$cycle" ($lastDesktop -lt $firstSettled) "last_desktop=$lastDesktop first_settled=$firstSettled"

    $from = [math]::Max(0, $lastDesktop - 1)
    $to = [math]::Min($strip.Count - 1, [math]::Max($firstSettled + 3, $from + 7))
    $sheet = Join-Path $OutDir "filmstrip-$cycle.png"
    Save-Sheet $strip $from $to $sheet
    Write-Output "FILMSTRIP_$cycle=$sheet"
  }
  # Every position the window was given, in order. It is not one place: the
  # config names no position, so Windows names one, and it is a different one
  # each time. Recorded here because it is the thing a person watching the
  # round trip actually notices.
  Check 'WALK' $true ($walk -join ' -> ')
}
finally {
  Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 2
  $left = @(Get-Process goodvoice-client -ErrorAction SilentlyContinue).Count
  Check 'LEFTOVER' ($left -eq 0) $left
}

Write-Output ("RESULT=" + $(if ($script:ok) { 'PASS' } else { 'FAIL' }))
Write-Output 'Open the filmstrips. The numbers say how long the rebuild took to'
Write-Output 'settle; only eyes say whether what it did on the way was ugly.'
exit $(if ($script:ok) { 0 } else { 1 })
