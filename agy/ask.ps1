# Run Antigravity (agy: Gemini Flash on the user's free Google tier) non-interactively.
# Claude orchestrates; agy summarizes, extracts, searches, does low-risk reviews and small edits.
# Usage: C:\Users\thurs\.claude\skills\antigravity\SKILL.md
param(
    [string]$PromptFile,                      # prompt text (passed as one argument, so under ~30k characters)
    [string]$Dir = (Get-Location).Path,       # agy's workspace: where it reads and, for edit, writes
    [string]$Out,                             # answer; default <PromptFile>.answer.md (.json for extract)
    [ValidateSet('gist', 'extract', 'search', 'review', 'edit')][string]$Role = 'gist',
    [string]$Schema,                          # JSON schema file; extract only (required there)
    [string[]]$InputFile,                     # files pasted into the prompt: saves agy a read turn
    [string]$Model,                           # overrides the role's model
    [int]$TimeoutSec = 600
)
. "$PSScriptRoot\..\common\clierr.ps1"
$ErrorActionPreference = 'Stop'

trap { Exit-Cli 'agy' $_ }

# Role presets (user, 2026-10-28). Free and AI Plus tiers keep only Gemini Flash after 2026-11-02.
# Flash low returned empty schema output in tests, so extract runs medium.
# Headless agy refuses writes and commands; settings.json allows read_file(*), and edit adds
# --mode accept-edits (file edits only, never commands).
$roles = @{
    gist    = @{ Model = 'gemini-3.8-flash-medium'; Write = $false }
    extract = @{ Model = 'gemini-3.8-flash-medium'; Write = $false }
    search  = @{ Model = 'gemini-3.8-flash-medium'; Write = $false }
    review  = @{ Model = 'gemini-3.8-flash-high';   Write = $false }
    edit    = @{ Model = 'gemini-3.8-flash-medium'; Write = $true }
}

# CLAUDE.md is synced into Codex's AGENTS.md and agy reads rules files too; stop a helper from calling itself.
if ($env:AGY_CONSULT) { Stop-Cli BAD_ARG 'Already inside an agy consultation: answer directly.' 'run ask outside an agy consultation' }
if (-not $PromptFile) { Stop-Cli BAD_ARG 'Give -PromptFile.' 'write the prompt to a file and pass -PromptFile' }
if ($Role -eq 'extract' -and -not $Schema) { Stop-Cli BAD_ARG '-Role extract needs -Schema (a JSON schema file).' 'add -Schema <file>' }
if ($Schema -and $Role -ne 'extract') { Stop-Cli BAD_ARG '-Schema is for -Role extract only.' 'drop -Schema or use -Role extract' }

$PromptFile = (Resolve-Path -LiteralPath $PromptFile).Path
$Dir = (Resolve-Path -LiteralPath $Dir).Path
if ($Schema) { $Schema = (Resolve-Path -LiteralPath $Schema).Path }
$text = [IO.File]::ReadAllText($PromptFile)
if (-not $text.Trim()) { Stop-Cli BAD_INPUT "Prompt file is empty: $PromptFile" 'write the prompt into the file' }
if (-not $Model) { $Model = $roles[$Role].Model }
$ext = if ($Role -eq 'extract') { 'json' } else { 'md' }
$Out = if ($Out) { $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out) } else { "$PromptFile.answer.$ext" }
$raw = "$PromptFile.result.json"

$preamble = if ($roles[$Role].Write) {
    "You are working for Claude, which makes the final calls. Make the edits asked for, only inside $Dir. You cannot run commands. Then list each file you changed and what you changed. Mark anything you did not verify. Nobody can reply mid-run: state any assumption and carry on.`n`n"
} else {
    "You are working for Claude, which makes the final calls. Read and answer only: change no files. Be brief; mark unverified claims inline. Nobody can reply mid-run: state any assumption in one line and answer anyway.`n`n"
}
$prompt = $preamble + $text
foreach ($f in $InputFile) {
    $path = (Resolve-Path -LiteralPath $f).Path
    $prompt += "`n`n--- $path ---`n" + [IO.File]::ReadAllText($path)
}
$exe = if ($env:AGY_EXE) { $env:AGY_EXE } else { "$env:LOCALAPPDATA\agy\bin\agy.exe" }   # override: tests
if (-not (Test-Path -LiteralPath $exe)) { Stop-Cli NOT_FOUND "agy not found at $exe" 'install it: curl -fsSL https://antigravity.google/cli/install.cmd' }

