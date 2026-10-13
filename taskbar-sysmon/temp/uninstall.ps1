$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run uninstall.ps1 from an elevated PowerShell window.'
}

$taskName = 'Taskbar sysmon temp'
if (Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue) {
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
}
Get-Process -Name 'sysmon-temp' -ErrorAction SilentlyContinue | Stop-Process -Force
Remove-Item -LiteralPath (Join-Path $env:ProgramFiles 'TaskbarSysmon') -Recurse -Force -ErrorAction SilentlyContinue
