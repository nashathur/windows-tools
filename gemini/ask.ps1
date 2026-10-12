# Ask Gemini (free API key) one read-only question, within a per-model daily request budget.
# Claude orchestrates; Gemini gives second opinions and gists of large files. Rules: ~/.claude/CLAUDE.md "Gemini".
param(
    [Parameter(Mandatory)][string]$PromptFile,   # prompt text; `@path` in it attaches that file or folder (no extra request)
    [string]$Dir = (Get-Location).Path,          # Gemini's working root; @paths resolve against it
    [string]$Out,                                # answer; default <PromptFile>.answer.md
    [ValidateSet('flash', 'lite')][string]$Tier = 'flash',
    [string]$Model,                              # overrides the tier's model (its budget then defaults to 15)
    [int]$Budget                                 # overrides the tier's daily budget
)
$ErrorActionPreference = 'Stop'

# Free-tier quotas are per model (AI Studio rate-limit page, 2026-10-12). Pro models get 0 and each rejected
# try costs a request, so a model is always named. Budgets leave a few requests for the user's own use.
$tiers = @{
    flash = @{ Model = 'gemini-3.8-flash';      Budget = 15 }    # 20 RPD, 5 RPM: second opinions
    lite  = @{ Model = 'gemini-3.5-flash-lite'; Budget = 400 }   # 500 RPD, 15 RPM: gists of large material
}
if (-not $Model) { $Model = $tiers[$Tier].Model; if (-not $Budget) { $Budget = $tiers[$Tier].Budget } }
if (-not $Budget) { $Budget = 15 }

# Codex reads a synced copy of CLAUDE.md, so it may try to call Gemini too; only Claude spends this quota.
if ($env:CODEX_CONSULT) { throw 'Called from inside a Codex run: answer directly.' }
$runs = Join-Path $PSScriptRoot 'runs'
New-Item -ItemType Directory -Force $runs | Out-Null   # gitignored, so absent on a fresh clone

# Free-tier quotas reset at midnight Pacific; one ledger file per Pacific day.
$day = [TimeZoneInfo]::ConvertTimeBySystemTimeZoneId([DateTime]::UtcNow, 'Pacific Standard Time').ToString('yyyy-MM-dd')
$ledger = Join-Path $runs "usage-$day.txt"     # one line per model per call: "<requests> <model> <time> <prompt file>"
$exhausted = Join-Path $runs "exhausted-$day-$Model.txt"
if (Test-Path $exhausted) { throw "$Model quota hit today ($day Pacific): $(Get-Content -Raw $exhausted)" }
# Stats name the model actually served, sometimes with a suffix (e.g. -customtools): match by prefix.
function Get-Used { $n = 0; if (Test-Path $ledger) { Get-Content $ledger | ForEach-Object {
    $f = $_ -split ' '; if ($f[1] -like "$Model*") { $n += [int]$f[0] } } }; $n }
$used = Get-Used
if ($used -ge $Budget) { throw "$Model budget spent: $used of $Budget requests today ($day Pacific)." }

$Dir = (Resolve-Path -LiteralPath $Dir).Path
$PromptFile = (Resolve-Path -LiteralPath $PromptFile).Path
# Absolute against PowerShell's location: .NET writes relative paths against the process dir.
$Out = if ($Out) { $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out) } else { "$PromptFile.answer.md" }
if (Test-Path -LiteralPath $Out) { Remove-Item -LiteralPath $Out }   # never leave an old answer to be read as new
$log = "$PromptFile.log"

$preamble = @'
You are giving Claude, which orchestrates this work and makes the final calls, a second opinion.
Answer from the prompt and its attached files. You run on a small daily request budget: avoid tool calls
unless the answer truly needs one more file. Give findings, options and your recommendation, each with
its reasoning and the file:line or evidence behind it. Mark anything you did not verify. If something is
ambiguous, state your assumption and answer anyway: nobody can reply mid-run.

'@
$text = $preamble + [IO.File]::ReadAllText($PromptFile)

# PS 5.1 pipes to native commands in ASCII by default; send and read UTF-8 so equations survive.
$OutputEncoding = [Console]::OutputEncoding = New-Object Text.UTF8Encoding $false
# Read-only twice over: plan mode, plus readonly.toml denying writes, shell and leaving plan mode
# (headless v0.63 would switch to YOLO on exit_plan_mode). --skip-trust trusts $Dir for this run only.
# PS 5.1 turns any native stderr line (e.g. Gemini's 256-colour warning) into an error under 'Stop'.
$policy = Join-Path $PSScriptRoot 'readonly.toml'
Push-Location $Dir
$ErrorActionPreference = 'Continue'
try {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $raw = $text | & gemini.cmd -m $Model -o json --approval-mode plan --policy $policy --skip-trust 2> $log
    $code = $LASTEXITCODE
} finally { Pop-Location; $ErrorActionPreference = 'Stop' }
$raw = $raw -join "`n"
Set-Content -Encoding UTF8 "$PromptFile.json" $raw   # full output with per-model stats and errors

$json = $null
try { $json = $raw.Substring($raw.IndexOf('{')) | ConvertFrom-Json } catch { }
$requests = 0
$now = Get-Date -Format HH:mm:ss
if ($json.stats) {
    $json.stats.models.PSObject.Properties | ForEach-Object {
        $requests += $_.Value.api.totalRequests
        Add-Content -Encoding UTF8 $ledger "$($_.Value.api.totalRequests) $($_.Name) $now $PromptFile"
    }
} elseif ($code -ne 0) {   # fatal errors carry no stats: assume one request (may undercount)
    $requests = 1
    Add-Content -Encoding UTF8 $ledger "1 $Model $now $PromptFile"
}
$used = Get-Used

# A JSON error can arrive beside partial response text with exit 0, so it counts as failure too.
$failed = $code -ne 0 -or $json.error -or -not $json.response
if ($failed) {
    # Only error text, never a successful answer or its token counts, decides that the quota is spent.
    $errText = "$($json.error | ConvertTo-Json -Compress -Depth 5)`n$raw`n$(Get-Content -Raw $log -ErrorAction SilentlyContinue)"
    if ($errText -match 'RESOURCE_EXHAUSTED|\b429\b|[Qq]uota exceeded|exceeded your current quota') {
        Set-Content -Encoding UTF8 $exhausted "quota error at $(Get-Date -Format HH:mm) local; see $log"
    }
} else {
    [IO.File]::WriteAllText($Out, $json.response, (New-Object Text.UTF8Encoding $false))
}
$answer = if ($failed) { 'none' } else { $Out }
"gemini exit $code in $([int]$sw.Elapsed.TotalSeconds)s ($Model): $requests request(s), $used of $Budget used today ($day Pacific); answer: $answer; log: $log"
if ($failed) {
    '--- error ---'
    if ($json.error) { $json.error | ConvertTo-Json -Depth 5 }
    (("$raw`n" + (Get-Content -Raw $log -ErrorAction SilentlyContinue)) -split "`n" | Select-Object -Last 25) -join "`n"
    exit 1
}
