# Edit settings… from every widget's right-click menu opens the app on that widget's page: with the
# app closed, and with it already open on another page (one app process only).
. "$PSScriptRoot\common.ps1"
Stop-App
foreach ($w in $Widgets) {
    [void](Ctl $w menu click $Row.EditSettings)
    $ok = Wait-Until { (Ctl settings state).page -eq $w } 10000
    Check $ok "edit-settings/closed/$w" "page=$((Ctl settings state).page)"
    Stop-App
}
[void](Ctl $Widgets[0] menu click $Row.EditSettings); [void](Wait-Until { App-Running } 10000)
foreach ($w in @($Widgets | Select-Object -Skip 1) + $Widgets[0]) {
    [void](Ctl settings page dock)
    [void](Ctl $w menu click $Row.EditSettings)
    $ok = Wait-Until { (Ctl settings state).page -eq $w } 5000
    Check $ok "edit-settings/open/$w" "page=$((Ctl settings state).page)"
}
$n = @(Get-Process taskbar-dock-settings -ErrorAction SilentlyContinue).Count
Check ($n -eq 1) 'edit-settings/one-instance' "processes=$n"
Stop-App
