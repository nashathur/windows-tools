# Samples each running widget's process during shown and idle phases; settings are restored in finally.
# Switch: -Minutes (total duration, split evenly). Example: run.ps1 soak -Minutes 10.
param([double]$Minutes = 10)
. "$PSScriptRoot\common.ps1"
if ($Minutes -le 0) { throw '-Minutes must be greater than zero' }
if (-not ('LiveSoakNative' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class LiveSoakNative {
    [DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr OpenProcess(uint access, bool inherit, int processId);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool CloseHandle(IntPtr handle);
    [DllImport("user32.dll", SetLastError=true)] public static extern uint GetGuiResources(IntPtr process, uint flags);
}
'@
}
$rows = New-Object System.Collections.ArrayList
$saved = @{}
$idled = @{}
$phaseSeconds = $Minutes * 30.0
$logicalProcessors = [Environment]::ProcessorCount

function Get-SoakSample([string]$widget, [string]$phase) {
    $ping = Ctl $widget ping
    if (-not $ping -or -not $ping.pid) { [void](Fail "soak/$widget/$phase" 'process unavailable'); return $null }
    $pidValue = [int]$ping.pid
    $proc = $null; $nativeHandle = [IntPtr]::Zero
    try {
        $proc = [Diagnostics.Process]::GetProcessById($pidValue)
        $privateBytes = [double]$proc.PrivateMemorySize64
        $workingSet = [double]$proc.WorkingSet64
        $cpuSeconds = $proc.TotalProcessorTime.TotalSeconds
        $handles = [double]$proc.HandleCount
        $nativeHandle = [LiveSoakNative]::OpenProcess(0x1000, $false, $pidValue) # PROCESS_QUERY_LIMITED_INFORMATION
        if ($nativeHandle -eq [IntPtr]::Zero) { throw "OpenProcess failed for pid $pidValue ($([Runtime.InteropServices.Marshal]::GetLastWin32Error()))" }
        $gdi = [double][LiveSoakNative]::GetGuiResources($nativeHandle, 0)
        $user = [double][LiveSoakNative]::GetGuiResources($nativeHandle, 1)
        if ($gdi -eq 0 -and $user -eq 0) { [void](Fail "soak/$widget/$phase" 'GUI counts unavailable'); return $null }
        [pscustomobject]@{ Time = Get-Date; Widget = $widget; Phase = $phase; PID = $pidValue; PrivateBytes = $privateBytes; WorkingSet = $workingSet; CPUSeconds = $cpuSeconds; Handles = $handles; GDI = $gdi; USER = $user }
    } catch { [void](Fail "soak/$widget/$phase" "sample failed: $($_.Exception.Message)"); $null }
    finally {
        if ($nativeHandle -ne [IntPtr]::Zero) { [void][LiveSoakNative]::CloseHandle($nativeHandle) }
        if ($proc) { $proc.Dispose() }
    }
}

function Get-SlopePerMinute($samples, [string]$property) {
    if ($samples.Count -lt 2) { return 0.0 }
    $origin = $samples[0].Time
    $xs = @($samples | ForEach-Object { ($_.Time - $origin).TotalMinutes })
    $ys = @($samples | ForEach-Object { [double]$_.$property })
    $xMean = ($xs | Measure-Object -Average).Average; $yMean = ($ys | Measure-Object -Average).Average
    $numerator = 0.0; $denominator = 0.0
    for ($i = 0; $i -lt $xs.Count; $i++) { $numerator += ($xs[$i] - $xMean) * ($ys[$i] - $yMean); $denominator += ($xs[$i] - $xMean) * ($xs[$i] - $xMean) }
    if ($denominator -eq 0) { return 0.0 }
    $numerator / $denominator
}

function Invoke-SoakPhase([string]$phase, [string[]]$phaseWidgets = $Widgets) {
    $start = Get-Date; $end = $start.AddSeconds($phaseSeconds); $next = $start
    $phaseSamples = @{}
    foreach ($w in $phaseWidgets) { $phaseSamples[$w] = New-Object System.Collections.ArrayList }
    while ((Get-Date) -le $end) {
        foreach ($w in $phaseWidgets) {
            $sample = Get-SoakSample $w $phase
            if ($sample) {
                $previous = $phaseSamples[$w] | Select-Object -Last 1
                if ($previous -and $sample.PID -ne $previous.PID) { Fail "soak/$w/$phase" "restarted during soak (pid $($previous.PID) -> $($sample.PID))" }
                [void]$phaseSamples[$w].Add($sample)
                [void]$rows.Add($sample)
            }
        }
        $next = $next.AddSeconds(15)
        $waitMs = [int][Math]::Max(0, [Math]::Min(15000, ($next - (Get-Date)).TotalMilliseconds))
        if ((Get-Date) -lt $end -and $waitMs -gt 0) { Start-Sleep -Milliseconds $waitMs }
    }
    foreach ($w in $phaseWidgets) {
        $samples = @($phaseSamples[$w])
        if ($samples.Count -lt 2) { Note "soak/$w/$phase" 'insufficient samples for trends'; continue }
        $first = $samples[0]; $last = $samples[$samples.Count - 1]
        $cpuUsed = 0.0; $cpuWall = 0.0
        for ($i = 1; $i -lt $samples.Count; $i++) {
            if ($samples[$i].PID -eq $samples[$i - 1].PID) {
                $cpuUsed += [Math]::Max(0, $samples[$i].CPUSeconds - $samples[$i - 1].CPUSeconds)
                $cpuWall += ($samples[$i].Time - $samples[$i - 1].Time).TotalSeconds
            }
        }
        $cpuPercent = if ($cpuWall -gt 0) { 100.0 * $cpuUsed / $cpuWall / $logicalProcessors } else { 0.0 }
        Note "soak/$w/$phase/cpu" ('{0:N1} % of total machine capacity' -f $cpuPercent)
        $priv = (Get-SlopePerMinute $samples 'PrivateBytes') / 1MB
        $gdi = Get-SlopePerMinute $samples 'GDI'
        $user = Get-SlopePerMinute $samples 'USER'
        $handles = Get-SlopePerMinute $samples 'Handles'
        $detail = 'private {0:N2} MB/min; GDI {1:N2}/min; USER {2:N2}/min; handles {3:N2}/min' -f $priv, $gdi, $user, $handles
        $sampledMinutes = ($last.Time - $first.Time).TotalMinutes
        $longEnough = $sampledMinutes -ge 4
        $leak = $longEnough -and ($priv -gt 1 -or $gdi -gt 2 -or $user -gt 2)
        if (-not $longEnough) { Note "soak/$w/$phase/growth" ("too short to judge growth (needs -Minutes 8 or more); sampled {0:N1} min; {1}" -f $sampledMinutes,$detail) }
        elseif ($leak) { Fail "soak/$w/$phase/growth" $detail } else { Pass "soak/$w/$phase/growth" $detail }
    }
}

function Set-RealWidgetSetting([string]$widget, [string]$value) {
    $taskName = "TaskbarDockSoakRestore_${PID}_$widget"
    $tempRoot = Join-Path $Live.Out "restore_$widget"
    $scriptFile = Join-Path $tempRoot 'restore.ps1'
    $outputFile = Join-Path $tempRoot 'restore.json'
    New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null
    $quotedCtl = $Ctl.Replace("'", "''")
    $quotedOutput = $outputFile.Replace("'", "''")
    $body = @"
`$setText = & '$quotedCtl' '$widget' set show_on_taskbar '$value' 2>&1 | Out-String
`$setCode = `$LASTEXITCODE
`$getText = & '$quotedCtl' '$widget' get show_on_taskbar 2>&1 | Out-String
`$getCode = `$LASTEXITCODE
`$payload = @{ setCode = `$setCode; setText = `$setText; getCode = `$getCode; getText = `$getText }
[IO.File]::WriteAllText('$quotedOutput', (ConvertTo-Json -InputObject `$payload -Compress), (New-Object Text.UTF8Encoding `$false))
"@
    [IO.File]::WriteAllText($scriptFile, $body, (New-Object Text.UTF8Encoding $false))
    try {
        $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$scriptFile`""
        Register-ScheduledTask -TaskName $taskName -Action $action -Force | Out-Null
        Start-ScheduledTask -TaskName $taskName
        $watch = [Diagnostics.Stopwatch]::StartNew()
        while ($watch.Elapsed.TotalSeconds -lt 15 -and -not (Test-Path -LiteralPath $outputFile)) { Start-Sleep -Milliseconds 100 }
        if (-not (Test-Path -LiteralPath $outputFile)) { return $null }
        return [IO.File]::ReadAllText($outputFile) | ConvertFrom-Json -ErrorAction Stop
    } finally {
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $tempRoot -Force -Recurse -ErrorAction SilentlyContinue
    }
}

function Restore-Widget([string]$widget, $value) {
    $expected = [string]$value
    $reply = Ctl $widget set show_on_taskbar ($value | ConvertTo-Json -Compress)
    $setOk = $reply -and $reply.ok -eq $true
    $current = Ctl $widget get show_on_taskbar
    $verified = $current -and $current.ok -eq $true -and [string]$current.value -eq $expected
    if (-not $verified -and -not (Ctl $widget ping)) {
        $real = Set-RealWidgetSetting $widget ($value | ConvertTo-Json -Compress)
        if ($real -and $real.setCode -eq 0 -and $real.getCode -eq 0) {
            try { $realValue = ($real.getText | ConvertFrom-Json -ErrorAction Stop).value; $verified = [string]$realValue -eq $expected } catch { $verified = $false }
            $setOk = $true
        }
    }
    if ($verified) { return $true }
    [void](Fail "soak/$widget/restore" 'restore failed')
    return $false
}

try {
    Invoke-SoakPhase 'shown'
    foreach ($w in $Widgets) {
        try {
            $saved[$w] = Snapshot $w show_on_taskbar
            [void](Ctl $w set show_on_taskbar false)
            [void](Ctl $w panel close)
            if ((Wait-Until { (Ctl $w ping).idle -eq $true } 5000)) {
                $idled[$w] = $true
            } else {
                $idled[$w] = $false
                [void](Fail "soak/$w/idle" 'did not go idle')
                [void](Restore-Widget $w $saved[$w])
            }
        } catch { Fail "soak/$w/idle" $_.Exception.Message; $idled[$w] = $false }
    }
    Invoke-SoakPhase 'idle' @($Widgets | Where-Object { $idled[$_] })
    foreach ($w in $Widgets) {
        if ($idled[$w]) {
            $shownSamples = @($rows | Where-Object { $_.Widget -eq $w -and $_.Phase -eq 'shown' })
            $idleSamples = @($rows | Where-Object { $_.Widget -eq $w -and $_.Phase -eq 'idle' })
            $shownCpu = 0.0; $idleCpu = 0.0
            foreach ($group in @(@{ S = $shownSamples; K = 'shown' }, @{ S = $idleSamples; K = 'idle' })) {
                $s = @($group.S); if ($s.Count -lt 2) { continue }
                $used = 0.0; $wall = 0.0
                for ($i = 1; $i -lt $s.Count; $i++) { if ($s[$i].PID -eq $s[$i - 1].PID) { $used += [Math]::Max(0, $s[$i].CPUSeconds - $s[$i - 1].CPUSeconds); $wall += ($s[$i].Time - $s[$i - 1].Time).TotalSeconds } }
                $pct = if ($wall -gt 0) { 100.0 * $used / $wall / $logicalProcessors } else { 0.0 }
                if ($group.K -eq 'shown') { $shownCpu = $pct } else { $idleCpu = $pct }
            }
            Note "soak/$w/idle-saves" ('CPU {0:N1} % shown -> {1:N1} % idle' -f $shownCpu, $idleCpu)
        } else { Note "soak/$w/idle" 'not idled' }
    }
} finally {
    foreach ($w in $saved.Keys) { [void](Restore-Widget $w $saved[$w]) }
}
$csvPath = Join-Path $Live.Out 'soak.csv'
$rows | Select-Object @{N='Timestamp';E={$_.Time.ToString('o')}}, Widget, Phase, PID, PrivateBytes, WorkingSet, CPUSeconds, Handles, GDI, USER | Export-Csv -LiteralPath $csvPath -NoTypeInformation -Encoding UTF8
"soak samples: $csvPath"
