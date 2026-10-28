$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) "rules-hook-$PID"
[void](New-Item -ItemType Directory -Force $root)
$hook = Join-Path $PSScriptRoot '..\rules-hook.ps1'
$cwd = 'C:\Tools\codex\runs\job1'
$failed = 0

# Feeds one PreToolUse input to the hook: blocked means exit 2 with a reason on stderr, allowed means exit 0.
function Run-Case([string]$Command, [bool]$Blocked) {
    $json = @{ hook_event_name = 'PreToolUse'; tool_name = 'Bash'; cwd = $cwd; tool_input = @{ command = $Command } } | ConvertTo-Json -Compress
    $stdin = Join-Path $root 'in.json'
    $err = Join-Path $root 'err.txt'
    [IO.File]::WriteAllText($stdin, $json, (New-Object Text.UTF8Encoding $false))
    $p = Start-Process powershell.exe -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File',$hook) -NoNewWindow -Wait -PassThru `
        -RedirectStandardInput $stdin -RedirectStandardError $err -RedirectStandardOutput (Join-Path $root 'out.txt')
    $stderr = [IO.File]::ReadAllText($err).Trim()
    $problem = $null
    if ($Blocked -and ($p.ExitCode -ne 2 -or $stderr -notlike "Blocked by the user's rules: *")) { $problem = "not blocked (exit $($p.ExitCode)): $stderr" }
    elseif (-not $Blocked -and $p.ExitCode -ne 0) { $problem = "blocked (exit $($p.ExitCode)): $stderr" }
    if ($problem) { $script:failed++; "FAIL [$Command]: $problem" }
}

$forbidden = @(
    'taskkill /F /IM python.exe',
    'Get-Process codex | Stop-Process -Force',
    'Stop-Process -Name node',
    'C:\Users\thurs\AppData\Local\Programs\Python\Python313\python.exe x.py',
    'C:\Users\thurs\miniconda3\envs\dev\python.exe x.py',
    'python x.py',
    'cd src; pip install requests',
    'conda install numpy',
    'conda run -n base python -m pip install x',
    'net user sandbox /active:yes',
    'reg add HKLM\SOFTWARE\X /v Y /d 1',
    'Set-MpPreference -DisableRealtimeMonitoring $true',
    'Remove-Item -Recurse -Force C:\Tools\pdf2md',
    'rm -rf ../other',
    'cmd /c rd /s /q C:\Users\thurs',
    'git push origin main',
    'git -C C:\Tools reset --hard HEAD',
    'git stash'
)
$allowed = @(
    'Get-ChildItem -Force',
    'taskkill /PID 1234 /F',
    'conda run -n dev python -m pytest -q',
    'C:\Users\thurs\miniconda3\Scripts\conda.exe run -n dev python x.py',
    'conda install -n dev numpy',
    'Remove-Item -Recurse -Force .\build 2>$null',
    'Remove-Item -Recurse C:\Tools\codex\runs\job1\out',
    'git status; git diff --stat',
    'git commit -m "fix push and stash notes"',
    '"C:\WINDOWS\System32\WindowsPowerShell\v1.0\powershell.exe" -Command "Set-Content -LiteralPath hello.txt -Value hi"'
)
foreach ($c in $forbidden) { Run-Case $c $true }
foreach ($c in $allowed) { Run-Case $c $false }

Remove-Item -LiteralPath $root -Recurse -Force
if ($failed) { exit 1 }
