# Test: can Firefox be skipped +15 s via pause -> read position -> seek -> play?
param([string]$App = '308046B0AF4A39CB', [int]$Skip = 15)

Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and
    $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
function Await($op, [Type]$type) {
    $t = $asTask.MakeGenericMethod($type).Invoke($null, @($op)); $t.Wait(-1) | Out-Null; $t.Result
}
$null = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]
$M = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager]
$mgr = Await ($M::RequestAsync()) $M
$s = $mgr.GetSessions() | Where-Object { $_.SourceAppUserModelId -eq $App } | Select-Object -First 1
if (-not $s) { "Session $App not found - stopping."; return }

function State { $tl = $s.GetTimelineProperties()
    "status=$($s.GetPlaybackInfo().PlaybackStatus) pos=$([math]::Round($tl.Position.TotalSeconds,2))s end=$([math]::Round($tl.EndTime.TotalSeconds,1))s" }
function WaitFor($status, $ms) { $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) {
        $tl = $s.GetTimelineProperties()
        if ("$($s.GetPlaybackInfo().PlaybackStatus)" -eq $status -and $tl.EndTime.TotalSeconds -gt 0) { return $sw.ElapsedMilliseconds }
        Start-Sleep -Milliseconds 20 }
    -1 }

"Start:        $(State)"
if ("$($s.GetPlaybackInfo().PlaybackStatus)" -ne 'Playing') {
    "play ->       ok=$(Await ($s.TryPlayAsync()) ([bool]))"; Start-Sleep -Seconds 3
    "Playing:      $(State)"
}

$gap = [Diagnostics.Stopwatch]::StartNew()
"pause ->      ok=$(Await ($s.TryPauseAsync()) ([bool]))"
$ms = WaitFor 'Paused' 3000
"Paused after ${ms} ms: $(State)"
$pos = $s.GetTimelineProperties().Position
$target = $pos + [TimeSpan]::FromSeconds($Skip)
"seek to $([math]::Round($target.TotalSeconds,2))s -> ok=$(Await ($s.TryChangePlaybackPositionAsync($target.Ticks)) ([bool]))"
Start-Sleep -Milliseconds 400
"After seek:   $(State)"
"play ->       ok=$(Await ($s.TryPlayAsync()) ([bool]))"
"Gap (pause -> play sent): $($gap.ElapsedMilliseconds) ms"
Start-Sleep -Seconds 2
"2 s later:    $(State)"
