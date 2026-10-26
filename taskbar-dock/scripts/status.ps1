# Show registered widget, settings app, dockctl and launcher status, freshness and recent log problems.
# Switches: -Since <minutes> (default 60) controls the recent widget log window.
# Example: .\status.ps1 -Since 30
param([double]$Since = 60)

$ErrorActionPreference = 'Continue'
$dock = Split-Path $PSScriptRoot
$repo = Split-Path $dock
$dockctl = Join-Path $dock 'dockctl.exe'
$cutoff = (Get-Date).AddMinutes(-[Math]::Max(0, $Since))
$outOfDate = $false
$allWidgetsRunning = $true

function Get-Dockctl([string[]]$arguments) {
    if (-not (Test-Path -LiteralPath $dockctl)) { return $null }
    $text = & $dockctl @arguments 2>&1 | Out-String
    try { return ($text | ConvertFrom-Json -ErrorAction Stop) } catch { return $null }
}

function Get-NewestSource([string[]]$roots, [switch]$ExcludeBin) {
    $files = New-Object System.Collections.ArrayList
    foreach ($rootPath in $roots) {
        if (-not (Test-Path -LiteralPath $rootPath)) { continue }
        if ((Get-Item -LiteralPath $rootPath).PSIsContainer) {
            $isSourceDir = (Split-Path $rootPath -Leaf) -eq 'src'
            Get-ChildItem -LiteralPath $rootPath -File -Recurse -ErrorAction SilentlyContinue | Where-Object { (-not $isSourceDir -or $_.Extension -eq '.rs') -and (-not $ExcludeBin -or $_.FullName -notmatch '[\\/]bin[\\/]') } | ForEach-Object { [void]$files.Add($_) }
        } else { [void]$files.Add((Get-Item -LiteralPath $rootPath)) }
    }
    if (-not $files.Count) { return $null }
    return ($files | Sort-Object LastWriteTime -Descending | Select-Object -First 1).LastWriteTime
}

function Is-OutOfDate([string]$exe, $newest) {
    if (-not $exe -or -not (Test-Path -LiteralPath $exe -PathType Leaf)) { return $true }
    if ($null -eq $newest) { return $false }
    return (Get-Item -LiteralPath $exe).LastWriteTime -lt [datetime]$newest
}

function Get-WrapperValue([string]$wrapper, [string]$parameter, [string]$folder) {
    if (-not (Test-Path -LiteralPath $wrapper)) { return $null }
    $content = Get-Content -LiteralPath $wrapper -Raw
    $match = [regex]::Match($content, '(?is)-' + [regex]::Escape($parameter) + '\s+(?:"([^"]+)"|([^\s]+))')
    if (-not $match.Success) { return $null }
    $value = if ($match.Groups[1].Success) { $match.Groups[1].Value } else { $match.Groups[2].Value }
    $value = $value -replace '(?i)\$PSScriptRoot', $folder
    return [IO.Path]::GetFullPath($value)
}

function Test-ExeFresh([string]$exe, [string[]]$sourcePaths, [switch]$ExcludeBin) {
    $newest = Get-NewestSource $sourcePaths -ExcludeBin:$ExcludeBin
    $stale = Is-OutOfDate $exe $newest
    if ($stale) { $script:outOfDate = $true }
    return $stale
}