# Headless agy honours a saved Agent Mode and saved allow rules, so name the mode on every call and
# refuse settings that would let any role run commands or write outside edit (Codex review, 2026-10-28).
$settings = if ($env:AGY_SETTINGS) { $env:AGY_SETTINGS } else { "$env:USERPROFILE\.gemini\antigravity-cli\settings.json" }
if (Test-Path -LiteralPath $settings) {
    $extra = @((Get-Content -LiteralPath $settings -Raw | ConvertFrom-Json).permissions.allow |
        Where-Object { $_ -and $_ -notmatch '^(read_file|search_web)(\(.*\))?$' })
    if ($extra) {
        Stop-Cli UNSAFE_SETTINGS "agy settings allow more than reading: $($extra -join ', ')" "remove those entries from permissions.allow in $settings"
    }
}
$a = @('--model', $Model, '--output-format', 'json', '--print-timeout', "${TimeoutSec}s", '--mode')
$a += if ($roles[$Role].Write) { 'accept-edits' } else { 'default' }
if ($Schema) { $a += @('--json-schema', $Schema) }
$a += @('-p', $prompt)
# Windows caps a command line at 32,767 characters, and the prompt travels as one (escaped) argument.
$cmdLen = $exe.Length + 3 + (Join-CliArgs $a).Length
if ($cmdLen -gt 32000) {
    Stop-Cli BAD_INPUT "Command line is $cmdLen characters (limit 32000)" 'point agy at the files by path instead of pasting them'
}

$env:AGY_CONSULT = '1'
$sw = [Diagnostics.Stopwatch]::StartNew()
try {
    $run = Invoke-CliTool $exe $a -Name agy -WorkingDirectory $Dir -TimeoutSec ($TimeoutSec + 30) -Fix 'read the agy stderr in the log'
} catch {
    $f = $_.Exception.Data['clierr']
    # agy prints AGY_ERROR: {...} on stderr and exits 3 when a turn fails on the model API.
    if ($f -and $f.ToolRun -and $f.ToolRun.StdErr -match '(?im)^AGY_ERROR:.*(RESOURCE_EXHAUSTED|quota|"code":\s*"?429)') {
        Stop-Cli QUOTA "agy quota exhausted for $Model" 'run C:\Tools\quota\quota.ps1, then wait for the reset or use another helper' -ToolRun $f.ToolRun
    }
    throw
} finally { Remove-Item Env:AGY_CONSULT -ErrorAction SilentlyContinue }

$json = @($run.StdOut -split "`r?`n" | Where-Object { $_.TrimStart().StartsWith('{') })[-1]
if (-not $json) { Stop-Cli TOOL_FAILED 'agy printed no JSON result' 'read the agy output in the log' -ToolRun $run }
[IO.File]::WriteAllText($raw, $json, (New-Object Text.UTF8Encoding $false))
$r = $json | ConvertFrom-Json
if ($r.status -ne 'SUCCESS') { Stop-Cli TOOL_FAILED "agy status $($r.status)" "read $raw" -ToolRun $run }

$denied = @($r.denied_actions | ForEach-Object { $_.action }) -join ', '
# Soft denials are documented as a stderr notice; the JSON field is not guaranteed.
if (-not $denied -and $run.StdErr -match 'required the "(\w+)" permission') { $denied = $Matches[1] }
# -InputObject: piping would unwrap a one-item array and turn an empty one into nothing.
$answer = if ($Schema) { if ($null -ne $r.structured_output) { ConvertTo-Json -InputObject $r.structured_output -Depth 20 } } else { $r.response }
if (-not "$answer".Trim()) {
    if ($denied) { Stop-Cli DENIED "agy was refused: $denied" 'commands are never allowed; file writes need -Role edit' -ToolRun $run }
    Stop-Cli TOOL_FAILED 'agy returned an empty answer' "read $raw" -ToolRun $run
}
if ($denied) { Write-CliWarn 'agy' DENIED "agy was refused: $denied (answer may be partial)" 'commands are never allowed; file writes need -Role edit' }
[IO.File]::WriteAllText($Out, "$answer", (New-Object Text.UTF8Encoding $false))
"agy done in $([int]$sw.Elapsed.TotalSeconds)s ($Role`: $Model); tokens $($r.usage.input_tokens) in / $($r.usage.output_tokens) out; answer: $Out"
