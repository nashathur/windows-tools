# Read-only smoke check for the installed settings app and its dockctl contract.
# Starts the app through a one-off scheduled task so it reads the user's real settings.
$ErrorActionPreference = 'Stop'
$app = Join-Path $PSScriptRoot '..\taskbar-dock-settings.exe'
$dockctl = Join-Path $PSScriptRoot '..\dockctl.exe'
$task = "TaskbarDockSettingsSmoke_$PID"
$processName = [IO.Path]::GetFileNameWithoutExtension($app)
$started = $false

function Fail([string]$why) {
    throw $why
}

function Invoke-Dockctl([string[]]$arguments) {
    $output = & $dockctl @arguments 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        Fail "dockctl $($arguments -join ' '): exit $LASTEXITCODE; $($output.Trim())"
    }
    try {
        return $output | ConvertFrom-Json -ErrorAction Stop
    } catch {
        Fail "dockctl $($arguments -join ' ') returned invalid JSON: $($output.Trim())"
    }
}

function Assert-Ok($reply, [string]$label) {
    if (-not $reply -or $reply.ok -ne $true) {
        Fail "$label failed: $($reply.error)"
    }
}

function Json-Value($value) {
    return ConvertTo-Json -InputObject $value -Compress -Depth 100
}

function Get-Rows($state) {
    if ($state.rows) {
        return @($state.rows)
    }
    return @()
}

function Get-AdvancedRows($state) {
    $advanced = $state.cards.advanced
    $rows = @()
    foreach ($item in @($advanced)) {
        if ($item.id) {
            $rows += $item
        } elseif ($item.rows) {
            $rows += @($item.rows)
        }
    }
    return $rows
}

try {
    if (-not (Test-Path -LiteralPath $app)) { Fail "installed settings app is missing: $app" }
    if (-not (Test-Path -LiteralPath $dockctl)) { Fail "installed dockctl is missing: $dockctl" }
    if (Get-Process -Name $processName -ErrorAction SilentlyContinue) {
        Fail 'settings app is already running; close it before the smoke check'
    }

    $action = New-ScheduledTaskAction -Execute $app -WorkingDirectory (Split-Path $app)
    Register-ScheduledTask -TaskName $task -Action $action -Force | Out-Null
    Start-ScheduledTask -TaskName $task
    $started = $true

    $deadline = [Diagnostics.Stopwatch]::StartNew()
    $initial = $null
    while ($deadline.Elapsed.TotalSeconds -lt 5) {
        try {
            $initial = Invoke-Dockctl @('--timeout', '1', 'settings', 'state')
            if ($initial.ok -eq $true) { break }
        } catch { }
        Start-Sleep -Milliseconds 100
    }
    Assert-Ok $initial 'settings state within 5 seconds'
    Write-Output 'PASS settings state responds within 5 seconds'

    $widgets = @()
    foreach ($page in @('widgets', 'dock')) {
        $changed = Invoke-Dockctl @('settings', 'page', $page)
        Assert-Ok $changed "visit $page page"
        $state = Invoke-Dockctl @('settings', 'state')
        Assert-Ok $state "$page page state"
        if ($state.page -ne $page) { Fail "expected page $page, got $($state.page)" }
        if ($page -eq 'widgets') {
            # The registry loads on a worker after the window appears: wait for its rows.
            $listed = [Diagnostics.Stopwatch]::StartNew()
            while (-not @($state.rows).Count -and $listed.Elapsed.TotalSeconds -lt 5) {
                Start-Sleep -Milliseconds 200
                $state = Invoke-Dockctl @('settings', 'state')
            }
            $widgets = @($state.rows)
            if (-not $widgets.Count) { Fail 'the Widgets page lists no registered widget' }
        }
        Write-Output "PASS page $page and state"
    }

    $developerPage = Invoke-Dockctl @('settings', 'page', 'developer')
    Assert-Ok $developerPage 'visit Developer page'
    $developer = Invoke-Dockctl @('settings', 'state')
    Assert-Ok $developer 'Developer state'
    if ($developer.page -ne 'developer') { Fail "expected Developer page, got $($developer.page)" }
    Write-Output 'PASS page developer and state'

    foreach ($widget in $widgets) {
        $name = [string]$widget.name
        $pageReply = Invoke-Dockctl @('settings', 'page', $name)
        Assert-Ok $pageReply "visit $name page"
        $pageState = Invoke-Dockctl @('settings', 'state')
        Assert-Ok $pageState "$name page state"
        $pageRows = @(Get-Rows $pageState)

        $toDeveloper = Invoke-Dockctl @('settings', 'page', 'developer')
        Assert-Ok $toDeveloper 'visit Developer page'
        $devState = Invoke-Dockctl @('settings', 'state')
        Assert-Ok $devState 'Developer state'
        $advancedRows = @(Get-AdvancedRows $devState | Where-Object { $_.id -like "$name.*" -and $_.id -ne "$name.startup" })

        $actualRows = @($pageRows | Where-Object { $_.id -like "$name.*" -and $_.id -ne "$name.startup" }) + $advancedRows
        $actualKeys = @($actualRows | ForEach-Object { ([string]$_.id).Substring($name.Length + 1) } | Sort-Object -Unique)

        $description = Invoke-Dockctl @($name, 'describe')
        Assert-Ok $description "$name describe"
        $settings = @($description.settings)
        $expectedKeys = @($settings | ForEach-Object { [string]$_.key } | Sort-Object -Unique)
        $missing = @($expectedKeys | Where-Object { $_ -notin $actualKeys })
        $extra = @($actualKeys | Where-Object { $_ -notin $expectedKeys })
        if ($missing.Count -or $extra.Count) {
            Fail "$name row key union mismatch; missing [$($missing -join ', ')], extra [$($extra -join ', ')]"
        }
        Write-Output "PASS $name page and Developer rows cover described keys"

        foreach ($setting in $settings) {
            $key = [string]$setting.key
            $id = "$name.$key"
            $row = @($actualRows | Where-Object { $_.id -eq $id } | Select-Object -First 1)[0]
            if (-not $row) { Fail "$name missing row $id" }
            $current = Invoke-Dockctl @($name, 'get', $key)
            Assert-Ok $current "$name get $key"
            if ((Json-Value $row.value) -ne (Json-Value $current.value)) {
                Fail "$name row value differs from dockctl get for $key"
            }
            Write-Output "PASS $name row $key matches get"
        }
        Write-Output "PASS $name row values checked with secrets masked"
    }

    $quit = Invoke-Dockctl @('settings', 'quit')
    Assert-Ok $quit 'settings quit'
    $quitDeadline = [Diagnostics.Stopwatch]::StartNew()
    while ($quitDeadline.Elapsed.TotalSeconds -lt 5 -and (Get-Process -Name $processName -ErrorAction SilentlyContinue)) {
        Start-Sleep -Milliseconds 100
    }
    if (Get-Process -Name $processName -ErrorAction SilentlyContinue) {
        Fail 'settings process remained after quit'
    }
    Write-Output 'PASS settings process exits after quit'
    Write-Output 'SETTINGS SMOKE OK'
} catch {
    Write-Output "SETTINGS SMOKE FAILED: $($_.Exception.Message)"
    if ($started) {
        try { & $dockctl settings quit | Out-Null } catch { }
    }
    exit 1
} finally {
    Unregister-ScheduledTask -TaskName $task -Confirm:$false -ErrorAction SilentlyContinue
}
