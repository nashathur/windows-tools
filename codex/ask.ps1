# Run Codex (ChatGPT plan, via the Codex desktop app's bundled CLI) non-interactively.
# Claude orchestrates; Codex builds, investigates, reviews and suggests. Usage: C:\Users\thurs\.claude\skills\codex\SKILL.md
param(
    [string]$PromptFile,                      # prompt text, fed through stdin (no PS 5.1 quote stripping)
    [string]$Dir = (Get-Location).Path,       # Codex's working root
    [string]$Out,                             # final answer; default <PromptFile>.answer.md
    [ValidateSet('read-only', 'workspace-write')][string]$Sandbox = 'read-only',
    [string]$Review,                          # 'uncommitted', 'base:<branch>', 'commit:<sha>', or 'custom' (-PromptFile names target and focus)
    [switch]$Search,                          # live web search; read-only ask calls only
    [ValidateSet('build', 'review', 'quick', 'deep')][string]$Role,  # model/effort/sandbox preset; inferred from the mode when omitted
    [string]$Model,
    [ValidateSet('low', 'medium', 'high', 'xhigh', 'max', 'ultra')][string]$Effort = 'high',  # CLI default is 'none'
    [switch]$Worktree,                        # write in a fresh git worktree off HEAD (implies workspace-write)
    [switch]$Keep,                            # persist the session so it can be resumed
    [string]$Resume,                          # session id to continue, in its recorded dir and sandbox
    [string]$Accept,                          # worktree path: apply its changes to the origin checkout, remove it
    [string]$Discard,                         # worktree path: remove it unapplied
    [switch]$Workshop,                        # run in the WSL workshop's clone of windows-tools (pushed commits only)
    [switch]$Status                           # print other Codex jobs and exit
)
$hasSandbox = $PSBoundParameters.ContainsKey('Sandbox')
$hasModel = $PSBoundParameters.ContainsKey('Model')
$hasEffort = $PSBoundParameters.ContainsKey('Effort')
. "$PSScriptRoot\..\common\clierr.ps1"
$ErrorActionPreference = 'Stop'

trap { Exit-Cli 'ask' $_ }

$runs = Join-Path $PSScriptRoot 'runs'
New-Item -ItemType Directory -Force $runs | Out-Null   # gitignored, so absent on a fresh clone

function Show-Jobs {
    $script:jobsFound = $false
    $lines = New-Object 'System.Collections.Generic.List[string]'
    foreach ($file in Get-ChildItem -LiteralPath $runs -Filter 'job-*.txt' -File) {
        if ($script:jobFile -and $file.FullName -eq $script:jobFile) { continue }
        try {
            $record = @{}
            foreach ($line in Get-Content -LiteralPath $file.FullName -Encoding UTF8) {
                $parts = $line -split '=', 2
                if ($parts.Count -ne 2 -or -not $parts[0]) { Stop-Cli BAD_INPUT 'bad field in job record' 'remove or repair the unreadable job record' }
                $record[$parts[0]] = $parts[1]
            }
            foreach ($key in @('pid', 'pstart', 'started', 'role', 'model', 'name', 'answer')) {
                if (-not $record.ContainsKey($key)) { Stop-Cli BAD_INPUT "missing $key in job record" 'remove or repair the unreadable job record' }
            }
            $processId = 0
            if (-not [int]::TryParse($record.pid, [ref]$processId)) { Stop-Cli BAD_INPUT 'bad pid in job record' 'remove or repair the unreadable job record' }
            $started = [DateTime]::Parse($record.started, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::RoundtripKind)
            $pstart = [DateTime]::Parse($record.pstart, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::RoundtripKind)
            $elapsed = [int][Math]::Floor(((Get-Date) - $started).TotalMinutes)
            if ($record.ContainsKey('ended')) {
                $ended = [DateTime]::Parse($record.ended, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::RoundtripKind)
                if (-not $record.ContainsKey('exit')) { Stop-Cli BAD_INPUT 'missing exit in job record' 'remove or repair the unreadable job record' }
                $ago = [int][Math]::Floor(((Get-Date) - $ended).TotalMinutes)
                $line = "finished $($ago)m ago, exit $($record.exit): $($record.name) -> $($record.answer)"
                if ($record.ContainsKey('worktree')) { $line += " worktree: $($record.worktree) (review, then -Accept or -Discard)" }
                $lines.Add($line)
                Remove-Item -LiteralPath $file.FullName
            } else {
                $process = Get-Process -Id $processId -ErrorAction SilentlyContinue
                $alive = $false
                if ($process) {
                    $actualStart = $process.StartTime
                    $alive = $actualStart.ToUniversalTime().Ticks -eq $pstart.ToUniversalTime().Ticks
                }
                if ($alive) { $lines.Add("running $($elapsed)m: $($record.name) ($($record.role), $($record.model))") }
                else {
                    $lines.Add("wrapper stopped after $($elapsed)m (no result recorded): $($record.name); Codex may still be running, check $($record.answer) and its .log")
                    Remove-Item -LiteralPath $file.FullName
                }
            }
        } catch {
            # A record seconds old may be mid-write by another call; only stale ones are reported and dropped.
            if (((Get-Date) - $file.LastWriteTime).TotalSeconds -lt 60) { continue }
            $lines.Add("unreadable job record $($file.Name)")
            Remove-Item -LiteralPath $file.FullName -ErrorAction SilentlyContinue
        }
    }
    if ($lines.Count -gt 0) {
        $script:jobsFound = $true
        '--- codex jobs ---'
        $lines
    }
}

