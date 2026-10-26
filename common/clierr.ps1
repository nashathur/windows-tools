# One-line failures for scripts Claude runs. stderr gets
#   ERROR <tool> <CODE>: <what> | fix: <hint> | log: <path>
# and the exit is non-zero (2 for BAD_ARG); the command line, stack and external tools' output go to
# a log under C:\Tools\logs\<tool>\ (last 50 kept). Warnings: WARN <tool> <CODE>: <what> | fix: <hint>.
#
#   param(...)                                   # stays first
#   . "$PSScriptRoot\..\common\clierr.ps1"       # path relative to the script
#   trap { Exit-Cli 'status' $_ }                # script scope: catches every terminating error
#   if (-not (Test-Path $exe)) { Stop-Cli NOT_FOUND "$exe missing" 'build it with dev.ps1' }
#   $r = Invoke-CliTool cargo @('build', '--release') -Fix 'fix the first compiler error'
#
# The trap sits in the script itself (a wrapper scriptblock would move the body into a child scope
# and split its $script: variables). Codes: NOT_FOUND, LOCKED, BAD_ARG, BAD_INPUT, TOOL_FAILED,
# TIMEOUT, INTERRUPTED, INTERNAL, plus tool-specific ones. A PowerShell parameter-binding error
# happens before the script runs, so it keeps PowerShell's own message.

$script:CliLogRoot = if ($env:CLIERR_LOG_ROOT) { $env:CLIERR_LOG_ROOT } else { 'C:\Tools\logs' }
$script:CliKeep = 50
$script:CliMaxLog = 200000
$script:CliTail = 30

function Stop-Cli {
    # Throws a failure carrying its code and fix; Invoke-Cli turns it into the ERROR line.
    # -Detail (text) and -ToolRun (an Invoke-CliTool result) go to the log only.
    param([Parameter(Mandatory)][string]$Code, [Parameter(Mandatory)][string]$What,
          [Parameter(Mandatory)][string]$Fix, [string]$Detail, $ToolRun)
    $e = New-Object System.Exception $What
    $e.Data['clierr'] = @{ Code = $Code; What = $What; Fix = $Fix; Detail = $Detail; ToolRun = $ToolRun }
    throw $e
}

function Write-CliWarn {
    param([string]$Tool, [string]$Code, [string]$What, [string]$Fix)
    $What = ("$What" -replace '\s*[\r\n]+\s*', ' ').Trim()
    $Fix = ("$Fix" -replace '\s*[\r\n]+\s*', ' ').Trim()
    [Console]::Error.WriteLine("WARN $Tool ${Code}: $What | fix: $Fix")
}

function ConvertTo-CliText([string]$Text) {
    # Without colour codes, keeping only the last state of a progress line redrawn with \r.
    $Text = [regex]::Replace("$Text", '\x1b\[[0-9;?]*[ -/]*[@-~]', '')
    (($Text -split "`n") | ForEach-Object { ($_.TrimEnd("`r") -split "`r")[-1] }) -join "`n"
}

function Get-RelevantLine([string]$Text) {
    # The first line mentioning error/fatal/failed/exception, else the last non-empty one.
    $lines = @((ConvertTo-CliText $Text) -split "`n" | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    if (-not $lines) { return '(no output)' }
    $hit = $lines | Where-Object { $_ -match 'error|fatal|failed|exception' } | Select-Object -First 1
    $line = if ($hit) { $hit } else { $lines[-1] }
    if ($line.Length -gt 300) { $line.Substring(0, 300) } else { $line }
}

function Get-CargoError([string]$Text) {
    # cargo/rustc output -> "error[E0308]: mismatched types --> src\main.rs:10:5", or $null.
    $lines = (ConvertTo-CliText $Text) -split "`n"
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^error(\[E\d+\])?: ' -and $lines[$i] -notmatch '^error: (could not compile|aborting)') {
            $at = $lines[($i + 1)..([Math]::Min($i + 4, $lines.Count - 1))] | Where-Object { $_ -match '^\s*--> ' } |
                Select-Object -First 1
            if ($at) { return "$($lines[$i].Trim()) $($at.Trim())" }
            return $lines[$i].Trim()
        }
    }
    $null
}

