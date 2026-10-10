# Build, test and restart the weather widget (it runs straight from target\release). Switches: -NoTest -NoRestart -Lib -Start -Fast.
& "$PSScriptRoot\..\taskbar-dock\scripts\dev.ps1" -Crate $PSScriptRoot -Name taskbar-weather -Deploy "$PSScriptRoot\target\release\taskbar-weather.exe" -Log "$env:APPDATA\TaskbarDock\taskbar-weather.log" @args
