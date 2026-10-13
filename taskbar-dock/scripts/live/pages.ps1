# Visits every app page (Widgets, each widget, Dock, Developer), checks each one answers, and
# screenshots it after its slide-in has finished.
. "$PSScriptRoot\common.ps1"
$started = -not (App-Running)
Check (Start-App) 'pages/app-starts'
foreach ($p in @('widgets') + $Widgets + @('dock', 'developer')) {
    [void](Ctl settings page $p); Start-Sleep -Milliseconds 900
    $s = Ctl settings state
    Check ($s.page -eq $p) "pages/$p" "rows=$(@($s.rows).Count)"
    [void](Shot-App "page_$p")
}
if ($started) { Stop-App }
