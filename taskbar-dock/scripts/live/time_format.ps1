# The Dock page's Time format, changed in the app, reaches every widget: each panel is captured at
# 12h and 24h to compare (AM/PM). Puts the old value back.
. "$PSScriptRoot\common.ps1"
$was = Snapshot $Widgets[0] time_format -Dock
$started = -not (App-Running)
try {
    Check (Start-App) 'time-format/app-starts'
    [void](Ctl settings page dock)
    foreach ($f in '12h', '24h') {
        [void](Ctl settings row dock.time_format set $f); Start-Sleep -Milliseconds 800
        Check ((Ctl $Widgets[0] get --dock time_format).value -eq $f) "time-format/$f/saved"
        foreach ($w in $Widgets) {
            $p = Shot-Panel $w "time_${f}_$w"
            Check ($null -ne $p) "time-format/$f/$w-panel"
        }
    }
} finally {
    # through a widget, not the app: this must work even if the app died mid-check
    [void](Ctl $Widgets[0] set --dock time_format $was)
    if ($started) { Stop-App }
}
Check ((Ctl $Widgets[0] get --dock time_format).value -eq $was) 'time-format/restored' "time_format=$was"
Note 'time-format/look' 'compare time_12h_*.png (AM/PM) with time_24h_*.png'
