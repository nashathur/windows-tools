$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) "agy-errors-$PID"
[void](New-Item -ItemType Directory -Force $root)
$env:CLIERR_LOG_ROOT = Join-Path $root 'logs'
[void](New-Item -ItemType Directory -Force $env:CLIERR_LOG_ROOT)
$target = Join-Path $PSScriptRoot '..\ask.ps1'
$prompt = Join-Path $root 'p.md'
Set-Content -LiteralPath $prompt 'hello'
# Fake agy builds: one hits the quota (exit 3 + AGY_ERROR), one is refused a write and answers nothing.
Set-Content -LiteralPath (Join-Path $root 'quota.cmd') "@echo AGY_ERROR: {`"status`":`"RESOURCE_EXHAUSTED`",`"code`":429} 1>&2`r`n@exit /b 3"
Set-Content -LiteralPath (Join-Path $root 'denied.cmd') "@echo {`"status`":`"SUCCESS`",`"response`":`"`",`"denied_actions`":[{`"action`":`"write_file`"}],`"usage`":{}}"
$env:AGY_SETTINGS = Join-Path $root 'settings.json'
Set-Content -LiteralPath $env:AGY_SETTINGS '{"permissions":{"allow":["read_file(*)"]}}'
$failed = 0

function Run-Case([string]$Name, [string[]]$Arguments, [string]$Exe, [int]$Exit, [string]$Code, [string]$Contains) {
    $err = Join-Path $root "$Name.err"
    $out = Join-Path $root "$Name.out"
    $a = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $target, '-PromptFile', $prompt, '-Dir', $root) + $Arguments
    $env:AGY_EXE = $Exe
    $p = Start-Process powershell.exe -ArgumentList $a -NoNewWindow -Wait -PassThru -RedirectStandardError $err -RedirectStandardOutput $out
    Remove-Item Env:AGY_EXE
    $stderr = [IO.File]::ReadAllText($err)
    $lines = @($stderr -split "`r?`n" | Where-Object { $_ })
    $problem = $null
    if ($p.ExitCode -ne $Exit) { $problem = "exit $($p.ExitCode), expected $Exit" }
    elseif ($lines.Count -ne 1 -or $lines[0] -notlike "ERROR agy ${Code}: *") { $problem = "stderr is not one ERROR agy ${Code} line: $stderr" }
    elseif (-not $lines[0].Contains($Contains)) { $problem = "line lacks '$Contains'" }
    else {
        $log = ($lines[0] -split '\| log: ')[-1]
        if (-not (Test-Path -LiteralPath $log)) { $problem = "log missing: $log" }
    }
    if ($problem) { $script:failed++; "FAIL ${Name}: $problem" }
}

Run-Case 'no-schema' @('-Role', 'extract') (Join-Path $root 'denied.cmd') 2 'BAD_ARG' 'needs -Schema'
Run-Case 'missing' @() (Join-Path $root 'nope.exe') 1 'NOT_FOUND' 'agy not found'
Run-Case 'quota' @() (Join-Path $root 'quota.cmd') 1 'QUOTA' 'quota exhausted'
Run-Case 'denied' @() (Join-Path $root 'denied.cmd') 1 'DENIED' 'write_file'
$big = Join-Path $root 'big.txt'
Set-Content -LiteralPath $big ('"' * 17000)   # 17k characters, but each quote escapes to two
Run-Case 'too-long' @('-InputFile', $big) (Join-Path $root 'denied.cmd') 1 'BAD_INPUT' 'limit 32000'

# Settings that allow commands are refused before agy starts.
Set-Content -LiteralPath $env:AGY_SETTINGS '{"permissions":{"allow":["read_file(*)","command(git status)"]}}'
Run-Case 'unsafe' @() (Join-Path $root 'denied.cmd') 1 'UNSAFE_SETTINGS' 'command(git status)'
Set-Content -LiteralPath $env:AGY_SETTINGS '{"permissions":{"allow":["read_file(*)"]}}'

# A denial reported only on stderr (no denied_actions in the JSON) still counts.
Set-Content -LiteralPath (Join-Path $root 'stderr-denied.cmd') "@echo jetski: a tool required the `"write_file`" permission that headless mode cannot prompt for 1>&2`r`n@echo {`"status`":`"SUCCESS`",`"response`":`"`",`"usage`":{}}"
Run-Case 'stderr-denied' @() (Join-Path $root 'stderr-denied.cmd') 1 'DENIED' 'write_file'

# Schema output keeps its shape: a one-item array stays an array, an empty one is a valid answer.
$schema = Join-Path $root 's.json'
Set-Content -LiteralPath $schema '{"type":"array"}'
foreach ($case in @(@{ Name = 'one'; Out = '[{"id":1}]'; Want = 1 }, @{ Name = 'empty'; Out = '[]'; Want = 0 })) {
    $cmd = Join-Path $root "$($case.Name).cmd"
    Set-Content -LiteralPath $cmd ("@echo {`"status`":`"SUCCESS`",`"response`":`"x`",`"structured_output`":" + $case.Out + ",`"usage`":{}}")
    $env:AGY_EXE = $cmd
    $ans = Join-Path $root "$($case.Name).json"
    $p = Start-Process powershell.exe -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $target, '-PromptFile', $prompt, '-Dir', $root, '-Role', 'extract', '-Schema', $schema, '-Out', $ans) -NoNewWindow -Wait -PassThru -RedirectStandardError (Join-Path $root "$($case.Name).err") -RedirectStandardOutput (Join-Path $root "$($case.Name).out")
    Remove-Item Env:AGY_EXE
    $got = if (Test-Path -LiteralPath $ans) { Get-Content -LiteralPath $ans -Raw | ConvertFrom-Json } else { 'missing' }
    if ($p.ExitCode -ne 0 -or $got -isnot [array] -or @($got).Count -ne $case.Want) { $failed++; "FAIL schema-$($case.Name): exit $($p.ExitCode), answer $(Get-Content -LiteralPath $ans -Raw -ErrorAction SilentlyContinue)" }
}

Remove-Item Env:AGY_SETTINGS
Remove-Item -LiteralPath $root -Recurse -Force
if ($failed) { exit 1 }
'agy errors: all passed'
