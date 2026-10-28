$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) "quota-errors-$PID"
[void](New-Item -ItemType Directory -Force $root)
$env:CLIERR_LOG_ROOT = Join-Path $root 'logs'
$target = Join-Path $PSScriptRoot '..\quota.ps1'
$failed = 0

# A Codex session log whose 5-hour window has already reset (used must read 0) and whose week has not.
$sessions = Join-Path $root '.codex\sessions\2026'
[void](New-Item -ItemType Directory -Force $sessions)
$past = [DateTimeOffset]::UtcNow.AddHours(-1).ToUnixTimeSeconds()
$future = [DateTimeOffset]::UtcNow.AddDays(3).ToUnixTimeSeconds()
Set-Content -LiteralPath (Join-Path $sessions 'rollout-x.jsonl') ('{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":40.0,"resets_at":' + $past + '},"secondary":{"used_percent":12.0,"resets_at":' + $future + '}}}}')

function Run-Quota([string]$Home_, [string]$Name, [string]$Agy = (Join-Path $root 'nope.exe')) {
    $savedHome = $env:USERPROFILE
    $env:USERPROFILE = $Home_
    $env:AGY_EXE = $Agy
    $p = Start-Process powershell.exe -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $target, '-Json') -NoNewWindow -Wait -PassThru `
        -RedirectStandardError (Join-Path $root "$Name.err") -RedirectStandardOutput (Join-Path $root "$Name.out")
    $env:USERPROFILE = $savedHome
    Remove-Item Env:AGY_EXE
    @{ Exit = $p.ExitCode; Out = [IO.File]::ReadAllText((Join-Path $root "$Name.out")); Err = [IO.File]::ReadAllText((Join-Path $root "$Name.err")) }
}

# Codex only: reset window reads 0, live window keeps its value; missing agy is a warning, not a failure.
$r = Run-Quota $root 'codex'
$rows = $r.Out | ConvertFrom-Json
$h5 = $rows | Where-Object window -eq '5h'
$week = $rows | Where-Object window -eq 'week'
if ($r.Exit -ne 0) { $failed++; "FAIL codex: exit $($r.Exit) $($r.Err)" }
elseif ($h5.used -ne 0 -or $week.used -ne 12) { $failed++; "FAIL codex: 5h=$($h5.used) week=$($week.used), expected 0 and 12" }
elseif ($r.Err -notmatch 'WARN quota NOT_FOUND') { $failed++; "FAIL codex: no agy warning: $($r.Err)" }

# The newest quota event wins by its own timestamp, not by which file was written last; a failing agy
# (signed out) is a warning and Codex's rows still print.
$two = Join-Path $root 'two\.codex\sessions'
[void](New-Item -ItemType Directory -Force $two)
$line = { param($ts, $pct) '{"timestamp":"' + $ts + '","type":"event_msg","payload":{"type":"token_count","rate_limits":{"secondary":{"used_percent":' + $pct + ',"resets_at":' + $future + '}}}}' }
Set-Content -LiteralPath (Join-Path $two 'a.jsonl') (& $line ([DateTime]::UtcNow.AddMinutes(-1).ToString('o')) 95)
Set-Content -LiteralPath (Join-Path $two 'b.jsonl') @((& $line ([DateTime]::UtcNow.AddMinutes(-10).ToString('o')) 5), '{"type":"later output"}')
(Get-Item (Join-Path $two 'a.jsonl')).LastWriteTime = (Get-Date).AddMinutes(-5)
$failAgy = Join-Path $root 'failagy.cmd'
Set-Content -LiteralPath $failAgy "@1>&2 echo Error: Please sign in`r`n@exit /b 1"
$r = Run-Quota (Join-Path $root 'two') 'newest' $failAgy
$week = ($r.Out | ConvertFrom-Json) | Where-Object window -eq 'week'
if ($r.Exit -ne 0) { $failed++; "FAIL newest: exit $($r.Exit) $($r.Err)" }
elseif ($week.used -ne 95) { $failed++; "FAIL newest: week=$($week.used), expected 95 from the newer event" }
elseif ($r.Err -notmatch 'WARN quota TOOL_FAILED') { $failed++; "FAIL newest: no warning for the failing agy: $($r.Err)" }

# Nothing readable: exactly one ERROR line plus its log.
$r = Run-Quota (Join-Path $root 'empty') 'none'
$errors = @($r.Err -split "`r?`n" | Where-Object { $_ -like 'ERROR *' })
if ($r.Exit -ne 1) { $failed++; "FAIL none: exit $($r.Exit), expected 1" }
elseif ($errors.Count -ne 1 -or $errors[0] -notlike 'ERROR quota NOT_FOUND: *') { $failed++; "FAIL none: not one ERROR quota NOT_FOUND line: $($r.Err)" }
elseif (-not (Test-Path -LiteralPath ($errors[0] -split '\| log: ')[-1])) { $failed++; 'FAIL none: log missing' }

Remove-Item -LiteralPath $root -Recurse -Force
if ($failed) { exit 1 }
'quota errors: all passed'
