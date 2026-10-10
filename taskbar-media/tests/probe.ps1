# Read-only probe of Windows' media sessions (GSMTC): what each app reports.
param([int]$Seconds = 30)

Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and
    $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
function Await($op, [Type]$type) {
    $t = $asTask.MakeGenericMethod($type).Invoke($null, @($op))
    $t.Wait(-1) | Out-Null
    $t.Result
}

$null = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]
$mgr = Await ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager]::RequestAsync()) `
    ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager])

$propsType = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionMediaProperties]
$seen = @{}   # app -> info, plus timeline samples

$end = (Get-Date).AddSeconds($Seconds)
while ((Get-Date) -lt $end) {
    foreach ($s in $mgr.GetSessions()) {
        $app = $s.SourceAppUserModelId
        if (-not $seen.ContainsKey($app)) {
            $p = Await ($s.TryGetMediaPropertiesAsync()) $propsType
            $c = $s.GetPlaybackInfo().Controls
            $seen[$app] = [ordered]@{
                Title = $p.Title; Artist = $p.Artist; Album = $p.AlbumTitle
                Thumbnail = [bool]$p.Thumbnail
                Play = $c.IsPlayEnabled; Pause = $c.IsPauseEnabled
                Next = $c.IsNextEnabled; Prev = $c.IsPreviousEnabled
                Seek = $c.IsPlaybackPositionEnabled
                Samples = New-Object System.Collections.ArrayList
            }
        }
        $tl = $s.GetTimelineProperties()
        $st = $s.GetPlaybackInfo().PlaybackStatus
        [void]$seen[$app].Samples.Add([pscustomobject]@{
            At = (Get-Date).ToString('HH:mm:ss'); Status = "$st"
            Pos = [math]::Round($tl.Position.TotalSeconds, 1)
            End = [math]::Round($tl.EndTime.TotalSeconds, 1)
            Updated = $tl.LastUpdatedTime.ToLocalTime().ToString('HH:mm:ss')
        })
    }
    Start-Sleep -Milliseconds 1000
}

if ($seen.Count -eq 0) { "No media sessions found."; return }
foreach ($app in $seen.Keys) {
    $i = $seen[$app]
    "=== $app"
    "Title: $($i.Title) | Artist: $($i.Artist) | Album: $($i.Album) | Cover: $($i.Thumbnail)"
    "Buttons: play=$($i.Play) pause=$($i.Pause) next=$($i.Next) prev=$($i.Prev) seek=$($i.Seek)"
    $u = @($i.Samples | Select-Object -ExpandProperty Updated -Unique)
    "Timeline: $($i.Samples.Count) samples, position updated $($u.Count) times: $($u -join ', ')"
    $i.Samples | Select-Object -First 1 | ForEach-Object { "  first: $($_.At) $($_.Status) pos=$($_.Pos)s end=$($_.End)s" }
    $i.Samples | Select-Object -Last 1  | ForEach-Object { "  last:  $($_.At) $($_.Status) pos=$($_.Pos)s end=$($_.End)s" }
    ""
}
