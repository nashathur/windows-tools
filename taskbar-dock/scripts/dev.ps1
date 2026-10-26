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
. "$PSScriptRoot\..\..\common\clierr.ps1"
# Continue: PS 5.1 turns cargo's stderr (warnings) into errors, which 'Stop' would throw on
$ErrorActionPreference = 'Continue'
trap { Exit-Cli 'dev' $_ }
$buildProfile = if ($Fast) { 'fast' } else { 'release' }
$built = Join-Path $Crate "target\$buildProfile\$Name.exe"

function Run-Cargo($dir, $cargoArgs, [switch]$Tests) {
    $run = $null
    try { $run = Invoke-CliTool cargo $cargoArgs -Name cargo -Fix 'fix the first compiler or test error and rerun' -WorkingDirectory $dir }
    catch { $run = $_.Exception.Data['clierr'].ToolRun; if (-not $run) { throw } }
    $out = @($run.StdOut -split "`r?`n") + @($run.StdErr -split "`r?`n")
    if ($run.ExitCode -eq 0) {
        # a failure prints only its ERROR line below; the full output goes to the log
        $out | Select-String -Pattern '^warning' -Context 0,6 | ForEach-Object { $_.Line; $_.Context.PostContext } | Out-Host
        $out | Where-Object { $_ -match '^test result' -and $_ -notmatch 'ok. 0 passed' } | Out-Host
    } else {
        $detail = $out -join "`n"
        if ($Tests) {
            # quiet output: "---- tests::name stdout ----" blocks, a "failures:" name list, then
            # "test result: FAILED. 3 passed; 1 failed; ..."
            $result = $out | Where-Object { $_ -match '^test result: FAILED\..*?(\d+) failed' } | Select-Object -First 1
            $count = if ($result -match '(\d+) failed') { $Matches[1] } else { 'some tests' }
            $first = $out | Where-Object { $_ -match '^---- (\S+) stdout ----' } | Select-Object -First 1
            $testName = if ($first -match '^---- (\S+) stdout ----') { $Matches[1] } else { $null }
            if (-not $testName) {
                $at = [Array]::IndexOf($out, 'failures:')
                $entry = if ($at -ge 0) { $out[($at + 1)..($out.Count - 1)] | Where-Object { $_ -match '^\s+\S+\s*$' } | Select-Object -First 1 }
                $testName = if ($entry) { $entry.Trim() } else { 'unknown test' }
            }
            Stop-Cli TESTS_FAILED "$count failed, first: $testName" 'fix the failing test output in the log, then rerun' -Detail $detail -ToolRun $run
        }
        $cargoError = Get-CargoError $detail
        if (-not $cargoError) { $cargoError = Get-RelevantLine $detail }
        Stop-Cli BUILD_FAILED $cargoError 'fix the first compiler error in the log, then rerun' -Detail $detail -ToolRun $run
    }
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
$lintRun = $null
try { $lintRun = Invoke-CliTool cargo @('test','--profile','fast','--quiet','--test','theme_lint','widget_sources') -Name cargo -Fix 'fix the theme lint or compiler error and rerun dev.ps1' -WorkingDirectory (Get-Location).Path }
catch { $lintRun = $_.Exception.Data['clierr'].ToolRun; if (-not $lintRun) { throw } }
$lint = @($lintRun.StdOut -split "`r?`n") + @($lintRun.StdErr -split "`r?`n")
$lintOk = $lintRun.ExitCode -eq 0
Remove-Item Env:THEME_LINT_SRC
Pop-Location
if (-not $lintOk) {
    $hits = @($lint | Where-Object { $_ -match ': (raw colour|font name|literal font size) ' })
    $what = if ($hits) { "$($hits.Count) theme lint hit(s), first: $($hits[0].Trim())" } else { Get-CargoError ($lint -join "`n") }
    if (-not $what) { $what = Get-RelevantLine ($lint -join "`n") }
    Stop-Cli THEME_LINT_FAILED $what 'use theme values for every colour, font and size (all hits in the log)' -Detail ($lint -join "`n")
}
Run-Cargo $Crate @('build', '--profile', $buildProfile, '--quiet')
if (-not $NoTest) {
    Run-Cargo $Crate @('test', '--profile', 'fast', '--quiet') -Tests
    if ($Lib) { Run-Cargo (Join-Path $PSScriptRoot '..') @('test', '--profile', 'fast', '--quiet') -Tests }
}
if ($NoRestart) { Write-Output 'built'; exit 0 }
if (-not $running -and -not $Start) {
    if ((Resolve-Path $built).Path -ne [IO.Path]::GetFullPath($Deploy)) {
        for ($i = 0; ; $i++) {
            try { Copy-Item $built $Deploy -Force -ErrorAction Stop; break }
            catch { if ($i -ge 30) { Stop-Cli LOCKED "COPY FAILED: $($_.Exception.Message)" 'close the process locking the deployed executable and rerun' -Detail $_.ToString() }; Start-Sleep -Milliseconds 100 }
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
        catch { if ($i -ge 30) { Stop-Cli LOCKED "COPY FAILED: $($_.Exception.Message)" 'close the process locking the deployed executable and rerun' -Detail $_.ToString() }; Start-Sleep -Milliseconds 100 }
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
if (-not $p) { Stop-Cli RESTART_FAILED 'widget process exited after restart' 'check the widget log and resolve its startup error' }
if ($NoSmoke) { exit 0 }

# Smoke check: the restarted widget answers dockctl through Block's real paths, and is still the
# same process afterwards (widgets build with panic = "abort"; the launcher would restart a
# crashed one under a new pid). Read-only commands only: every widget's `do now` reports what it
# already has (no network, no timetable, no playback).
$dockctl = Join-Path $PSScriptRoot '..\dockctl.exe'
$target = switch ($Name) { 'buswidget' { 'bus' } default { $Name -replace '^taskbar-', '' } }
function Ask-Widget([string[]]$cmd) {
    try { $run = Invoke-CliTool $dockctl $cmd -Name dockctl -Fix 'check dockctl output and the widget log' }
    catch { $run = $_.Exception.Data['clierr'].ToolRun; if (-not $run) { throw } }
    $out = "$($run.StdOut)`n$($run.StdErr)"
    try { $out | ConvertFrom-Json } catch { [pscustomobject]@{ ok = $false; error = "not JSON: $($out.Trim())" } }
}
function Smoke-Failed([string]$why) { Stop-Cli SMOKE_FAILED $why 'check dockctl and the widget log, then rerun dev.ps1' }
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
