# Runs live checks and writes one report: _tmp\live\<time>\report.md (PASS / FAIL / NOTE lines,
# screenshots, and optional baseline comparisons). Switches: -Tag, -Widgets, -Minutes (soak),
# -Baseline, -Compare. Examples: run.ps1 idle -Tag manual; run.ps1 soak -Minutes 10;
# run.ps1 -Baseline; run.ps1 -Compare. Read-mostly: checks restore settings they change.
#   run.ps1                         all checks except scale
#   run.ps1 idle,highlight          chosen checks
#   run.ps1 scale -Tag 125          after the user changes the display scale
#   run.ps1 -Widgets weather,bus    only these widgets (default: every running widget); lists take
#                                   commas (a space-separated second name is read as a check name)
# Checks: edit_settings, idle, time_format, highlight, colour_rows, pages, scale. Never run two
# at once.
# A new widget needs nothing here: every check covers each running widget through Block's shared
# parts. Its own content checks go in extra\<widget>.ps1, called with -Check idle-awake | scale
# (see extra\weather.ps1).
[CmdletBinding()]
param([string[]]$Checks = @('edit_settings', 'idle', 'time_format', 'highlight', 'colour_rows', 'pages'), [string]$Tag = '', [string[]]$Widgets = @(), [double]$Minutes = 10, [switch]$Baseline, [switch]$Compare)
$global:Live = $null
if ($Widgets) { $global:Live = @{ Out = "C:\Tools\_tmp\live\$(Get-Date -Format 'yyyyMMdd-HHmmss')"; Results = New-Object System.Collections.ArrayList; Widgets = $Widgets } }
. "$PSScriptRoot\common.ps1"
. "$PSScriptRoot\compare_images.ps1"
$Live.Comparisons = New-Object System.Collections.ArrayList
$t0 = Get-Date
"widgets: " + (($Widgets | ForEach-Object { $p = Ctl $_ ping; if ($p) { "$_ pid $($p.pid)$(if ($p.idle) { ' idle' })" } else { "$_ NOT RUNNING" } }) -join ', ')
foreach ($c in $Checks) {
    "== $c"
    try {
        if ($c -eq 'scale') { & "$PSScriptRoot\scale.ps1" -Tag $Tag }
        elseif ($c -eq 'soak') { & "$PSScriptRoot\soak.ps1" -Minutes $Minutes }
        else { & "$PSScriptRoot\$c.ps1" }
    } catch { Fail "$c/crashed" ($_.Exception.Message) }
}
$res = $Live.Results
$fails = @($res | Where-Object R -eq 'FAIL').Count
$lines = @("# Live checks $(Get-Date -Format 'yyyy-MM-dd HH:mm')", '',
    "$(@($res | Where-Object R -eq 'PASS').Count) passed, $fails failed, $([int]((Get-Date) - $t0).TotalSeconds) s. Checks: $($Checks -join ', ')", '')
$lines += $res | ForEach-Object { "- **$($_.R)** $($_.Name)$(if ($_.Detail) { ": $($_.Detail)" })" }
$lines += '', '## Screenshots', ''
$lines += Get-ChildItem $Live.Out -Filter *.png | Sort-Object Name | ForEach-Object { "- $($_.Name)" }
if ($Compare) {
    $baselineDir = 'C:\Tools\_tmp\live\baseline'
    $comparison = Compare-LiveImages -CurrentDirectory $Live.Out -BaselineDirectory $baselineDir
    foreach ($item in $comparison) { [void]$Live.Comparisons.Add($item) }
    $lines += '', '## Compared with baseline', ''
    $same = @($Live.Comparisons | Where-Object { $null -ne $_.Percent -and $_.Percent -lt 0.5 })
    $lines += "$($same.Count) unchanged (under 0.5 %)."
    $lines += $Live.Comparisons | Where-Object { $same -notcontains $_ } | Sort-Object @{ Expression = { if ($null -eq $_.Percent) { -1 } else { $_.Percent } }; Descending = $true }, Name | ForEach-Object { "- **NOTE** $($_.Name): $($_.Detail)" }
}
if ($Baseline) {
    $baselineDir = 'C:\Tools\_tmp\live\baseline'
    if (Test-Path $baselineDir) { Remove-Item $baselineDir -Recurse -Force }
    New-Item -ItemType Directory -Force $baselineDir | Out-Null
    Get-ChildItem $Live.Out -Filter *.png | Copy-Item -Destination $baselineDir -Force
    $checksText = $Checks -join ', '
    [IO.File]::WriteAllText((Join-Path $baselineDir 'baseline.txt'), "Time: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')`r`nChecks: $checksText`r`n", (New-Object Text.UTF8Encoding $false))
    "baseline: $baselineDir"
}
$report = Join-Path $Live.Out 'report.md'
[IO.File]::WriteAllText($report, ($lines -join "`r`n"), (New-Object Text.UTF8Encoding $false))
"report: $report ($fails failed)"
if ($fails) { exit 1 }
