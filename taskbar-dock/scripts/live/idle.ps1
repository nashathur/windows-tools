# Idle (BLOCK.md "Idle"), every widget: Show on taskbar off -> idle with the "paused" tooltip; a
# tray click wakes it with its real tooltip; closing the panel idles it again. A widget's content
# check while awake (e.g. "fetched at once") lives in extra\<widget>.ps1 -Check idle-awake.
# Puts Show on taskbar back.
param([string[]]$Only = @())
. "$PSScriptRoot\common.ps1"
foreach ($w in $(if ($Only) { $Only } else { $Widgets })) {
    $was = Snapshot $w show_on_taskbar
    try {
        [void](Ctl $w set show_on_taskbar false)
        if (-not (Wait-Until { (Ctl $w state).tray.shown } 3000)) { Note "idle/$w" 'no tray icon while switched off: never idle'; continue }
        Check (Wait-Until { (Ctl $w ping).idle -eq $true } 5000) "idle/$w/goes-idle"
        $tray = (Ctl $w state).tray
        $keepsTip = [bool]$tray.live_while_idle   # its data stays live: it keeps its own tooltip
        $tip = ($tray.tip -split "`n")[0]
        Check (($tip -like '*paused*') -ne $keepsTip) "idle/$w/$(if ($keepsTip) { 'live-tip' } else { 'paused-tip' })" "tip='$tip'"
        [void](Ctl $w tray click left); Start-Sleep -Milliseconds 400
        $s = Ctl $w state
        Check ($s.panel.open -and -not (Ctl $w ping).idle) "idle/$w/wakes-on-tray-click"
        Check ($s.tray.tip -notlike '*paused*') "idle/$w/real-tip-while-open" "tip='$(($s.tray.tip -split "`n")[0])'"
        Extra $w 'idle-awake'
        [void](Ctl $w panel close)
        Check (Wait-Until { (Ctl $w ping).idle -eq $true } 3000) "idle/$w/idle-again-on-close"
        Check (((Ctl $w state).tray.tip -like '*paused*') -ne $keepsTip) "idle/$w/idle-tip-again"
    } finally {
        [void](Ctl $w set show_on_taskbar ($was | ConvertTo-Json))
    }
    Check ((Ctl $w get show_on_taskbar).value -eq $was) "idle/$w/restored" "show_on_taskbar=$was"
}
