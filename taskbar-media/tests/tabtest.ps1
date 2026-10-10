# Read-only: what UI Automation exposes about Firefox tabs, and whether the playing tab can be matched.
# Prints full titles only for the matched tab; other tabs show only a known site name or "other".
param([string]$App = '308046B0AF4A39CB')

# Media title from Windows' media session
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
function Await($op, [Type]$type) { $t = $asTask.MakeGenericMethod($type).Invoke($null, @($op)); $t.Wait(-1) | Out-Null; $t.Result }
$null = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]
$M = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager]
$s = (Await ($M::RequestAsync()) $M).GetSessions() | Where-Object { $_.SourceAppUserModelId -eq $App } | Select-Object -First 1
if (-not $s) { "No Firefox media session."; return }
$media = (Await ($s.TryGetMediaPropertiesAsync()) ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionMediaProperties])).Title
"Media title: $media  (status=$($s.GetPlaybackInfo().PlaybackStatus))"

$sites = [ordered]@{ 'YouTube Music' = 'YouTube Music'; 'YouTube' = 'YouTube'; 'Twitch' = 'Twitch'; 'SoundCloud' = 'SoundCloud';
    'Spotify' = 'Spotify'; 'Bandcamp' = 'Bandcamp'; 'Netflix' = 'Netflix'; 'Mixcloud' = 'Mixcloud'; 'Deezer' = 'Deezer' }
$dashes = [string][char]0x2013 + [char]0x2014
function SiteOf($title) { foreach ($k in $sites.Keys) { $e = [regex]::Escape($k); if ($title -match ("[-|" + $dashes + "]\s*" + $e + "\b|on " + $e + "\b")) { return $sites[$k] } }; 'other' }

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]
$root = $A::RootElement
$winCond = New-Object System.Windows.Automation.PropertyCondition($A::ClassNameProperty, 'MozillaWindowClass')
$tabCond = New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, [System.Windows.Automation.ControlType]::TabItem)
$wins = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $winCond)
"Firefox windows: $($wins.Count)"

$sw = [Diagnostics.Stopwatch]::StartNew()
$w = 0
foreach ($win in $wins) {
    $w++
    $tabs = $win.FindAll([System.Windows.Automation.TreeScope]::Descendants, $tabCond)
    "Window ${w}: $($tabs.Count) tabs"
    $i = 0
    foreach ($tab in $tabs) {
        $i++
        $name = $tab.Current.Name
        $sel = try { ($tab.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern)).Current.IsSelected } catch { '?' }
        # Children of the tab (e.g. a mute / "playing" indicator)
        $kids = $tab.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition)
        $kidInfo = @($kids | ForEach-Object { "$($_.Current.ControlType.ProgrammaticName -replace 'ControlType\.','')'$($_.Current.Name)'" }) -join ', '
        $match = $media -and $name -and $name.Contains($media)
        if ($match) {
            "  tab ${i}: MATCH  site=$(SiteOf $name)  selected=$sel"
            "    full title: $name"
            "    children:   $kidInfo"
        } else {
            $sound = if ($kidInfo -match 'mute|unmute|play|sound|audio') { 'has sound control' } else { '' }
            "  tab ${i}: site=$(SiteOf $name) selected=$sel $sound"
        }
    }
}
"UIA scan took $($sw.ElapsedMilliseconds) ms"
