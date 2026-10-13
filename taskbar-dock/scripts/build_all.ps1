# Rebuild, install and check the shared library tools, settings app and every registered widget.
# Switches: -Fast (widgets' fast profile), -Only <lib|tools|app|widget-folder ...>, -Live.
# Example: .\build_all.ps1 -Fast -Only tools app taskbar-weather -Live
[CmdletBinding()]
param(
    [switch]$Fast,
    [string[]]$Only = @(),
    [switch]$Live,
    # the names after the first in "-Only a b c": PowerShell binds only "a" to -Only (and without
    # this would silently drop the rest)
    [Parameter(ValueFromRemainingArguments = $true)][string[]]$MoreOnly = @()
)
$Only = @(@($Only) + @($MoreOnly) | ForEach-Object { $_ -split ',' } | Where-Object { $_ })

$ErrorActionPreference = 'Continue'
$dock = Split-Path $PSScriptRoot
$repo = Split-Path $dock
$startedAt = [Diagnostics.Stopwatch]::StartNew()
$allStages = -not $Only
$runLib = $allStages -or ($Only -contains 'lib')
$runTools = $allStages -or ($Only -contains 'tools')
$runApp = $allStages -or ($Only -contains 'app')
$runWidgets = $allStages -or @($Only | Where-Object { $_ -notin @('lib','tools','app') }).Count -gt 0
$selectedWidgets = @($Only | Where-Object { $_ -notin @('lib','tools','app') })

function Failed([string]$step, [string]$detail) {
    if ($detail) { Write-Output "FAILED: $step - $detail" } else { Write-Output "FAILED: $step" }
    exit 1
}

function Invoke-Cargo([string]$folder, [string[]]$arguments, [string]$step) {
    Push-Location $folder
    try {
        $output = @(& cargo @arguments 2>&1 | ForEach-Object { "$_" })
        $code = $LASTEXITCODE
    } finally { Pop-Location }
    $output | Where-Object { $_ -match '^(error|warning)(:|\[)' } | Select-Object -First 12 | ForEach-Object { Write-Output $_ }
    if ($code -ne 0) { Failed $step (($output | Select-Object -Last 8) -join ' | ') }
    Write-Output "$step ok"
}

function Copy-InstalledExe([string]$built, [string]$installed, [string]$label) {
    try {
        Copy-Item -LiteralPath $built -Destination $installed -Force -ErrorAction Stop
        Write-Output "$label installed"
        return
    } catch {
        $procName = [IO.Path]::GetFileNameWithoutExtension($installed)
        $running = Get-Process -Name $procName -ErrorAction SilentlyContinue
        if (-not $running) { Failed "$label copy" $_.Exception.Message }
        $old = Join-Path (Split-Path $installed) (([IO.Path]::GetFileNameWithoutExtension($installed)) + '.old.exe')
        Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue
        try {
            Move-Item -LiteralPath $installed -Destination $old -Force -ErrorAction Stop
            Copy-Item -LiteralPath $built -Destination $installed -Force -ErrorAction Stop
            Write-Output "$label replaced while running"
        } catch { Failed "$label replace while running" $_.Exception.Message }
    }
}

function Invoke-ChildScript([string]$path, [string[]]$arguments, [string]$step) {
    $output = @(& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $path @arguments 2>&1 | ForEach-Object { "$_" })
    $code = $LASTEXITCODE
    if ($code -ne 0) {
        $output | ForEach-Object { Write-Output $_ }
        Failed $step "exit $code"
    }
    return ,$output
}