# Codex syncs CLAUDE.md into its AGENTS.md, so it may read "call Codex"; stop it from calling itself.
if ($env:CODEX_CONSULT) { Stop-Cli BAD_ARG 'Already inside a Codex consultation: answer directly.' 'run ask outside a Codex consultation' }
$env:CODEX_CONSULT = '1'
if ($Accept -and $Discard) { Stop-Cli BAD_ARG 'Give -Accept or -Discard, not both.' 'use only -Accept or -Discard' }
Show-Jobs   # after the guard: a refused nested call must not use up the once-only finished notices
if ($Status) {
    if (-not $script:jobsFound) { 'no other codex jobs' }
    exit 0
}

# Absolute against PowerShell's location: Codex runs in $Dir and .NET in the process dir, so relative paths would drift.
function Abs($p) { $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($p) }
# $ErrorActionPreference doesn't stop on a failed native command in PS 5.1; check every git call.
function Invoke-Git {   # returns git's stdout
    $run = Invoke-CliTool git -ArgumentList ([string[]]$args) -Name git -Fix 'check the git error in the log and resolve it'
    if ($run.StdOut) { $run.StdOut.TrimEnd() }
}

# The workshop (WSL distro, user agent) can't reach Windows, so Windows reaches in: files through the UNC share,
# commands through wsl.exe running a script file (no quoting crosses PS 5.1). Jobs use their own clone, checked out
# at the Windows HEAD (so only pushed commits work), apart from ~/src/windows-tools where workshop sessions edit.
$wsRepo = '/home/agent/src/windows-tools-codex'
$wsRuns = '/home/agent/codex-runs'
$wsUnc = '\\wsl.localhost\workshop\home\agent\codex-runs'
function Quote-Bash([string]$s) { "'" + $s.Replace("'", "'\''") + "'" }
# Script names are generated ([a-z0-9-] only) and run with --exec, so no Linux shell parses the command line.
function Write-WorkshopScript([string]$name, [string[]]$lines) {   # returns the script's Linux path
    $null = Invoke-CliTool wsl.exe @('-d', 'workshop', '--exec', 'mkdir', '-p', $wsRuns) -Name workshop -Fix 'check that the workshop distro starts: wsl -d workshop'
    # ~/.profile for PATH (codex, conda); not bash -l, whose ~/.bash_logout turns every exit code into 1.
    $lines = @('. "$HOME/.profile"') + $lines
    [IO.File]::WriteAllText("$wsUnc\$name.sh", (($lines -join "`n") + "`n"), (New-Object Text.UTF8Encoding $false))
    "$wsRuns/$name.sh"
}
function Invoke-Workshop([string]$name, [string[]]$lines, [string]$fix) {   # returns stdout
    $sh = Write-WorkshopScript $name (@('set -e') + $lines)
    try { $run = Invoke-CliTool wsl.exe @('-d', 'workshop', '--exec', 'bash', $sh) -Name workshop -Fix $fix }
    finally { Remove-Item -LiteralPath "$wsUnc\$name.sh" -ErrorAction SilentlyContinue }
    if ($run.StdOut) { $run.StdOut.TrimEnd() }
}

