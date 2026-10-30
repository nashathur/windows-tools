$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) "ask-errors-$PID"
[void](New-Item -ItemType Directory -Force $root)
$env:CLIERR_LOG_ROOT = Join-Path $root 'logs'
[void](New-Item -ItemType Directory -Force $env:CLIERR_LOG_ROOT)
$target = Join-Path $PSScriptRoot '..\ask.ps1'
$failed = 0

function Run-Case([string]$Name, [string[]]$Arguments, [int]$Exit, [string]$Code, [string]$Contains) {
    $err = Join-Path $root "$Name.err"
    $out = Join-Path $root "$Name.out"
    $args = @('-NoProfile','-ExecutionPolicy','Bypass','-File',$target) + $Arguments
    $savedConsult = $env:CODEX_CONSULT
    Remove-Item Env:CODEX_CONSULT -ErrorAction SilentlyContinue
    $p = Start-Process powershell.exe -ArgumentList $args -NoNewWindow -Wait -PassThru -RedirectStandardError $err -RedirectStandardOutput $out
    if ($null -ne $savedConsult) { $env:CODEX_CONSULT = $savedConsult }
    $stderr = [IO.File]::ReadAllText($err)
    $lines = @($stderr -split "`r?`n" | Where-Object { $_ })
    $problem = $null
    if ($p.ExitCode -ne $Exit) { $problem = "exit $($p.ExitCode), expected $Exit" }
    elseif ($lines.Count -ne 1 -or $lines[0] -notlike "ERROR ask ${Code}: *") { $problem = "stderr is not one ERROR ask ${Code} line: $stderr" }
    elseif (-not $lines[0].Contains($Contains)) { $problem = "line lacks '$Contains'" }
    else {
        $log = ($lines[0] -split '\| log: ')[-1]
        if (-not (Test-Path -LiteralPath $log)) { $problem = "log missing: $log" }
    }
    if ($problem) { $script:failed++; "FAIL ${Name}: $problem" }
}

Run-Case 'conflict' @('-Accept','x','-Discard','y') 2 'BAD_ARG' 'use only -Accept or -Discard'
Run-Case 'deep-write' @('-PromptFile',$PSCommandPath,'-Role','deep','-Worktree') 2 'BAD_ARG' 'drop write access or use -Role build'
Run-Case 'ws-uncommitted' @('-Workshop','-Review','uncommitted') 2 'BAD_ARG' 'review uncommitted work on Windows'
Run-Case 'ws-keep' @('-Workshop','-Keep','-PromptFile',$PSCommandPath) 2 'BAD_ARG' 'drop -Keep/-Resume'
Run-Case 'ws-live' @('-Workshop','-Sandbox','workspace-write','-PromptFile',$PSCommandPath) 2 'BAD_ARG' 'add -Worktree'
Run-Case 'ws-other-repo' @('-Workshop','-PromptFile',$PSCommandPath,'-Dir',($env:USERPROFILE + '\.claude')) 2 'BAD_ARG' 'drop -Workshop'

Remove-Item -LiteralPath $root -Recurse -Force
if ($failed) { exit 1 }
