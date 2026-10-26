# Build, test and optionally start this widget through the shared dev helper.
& "$PSScriptRoot\..\taskbar-dock\scripts\dev.ps1" -Crate $PSScriptRoot -Name "taskbar-sysmon" -Deploy "$PSScriptRoot\target\release\taskbar-sysmon.exe" -Log "$env:APPDATA\TaskbarDock\taskbar-sysmon.log" @args
exit $LASTEXITCODE   # pass the shared script's exit code on
