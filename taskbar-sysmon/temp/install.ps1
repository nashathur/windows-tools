. "$PSScriptRoot\..\..\common\clierr.ps1"
$ErrorActionPreference = 'Stop'
trap { Exit-Cli 'install' $_ }
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Stop-Cli NOT_ELEVATED 'Run install.ps1 from an elevated PowerShell window.' 'rerun from an elevated PowerShell'
}

$taskName = 'Taskbar sysmon temp'
$source = Join-Path $PSScriptRoot 'target\release\sysmon-temp.exe'
$installDir = Join-Path $env:ProgramFiles 'TaskbarSysmon'
$exe = Join-Path $installDir 'sysmon-temp.exe'

if (Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue) {
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
}
Get-Process -Name 'sysmon-temp' -ErrorAction SilentlyContinue | Stop-Process -Force
New-Item -ItemType Directory -Path $installDir -Force | Out-Null
Copy-Item -LiteralPath $source -Destination $exe -Force

$action = New-ScheduledTaskAction -Execute $exe
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $identity.Name
$principalTask = New-ScheduledTaskPrincipal -UserId $identity.Name -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Principal $principalTask -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName $taskName
Start-Sleep -Seconds 2
# a windowless exe: & would not wait for it; it prints through the parent console
$check = Start-Process -FilePath $exe -ArgumentList '--once' -NoNewWindow -Wait -PassThru
"sysmon-temp --once exit code: $($check.ExitCode)"
