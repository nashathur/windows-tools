# Build, test and restart one taskbar widget with short output (errors, warnings, test totals,
# new log lines). Each widget has a one-line dev.ps1 that calls this with its own paths.
#   -Crate   folder with Cargo.toml          -Name  process / exe name (no .exe)
#   -Deploy  where the running exe lives (copied there unless it is the build output itself)
#   -Log     the widget's log file (optional)
#   -NoTest  skip tests   -NoRestart  build (+ test) only   -Lib  also test taskbar-dock
#   -Start   start the widget even if it wasn't running (by default a widget that was not
#            running is left stopped)
#   -Fast    build the widget with the `fast` profile (no LTO, incremental: rebuilds in seconds,
#            exe a bit larger) instead of `release`. Tests always use `fast`.
#   -NoSmoke skip the smoke check after a restart (dockctl `state`, `do help`, `do now` if the
#            widget has it: read-only commands only; then the same pid must still be running)
# Before building, the theme lint (tests\theme_lint.rs) checks the widget's src for raw colours,
# font names and literal font sizes (they come from taskbar_dock::theme).
param(
    [Parameter(Mandatory)] [string]$Crate,
    [Parameter(Mandatory)] [string]$Name,
    [Parameter(Mandatory)] [string]$Deploy,
    [string]$Log,
    [switch]$NoTest, [switch]$NoRestart, [switch]$Lib, [switch]$Start, [switch]$Fast, [switch]$NoSmoke
)
# Continue: PS 5.1 turns cargo's stderr (warnings) into errors, which 'Stop' would throw on
$ErrorActionPreference = 'Continue'
$buildProfile = if ($Fast) { 'fast' } else { 'release' }
$built = Join-Path $Crate "target\$buildProfile\$Name.exe"

function Run-Cargo($dir, $cargoArgs) {
    Push-Location $dir
    $out = & cargo @cargoArgs 2>&1 | ForEach-Object { "$_" }
    $ok = $LASTEXITCODE -eq 0
    Pop-Location
    $out | Select-String -Pattern '^(error|warning)' -Context 0,6 | ForEach-Object { $_.Line; $_.Context.PostContext } | Out-Host
    $out | Where-Object { $_ -match '^test result|FAILED|panicked' -and $_ -notmatch 'ok. 0 passed' } | Out-Host
    return $ok
}

# stop the widget and wait until it's gone (its exe is free), instead of a fixed sleep
function Stop-Widget {
    $p = Get-Process $Name -ErrorAction SilentlyContinue
    if ($p) { $p | Stop-Process -Force; $p | Wait-Process -Timeout 3 -ErrorAction SilentlyContinue }
}

$running = [bool](Get-Process $Name -ErrorAction SilentlyContinue)
# a widget that runs straight from target\release locks its exe: stop it before building
if ($running -and -not $NoRestart -and ((Resolve-Path $Deploy -ErrorAction SilentlyContinue).Path -eq (Resolve-Path $built -ErrorAction SilentlyContinue).Path)) {
    Stop-Widget
}
# design code: no raw colours, font names or literal font sizes in the widget's source
# (the library's tests\theme_lint.rs, over this widget only)
Push-Location (Join-Path $PSScriptRoot '..')
$env:THEME_LINT_SRC = Join-Path $Crate 'src'
$lint = & cargo test --profile fast --quiet --test theme_lint widget_sources 2>&1 | ForEach-Object { "$_" }
$lintOk = $LASTEXITCODE -eq 0
Remove-Item Env:THEME_LINT_SRC
Pop-Location
if (-not $lintOk) {
    $hits = $lint | Where-Object { $_ -match ': (raw colour|font name|literal font size) ' }
    if (-not $hits) { $hits = $lint | Select-String -Pattern '^error' -Context 0,6 | ForEach-Object { $_.Line; $_.Context.PostContext } }
    $hits | Out-Host
    Write-Output 'THEME LINT FAILED'; exit 1
}
if (-not (Run-Cargo $Crate @('build', '--profile', $buildProfile, '--quiet'))) { Write-Output 'BUILD FAILED'; exit 1 }
if (-not $NoTest) {
    if (-not (Run-Cargo $Crate @('test', '--profile', 'fast', '--quiet'))) { Write-Output 'TESTS FAILED'; exit 1 }
    if ($Lib -and -not (Run-Cargo (Join-Path $PSScriptRoot '..') @('test', '--profile', 'fast', '--quiet'))) { Write-Output 'LIBRARY TESTS FAILED'; exit 1 }
}
if ($NoRestart) { Write-Output 'built'; exit 0 }
if (-not $running -and -not $Start) {
    if ((Resolve-Path $built).Path -ne [IO.Path]::GetFullPath($Deploy)) {
        for ($i = 0; ; $i++) {
            try { Copy-Item $built $Deploy -Force -ErrorAction Stop; break }
            catch { if ($i -ge 30) { Write-Output "COPY FAILED: $($_.Exception.Message)"; exit 1 }; Start-Sleep -Milliseconds 100 }
        }
    }
    Write-Output "built and installed; $Name was not running, left stopped (-Start to start it)"
    exit 0
}

