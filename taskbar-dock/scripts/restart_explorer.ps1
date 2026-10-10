# Restarts Explorer without keystrokes: the taskbar's own "Exit Explorer" message (what
# Ctrl+Shift+right-click on the taskbar > Exit Explorer sends: WM_USER+436 to Shell_TrayWnd),
# then explorer.exe started again through a one-off scheduled task (outside the Claude app's
# MSIX container, as at sign-in). Prints how long each step took.
$ErrorActionPreference = 'Stop'
Add-Type -Namespace W -Name U -MemberDefinition @'
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
[DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
'@
$tray = [W.U]::FindWindow('Shell_TrayWnd', [NullString]::Value)
if ($tray -eq [IntPtr]::Zero) { Write-Output 'no taskbar (Shell_TrayWnd) found; not touching anything'; exit 1 }
$sw = [Diagnostics.Stopwatch]::StartNew()
[W.U]::PostMessage($tray, 0x5B4, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
while ([W.U]::FindWindow('Shell_TrayWnd', [NullString]::Value) -ne [IntPtr]::Zero -and $sw.ElapsedMilliseconds -lt 10000) { Start-Sleep -Milliseconds 100 }
if ([W.U]::FindWindow('Shell_TrayWnd', [NullString]::Value) -ne [IntPtr]::Zero) { Write-Output 'Explorer did not exit within 10 s'; exit 1 }
Write-Output ("taskbar gone after {0} ms" -f $sw.ElapsedMilliseconds)
Start-Sleep -Milliseconds 500
$task = 'ClaudeRestartExplorer'
$action = New-ScheduledTaskAction -Execute "$env:WINDIR\explorer.exe"
Register-ScheduledTask -TaskName $task -Action $action -Force | Out-Null
Start-ScheduledTask -TaskName $task
$sw.Restart()
while ([W.U]::FindWindow('Shell_TrayWnd', [NullString]::Value) -eq [IntPtr]::Zero -and $sw.ElapsedMilliseconds -lt 20000) { Start-Sleep -Milliseconds 200 }
Start-Sleep -Milliseconds 500
Unregister-ScheduledTask -TaskName $task -Confirm:$false -ErrorAction SilentlyContinue
if ([W.U]::FindWindow('Shell_TrayWnd', [NullString]::Value) -eq [IntPtr]::Zero) { Write-Output 'TASKBAR NOT BACK after 20 s'; exit 1 }
Write-Output ("taskbar back after {0} ms" -f $sw.ElapsedMilliseconds)
