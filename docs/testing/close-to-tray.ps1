# plan.md 4.1: does closing the window hide it and leave the app running?
Add-Type -Namespace Win -Name Close -MemberDefinition @'
[DllImport("user32.dll")] public static extern int PostMessage(IntPtr hWnd, uint msg, IntPtr wp, IntPtr lp);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int cmd);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
[DllImport("user32.dll")] public static extern int GetClassName(IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr hWnd, out int pid);
public delegate bool EnumProc(IntPtr hWnd, IntPtr lp);
'@
# The debug build to drive. Adjust if your CARGO_TARGET_DIR is elsewhere.
$exe = "$env:CARGO_TARGET_DIR\debug\goodvoice-client.exe"
if (-not (Test-Path $exe)) { $exe = 'client\src-tauri\target\debug\goodvoice-client.exe' }
$p = Start-Process -FilePath $exe -PassThru
# By window class: a debug build also owns a console window, and closing that
# one kills the process without Tauri ever seeing a thing.
$script:found = [IntPtr]::Zero
$cb = [Win.Close+EnumProc]{
  param($h, $l)
  $owner = 0
  [Win.Close]::GetWindowThreadProcessId($h, [ref]$owner) | Out-Null
  if ($owner -eq $p.Id) {
    $c = New-Object System.Text.StringBuilder 256
    [Win.Close]::GetClassName($h, $c, 256) | Out-Null
    if ($c.ToString() -eq 'Tauri Window') { $script:found = $h; return $false }
  }
  return $true
}
for ($i = 0; $i -lt 40 -and $script:found -eq [IntPtr]::Zero; $i++) {
  Start-Sleep -Milliseconds 500
  [Win.Close]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
}
$h = $script:found
if ($h -eq [IntPtr]::Zero) { Stop-Process -Id $p.Id -Force; Write-Output 'NO_WINDOW'; exit 1 }
# The app has to be past its own startup before a window message means anything
# to it: a close in the first second is handled by Windows, not by Tauri.
Start-Sleep -Seconds 8
Write-Output ("VISIBLE_BEFORE=" + [Win.Close]::IsWindowVisible($h))

[Win.Close]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null   # WM_CLOSE
Start-Sleep -Seconds 3
$p.Refresh()
Write-Output ("ALIVE_AFTER_CLOSE=" + (-not $p.HasExited))
Write-Output ("VISIBLE_AFTER_CLOSE=" + [Win.Close]::IsWindowVisible($h))

Write-Output ("ICONIC_WHILE_HIDDEN=" + [Win.Close]::IsIconic($h))

# The restore the tray click performs, in Windows' own terms: un-minimise a
# window that is hidden, then show it. If this cannot bring it back, neither
# can `tray::show`.
[Win.Close]::ShowWindow($h, 9) | Out-Null   # SW_RESTORE
[Win.Close]::ShowWindow($h, 5) | Out-Null   # SW_SHOW
Start-Sleep -Seconds 2
Write-Output ("VISIBLE_AFTER_RESTORE=" + [Win.Close]::IsWindowVisible($h))

Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
Write-Output ("LEFTOVER=" + (Get-Process goodvoice-client -ErrorAction SilentlyContinue).Count)
