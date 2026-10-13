[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidatePattern('^taskbar-[a-z0-9]+(?:-[a-z0-9]+)*$')][string]$Name,
    [ValidatePattern('^[a-z][a-z0-9_-]*$')][string]$Section,
    [Parameter(Mandatory = $true)][ValidateNotNullOrEmpty()][string]$Title,
    [string]$Description = 'A new taskbar widget',
    [switch]$Register,
    [switch]$NoRegister
)

$ErrorActionPreference = 'Stop'
$expectedSection = $Name -replace '^taskbar-', ''
if (-not $PSBoundParameters.ContainsKey('Section')) { $Section = $expectedSection }
if ($Section -cne $expectedSection) { throw "Section must equal Name without the taskbar- prefix: $expectedSection" }
if ($Section -in @('dock','startup','widgets','settings')) { throw "Section is reserved: $Section" }
foreach ($field in @(@('Title',$Title),@('Description',$Description))) {
    if ($field[1] -match '["\\\r\n\x00-\x1F\x7F]') { throw "$($field[0]) must use plain text (no quotes, backslashes, newlines or control characters)" }
}
$dock = Split-Path -Parent $PSScriptRoot
$root = Split-Path -Parent $dock
$template = Join-Path $dock 'template'
$destination = Join-Path $root $Name
$extraDir = Join-Path $PSScriptRoot 'live\extra'
$extra = Join-Path $extraDir "$Section.ps1"

if (-not (Test-Path -LiteralPath $template -PathType Container)) { throw "Template not found: $template" }
# A folder holding only plan notes written ahead (PLAN.md, a mockup) is kept; the template's
# files must not be there yet.
$templateFiles = @(Get-ChildItem -LiteralPath $template -File -Recurse | ForEach-Object { $_.FullName.Substring($template.Length + 1) })
if (Test-Path -LiteralPath $destination) {
    $clash = @($templateFiles | Where-Object { Test-Path -LiteralPath (Join-Path $destination $_) })
    if ($clash.Count -gt 0 -or (Test-Path -LiteralPath (Join-Path $destination 'Cargo.toml'))) {
        throw "Widget folder already has a crate: $destination ($($clash -join ', '))"
    }
}
if (Test-Path -LiteralPath $extra) { throw "Live extra already exists: $extra" }

# Section names are unique across widget Specs. Check both the current source forms:
# named WIDGET_SECTION constants and literal Spec::new(class, section, ...).
$sections = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
foreach ($crate in Get-ChildItem -LiteralPath $root -Directory) {
    if ($crate.Name -eq 'taskbar-dock' -or $crate.Name -eq 'template') { continue }
    $sources = @(Get-ChildItem -LiteralPath $crate.FullName -Filter '*.rs' -File -Recurse -ErrorAction SilentlyContinue)
    foreach ($source in $sources) {
        $text = [IO.File]::ReadAllText($source.FullName)
        foreach ($match in [regex]::Matches($text, '(?:WIDGET_SECTION|SECTION)\s*:\s*&str\s*=\s*"([^"]+)"|Spec::new\(\s*"[^"]+"\s*,\s*"([^"]+)"')) {
            foreach ($capture in 1, 2) {
                if ($match.Groups[$capture].Success) { [void]$sections.Add($match.Groups[$capture].Value) }
            }
        }
    }
}
if ($sections.Contains($Section)) { throw "Settings section is already used by a sibling widget: $Section" }

# cargo writes progress to stderr; under 'Stop' PS 5.1 turns that into a terminating error when
# the caller captures output (2>&1), so native calls run with 'Continue' and are judged by exit code.
function Invoke-Cargo([string[]]$CargoArgs) {
    $ErrorActionPreference = 'Continue'
    & cargo @CargoArgs 2>&1 | ForEach-Object { "$_" }
    if ($LASTEXITCODE -ne 0) { throw "cargo $($CargoArgs -join ' ') failed with exit code $LASTEXITCODE" }
}

$utf8NoBom = [Text.UTF8Encoding]::new($false)
$tokens = @{
    '__NAME__' = $Name
    '__SECTION__' = $Section
    '__TITLE__' = $Title
    '__DESC__' = $Description
}
# Derive a readable PascalCase class from the suffix (taskbar-clock -> ClockStrip).
$className = (($Name -replace '^taskbar-', '') -split '-' | ForEach-Object { (Get-Culture).TextInfo.ToTitleCase($_) }) -join ''
$tokens['__CLASS__'] = "Taskbar${className}Strip"

# What already exists is left alone by a rollback (plan notes, a src\ with notes in it, ...).
$templateDirs = @($templateFiles | ForEach-Object { Split-Path -Parent $_ } | Where-Object { $_ } | Sort-Object -Unique -Descending)
$preexisting = @(@($destination) + @($templateDirs + @('Cargo.lock', 'target') | ForEach-Object { Join-Path $destination $_ }) | Where-Object { Test-Path -LiteralPath $_ })

