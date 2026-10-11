# Run Codex (ChatGPT plan, via the Codex desktop app's bundled CLI) non-interactively.
# Claude orchestrates; Codex builds, investigates, reviews and suggests. Usage: C:\Users\thurs\.claude\skills\codex\SKILL.md
param(
    [string]$PromptFile,                      # prompt text, fed through stdin (no PS 5.1 quote stripping)
    [string]$Dir = (Get-Location).Path,       # Codex's working root
    [string]$Out,                             # final answer; default <PromptFile>.answer.md
    [ValidateSet('read-only', 'workspace-write')][string]$Sandbox = 'read-only',
    [string]$Review,                          # 'uncommitted', 'base:<branch>' or 'commit:<sha>'
    [string]$Model,
    [ValidateSet('low', 'medium', 'high', 'xhigh', 'max', 'ultra')][string]$Effort = 'high',  # CLI default is 'none'
    [switch]$Worktree,                        # write in a fresh git worktree off HEAD (implies workspace-write)
    [switch]$Keep,                            # persist the session so it can be resumed
    [string]$Resume,                          # session id to continue, in its recorded dir and sandbox
    [string]$Accept,                          # worktree path: apply its changes to the origin checkout, remove it
    [string]$Discard                          # worktree path: remove it unapplied
)
$ErrorActionPreference = 'Stop'

# Codex syncs CLAUDE.md into its AGENTS.md, so it may read "call Codex"; stop it from calling itself.
if ($env:CODEX_CONSULT) { throw 'Already inside a Codex consultation: answer directly.' }
$env:CODEX_CONSULT = '1'
$runs = Join-Path $PSScriptRoot 'runs'
New-Item -ItemType Directory -Force $runs | Out-Null   # gitignored, so absent on a fresh clone

# Absolute against PowerShell's location: Codex runs in $Dir and .NET in the process dir, so relative paths would drift.
function Abs($p) { $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($p) }
# $ErrorActionPreference doesn't stop on a failed native command in PS 5.1; check every git call.
function Invoke-Git { git @args; if ($LASTEXITCODE -ne 0) { throw "git $args failed (exit $LASTEXITCODE)" } }

# -Worktree records each worktree (path, origin checkout, start commit); -Accept/-Discard act only on recorded ones.
function Get-WorktreeRecord($path) {
    $wt = (Resolve-Path -LiteralPath $path).Path
    $rec = Join-Path $runs "wt-$(Split-Path (Split-Path $wt -Parent) -Leaf).txt"
    if (-not (Test-Path $rec)) { throw "$wt was not created by ask.ps1 -Worktree (no record $rec)" }
    $r = Get-Content -Encoding UTF8 $rec
    if ($r[0] -ne $wt) { throw "Record $rec is for $($r[0]), not $wt" }
    [pscustomobject]@{ Wt = $wt; Origin = $r[1]; Base = $r[2]; File = $rec }
}
function Remove-Worktree($r) {
    Invoke-Git -C $r.Origin worktree remove --force $r.Wt
    $parent = Split-Path $r.Wt -Parent   # ~/.codex/worktrees/<id>: drop it too once empty
    if (-not (Get-ChildItem -LiteralPath $parent -Force)) { Remove-Item -LiteralPath $parent }
    Remove-Item $r.File
}

if ($Accept -and $Discard) { throw 'Give -Accept or -Discard, not both.' }
if ($Accept) {
    $r = Get-WorktreeRecord $Accept
    # Diff from the start commit, so work Codex committed comes across too.
    $patch = Join-Path $runs "accept-$(Get-Date -Format yyyyMMdd-HHmmss).patch"
    Invoke-Git -C $r.Wt add -A
    Invoke-Git -C $r.Wt diff --cached --binary $r.Base --output=$patch
    if ((Get-Item $patch).Length -eq 0) { "no changes in $($r.Wt)" }
    else {
        git -C $r.Origin apply --whitespace=nowarn $patch
        if ($LASTEXITCODE -ne 0) { "apply failed; worktree kept at $($r.Wt), patch at $patch"; exit 1 }
        "applied to $($r.Origin) (unstaged):"
        Invoke-Git -C $r.Origin diff --stat
    }
    Remove-Worktree $r
    exit 0
}
if ($Discard) {
    $r = Get-WorktreeRecord $Discard
    Remove-Worktree $r
    "removed $($r.Wt)"
    exit 0
}

$exe = Get-ChildItem "$env:LOCALAPPDATA\OpenAI\Codex\bin\*\codex.exe" |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $exe) { throw 'codex.exe not found under %LOCALAPPDATA%\OpenAI\Codex\bin (is the Codex app installed?)' }

