$ErrorActionPreference = 'Stop'
# A copy of ask.ps1 in a temp tree, so its runs\ (job records, lockout marker) stays out of the real one.
$root = Join-Path ([IO.Path]::GetTempPath()) "ask-lockout-$PID"
foreach ($d in 'codex', 'common', 'logs', 'work') { [void](New-Item -ItemType Directory -Force (Join-Path $root $d)) }
Copy-Item (Join-Path $PSScriptRoot '..\ask.ps1') (Join-Path $root 'codex')
Copy-Item (Join-Path $PSScriptRoot '..\..\common\clierr.ps1') (Join-Path $root 'common')
$env:CLIERR_LOG_ROOT = Join-Path $root 'logs'
$target = Join-Path $root 'codex\ask.ps1'
$marker = Join-Path $root 'codex\runs\sandbox-locked.txt'
$ran = Join-Path $root 'ran.txt'
$failed = 0

# Fake codex: logs the sandbox's lockout line, then hangs, as a real run retrying would.
# The code is split so this file never matches the launcher's pattern itself.
$fake = Join-Path $root 'codex.cmd'
# The real line sits inside the command block (between the command and "exited -1"), as Codex writes it.
$lockText = 'ERROR codex_core::tools::router: error=exec_command failed: CreateProcess { message: Rejected(Failed to create unified exec process: CreateProcessWithLogonW failed: ' + '19' + '09) }'
$lockLine = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ss.ffffffZ') + ' ' + $lockText
[IO.File]::WriteAllText($fake, "@echo off`r`necho ran>> `"$ran`"`r`n1>&2 echo workdir: x`r`n1>&2 echo exec`r`n1>&2 echo powershell -Command Get-Location`r`n1>&2 echo $lockLine`r`n1>&2 echo  exited -1 in 0ms:`r`nping -n 60 127.0.0.1 >nul`r`n")
$env:ASK_CODEX_EXE = $fake
$prompt = Join-Path $root 'p.md'
[IO.File]::WriteAllText($prompt, 'test')

function Run-Ask([string]$Name) {
    $err = Join-Path $root "$Name.err"
    $args = @('-NoProfile','-ExecutionPolicy','Bypass','-File',$target,'-Role','quick','-Sandbox','workspace-write',
              '-Dir',(Join-Path $root 'work'),'-PromptFile',$prompt)
    $savedConsult = $env:CODEX_CONSULT
    Remove-Item Env:CODEX_CONSULT -ErrorAction SilentlyContinue
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process powershell.exe -ArgumentList $args -NoNewWindow -Wait -PassThru -RedirectStandardError $err -RedirectStandardOutput (Join-Path $root "$Name.out")
    if ($null -ne $savedConsult) { $env:CODEX_CONSULT = $savedConsult }
    [pscustomobject]@{ Exit = $p.ExitCode; Seconds = $sw.Elapsed.TotalSeconds
                       Lines = @([IO.File]::ReadAllText($err) -split "`r?`n" | Where-Object { $_ }) }
}

# 0. Echoes are no lockout (both happened 2026-10-28): a command quoting the lockout text, and a command
#    printing an old transcript whose Codex log line is stamped before this run.
$quoting = Join-Path $root 'quoting.cmd'
$oldLine = '2026-10-13T13:10:19.499114Z ' + $lockText
[IO.File]::WriteAllText($quoting, "@echo off`r`n1>&2 echo exec`r`n1>&2 echo powershell -Command `"'$lockText' -match `$p`"`r`n1>&2 echo  succeeded in 5ms:`r`n1>&2 echo $oldLine`r`n")
$env:ASK_CODEX_EXE = $quoting
$r = Run-Ask 'quoting'
$env:ASK_CODEX_EXE = $fake
if (($r.Lines -join ' ') -match 'SANDBOX_LOCKED' -or (Test-Path -LiteralPath $marker)) { $failed++; "FAIL quoting: command text was taken for a lockout: $($r.Lines -join ' / ')" }

# 1. The lockout line stops the run: one ERROR line with its log, Codex killed early, marker written.
$r = Run-Ask 'first'
$problem = $null
if ($r.Exit -ne 1) { $problem = "exit $($r.Exit), expected 1" }
elseif ($r.Lines.Count -ne 1 -or $r.Lines[0] -notlike 'ERROR ask SANDBOX_LOCKED: *') { $problem = "stderr is not one ERROR ask SANDBOX_LOCKED line: $($r.Lines -join ' / ')" }
elseif (-not (Test-Path -LiteralPath (($r.Lines[0] -split '\| log: ')[-1]))) { $problem = 'log missing' }
elseif ($r.Seconds -gt 30) { $problem = "took $([int]$r.Seconds)s: Codex was not stopped at the lockout" }
elseif (-not (Test-Path -LiteralPath $marker)) { $problem = 'no lockout marker' }
if ($problem) { $failed++; "FAIL lockout: $problem" }

# 2. While the marker is fresh, the next run refuses to start Codex.
$r = Run-Ask 'second'
$problem = $null
if ($r.Exit -ne 1) { $problem = "exit $($r.Exit), expected 1" }
elseif ($r.Lines.Count -ne 1 -or $r.Lines[0] -notlike 'WARN ask SANDBOX_LOCKED: *| fix: retry after *') { $problem = "stderr is not one WARN ask SANDBOX_LOCKED line: $($r.Lines -join ' / ')" }
elseif (@(Get-Content -LiteralPath $ran).Count -ne 1) { $problem = 'fake codex started again' }
if ($problem) { $failed++; "FAIL marker: $problem" }

Remove-Item Env:ASK_CODEX_EXE   # the fake is deleted with $root; a later ask.ps1 in this shell must find the real one
Remove-Item -LiteralPath $root -Recurse -Force
if ($failed) { exit 1 }
