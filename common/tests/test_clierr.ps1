# clierr.ps1: every failure prints exactly one ERROR line and leaves its log. Prints only failures;
# exit 1 if any. Run: powershell -NoProfile -ExecutionPolicy Bypass -File C:\Tools\common\tests\test_clierr.ps1
$ErrorActionPreference = 'Stop'
$helper = Join-Path (Split-Path $PSScriptRoot) 'clierr.ps1'
$root = Join-Path ([IO.Path]::GetTempPath()) "clierr-test-$PID"
[void](New-Item -ItemType Directory -Force $root)
$failed = 0

function Run-Case([string]$Name, [string]$Body, [int]$Exit, [string]$Code, [string[]]$LineHas = @(), [string[]]$LogHas = @()) {
    $script = Join-Path $root "$Name.ps1"
    [IO.File]::WriteAllText($script, ". '$helper'`ntrap { Exit-Cli 'demo' `$_ }`n$Body`n", (New-Object Text.UTF8Encoding $false))
    $env:CLIERR_LOG_ROOT = $root
    $errFile = Join-Path $root "$Name.err"
    $p = Start-Process powershell -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$script`"" `
        -NoNewWindow -Wait -PassThru -RedirectStandardError $errFile -RedirectStandardOutput (Join-Path $root "$Name.out")
    $stderr = (Get-Content $errFile -Raw -Encoding UTF8)
    $lines = @("$stderr" -split "`r?`n" | Where-Object { $_ })
    $problems = @()
    if ($p.ExitCode -ne $Exit) { $problems += "exit $($p.ExitCode), expected $Exit" }
    if ($Code) {
        if ($lines.Count -ne 1 -or $lines[0] -notlike "ERROR demo ${Code}: *") { $problems += "stderr not one ERROR $Code line: $stderr" }
        else {
            foreach ($s in $LineHas) { if (-not $lines[0].Contains($s)) { $problems += "line lacks '$s'" } }
            $log = ($lines[0] -split '\| log: ')[-1]
            if (-not (Test-Path $log)) { $problems += "log missing: $log" }
            else {
                $text = [IO.File]::ReadAllText($log)
                foreach ($s in $LogHas) { if (-not $text.Contains($s)) { $problems += "log lacks '$s'" } }
            }
        }
    } elseif ($lines.Count -and $Name -ne 'warn') { $problems += "unexpected stderr: $stderr" }
    if ($problems) { $script:failed++; "FAIL ${Name}: $($problems -join '; ')" }
}

Run-Case 'stop' "Stop-Cli BAD_PAGE 'page 12 of 10' 'use 1-10'" 1 'BAD_PAGE' @('page 12 of 10 | fix: use 1-10 | log: ') @('== error ==')
Run-Case 'badarg' "Stop-Cli BAD_ARG 'no -Dir' 'give -Dir'" 2 'BAD_ARG'
Run-Case 'throw' "throw 'Give -Accept or -Discard, not both.'" 1 'BAD_INPUT' @('Give -Accept or -Discard')
Run-Case 'internal' "`$null.Foo()" 1 'INTERNAL' @('internal.ps1:') @('== traceback ==')
Run-Case 'notfound' "Get-Item 'C:\no\such\file.txt' -ErrorAction Stop" 1 'NOT_FOUND'
Run-Case 'tool' "Invoke-CliTool cmd @('/c', 'echo noise 1>&2 & echo fatal: disk full 1>&2 & exit 3') -Name helper" `
    1 'TOOL_FAILED' @('helper exit 3: fatal: disk full') @('== tool stderr (last 30) ==', 'noise')
Run-Case 'cargo' ("Invoke-CliTool cmd @('/c', 'echo warning: unused 1>&2 & echo error[E0308]: mismatched types 1>&2 & " +
    "echo    --^> src\main.rs:10:5 1>&2 & echo error: could not compile 1>&2 & exit 101') -Name cargo") `
    1 'TOOL_FAILED' @('cargo exit 101: error[E0308]: mismatched types --> src\main.rs:10:5')
Run-Case 'quoting' "`$r = Invoke-CliTool cmd @('/c', 'echo', 'a b \`"c\`"'); if (`$r.StdOut -notmatch 'a b') { Stop-Cli BAD_INPUT `$r.StdOut x }" 0 ''
Run-Case 'multiline' "Stop-Cli X ""two``nlines"" ""fix``nthis""" 1 'X' @('X: two lines | fix: fix this | log: ')
Run-Case 'ok' "'fine'" 0 ''
# a function's $script: variable is the one the script body reads (the trap keeps one scope)
Run-Case 'scope' "`$flag = 0; function Set-Flag { `$script:flag = 1 }; Set-Flag; if (`$flag -ne 1) { exit 3 }" 0 ''
Run-Case 'warn' "Write-CliWarn demo SKIPPED 'one file' 'nothing'" 0 ''

# the warning case expects stderr, so check it directly
$w = Get-Content (Join-Path $root 'warn.err') -Raw
if ($w.Trim() -ne 'WARN demo SKIPPED: one file | fix: nothing') { $failed++; "FAIL warn line: $w" }

$folder = Join-Path $root 'demo'
1..60 | ForEach-Object { [IO.File]::WriteAllText((Join-Path $folder ('20000101-{0:d6}-1.log' -f $_)), 'old') }
Run-Case 'prune' "Stop-Cli X 'y' 'z'" 1 'X'
$n = @(Get-ChildItem $folder -Filter *.log).Count
if ($n -ne 50) { $failed++; "FAIL prune: $n logs, expected 50" }

# two logs in one second from one process get separate files
. $helper
$script:CliLogRoot = $root
$a = Write-CliLog -Tool 'twice' -Code X -What y -Fix z
$b = Write-CliLog -Tool 'twice' -Code X -What y -Fix z
if ($a -eq $b -or -not (Test-Path $a) -or -not (Test-Path $b)) { $failed++; "FAIL unique logs: $a / $b" }

Remove-Item -Recurse -Force $root
if ($failed) { exit 1 }
'clierr.ps1: all cases pass'