$validOnly = @('lib','tools','app')
$widgetDirs = @(Get-ChildItem -LiteralPath $repo -Directory | Where-Object {
    $wrapper = Join-Path $_.FullName 'dev.ps1'
    (Test-Path -LiteralPath $wrapper) -and ((Get-Content -LiteralPath $wrapper -Raw) -match '(?i)taskbar-dock[\\/]scripts[\\/]dev\.ps1')
})
$widgetNames = @($widgetDirs | ForEach-Object Name)
foreach ($name in $Only) {
    if (($name -notin $validOnly) -and ($name -notin $widgetNames)) { Failed 'arguments' "unknown -Only name '$name'; valid: $((@($validOnly) + $widgetNames) -join ', ')" }
}
if (-not $Only) { $selectedWidgets = $widgetNames }
elseif ($runWidgets) {
    $unknownWidget = @($selectedWidgets | Where-Object { $_ -notin $widgetNames })
    if ($unknownWidget) { Failed 'arguments' "unknown widget folder '$($unknownWidget[0])'" }
}

if ($runLib) { Invoke-Cargo $dock @('test','--profile','fast','--quiet') 'library tests' }

if ($runTools) {
    Invoke-Cargo $dock @('build','--release','--bin','dockctl','--bin','taskbar-dock-start') 'dockctl and launcher build'
    Copy-InstalledExe (Join-Path $dock 'target\release\dockctl.exe') (Join-Path $dock 'dockctl.exe') 'dockctl'
    Copy-InstalledExe (Join-Path $dock 'target\release\taskbar-dock-start.exe') (Join-Path $dock 'taskbar-dock-start.exe') 'launcher'
}

if ($runApp) {
    $appName = 'taskbar-dock-settings'
    if (Get-Process -Name $appName -ErrorAction SilentlyContinue) {
        $ctl = Join-Path $dock 'dockctl.exe'
        if (-not (Test-Path -LiteralPath $ctl)) { Failed 'settings app close' 'dockctl.exe is missing; build tools first' }
        $replyText = (& $ctl settings quit 2>&1 | Out-String)
        if ($LASTEXITCODE -ne 0) { Failed 'settings app close' $replyText.Trim() }
        $wait = [Diagnostics.Stopwatch]::StartNew()
        while ($wait.Elapsed.TotalSeconds -lt 5 -and (Get-Process -Name $appName -ErrorAction SilentlyContinue)) { Start-Sleep -Milliseconds 100 }
        if (Get-Process -Name $appName -ErrorAction SilentlyContinue) { Failed 'settings app close' 'process did not exit within 5 seconds' }
        Write-Output 'settings app closed'
    }
    $appCrate = Join-Path $dock 'settings-app'
    Invoke-Cargo $appCrate @('build','--release') 'settings app build'
    Copy-InstalledExe (Join-Path $appCrate 'target\release\taskbar-dock-settings.exe') (Join-Path $dock 'taskbar-dock-settings.exe') 'settings app'
}

if ($runWidgets) {
    foreach ($name in $selectedWidgets) {
        $dir = Join-Path $repo $name
        $devArgs = if ($Fast) { @('-Fast') } else { @() }
        $lines = Invoke-ChildScript (Join-Path $dir 'dev.ps1') $devArgs "widget $name"
        $lines | ForEach-Object { Write-Output "${name}: $_" }
        if ($lines | Where-Object { $_ -match 'was not running, left stopped' }) { continue }   # dev.ps1 builds a stopped widget without starting it
        if (-not ($lines | Where-Object { $_ -match '^restarted .*pid' })) { Failed "widget $name" 'missing restarted/pid result' }
        if (-not ($lines | Where-Object { $_ -match '^smoke ok' })) { Failed "widget $name" 'missing smoke ok result' }
    }
}

if ($runApp) {
    $smokeLines = Invoke-ChildScript (Join-Path $PSScriptRoot 'settings_smoke.ps1') @() 'settings smoke'
    Write-Output "settings smoke: $($smokeLines[-1])"   # the full list prints only on failure
}

if ($Live) {
    $liveLines = Invoke-ChildScript (Join-Path $PSScriptRoot 'live\run.ps1') @() 'live checks'
    if (-not $liveLines.Count) { Failed 'live checks' 'no report line returned' }
    Write-Output $liveLines[-1]
}

$startedAt.Stop()
Write-Output ("ALL OK ({0:N1}s)" -f $startedAt.Elapsed.TotalSeconds)
