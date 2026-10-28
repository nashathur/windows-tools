# How much of each helper plan is used: Codex (ChatGPT) and Antigravity (Google). Claude's own limits
# are read in-app (get_usage), not here. Routing by these numbers: global CLAUDE.md "Delegation".
param([switch]$Json)
. "$PSScriptRoot\..\common\clierr.ps1"
$ErrorActionPreference = 'Stop'

trap { Exit-Cli 'quota' $_ }

function Format-Reset([DateTime]$t) { $t.ToLocalTime().ToString('ddd d MMM HH:mm') }
$rows = New-Object 'System.Collections.Generic.List[object]'

# Codex: the newest rate_limits event across recent session logs, by the event's own timestamp (a long
# session can keep writing after its last quota update). A window whose reset has passed is empty again.
$sessions = Join-Path $env:USERPROFILE '.codex\sessions'
$limits = $null
$asOf = [DateTime]::MinValue
if (Test-Path -LiteralPath $sessions) {
    foreach ($f in Get-ChildItem -LiteralPath $sessions -Recurse -Filter *.jsonl | Sort-Object LastWriteTime -Descending | Select-Object -First 20) {
        $line = Get-Content -LiteralPath $f.FullName | Select-String '"rate_limits":\{' | Select-Object -Last 1
        if (-not $line) { continue }
        $ev = $line.Line | ConvertFrom-Json
        $stamp = if ($ev.timestamp) { [DateTime]::Parse($ev.timestamp, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::AdjustToUniversal).ToLocalTime() } else { $f.LastWriteTime }
        if ($stamp -gt $asOf) { $limits = $ev.payload.rate_limits; $asOf = $stamp }
    }
}
if ($limits) {
    foreach ($w in @(@{ Name = '5h'; L = $limits.primary }, @{ Name = 'week'; L = $limits.secondary })) {
        if (-not $w.L) { continue }
        $reset = [DateTimeOffset]::FromUnixTimeSeconds([long]$w.L.resets_at).UtcDateTime
        $used = if ($reset -lt [DateTime]::UtcNow) { 0 } else { [int][Math]::Round([double]$w.L.used_percent) }
        $rows.Add([pscustomobject]@{ plan = 'codex'; window = $w.Name; used = $used; resets = Format-Reset $reset; asof = $asOf.ToString('d MMM HH:mm') })
    }
} else { Write-CliWarn 'quota' NOT_FOUND 'no Codex rate limits in its session logs yet' 'run any Codex job, then retry' }

# Antigravity: its /usage command (local, costs no quota).
$agy = if ($env:AGY_EXE) { $env:AGY_EXE } else { "$env:LOCALAPPDATA\agy\bin\agy.exe" }
if (Test-Path -LiteralPath $agy) {
    try {   # a signed-out or failing agy must not hide Codex's numbers
        $run = Invoke-CliTool $agy @('-p', '/usage', '--output-format', 'json') -Name agy -TimeoutSec 60
        $usage = ($run.StdOut | ConvertFrom-Json).command.data
        if (-not $usage.groups) { throw 'no usage groups in agy /usage output' }
        foreach ($g in $usage.groups) {
            foreach ($b in $g.buckets) {
                $reset = [DateTime]::Parse($b.reset_time, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::AdjustToUniversal)
                $rows.Add([pscustomobject]@{ plan = "agy $($g.name)"; window = $b.window; used = [int][Math]::Round((1 - [double]$b.remaining_fraction) * 100); resets = Format-Reset $reset; asof = 'now' })
            }
        }
    } catch { Write-CliWarn 'quota' TOOL_FAILED "agy usage unreadable: $($_.Exception.Message -replace '\s+', ' ')" 'sign in by running agy in a terminal' }
} else { Write-CliWarn 'quota' NOT_FOUND "agy not found at $agy" 'install Antigravity, or ignore if unused' }

if (-not $rows.Count) { Stop-Cli NOT_FOUND 'no quota source could be read' 'see the warnings above' }
if ($Json) { ConvertTo-Json -InputObject $rows.ToArray() } else { $rows | Format-Table plan, window, @{ n = 'used %'; e = { $_.used } }, resets, asof -AutoSize | Out-String -Width 200 }
