# At the current display scale: every shown strip is 48 px x scale tall and reports that scale;
# screenshots of the taskbar, every panel, bus's menu and two app pages. -Tag names the files.
param([string]$Tag = '')
. "$PSScriptRoot\common.ps1"
$tb = [LiveW]::FindWindow('Shell_TrayWnd', [NullString]::Value); $r = New-Object LiveW+R; [void][LiveW]::GetWindowRect($tb, [ref]$r)
$scale = ($r.B - $r.T) / 48.0
if (-not $Tag) { $Tag = "{0}" -f [int]($scale * 100) }
Note "scale/$Tag" "taskbar height $($r.B - $r.T) px -> about $([int]($scale * 100))%"
[void](Shot-Taskbar "scale${Tag}_taskbar")
foreach ($w in $Widgets) {
    $st = Ctl $w state
    if (-not $st) { Fail "scale/$Tag/$w" 'not answering'; continue }
    if ($st.strip.shown -and $st.strip.rect) {
        Check ([math]::Abs($st.strip.rect.h - ($r.B - $r.T)) -le 1) "scale/$Tag/$w-strip-height" "h=$($st.strip.rect.h) look.scale=$($st.look.scale)"
        Check ([math]::Abs($st.look.scale - $scale) -lt 0.02) "scale/$Tag/$w-scale"
    } else { Note "scale/$Tag/$w" 'strip not shown' }
    Check ($null -ne (Shot-Panel $w "scale${Tag}_panel_$w")) "scale/$Tag/$w-panel"
}
[void](Shot-Menu $Widgets[0] "scale${Tag}_menu_$($Widgets[0])")
foreach ($w in $Widgets) { Extra $w 'scale' }
$started = -not (App-Running)
if (Start-App) {
    Start-Sleep 1; [void](Shot-App "scale${Tag}_app_widgets")
    [void](Ctl settings page weather); Start-Sleep 1; [void](Shot-App "scale${Tag}_app_weather")
    if ($started) { Stop-App }
} else { Fail "scale/$Tag/app" 'did not start' }
