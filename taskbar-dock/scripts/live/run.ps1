# Runs live checks and writes one report: _tmp\live\<time>\report.md (PASS / FAIL / NOTE lines,
# then the screenshots). Read-mostly: each check puts back every setting it changes.
#   run.ps1                         all checks except scale
#   run.ps1 idle,highlight          chosen checks
#   run.ps1 scale -Tag 125          after the user changes the display scale
#   run.ps1 -Widgets weather        only these widgets (default: every running widget)
# Checks: edit_settings, idle, time_format, highlight, colour_rows, pages, scale. Never run two
# at once.
# A new widget needs nothing here: every check covers each running widget through Block's shared
# parts. Its own content checks go in extra\<widget>.ps1, called with -Check idle-awake | scale
# (see extra\weather.ps1).
param([string[]]$Checks = @('edit_settings', 'idle', 'time_format', 'highlight', 'colour_rows', 'pages'), [string]$Tag = '', [string[]]$Widgets = @())
$global:Live = $null
if ($Widgets) { $global:Live = @{ Out = "C:\Users\thurs\tools\_tmp\live\$(Get-Date -Format 'yyyyMMdd-HHmmss')"; Results = New-Object System.Collections.ArrayList; Widgets = $Widgets } }
. "$PSScriptRoot\common.ps1"
$t0 = Get-Date
"widgets: " + (($Widgets | ForEach-Object { $p = Ctl $_ ping; if ($p) { "$_ pid $($p.pid)$(if ($p.idle) { ' idle' })" } else { "$_ NOT RUNNING" } }) -join ', ')
foreach ($c in $Checks) {
    "== $c"
    try {
        if ($c -eq 'scale') { & "$PSScriptRoot\scale.ps1" -Tag $Tag } else { & "$PSScriptRoot\$c.ps1" }
    } catch { Fail "$c/crashed" ($_.Exception.Message) }
}
$res = $Live.Results
$fails = @($res | Where-Object R -eq 'FAIL').Count
$lines = @("# Live checks $(Get-Date -Format 'yyyy-MM-dd HH:mm')", '',
    "$(@($res | Where-Object R -eq 'PASS').Count) passed, $fails failed, $([int]((Get-Date) - $t0).TotalSeconds) s. Checks: $($Checks -join ', ')", '')
$lines += $res | ForEach-Object { "- **$($_.R)** $($_.Name)$(if ($_.Detail) { ": $($_.Detail)" })" }
$lines += '', '## Screenshots', ''
$lines += Get-ChildItem $Live.Out -Filter *.png | Sort-Object Name | ForEach-Object { "- $($_.Name)" }
$report = Join-Path $Live.Out 'report.md'
[IO.File]::WriteAllText($report, ($lines -join "`r`n"), (New-Object Text.UTF8Encoding $false))
"report: $report ($fails failed)"
if ($fails) { exit 1 }