$lines = if ($Log -and (Test-Path $Log)) { (Get-Content $Log).Count } else { 0 }
Stop-Widget
if ((Resolve-Path $built).Path -ne [IO.Path]::GetFullPath($Deploy)) {
    # the exe can stay locked for a moment after the process is gone: retry for up to 3 s
    for ($i = 0; ; $i++) {
        try { Copy-Item $built $Deploy -Force -ErrorAction Stop; break }
        catch { if ($i -ge 30) { Write-Output "COPY FAILED: $($_.Exception.Message)"; exit 1 }; Start-Sleep -Milliseconds 100 }
    }
}
# Start it through a one-off scheduled task, not Start-Process: from a Claude desktop session
# (MSIX-packaged) a child process would see the app's private %APPDATA% copy, so it would read
# a different order.txt / settings.json than the other widgets and overlap them in the dock.
$task = "TaskbarDockDev_$Name"
$action = New-ScheduledTaskAction -Execute $Deploy -WorkingDirectory (Split-Path $Deploy)
# the task doesn't inherit this shell's environment: pass the test aids on (every TASKBAR_*
# variable: TASKBAR_DOCK_TEST_ICONS_RIGHT for fit_test.ps1, TASKBAR_DISCORD_TEST, ...)
$aids = @(Get-ChildItem Env: | Where-Object { $_.Name -like 'TASKBAR_*' -and $_.Value })
if ($aids) {
    $sets = ($aids | ForEach-Object { "set `"$($_.Name)=$($_.Value)`" && " }) -join ''
    $action = New-ScheduledTaskAction -Execute 'cmd.exe' -WorkingDirectory (Split-Path $Deploy) `
        -Argument "/c $sets start `"`" `"$Deploy`""
}
Register-ScheduledTask -TaskName $task -Action $action -Force | Out-Null
Start-ScheduledTask -TaskName $task
# wait for start-up instead of a fixed 3 s: done once the log got new lines and then stayed
# quiet for 300 ms, or after 1 s without new lines (no log, or nothing logged at start); at
# most 3 s. A widget that exits straight away is reported below.
$size = { if ($Log -and (Test-Path $Log)) { (Get-Item $Log).Length } else { 0 } }
$sw = [Diagnostics.Stopwatch]::StartNew(); $last = & $size; $grewAt = $null
$seen = $false
while ($sw.ElapsedMilliseconds -lt 3000) {
    $alive = [bool](Get-Process $Name -ErrorAction SilentlyContinue)
    if ($alive) { $seen = $true } elseif ($seen) { break } # started, then exited
    Start-Sleep -Milliseconds 100
    $now = & $size
    if ($now -ne $last) { $last = $now; $grewAt = $sw.ElapsedMilliseconds }
    elseif ($grewAt -ne $null) { if ($sw.ElapsedMilliseconds - $grewAt -ge 300) { break } }
    elseif ($sw.ElapsedMilliseconds -ge 1000) { break }
}
Unregister-ScheduledTask -TaskName $task -Confirm:$false -ErrorAction SilentlyContinue
$p = Get-Process $Name -ErrorAction SilentlyContinue
if ($p) { Write-Output ("restarted {0:HH:mm:ss}, pid {1}" -f (Get-Date), $p.Id) } else { Write-Output 'NOT RUNNING after start' }
if ($Log -and (Test-Path $Log)) { Get-Content $Log | Select-Object -Skip $lines | ForEach-Object { "log: $_" } }
if (-not $p) { exit 1 }
if ($NoSmoke) { exit 0 }

# Smoke check: the restarted widget answers dockctl through Block's real paths, and is still the
# same process afterwards (widgets build with panic = "abort"; the launcher would restart a
# crashed one under a new pid). Read-only commands only: every widget's `do now` reports what it
# already has (no network, no timetable, no playback).
$dockctl = Join-Path $PSScriptRoot '..\dockctl.exe'
$target = switch ($Name) { 'buswidget' { 'bus' } default { $Name -replace '^taskbar-', '' } }
function Ask-Widget([string[]]$cmd) {
    $out = & $dockctl @cmd 2>&1 | Out-String
    try { $out | ConvertFrom-Json } catch { [pscustomobject]@{ ok = $false; error = "not JSON: $($out.Trim())" } }
}
function Smoke-Failed([string]$why) { Write-Output "SMOKE FAILED: $why"; exit 1 }
# Block may still be setting up its main window: give `state` a moment
for ($i = 0; ; $i++) {
    $state = Ask-Widget @($target, 'state')
    if ($state.ok) { break }
    if ($i -ge 10) { Smoke-Failed "state: $($state.error)$($state.absent)" }
    Start-Sleep -Milliseconds 200
}
$help = Ask-Widget @($target, 'do', 'help')
if (-not $help.ok) { Smoke-Failed "do help: $($help.error)" }
$names = @($help.commands | ForEach-Object { $_.name })
$checked = "state, do help ($($names.Count) commands)"
if ($names -contains 'now') {
    # an error reply is fine (media with nothing playing says so); no answer or a gone widget isn't
    $now = Ask-Widget @('--timeout', '10', $target, 'do', 'now')
    if (-not $now.ok -and ($now.absent -or "$($now.error)" -match '^(no answer within|not JSON)')) {
        Smoke-Failed "do now: $($now.error)$($now.absent)"
    }
    $checked += ', do now'
}
$after = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
if (-not $after -or $after.ProcessName -ne $Name) { Smoke-Failed "pid $($p.Id) is gone after the checks (crashed? see the log)" }
Write-Output "smoke ok: $checked"
# not dockctl's last exit code (an error reply to `do now` is fine)
exit 0