try {
    [IO.Directory]::CreateDirectory($destination) | Out-Null
    foreach ($relative in $templateFiles) {
        $target = Join-Path $destination $relative
        [IO.Directory]::CreateDirectory((Split-Path -Parent $target)) | Out-Null
        Copy-Item -LiteralPath (Join-Path $template $relative) -Destination $target
    }
    foreach ($file in $templateFiles | ForEach-Object { Get-Item -LiteralPath (Join-Path $destination $_) }) {
        $content = [IO.File]::ReadAllText($file.FullName)
        foreach ($token in $tokens.Keys) { $content = $content.Replace($token, $tokens[$token]) }
        [IO.File]::WriteAllText($file.FullName, $content, $utf8NoBom)
        $newName = $file.Name
        foreach ($token in $tokens.Keys) { $newName = $newName.Replace($token, $tokens[$token]) }
        if ($newName -ne $file.Name) { Rename-Item -LiteralPath $file.FullName -NewName $newName }
    }

    $liveTemplate = Join-Path $destination 'scripts-live-extra.ps1'
    $liveText = [IO.File]::ReadAllText($liveTemplate)
    foreach ($token in $tokens.Keys) { $liveText = $liveText.Replace($token, $tokens[$token]) }
    [IO.Directory]::CreateDirectory($extraDir) | Out-Null
    [IO.File]::WriteAllText($extra, $liveText, $utf8NoBom)
    Remove-Item -LiteralPath $liveTemplate   # it lives in scripts\live\extra now

    Push-Location $destination
    try {
        Invoke-Cargo @('build', '--profile', 'fast')
        Invoke-Cargo @('test', '--profile', 'fast')
    } finally {
        Pop-Location
    }
} catch {
    # Leave the folder as it was so the starter can simply be run again: remove only the files this
    # run copied, the build output it made, and the folders it created once they are empty.
    foreach ($relative in $templateFiles) {
        $path = Join-Path $destination ($relative -replace '__NAME__', $Name)
        if (Test-Path -LiteralPath $path -PathType Leaf) { Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue }
    }
    foreach ($made in @('Cargo.lock', 'target') | ForEach-Object { Join-Path $destination $_ }) {
        if (($made -notin $preexisting) -and (Test-Path -LiteralPath $made)) { Remove-Item -LiteralPath $made -Recurse -Force -ErrorAction SilentlyContinue }
    }
    foreach ($dir in @($templateDirs | ForEach-Object { Join-Path $destination $_ }) + @($destination)) {
        if (($dir -notin $preexisting) -and -not (Get-ChildItem -LiteralPath $dir -Force -ErrorAction SilentlyContinue)) { Remove-Item -LiteralPath $dir -Force -ErrorAction SilentlyContinue }
    }
    Remove-Item -LiteralPath $extra -Force -ErrorAction SilentlyContinue
    throw
}

Write-Output "Created $destination"
Write-Output "Live check: $extra"
$exe = Join-Path $destination "target\release\$Name.exe"
Push-Location $destination
try {
    Invoke-Cargo @('build', '--release')
} finally { Pop-Location }

if (-not $NoRegister) {
    $taskName = "TaskbarDockRegister_$Name"
    $taskScript = Join-Path $env:TEMP "tdregister_$PID.ps1"
    $outputFile = Join-Path $env:TEMP "tdregister_$PID.exit"
    $quotedExe = $exe.Replace("'", "''")
    $quotedDock = $dock.Replace("'", "''")
    $quotedOutput = $outputFile.Replace("'", "''")
    $taskBody = @"
`$ErrorActionPreference = 'Stop'
Push-Location '$quotedDock'
try {
    & (Join-Path '$quotedDock' 'dockctl.exe') widgets register '$quotedExe'
    `$code = `$LASTEXITCODE
    [IO.File]::WriteAllText('$quotedOutput', [string]`$code, (New-Object Text.UTF8Encoding `$false))
    exit `$code
} finally { Pop-Location }
"@
    [IO.File]::WriteAllText($taskScript, $taskBody, (New-Object Text.UTF8Encoding $false))
    try {
        $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$taskScript`""
        Register-ScheduledTask -TaskName $taskName -Action $action -Force | Out-Null
        Start-ScheduledTask -TaskName $taskName
        $watch = [Diagnostics.Stopwatch]::StartNew()
        while ($watch.Elapsed.TotalSeconds -lt 15 -and -not (Test-Path -LiteralPath $outputFile)) { Start-Sleep -Milliseconds 100 }
        if (-not (Test-Path -LiteralPath $outputFile)) { throw 'Widget registration timed out after 15 seconds' }
        while ($watch.Elapsed.TotalSeconds -lt 15) {
            $task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
            if (-not $task -or $task.State -ne 'Running') { break }
            Start-Sleep -Milliseconds 100
        }
        $code = [int][IO.File]::ReadAllText($outputFile).Trim()
        if ($code -ne 0) { throw "dockctl widgets register failed with exit code $code" }
    } finally {
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $taskScript,$outputFile -Force -ErrorAction SilentlyContinue
    }
    & (Join-Path $PSScriptRoot 'sync_session_settings.ps1')
    Write-Output "Next: .\dev.ps1 -Start (from $destination)"
} else {
    Write-Output "Manual steps (in order): run `"$dock\dockctl.exe`" widgets register `"$exe`" in a one-off real-context scheduled task, run $PSScriptRoot\sync_session_settings.ps1, then run .\dev.ps1 -Start from $destination."
}