# Reads, in the real settings context (one-off scheduled task), the startup list, each widget's
# Show on taskbar and the tail of its log. Each reply goes to its own file in tools\_tmp (not
# %TEMP%: from a Claude session those writes can be private, unseen by the task). Returns
# @{ startup; show = @{name=reply}; logs = @{name=lines} }, or $null; messages go to the host so
# they never mix into the returned value.
function Get-RealSettingsStatus($widgetEntries) {
    $taskName = "TaskbarDockStatusReal_$PID"
    $dir = "C:\Tools\_tmp\tdstatus_$PID"
    $done = Join-Path $dir 'done.txt'
    try {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        $lines = @("`$c = '$($dockctl.Replace("'", "''"))'", "`$d = '$dir'",
            "& `$c all startup get | Out-File -Encoding utf8 (Join-Path `$d 'startup.json')")
        foreach ($entry in $widgetEntries) {
            $n = [string]$entry.name
            $folder = Split-Path ([string]$entry.exe)
            if ((Split-Path $folder -Leaf) -in @('release', 'debug', 'fast')) { $folder = Split-Path (Split-Path $folder) }
            $fallbackLog = "TaskbarDock\$(Split-Path $folder -Leaf).log"
            $lines += "& `$c '$n' get show_on_taskbar | Out-File -Encoding utf8 (Join-Path `$d 'show_$n.json')"
            $lines += "`$desc = try { (& `$c '$n' describe | Out-String) | ConvertFrom-Json } catch { `$null }"
            $lines += "`$log = if (`$desc -and `$desc.log_file) { [string]`$desc.log_file } else { Join-Path `$env:APPDATA '$fallbackLog' }"
            $lines += "if (Test-Path -LiteralPath `$log) { Get-Content -LiteralPath `$log -Encoding UTF8 -Tail 400 | Out-File -Encoding utf8 (Join-Path `$d 'log_$n.txt') }"
        }
        $lines += "'ok' | Out-File (Join-Path `$d 'done.txt')"
        $script = Join-Path $dir 'real.ps1'
        [IO.File]::WriteAllText($script, ($lines -join "`r`n"), (New-Object Text.UTF8Encoding $false))
        $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$script`""
        Register-ScheduledTask -TaskName $taskName -Action $action -Force | Out-Null
        Start-ScheduledTask -TaskName $taskName
        $watch = [Diagnostics.Stopwatch]::StartNew()
        while ($watch.Elapsed.TotalSeconds -lt 20 -and -not (Test-Path -LiteralPath $done)) { Start-Sleep -Milliseconds 100 }
        if (-not (Test-Path -LiteralPath $done)) { Write-Host "real settings unavailable (no answer in 20 s); script kept in $dir"; return $null }
        $read = { param($f) $p = Join-Path $dir $f; if (Test-Path -LiteralPath $p) { try { (Get-Content -LiteralPath $p -Raw -Encoding UTF8) | ConvertFrom-Json } catch { $null } } }
        $result = @{ startup = (& $read 'startup.json'); show = @{}; logs = @{} }
        foreach ($entry in $widgetEntries) {
            $n = [string]$entry.name
            $result.show[$n] = & $read "show_$n.json"
            $logFile = Join-Path $dir "log_$n.txt"
            $result.logs[$n] = if (Test-Path -LiteralPath $logFile) { @(Get-Content -LiteralPath $logFile -Encoding UTF8) } else { @() }
        }
        Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
        return $result
    } catch {
        Write-Host "real settings unavailable: $($_.Exception.Message)"
        return $null
    } finally {
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    }
}

function Get-WidgetLogProblems([string]$name, $logLines) {
    $lines = @($logLines)
    $hits = @()
    foreach ($line in $lines) {
        if ($line -notmatch '(?i)error|panic|crash|failed|not responding') { continue }
        if ($line -match '^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})\s') {
            $when = [datetime]::MinValue
            if ([datetime]::TryParseExact($matches[1], 'yyyy-MM-dd HH:mm:ss', [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::AssumeLocal, [ref]$when) -and $when -ge $cutoff) { $hits += $line }
        }
    }
    foreach ($line in @($hits | Select-Object -First 5)) { Write-Output "  log: $line" }
    if ($hits.Count -gt 5) { Write-Output "  log: $($hits.Count - 5) additional matching lines omitted" }
    if ($hits.Count) { Write-Output "  log problems: $($hits.Count) since $Since min" }
}

if (-not (Test-Path -LiteralPath $dockctl)) {
    Write-Output 'dockctl | missing | OUT OF DATE'
    $outOfDate = $true
}

$syncScript = Join-Path $PSScriptRoot 'sync_session_settings.ps1'
if (Test-Path -LiteralPath $syncScript) { & $syncScript }
# the registry as just synced (dockctl refuses `widgets list` inside a Claude session)
$widgetEntries = @()
try {
    $synced = [IO.File]::ReadAllText((Join-Path $env:APPDATA 'TaskbarDock\settings.json')) | ConvertFrom-Json -ErrorAction Stop
    $widgetEntries = @($synced.widgets | Where-Object { $_.name })
} catch { Write-Output "settings unreadable: $($_.Exception.Message)" }
$names = @($widgetEntries | ForEach-Object { [string]$_.name })
if (-not $widgetEntries.Count) {
    Write-Output 'widgets | registration/status unavailable'
    $allWidgetsRunning = $false
}
$realSettings = Get-RealSettingsStatus $widgetEntries
$startup = if ($realSettings) { $realSettings.startup } else { $null }
$widgetDirs = @(Get-ChildItem -LiteralPath $repo -Directory | Where-Object {
    $wrapper = Join-Path $_.FullName 'dev.ps1'
    (Test-Path -LiteralPath $wrapper) -and ((Get-Content -LiteralPath $wrapper -Raw) -match '(?i)taskbar-dock[\\/]scripts[\\/]dev\.ps1')
})

foreach ($name in $names) {
    $statusReply = Get-Dockctl @($name,'status')
    $stateReply = Get-Dockctl @($name,'state')
    $settingReply = if ($realSettings -and $null -ne $realSettings.show[$name]) { $realSettings.show[$name] } else { Get-Dockctl @($name,'get','show_on_taskbar') }
    $status = if ($statusReply.status) { [string]$statusReply.status } else { 'unknown' }
    $pidText = if ($statusReply.pid) { " (pid $($statusReply.pid))" } else { '' }
    if ($status -notin @('running','idle')) { $allWidgetsRunning = $false }
    $detail = if ($status -eq 'idle' -or $stateReply.idle -eq $true) { 'idle' } else { $status }
    if ($statusReply.pid) { $detail += " (pid $($statusReply.pid))" }
    if ($stateReply.strip) {
        $detail += if ($stateReply.strip.shown) { '; strip shown' } else { '; strip hidden' }
    }
    $show = if ($settingReply -and $null -ne $settingReply.value) { [string]$settingReply.value } else { 'unknown' }
    $start = 'unknown'
    if ($startup -and $startup.PSObject.Properties[$name]) {
        $entry = $startup.$name
        if ($entry.ok -eq $true) { $start = [string]$entry.startup }
    }
    $exe = if ($stateReply.exe) { [string]$stateReply.exe } else { $null }
    if (-not $exe) {
        $folder = $widgetDirs | Where-Object Name -eq $name | Select-Object -First 1
        if (-not $folder) {
            $folder = $widgetDirs | Where-Object { (Get-Content (Join-Path $_.FullName 'dev.ps1') -Raw) -match [regex]::Escape($name) } | Select-Object -First 1
        }
        if ($folder) { $exe = Get-WrapperValue (Join-Path $folder.FullName 'dev.ps1') 'Deploy' $folder.FullName }
    }
    $crate = $null
    $folder = $widgetDirs | Where-Object Name -eq $name | Select-Object -First 1
    if (-not $folder) { $folder = $widgetDirs | Where-Object { (Get-Content (Join-Path $_.FullName 'dev.ps1') -Raw) -match [regex]::Escape($name) } | Select-Object -First 1 }
    if ($folder) { $crate = Get-WrapperValue (Join-Path $folder.FullName 'dev.ps1') 'Crate' $folder.FullName }
    $stale = $false
    if ($crate) {
        $stale = Test-ExeFresh $exe @((Join-Path $crate 'src'),(Join-Path $crate 'Cargo.toml'),(Join-Path $crate 'build.rs'),(Join-Path $dock 'src'),(Join-Path $dock 'Cargo.toml')) -ExcludeBin
    } else {
        $script:outOfDate = $true
    }
    $freshText = if ($stale) { ' | OUT OF DATE' } elseif (-not $crate) { ' | freshness unknown' } else { '' }
    Write-Output ("widget {0} | {1} | Show on taskbar {2} | Start with Windows {3}{4}" -f $name,$detail,$show,$start,$freshText)
    $logLines = if ($realSettings -and $realSettings.logs.ContainsKey($name)) { $realSettings.logs[$name] } else { @() }
    Get-WidgetLogProblems $name $logLines
}

$appExe = Join-Path $dock 'taskbar-dock-settings.exe'
$appRunning = Get-Process -Name 'taskbar-dock-settings' -ErrorAction SilentlyContinue | Select-Object -First 1
$appDetail = if ($appRunning) { "running (pid $($appRunning.Id))" } else { 'stopped' }
$appSources = @((Join-Path $dock 'settings-app\src'),(Join-Path $dock 'settings-app\vendor'),(Join-Path $dock 'settings-app\Cargo.toml'),(Join-Path $dock 'settings-app\build.rs'),(Join-Path $dock 'src'),(Join-Path $dock 'Cargo.toml'))
$appStale = Test-ExeFresh $appExe $appSources -ExcludeBin
Write-Output ("settings app | {0}{1}" -f $appDetail, $(if ($appStale) { ' | OUT OF DATE' } else { '' }))

foreach ($tool in @(@('dockctl','dockctl.exe'),@('launcher','taskbar-dock-start.exe'))) {
    $processName = [IO.Path]::GetFileNameWithoutExtension($tool[1])
    $running = Get-Process -Name $processName -ErrorAction SilentlyContinue | Select-Object -First 1
    $detail = if ($running) { "running (pid $($running.Id))" } else { 'stopped' }
    $exe = Join-Path $dock $tool[1]
    $toolSources = @((Join-Path $dock 'src'),(Join-Path $dock 'Cargo.toml'),(Join-Path $dock 'build.rs'))
    if ($tool[0] -eq 'dockctl') { $toolSources += Join-Path $dock 'src\bin\dockctl.rs' }
    else { $toolSources += Join-Path $dock 'src\bin\taskbar-dock-start.rs' }
    $stale = Test-ExeFresh $exe $toolSources -ExcludeBin
    Write-Output ("{0} | {1}{2}" -f $tool[0],$detail,$(if ($stale) { ' | OUT OF DATE' } else { '' }))
}

if (-not $allWidgetsRunning -or $outOfDate) { exit 1 }
exit 0
