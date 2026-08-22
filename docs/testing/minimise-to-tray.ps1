# plan.md 4.1: does minimising hide the window into the tray?
Add-Type -Namespace Win -Name Api3 -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
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
$script:found = [IntPtr]::Zero
$cb = [Win.Api3+EnumProc]{
  param($h, $l)
  $owner = 0
  [Win.Api3]::GetWindowThreadProcessId($h, [ref]$owner) | Out-Null
  if ($owner -eq $p.Id) {
    $c = New-Object System.Text.StringBuilder 256
    [Win.Api3]::GetClassName($h, $c, 256) | Out-Null
    if ($c.ToString() -eq 'Tauri Window') { $script:found = $h; return $false }
  }
  return $true
}
for ($i = 0; $i -lt 40 -and $script:found -eq [IntPtr]::Zero; $i++) {
  Start-Sleep -Milliseconds 500
  [Win.Api3]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
}
$h = $script:found
if ($h -eq [IntPtr]::Zero) { Stop-Process -Id $p.Id -Force; Write-Output 'NO_WINDOW'; exit 1 }
# The app has to be past its own startup before a window message means anything
# to it: a close in the first second is handled by Windows, not by Tauri.
Start-Sleep -Seconds 8
Write-Output ("VISIBLE_BEFORE=" + [Win.Api3]::IsWindowVisible($h))

[Win.Api3]::ShowWindow($h, 6) | Out-Null   # SW_MINIMIZE, as the minimise button does
Start-Sleep -Seconds 3
$p.Refresh()
Write-Output ("ALIVE_AFTER_MINIMISE=" + (-not $p.HasExited))
Write-Output ("VISIBLE_AFTER_MINIMISE=" + [Win.Api3]::IsWindowVisible($h))

Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
Write-Output ("LEFTOVER=" + (Get-Process goodvoice-client -ErrorAction SilentlyContinue).Count)
