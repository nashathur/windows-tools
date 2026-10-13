# A Claude desktop session has a private APPDATA copy, so dockctl there cannot see newly
# registered widgets until the real settings registry is copied into that session's copy.
$ErrorActionPreference = 'Stop'
$tempRoot = 'C:\Users\thurs\tools\_tmp'
$tempFile = Join-Path $tempRoot ("settings_$PID.json")
$taskName = "TaskbarDockSyncSettings_$PID"
$taskScript = Join-Path $tempRoot "sync_settings_$PID.ps1"
$doneFile = Join-Path $tempRoot "sync_settings_$PID.done"
$utf8NoBom = New-Object Text.UTF8Encoding $false
try {
    New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null
    $quotedTemp = $tempFile.Replace("'", "''")
    $quotedDone = $doneFile.Replace("'", "''")
    $body = @"
`$source = Join-Path `$env:APPDATA 'TaskbarDock\settings.json'
`$bytes = [IO.File]::ReadAllBytes(`$source)
[IO.File]::WriteAllBytes('$quotedTemp', `$bytes)
[IO.File]::WriteAllText('$quotedDone', `$source, (New-Object Text.UTF8Encoding `$false))
"@
    [IO.File]::WriteAllText($taskScript, $body, $utf8NoBom)
    $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$taskScript`""
    Register-ScheduledTask -TaskName $taskName -Action $action -Force | Out-Null
    Start-ScheduledTask -TaskName $taskName
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt 15 -and -not (Test-Path -LiteralPath $doneFile)) { Start-Sleep -Milliseconds 100 }
    if (-not (Test-Path -LiteralPath $doneFile)) { throw 'Timed out reading real settings.json (15 seconds)' }
    while ($watch.Elapsed.TotalSeconds -lt 15) {
        $task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
        if (-not $task -or $task.State -ne 'Running') { break }
        Start-Sleep -Milliseconds 100
    }
    $bytes = [IO.File]::ReadAllBytes($tempFile)
    $privateSettings = Join-Path $env:APPDATA 'TaskbarDock\settings.json'
    # The path string is the same inside and outside the session: ask where the file really is on
    # disk, as settings::redirected does (GetFinalPathNameByHandleW).
    if (-not ('SyncFinalPath' -as [type])) {
        Add-Type @'
using System; using System.Text; using System.Runtime.InteropServices; using Microsoft.Win32.SafeHandles;
public static class SyncFinalPath {
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern uint GetFinalPathNameByHandleW(SafeFileHandle h, StringBuilder path, uint size, uint flags);
    public static string Of(string file) {
        using (var fs = new System.IO.FileStream(file, System.IO.FileMode.Open, System.IO.FileAccess.Read, System.IO.FileShare.ReadWrite | System.IO.FileShare.Delete)) {
            var sb = new StringBuilder(1024);
            return GetFinalPathNameByHandleW(fs.SafeFileHandle, sb, 1024, 0) > 0 ? sb.ToString().Replace(@"\\?\", "") : file;
        }
    }
}
'@
    }
    $redirected = (Test-Path -LiteralPath $privateSettings) -and ([SyncFinalPath]::Of($privateSettings) -ine $privateSettings)
    if ($redirected) {
        [IO.Directory]::CreateDirectory((Split-Path $privateSettings)) | Out-Null
        [IO.File]::WriteAllBytes($privateSettings, $bytes)
    }
    $settings = [IO.File]::ReadAllText($privateSettings) | ConvertFrom-Json -ErrorAction Stop
    $count = if ($settings.widgets -is [System.Array]) { $settings.widgets.Count } elseif ($settings.widgets) { @($settings.widgets).Count } else { 0 }
    Write-Output "session settings synced: $count widgets registered"
} finally {
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $tempFile,$taskScript,$doneFile -Force -ErrorAction SilentlyContinue
}