# -Worktree records each worktree (path, origin checkout, start commit); -Accept/-Discard act only on recorded ones.
# A workshop worktree has a Linux path; its origin is the Windows checkout its changes are applied to.
function Get-WorktreeRecord($path) {
    $ws = $path.StartsWith('/')
    $wt = if ($ws) { $path.TrimEnd('/') } else { (Resolve-Path -LiteralPath $path).Path }
    $leaf = if ($ws) { 'ws-' + ($wt -split '/')[-2] } else { Split-Path (Split-Path $wt -Parent) -Leaf }
    $rec = Join-Path $runs "wt-$leaf.txt"
    if (-not (Test-Path $rec)) { Stop-Cli NOT_FOUND "$wt was not created by ask.ps1 -Worktree (no record $rec)" 'use a worktree path recorded by ask.ps1' }
    $r = Get-Content -Encoding UTF8 $rec
    if ($r[0] -ne $wt) { Stop-Cli BAD_INPUT "Record $rec is for $($r[0]), not $wt" 'use the worktree path recorded in its record file' }
    [pscustomobject]@{ Wt = $wt; Origin = $r[1]; Base = $r[2]; File = $rec; Workshop = $ws }
}
function Remove-Worktree($r) {
    if ($r.Workshop) {
        $parent = $r.Wt.Substring(0, $r.Wt.LastIndexOf('/'))
        $null = Invoke-Workshop "remove-$PID" @("git -C $wsRepo worktree remove --force $(Quote-Bash $r.Wt)",
            "rmdir $(Quote-Bash $parent) 2>/dev/null || true") 'remove the worktree by hand: wsl -d workshop, then git worktree remove'
    } else {
        Invoke-Git -C $r.Origin worktree remove --force $r.Wt
        $parent = Split-Path $r.Wt -Parent   # ~/.codex/worktrees/<id>: drop it too once empty
        if (-not (Get-ChildItem -LiteralPath $parent -Force)) { Remove-Item -LiteralPath $parent }
    }
    Remove-Item $r.File
}

