$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) "dock-errors-$PID"
[void](New-Item -ItemType Directory -Force $root)
$env:CLIERR_LOG_ROOT = Join-Path $root 'logs'
[void](New-Item -ItemType Directory -Force $env:CLIERR_LOG_ROOT)
$scripts = $PSScriptRoot | Split-Path -Parent
$repo = $scripts | Split-Path -Parent | Split-Path -Parent
$failed = 0

function Run-Case([string]$Name, [string]$Target, [string[]]$Arguments, [int]$Exit, [string]$Tool, [string]$Code, [string]$Contains) {
    $err = Join-Path $root "$Name.err"
    $out = Join-Path $root "$Name.out"
    $args = @('-NoProfile','-ExecutionPolicy','Bypass','-File',$Target) + $Arguments
    $p = Start-Process powershell.exe -ArgumentList $args -NoNewWindow -Wait -PassThru -RedirectStandardError $err -RedirectStandardOutput $out
    $stderr = [IO.File]::ReadAllText($err)
    $lines = @($stderr -split "`r?`n" | Where-Object { $_ })
    $problem = $null
    if ($p.ExitCode -ne $Exit) { $problem = "exit $($p.ExitCode), expected $Exit" }
    elseif ($lines.Count -ne 1 -or $lines[0] -notlike "ERROR ${Tool} ${Code}: *") { $problem = "stderr is not one ERROR ${Tool} ${Code} line: $stderr" }
    elseif (-not $lines[0].Contains($Contains)) { $problem = "line lacks '$Contains'" }
    else {
        $log = ($lines[0] -split '\| log: ')[-1]
        if (-not (Test-Path -LiteralPath $log)) { $problem = "log missing: $log" }
    }
    if ($problem) { $script:failed++; "FAIL ${Name}: $problem" }
}

Run-Case 'build_all_bad_only' (Join-Path $scripts 'build_all.ps1') @('-Only','not-a-widget') 2 'build_all' 'BAD_ARG' 'unknown -Only name'
Run-Case 'new_widget_section' (Join-Path $scripts 'new_widget.ps1') @('-Name','taskbar-clierr-test','-Section','wrong','-Title','Sample','-NoRegister') 2 'new_widget' 'BAD_ARG' 'Section must equal Name'

$crate = Join-Path $root 'failtest'
[void](New-Item -ItemType Directory -Force (Join-Path $crate 'src'))
[IO.File]::WriteAllText((Join-Path $crate 'Cargo.toml'), "[package]`nname = `"failtest`"`nversion = `"0.1.0`"`nedition = `"2021`"`n`n[workspace]`n`n[profile.fast]`ninherits = `"release`"`n")
[IO.File]::WriteAllText((Join-Path $crate 'src\main.rs'), "fn main() {}`n#[cfg(test)]`nmod tests {`n    #[test]`n    fn passes() {}`n    #[test]`n    fn breaks() { assert_eq!(1, 2); }`n}`n")
Run-Case 'dev_tests_failed' (Join-Path $scripts 'dev.ps1') @('-Crate',$crate,'-Name','failtest','-Deploy',(Join-Path $crate 'failtest.exe'),'-NoRestart','-Fast') 1 'dev' 'TESTS_FAILED' '1 failed, first: tests::breaks'

$identity =[Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    $sysmonTemp = Join-Path $repo 'taskbar-sysmon\temp'
    Run-Case 'install_elevation' (Join-Path $sysmonTemp 'install.ps1') @() 1 'install' 'NOT_ELEVATED' 'rerun from an elevated PowerShell'
    Run-Case 'uninstall_elevation' (Join-Path $sysmonTemp 'uninstall.ps1') @() 1 'uninstall' 'NOT_ELEVATED' 'rerun from an elevated PowerShell'
}

Remove-Item -LiteralPath $root -Recurse -Force
if ($failed) { exit 1 }
