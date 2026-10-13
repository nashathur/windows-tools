# Every colour row on every widget page: expanded through UI Automation (no mouse), scrolled into
# view and captured (swatch grid, Custom colour line). Hidden rows are shown for the shot by their
# own "shown when" switch (the Highlight box mode), then put back. The app window is made tall.
param([string[]]$Only = @())
. "$PSScriptRoot\common.ps1"
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [Windows.Automation.AutomationElement]
function Expander([string]$id) {
    $win = $A::RootElement.FindFirst([Windows.Automation.TreeScope]::Children, (New-Object Windows.Automation.PropertyCondition $A::NameProperty, 'Taskbar Dock'))
    if (-not $win) { return $null }
    $win.FindFirst([Windows.Automation.TreeScope]::Descendants, (New-Object Windows.Automation.PropertyCondition $A::AutomationIdProperty, "color:$id"))
}
$started = -not (App-Running)
Check (Start-App) 'colour-rows/app-starts'
[void](Size-App 900 1030)
foreach ($w in $(if ($Only) { $Only } else { $Widgets })) {
    [void](Ctl settings page $w); Start-Sleep -Milliseconds 900
    $rows = @((Ctl settings state).rows | Where-Object kind -eq 'color')
    if (-not $rows) { Note "colour-rows/$w" 'no colour rows'; continue }
    $mode = Snapshot $w highlight
    try {
        if ($rows | Where-Object { $_.shown -eq $false }) { [void](Ctl settings row "$w.highlight" set custom); Start-Sleep -Milliseconds 600 }
        foreach ($r in $rows) {
            $e = Expander $r.id
            if (-not $e) { Fail "colour-rows/$($r.id)" 'no expander in the window'; continue }
            try { $e.GetCurrentPattern([Windows.Automation.ScrollItemPattern]::Pattern).ScrollIntoView() } catch { }
            $e.GetCurrentPattern([Windows.Automation.ExpandCollapsePattern]::Pattern).Expand(); Start-Sleep -Milliseconds 700
            try { $e.GetCurrentPattern([Windows.Automation.ScrollItemPattern]::Pattern).ScrollIntoView() } catch { }
            Start-Sleep -Milliseconds 300
            $p = Shot-App "colour_$($r.id)"
            Check ($null -ne $p) "colour-rows/$($r.id)" 'expanded'
            $e.GetCurrentPattern([Windows.Automation.ExpandCollapsePattern]::Pattern).Collapse()
        }
    } finally {
        [void](Ctl $w set highlight $mode)
    }
}
if ($started) { Stop-App }
Note 'colour-rows/look' 'check colour_*.png: solid swatches, grid at the left, the row is one card'