if ($Accept) {
    $r = Get-WorktreeRecord $Accept
    # Diff from the start commit, so work Codex committed comes across too.
    $stamp = Get-Date -Format yyyyMMdd-HHmmss
    $patch = Join-Path $runs "accept-$stamp.patch"
    if ($r.Workshop) {
        $null = Invoke-Workshop "accept-$PID" @("cd $(Quote-Bash $r.Wt)", 'git add -A',
            "git diff --cached --binary $($r.Base) --output=$wsRuns/accept-$stamp.patch") 'check the worktree in the workshop'
        Move-Item -LiteralPath "$wsUnc\accept-$stamp.patch" -Destination $patch
    } else {
        Invoke-Git -C $r.Wt add -A
        Invoke-Git -C $r.Wt diff --cached --binary $r.Base --output=$patch
    }
    if ((Get-Item $patch).Length -eq 0) { "no changes in $($r.Wt)" }
    else {
        $null = Invoke-CliTool git @('-C', $r.Origin, 'apply', '--whitespace=nowarn', $patch) -Name git `
            -Fix "worktree kept at $($r.Wt), patch at ${patch}: resolve the conflict, then git apply it"
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

$exe = if ($env:ASK_CODEX_EXE) { Get-Item -LiteralPath $env:ASK_CODEX_EXE } else {   # override: tests, version pinning
    Get-ChildItem "$env:LOCALAPPDATA\OpenAI\Codex\bin\*\codex.exe" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
}
if (-not $exe) { Stop-Cli NOT_FOUND 'codex.exe not found under %LOCALAPPDATA%\OpenAI\Codex\bin (is the Codex app installed?)' 'install the Codex app or repair its bin path' }

if (-not ($PromptFile -or $Review)) { Stop-Cli BAD_ARG 'Give -PromptFile, -Review, or -Review with -PromptFile (focus instructions).' 'provide -PromptFile or -Review' }
if ($Resume -and $Review) { Stop-Cli BAD_ARG '-Review starts a fresh review: drop -Resume.' 'drop -Resume or -Review' }
# The CLI takes review instructions only without a preset: the prompt itself names what to review and the focus.
if ($Review -eq 'custom' -and -not $PromptFile) { Stop-Cli BAD_ARG '-Review custom needs -PromptFile (what to review, and the focus).' 'add -PromptFile' }
if ($Review -and $Review -ne 'custom' -and $PromptFile) {
    Stop-Cli BAD_ARG "-Review $Review takes no instructions (CLI limit): use -Review custom -PromptFile, naming the diff or files to review." 'use -Review custom -PromptFile'
}
if ($Resume -and -not $PromptFile) { Stop-Cli BAD_ARG '-Resume needs -PromptFile (the follow-up message).' 'add -PromptFile' }
if ($Resume -and $Worktree) { Stop-Cli BAD_ARG '-Resume reruns in the session''s own dir (a worktree session keeps its worktree): drop -Worktree.' 'drop -Worktree' }
# `exec resume` runs in the launch dir and takes no -C/-s, so -Keep records both and -Resume restores them.
if ($Resume) {
    $sessionFile = Join-Path $runs "session-$Resume.txt"
    if (-not (Test-Path $sessionFile)) { Stop-Cli NOT_FOUND "No record for session $Resume (was it started with -Keep?)" 'use a session id recorded with -Keep' }
    $Dir, $Sandbox = Get-Content -Encoding UTF8 $sessionFile
}
if ($Review) { $Sandbox = 'read-only' }

# Role presets (user's rule, 2026-10-11): builds get Luna and write access; reviews, critiques and second
# opinions get Sol, read-only; quick (user, 2026-10-12) = Luna low for small bounded jobs, read-only
# or in a -Worktree as the call says; deep (user, 2026-10-28) = Astra, the strongest model, read-only,
# for the hardest problems and only after the user's OK per call. An explicit -Model or -Effort still wins.
$roles = @{
    build  = @{ Model = 'gpt-6-luna';  Effort = 'high' }
    review = @{ Model = 'gpt-6.1-sol'; Effort = 'high' }
    quick  = @{ Model = 'gpt-6-luna';  Effort = 'low' }
    deep   = @{ Model = 'gpt-6-astra'; Effort = 'high' }
}
if (-not $Role) { $Role = if ($Worktree -or $Sandbox -eq 'workspace-write') { 'build' } else { 'review' } }
$resumed = if ($Resume) { " (session $Resume was saved with that sandbox; start a fresh session instead)" } else { '' }
if ($Role -in 'review', 'deep' -and ($Worktree -or $Sandbox -ne 'read-only')) {
    Stop-Cli BAD_ARG "-Role $Role is read-only, but this call has write access$resumed. Drop -Worktree / -Sandbox workspace-write, or use -Role build." 'drop write access or use -Role build'
}
if ($Role -eq 'build') {
    if ($Review) { Stop-Cli BAD_ARG '-Review is a review: use -Role review, or omit -Role.' 'use -Role review or omit -Role' }
    # A build writes in a worktree unless -Sandbox workspace-write asks for the live checkout.
    if (-not $Resume -and -not $hasSandbox) { $Worktree = [switch]$true }
    if (-not $Worktree -and $Sandbox -eq 'read-only') { Stop-Cli BAD_ARG "-Role build needs write access, but this call is read-only$resumed." 'use -Worktree or -Sandbox workspace-write' }
}
# Web pages can carry instructions aimed at Codex, so search only where it can't write; never for a review.
if ($Search -and ($Review -or $Worktree -or $Sandbox -ne 'read-only')) {
    Stop-Cli BAD_ARG "-Search is for read-only ask calls only (no -Review, -Worktree or write access)$resumed." 'remove -Review and write access options'
}
if (-not $hasModel)  { $Model  = $roles[$Role].Model }
if (-not $hasEffort) { $Effort = $roles[$Role].Effort }
if ($Workshop) {
    if ($Keep -or $Resume) { Stop-Cli BAD_ARG '-Workshop runs are one-shot: -Keep and -Resume work on Windows only.' 'drop -Keep/-Resume, or drop -Workshop' }
    if ($Review -eq 'uncommitted') { Stop-Cli BAD_ARG 'The workshop sees only pushed commits, not uncommitted changes.' 'drop -Workshop to review uncommitted work on Windows' }
    if ($Sandbox -eq 'workspace-write' -and -not $Worktree) { Stop-Cli BAD_ARG 'In the workshop Codex writes only in a -Worktree, whose changes -Accept brings to Windows.' 'add -Worktree' }
}

# A locked-out sandbox account fails every command, and Codex retries until the plan is spent:
# after one lockout, refuse new runs for 30 minutes (after the argument checks, so bad calls still say why).
# Since 2026-10-28 ~/.codex/config.toml runs `[windows] sandbox = "unelevated"` and the legacy sandbox accounts
# are uninstalled (openai/codex#18620: parallel elevated runs lock them out); this stays as the safety net.
$lockMarker = Join-Path $runs 'sandbox-locked.txt'
if (-not $Workshop -and (Test-Path -LiteralPath $lockMarker)) {   # the Linux sandbox has no such accounts
    $until = (Get-Item -LiteralPath $lockMarker).LastWriteTime.AddMinutes(30)
    if ((Get-Date) -lt $until) {
        Write-CliWarn 'ask' SANDBOX_LOCKED "Codex sandbox account was locked out (1909); not starting Codex (marker $lockMarker)" "retry after $($until.ToString('HH:mm'))"
        exit 1
    }
    Remove-Item -LiteralPath $lockMarker
}

$Dir = (Resolve-Path -LiteralPath $Dir).Path
if ($PromptFile) { $PromptFile = (Resolve-Path -LiteralPath $PromptFile).Path }
# Review files go to runs\, outside the reviewed repo, so they never show up as untracked changes.
$base = if ($PromptFile) { $PromptFile } else { Join-Path $runs "review-$(Split-Path $Dir -Leaf)" }
$Out = if ($Out) { Abs $Out } else { "$base.answer.md" }
$log = "$base.log"
if ($Workshop) {
    # Same place in the workshop clone, at the same commit: Windows HEAD must be pushed, and the job checks it out on its own.
    $root = (Invoke-Git -C $Dir rev-parse --show-toplevel) -replace '/', '\'
    if ((Invoke-Git -C $root remote get-url origin) -notmatch 'nashathur/windows-tools(\.git)?$') {
        Stop-Cli BAD_ARG "$Dir is not in the windows-tools repo, the only one the workshop has." 'drop -Workshop'
    }
    $wsHead = Invoke-Git -C $root rev-parse HEAD
    $wsName = "job-$PID-$(Get-Date -Format HHmmssfff)"
    $wsCheckout = "$wsRuns/$wsName"   # this job's own detached checkout, removed after the run
    $wsDir = ($wsCheckout + '/' + ($Dir.Substring($root.Length).TrimStart('\') -replace '\\', '/')).TrimEnd('/')
    $wsOut = "$wsRuns/$wsName.answer.md"
}

$preamble = @'
You are working with Claude, which orchestrates this work and makes the final calls.
For a question: answer with findings, options and your recommendation, each with its reasoning and the file:line or evidence behind it.
For a task: do it within the spec, then report what you changed, what you ran and its result, and any choice the spec left open, with your reasoning.
Then add a "Suggestions" section: better approaches, risks, bugs or improvements you noticed beyond the spec, each with its reasoning. Leave them unimplemented; Claude decides which to take.
Run Python only through the conda env the prompt names (`conda run -n <env>`); with none named, leave it unrun and say what you would have run.
Mark anything you did not verify. If something is ambiguous, state your assumption and answer anyway: nobody can reply mid-run.

'@

$args_ = @('exec')
# Not the top-level --search flag: the CLI drops it on the way to `exec` (0.162), leaving search cached or off.
# Bare `live` (no TOML quotes, which PS 5.1 would strip) falls back to the literal string.
if ($Search) { $args_ += @('-c', 'web_search=live') }
if ($Resume) {
    $args_ += @('resume', $Resume, '-c', "sandbox_mode=$Sandbox")
} elseif ($Review) {
    $args_ += @('review', '-c', 'sandbox_mode=read-only')
    switch -Regex ($Review) {
        '^uncommitted$' { $args_ += '--uncommitted' }
        '^base:(.+)$'   { $args_ += @('--base', $Matches[1]) }
        '^commit:(.+)$' { $args_ += @('--commit', $Matches[1]) }
        '^custom$'      { }   # the prompt (stdin) names the target
        default         { Stop-Cli BAD_ARG "Bad -Review '$Review'" 'use uncommitted, base:<branch>, commit:<sha>, or custom' }
    }
} else {
    if ($Worktree) {
        $Sandbox = 'workspace-write'
        $args_ += '--worktree'
        $startCommit = Invoke-Git -C $Dir rev-parse HEAD   # the worktree branches from here
    }
    # Start-Process joins arguments unquoted, so Windows paths carry their own quotes; workshop ones are bash-quoted later.
    # Workshop builds run unsandboxed (user, 2026-10-31): the seal is the boundary, cc-safety-net's hook the accident guard.
    $args_ += $(if ($Workshop -and $Sandbox -eq 'workspace-write') { '--dangerously-bypass-approvals-and-sandbox' } else { @('-s', $Sandbox) })
    $args_ += @('-C', $(if ($Workshop) { $wsDir } else { "`"$Dir`"" }), '--skip-git-repo-check', '--color', 'never')
}
if (-not ($Keep -or $Resume -or $Worktree)) { $args_ += '--ephemeral' }   # the CLI rejects --worktree with --ephemeral
$args_ += @('-o', $(if ($Workshop) { $wsOut } else { "`"$Out`"" }), '-c', "model_reasoning_effort=$Effort")
if ($Model) { $args_ += @('-m', $Model) }

# Stdin carries the prompt: with the role preamble on a fresh ask, bare as review focus instructions
# (the preamble would override the review's own rubric); empty for a plain review, so nothing waits on it.
$stdin = "$base.stdin.txt"
$text = ''
if ($PromptFile) {
    $text = [IO.File]::ReadAllText($PromptFile)
    if (-not ($Resume -or $Review)) { $text = $preamble + $text }
    $args_ += '-'
}
[IO.File]::WriteAllText($stdin, $text, (New-Object Text.UTF8Encoding $false))

if (Test-Path -LiteralPath $Out) { Remove-Item -LiteralPath $Out }
$sw = [Diagnostics.Stopwatch]::StartNew()
# Written whole via a temp file and a move, so a concurrent Show-Jobs never reads half a record.
# Bookkeeping must never fail the run itself.
function Write-JobRecord($lines) {
    try {
        $tmp = Join-Path $runs ("tmp-" + (Split-Path $jobFile -Leaf))
        [IO.File]::WriteAllLines($tmp, [string[]]$lines, (New-Object Text.UTF8Encoding $false))
        Move-Item -LiteralPath $tmp -Destination $jobFile -Force
    } catch { }
}
$jobFile = Join-Path $runs ("job-{0}-{1}.txt" -f (Get-Date -Format 'yyyyMMdd-HHmmss'), $PID)
$script:jobFile = $jobFile
$procStart = (Get-Process -Id $PID).StartTime.ToString('o')
$jobStarted = (Get-Date).ToString('o')
$mode = if ($Resume) { 'resume' } elseif ($Review) { 'review' } elseif ($Worktree) { 'worktree' } else { 'ask' }
$jobName = Split-Path $base -Leaf
$job = @(
    "pid=$PID", "pstart=$procStart", "started=$jobStarted", "role=$Role", "model=$Model",
    "effort=$Effort", "mode=$mode", "name=$jobName", "answer=$Out"
)
Write-JobRecord $job
function Get-HeadField($name) {   # a failed run may log neither field
    $m = $head | Select-String "^${name}: (.+)$" | Select-Object -First 1
    if ($m) { $m.Matches[0].Groups[1].Value }
}
# The sandbox's spawn failure ("CreateProcessWithLogonW failed:" then the code); written with \s so this
# file, when Codex reads it, doesn't match (a bare 1909 also turns up as a line number).
$lockPattern = 'failed:\s1909(?!\d)|\(Windows\serror\s1909\)|account\sis\scurrently\slocked\sout'
$script:logPos = 0L
# Only Codex's own log line counts ("<UTC time> ERROR codex_core::... failed: 1909"), stamped during this run:
# commands that quote the pattern or print old transcripts echo the same text (two false lockouts, 2026-10-28).
# Command blocks are not skipped: in real lockouts this line lands between a command and its status line.
$runStartUtc = [DateTime]::UtcNow.AddMinutes(-1)
function Find-Lockout([switch]$Final) {   # first lockout line in the transcript written since the last call
    if (-not (Test-Path -LiteralPath $log)) { return }
    $fs = [IO.File]::Open($log, 'Open', 'Read', 'ReadWrite')
    try {
        $bytes = New-Object byte[] ([int]($fs.Length - $script:logPos))
        $fs.Position = $script:logPos
        $n = $fs.Read($bytes, 0, $bytes.Length)
    } finally { $fs.Dispose() }
    if ($n -le 0) { return }
    $end = if ($Final) { $n } else { [Array]::LastIndexOf($bytes, [byte]10, [Math]::Max($n - 1, 0)) + 1 }   # whole lines only
    if ($end -le 0) { return }
    $script:logPos += $end
    foreach ($line in [Text.Encoding]::UTF8.GetString($bytes, 0, $end) -split "`r?`n") {
        if ($line -notmatch '^(\d{4}-\d\d-\d\dT[\d:.]+Z)\s+ERROR\s+codex_core' -or $line -notmatch $lockPattern) { continue }
        $stamp = [DateTime]::Parse(($line -split '\s')[0], [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::AdjustToUniversal)
        if ($stamp -ge $runStartUtc) { return $line }
    }
}
$jobExit = 'interrupted'
$lockLine = $null
$savedPythonPath = $env:PYTHONPATH
try {
    # Lets tempfile work inside the unelevated sandbox (pyshim\sitecustomize.py says why); Codex's process only.
    $env:PYTHONPATH = (@("$PSScriptRoot\pyshim") + @($savedPythonPath | Where-Object { $_ })) -join ';'
    if ($Workshop) {
        # stdin (the prompt) passes through wsl.exe and bash to codex; the script file carries everything else.
        $sh = Write-WorkshopScript $wsName @(
            'set -e', 'export CODEX_CONSULT=1 GIT_TERMINAL_PROMPT=0',
            # Parallel jobs share the clone, so the lock covers clone, fetch and adding this job's checkout; each job
            # then runs in its own checkout, pinned at its commit. The clone stays detached so the fetch can move
            # every local branch to GitHub's (for -Review base:<branch>). Git reads /dev/null: stdin is codex's.
            "flock $wsRuns/repo.lock bash -c 'set -e",
            "  [ -d $wsRepo/.git ] || { git clone -q https://github.com/nashathur/windows-tools.git $wsRepo; git -C $wsRepo checkout -q --detach; }",
            "  git -C $wsRepo fetch -q --prune origin `"+refs/heads/*:refs/heads/*`"",
            "  git -C $wsRepo cat-file -e $wsHead^{commit} 2>/dev/null || { echo `"ERROR Windows HEAD $($wsHead.Substring(0, 7)) is not on GitHub: push it first`" >&2; exit 3; }",
            "  git -C $wsRepo worktree prune",
            "  git -C $wsRepo worktree add -q --detach $wsCheckout $wsHead' < /dev/null",
            "cd $(Quote-Bash $wsDir)",
            "rc=0; codex $(($args_ | ForEach-Object { Quote-Bash $_ }) -join ' ') || rc=`$?",
            "flock $wsRuns/repo.lock git -C $wsRepo worktree remove --force $wsCheckout < /dev/null || true",
            'exit $rc')
        $p = Start-Process -FilePath wsl.exe -ArgumentList @('-d', 'workshop', '--exec', 'bash', $sh) -NoNewWindow -PassThru `
            -RedirectStandardInput $stdin -RedirectStandardOutput "$log.out" -RedirectStandardError $log
    } else {
        $p = Start-Process -FilePath $exe.FullName -ArgumentList $args_ -WorkingDirectory $Dir -NoNewWindow -PassThru `
            -RedirectStandardInput $stdin -RedirectStandardOutput "$log.out" -RedirectStandardError $log
    }
    $null = $p.Handle   # keeps ExitCode readable after exit (PS 5.1)
    while (-not $p.WaitForExit(1000)) {
        $lockLine = Find-Lockout
        if ($lockLine) {
            # Only the process this call started, with its own children (the sandboxed commands).
            & taskkill.exe /PID $p.Id /T /F 2>&1 | Out-Null
            $p.WaitForExit()
            break
        }
    }
    if (-not $lockLine) { $lockLine = Find-Lockout -Final }
    Remove-Item -LiteralPath $stdin
    if ($Workshop -and (Test-Path -LiteralPath "$wsUnc\$wsName.answer.md")) { Move-Item -LiteralPath "$wsUnc\$wsName.answer.md" -Destination $Out -Force }
    if ($lockLine) {
        [IO.File]::WriteAllText($lockMarker, (Get-Date).ToString('o'))
        $jobExit = 'sandbox locked'
        Stop-Cli SANDBOX_LOCKED 'Codex sandbox account is locked out (1909); Codex stopped' 'wait for the lockout to clear, then rerun' `
            -Detail "Codex transcript: $log`nmatched: $lockLine`nnew runs refused until $((Get-Date).AddMinutes(30).ToString('HH:mm')) (marker $lockMarker)"
    }
    "codex exit $($p.ExitCode) in $([int]$sw.Elapsed.TotalSeconds)s ($Role`: $Model, $Effort); answer: $Out; log: $log"
    $head = Get-Content $log -TotalCount 15
    $wd = Get-HeadField 'workdir'
    if ($Keep -or $Resume) {
        $id = Get-HeadField 'session id'
        if ($Keep -and $id) { Set-Content -Encoding UTF8 (Join-Path $runs "session-$id.txt") @($wd, $Sandbox) }
        "session: $id  (continue with -Resume $id)"
    }
    if ($Worktree -and $wd -and $Workshop) {
        # The worktree's root (workdir may be a subfolder of it): ~/.codex/worktrees/<id>/<checkout name>.
        if ($wd -match '^(.*/\.codex/worktrees/[^/]+/[^/]+)') { $wd = $Matches[1] }
        Set-Content -Encoding UTF8 (Join-Path $runs "wt-ws-$(($wd -split '/')[-2]).txt") @($wd, $root, $wsHead)
        "worktree (workshop): $wd  (review with wsl -d workshop -- git -C $wd diff $($wsHead.Substring(0, 7)); then -Accept or -Discard it)"
    } elseif ($Worktree -and $wd) {
        Set-Content -Encoding UTF8 (Join-Path $runs "wt-$(Split-Path (Split-Path $wd -Parent) -Leaf).txt") @($wd, $Dir, $startCommit)
        "worktree: $wd  (review with git -C; then -Accept or -Discard it)"
    }
    $jobExit = if ($p.ExitCode -ne 0) { $p.ExitCode } elseif (-not (Test-Path -LiteralPath $Out)) { 'no answer' } else { 0 }
} finally {
    $env:PYTHONPATH = $savedPythonPath
    if ($Workshop) { Remove-Item -LiteralPath "$wsUnc\$wsName.sh" -ErrorAction SilentlyContinue }
    # Last, so a "finished" notice never precedes the worktree/session records it points to.
    $end = $job + @("ended=$((Get-Date).ToString('o'))", "exit=$jobExit")
    if ($Worktree -and $wd) { $end += "worktree=$wd" }
    Write-JobRecord $end
}
if ($p.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $Out)) {
    $tail = @(Get-Content $log -Tail 30)
    $relevant = Get-RelevantLine ($tail -join "`n")
    Stop-Cli TOOL_FAILED "Codex exit $($p.ExitCode): $relevant" 'read the Codex transcript and rerun after fixing the reported issue' `
        -Detail "Codex transcript: $log`n$($tail -join "`n")" `
        -ToolRun @{ Command = "$($exe.FullName) $($args_ -join ' ')"; ExitCode = $p.ExitCode; StdOut = ''; StdErr = ($tail -join "`n") }
}