function Join-CliArgs([string[]]$ArgumentList) {
    # One Windows command line from separate arguments (CommandLineToArgvW quoting rules).
    ($ArgumentList | ForEach-Object {
        if ($_ -ne '' -and $_ -notmatch '[\s"]') { $_ }
        else { '"' + ([regex]::Replace($_, '(\\*)"', '$1$1\"') -replace '(\\+)$', '$1$1') + '"' }
    }) -join ' '
}

function Invoke-CliTool {
    # Runs a native program with captured output (no PowerShell quote stripping). A non-zero exit
    # throws TOOL_FAILED quoting one stderr line (cargo: the first compiler error); a timeout throws
    # TIMEOUT. Returns @{ ExitCode; StdOut; StdErr; Command }.
    param([Parameter(Mandatory)][string]$Exe, [string[]]$ArgumentList = @(), [string]$Name,
          [string]$Fix, [int]$TimeoutSec = 0, [string]$WorkingDirectory = (Get-Location).Path)
    if (-not $Name) { $Name = [IO.Path]::GetFileNameWithoutExtension($Exe) }
    $path = (Get-Command $Exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source
    if (-not $path) { Stop-Cli NOT_FOUND "$Name not found ($Exe)" "install $Name or fix its path" }
    $psi = New-Object System.Diagnostics.ProcessStartInfo $path
    $psi.Arguments = Join-CliArgs $ArgumentList
    $psi.WorkingDirectory = $WorkingDirectory
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    $out = $p.StandardOutput.ReadToEndAsync()
    $err = $p.StandardError.ReadToEndAsync()
    $ms = if ($TimeoutSec -gt 0) { $TimeoutSec * 1000 } else { -1 }
    $run = @{ Command = "$path $($psi.Arguments)"; ExitCode = $null; StdOut = ''; StdErr = '' }
    if (-not $p.WaitForExit($ms)) {
        try { $p.Kill() } catch {}
        $p.WaitForExit()
        $run.ExitCode = 'timeout'; $run.StdOut = $out.Result; $run.StdErr = $err.Result
        Stop-Cli TIMEOUT "$Name ran over $TimeoutSec s" $(if ($Fix) { $Fix } else { 'raise the timeout or shrink the input' }) -ToolRun $run
    }
    $p.WaitForExit()
    $run.ExitCode = $p.ExitCode; $run.StdOut = $out.Result; $run.StdErr = $err.Result
    if ($p.ExitCode -ne 0) {
        $both = "$($run.StdErr)`n$($run.StdOut)"
        $line = Get-CargoError $both
        if (-not $line) { $line = Get-RelevantLine $(if ($run.StdErr.Trim()) { $run.StdErr } else { $run.StdOut }) }
        Stop-Cli TOOL_FAILED "$Name exit $($p.ExitCode): $line" $(if ($Fix) { $Fix } else { "see the log's tool stderr section" }) -ToolRun $run
    }
    $run
}

function Write-CliLog {
    # Writes one log, keeps the newest $CliKeep for the tool, returns its path (or a note).
    param([string]$Tool, [string]$Code, [string]$What, [string]$Fix, [string]$Detail, $ToolRun, [string]$Stack)
    $folder = Join-Path $script:CliLogRoot $Tool
    $path = Join-Path $folder ("{0}-{1}.log" -f (Get-Date -Format 'yyyyMMdd-HHmmss'), $PID)
    $sb = New-Object System.Text.StringBuilder
    $section = { param($title, $body) [void]$sb.Append("== $title ==`n$("$body".TrimEnd())`n`n") }
    & $section 'command' "$([Environment]::CommandLine)`ncwd $((Get-Location).Path)"
    & $section 'error' "${Code}: $What`nfix: $Fix"
    if ($Detail) { & $section 'detail' (ConvertTo-CliText $Detail) }
    if ($ToolRun) {
        & $section 'tool' "$($ToolRun.Command)`nexit $($ToolRun.ExitCode)"
        foreach ($stream in 'StdErr', 'StdOut') {   # stdout only when stderr is empty
            $tail = @((ConvertTo-CliText "$($ToolRun.$stream)").TrimEnd() -split "`n" | Select-Object -Last $script:CliTail) -join "`n"
            if ($tail.Trim() -or $stream -eq 'StdOut') {
                & $section "tool $($stream.ToLower()) (last $($script:CliTail))" $(if ($tail.Trim()) { $tail } else { '(empty)' })
                break
            }
        }
    }
    if ($Stack) { & $section 'traceback' $Stack }
    $text = $sb.ToString()
    if ($text.Length -gt $script:CliMaxLog) {
        $half = [int]($script:CliMaxLog / 2)
        $text = $text.Substring(0, $half) + "`n... $($text.Length - $script:CliMaxLog) characters cut ...`n" + $text.Substring($text.Length - $half)
    }
    try {
        [void](New-Item -ItemType Directory -Force -Path $folder -ErrorAction Stop)
        for ($n = 2; ; $n++) {   # exclusive create: two failures in one second keep both logs
            try {
                $fs = New-Object IO.FileStream $path, ([IO.FileMode]::CreateNew), ([IO.FileAccess]::Write)
                break
            } catch [System.IO.IOException] {
                if ($n -ge 100 -or -not (Test-Path -LiteralPath $path)) { throw }
                $path = Join-Path $folder ("{0}-{1}-{2}.log" -f (Get-Date -Format 'yyyyMMdd-HHmmss'), $PID, $n)
            }
        }
        $bytes = (New-Object Text.UTF8Encoding $false).GetBytes($text)
        try { $fs.Write($bytes, 0, $bytes.Length) } finally { $fs.Dispose() }
    } catch { return "(log not written: $($_.Exception.Message))" }
    Get-ChildItem $folder -Filter *.log -ErrorAction SilentlyContinue | Sort-Object Name | Select-Object -SkipLast $script:CliKeep |
        Remove-Item -Force -ErrorAction SilentlyContinue
    $path
}

function Exit-Cli {
    # Called from the script's `trap`: prints the ERROR line for the error record, writes its log,
    # and exits the script 1 (2 for BAD_ARG).
    param([Parameter(Mandatory)][string]$Tool, [Parameter(Mandatory)]$ErrorRecord)
    $e = $ErrorRecord
    $ex = $e.Exception
    while ($ex -is [System.Management.Automation.ActionPreferenceStopException] -and $ex.InnerException) { $ex = $ex.InnerException }
    $f = $ex.Data['clierr']
    $stack = $null
    if ($f) { $code = $f.Code; $what = $f.What; $fix = $f.Fix }
    else {
        $stack = "$($e.InvocationInfo.PositionMessage)`n$($e.ScriptStackTrace)`n$($ex.ToString())"
        $where = if ($e.InvocationInfo.ScriptName) { " at $(Split-Path $e.InvocationInfo.ScriptName -Leaf):$($e.InvocationInfo.ScriptLineNumber)" } else { '' }
        switch -Regex ($ex.GetType().FullName) {
            'ItemNotFound|FileNotFound|DirectoryNotFound|CommandNotFound' { $code = 'NOT_FOUND'; $fix = 'check the path or name'; break }
            'UnauthorizedAccess' { $code = 'LOCKED'; $fix = 'close the program holding it, or run elevated'; break }
            'ParameterBinding' { $code = 'BAD_ARG'; $fix = 'check the parameters (param block at the top of the script)'; break }
            'PipelineStopped' { $code = 'INTERRUPTED'; $fix = 'rerun'; break }
            default {
                if ($ex.Message -match 'being used by another process') { $code = 'LOCKED'; $fix = 'close the program holding it, then rerun' }
                elseif ($ex.GetType() -eq [System.Management.Automation.RuntimeException] -and $ex.WasThrownFromThrowStatement) {
                    $code = 'BAD_INPUT'; $fix = 'see the message'   # a plain `throw 'text'`
                }
                else { $code = 'INTERNAL'; $fix = 'a bug: read the log''s traceback' }
            }
        }
        $what = ("$($ex.GetType().Name): $($ex.Message)$where" -replace '\s*\r?\n\s*', ' ')
        if ($code -ne 'INTERNAL') { $what = ($ex.Message -replace '\s*\r?\n\s*', ' ') }
        if ($what.Length -gt 300) { $what = $what.Substring(0, 300) }
    }
    $log = Write-CliLog -Tool $Tool -Code $code -What $what -Fix $fix -Detail $f.Detail -ToolRun $f.ToolRun -Stack $stack
    $what = ("$what" -replace '\s*[\r\n]+\s*', ' ').Trim()
    $fix = ("$fix" -replace '\s*[\r\n]+\s*', ' ').Trim()
    [Console]::Error.WriteLine("ERROR $Tool ${code}: $what | fix: $fix | log: $log")
    exit $(if ($code -eq 'BAD_ARG') { 2 } else { 1 })
}
