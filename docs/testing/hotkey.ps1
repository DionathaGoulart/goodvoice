# plan.md 4.3: does the talk key reach goodvoice while something else has focus?
# `keybd_event` puts the key into the system input queue exactly as the keyboard
# does. It goes to whatever has focus — which is never the drill: it has no
# window at all, so anything it hears, it heard globally.
Add-Type -Namespace Win -Name Key -MemberDefinition @'
[DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
'@
function Send-Key([byte]$vk, [bool]$down) {
  [Win.Key]::keybd_event($vk, 0, $(if ($down) { 0 } else { 2 }), [UIntPtr]::Zero)  # 2 = KEYEVENTF_KEYUP
}

$exe = "$env:CARGO_TARGET_DIR\debug\hotkey-drill.exe"
if (-not (Test-Path $exe)) { $exe = 'client\src-tauri\target\debug\hotkey-drill.exe' }
# F13: a key this keyboard does not have and nothing else is listening for, so
# the presses below cannot disturb whatever is on screen.
# Started through .NET rather than Start-Process: only this one reliably has an
# ExitCode afterwards, and the exit code is the whole verdict.
$info = New-Object System.Diagnostics.ProcessStartInfo
$info.FileName = $exe
$info.Arguments = '--key F13 --seconds 8'
$info.UseShellExecute = $false
$info.RedirectStandardOutput = $true
$p = [System.Diagnostics.Process]::Start($info)
Start-Sleep -Seconds 3   # the hook goes on a moment after the process starts

foreach ($i in 1..3) {
  Send-Key 0x7C $true    # VK_F13 down
  Start-Sleep -Milliseconds 250
  Send-Key 0x7C $false   # ...and up
  Start-Sleep -Milliseconds 250
}

$transcript = $p.StandardOutput.ReadToEnd()
$p.WaitForExit()
Write-Output $transcript
# 0 only if the drill saw both edges of the key.
Write-Output ("EXIT=" + $p.ExitCode)