if ([bool]$PromptFile -eq [bool]$Review) { throw 'Give exactly one of -PromptFile or -Review (review presets take no prompt).' }
if ($Resume -and -not $PromptFile) { throw '-Resume needs -PromptFile (the follow-up message).' }
# `exec resume` runs in the launch dir and takes no -C/-s, so -Keep records both and -Resume restores them.
if ($Resume) {
    $sessionFile = Join-Path $runs "session-$Resume.txt"
    if (-not (Test-Path $sessionFile)) { throw "No record for session $Resume (was it started with -Keep?)" }
    $Dir, $Sandbox = Get-Content -Encoding UTF8 $sessionFile
}
if ($Review) { $Sandbox = 'read-only' }
$Dir = (Resolve-Path -LiteralPath $Dir).Path
if ($PromptFile) { $PromptFile = (Resolve-Path -LiteralPath $PromptFile).Path }
# Review files go to runs\, outside the reviewed repo, so they never show up as untracked changes.
$base = if ($PromptFile) { $PromptFile } else { Join-Path $runs "review-$(Split-Path $Dir -Leaf)" }
$Out = if ($Out) { Abs $Out } else { "$base.answer.md" }
$log = "$base.log"

$role = @'
You are working with Claude, which orchestrates this work and makes the final calls.
For a question: answer with findings, options and your recommendation, each with its reasoning and the file:line or evidence behind it.
For a task: do it within the spec, then report what you changed, what you ran and its result, and any choice the spec left open, with your reasoning.
Then add a "Suggestions" section: better approaches, risks, bugs or improvements you noticed beyond the spec, each with its reasoning. Leave them unimplemented; Claude decides which to take.
Run Python only through the conda env the prompt names (`conda run -n <env>`); with none named, leave it unrun and say what you would have run.
Mark anything you did not verify. If something is ambiguous, state your assumption and answer anyway: nobody can reply mid-run.

'@

$args_ = @('exec')
if ($Resume) {
    $args_ += @('resume', $Resume, '-c', "sandbox_mode=$Sandbox")
} elseif ($Review) {
    $args_ += @('review', '-c', 'sandbox_mode=read-only')
    switch -Regex ($Review) {
        '^uncommitted$' { $args_ += '--uncommitted' }
        '^base:(.+)$'   { $args_ += @('--base', $Matches[1]) }
        '^commit:(.+)$' { $args_ += @('--commit', $Matches[1]) }
        default         { throw "Bad -Review '$Review'" }
    }
} else {
    if ($Worktree) {
        $Sandbox = 'workspace-write'
        $args_ += '--worktree'
        $startCommit = Invoke-Git -C $Dir rev-parse HEAD   # the worktree branches from here
    }
    $args_ += @('-s', $Sandbox, '-C', "`"$Dir`"", '--skip-git-repo-check', '--color', 'never')
}
if (-not ($Keep -or $Resume -or $Worktree)) { $args_ += '--ephemeral' }   # the CLI rejects --worktree with --ephemeral
$args_ += @('-o', "`"$Out`"", '-c', "model_reasoning_effort=$Effort")
if ($Model) { $args_ += @('-m', $Model) }

# Stdin carries the prompt (with the role preamble on a fresh session); empty for a review, so nothing waits on it.
$stdin = "$base.stdin.txt"
$text = ''
if ($PromptFile) {
    $text = [IO.File]::ReadAllText($PromptFile)
    if (-not $Resume) { $text = $role + $text }
    $args_ += '-'
}
[IO.File]::WriteAllText($stdin, $text, (New-Object Text.UTF8Encoding $false))

if (Test-Path -LiteralPath $Out) { Remove-Item -LiteralPath $Out }
$sw = [Diagnostics.Stopwatch]::StartNew()
$p = Start-Process -FilePath $exe.FullName -ArgumentList $args_ -WorkingDirectory $Dir -NoNewWindow -Wait -PassThru `
    -RedirectStandardInput $stdin -RedirectStandardOutput "$log.out" -RedirectStandardError $log
Remove-Item -LiteralPath $stdin
"codex exit $($p.ExitCode) in $([int]$sw.Elapsed.TotalSeconds)s; answer: $Out; log: $log"
$head = Get-Content $log -TotalCount 15
function Get-HeadField($name) {   # a failed run may log neither field
    $m = $head | Select-String "^${name}: (.+)$" | Select-Object -First 1
    if ($m) { $m.Matches[0].Groups[1].Value }
}
$wd = Get-HeadField 'workdir'
if ($Keep -or $Resume) {
    $id = Get-HeadField 'session id'
    if ($Keep -and $id) { Set-Content -Encoding UTF8 (Join-Path $runs "session-$id.txt") @($wd, $Sandbox) }
    "session: $id  (continue with -Resume $id)"
}
if ($Worktree -and $wd) {
    Set-Content -Encoding UTF8 (Join-Path $runs "wt-$(Split-Path (Split-Path $wd -Parent) -Leaf).txt") @($wd, $Dir, $startCommit)
    "worktree: $wd  (review with git -C; then -Accept or -Discard it)"
}
if ($p.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $Out)) {
    '--- log tail ---'
    Get-Content $log -Tail 25
    exit 1
}
