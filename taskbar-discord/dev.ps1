# Build, test and restart the Discord voice widget (it runs straight from target\release). Switches: -NoTest -NoRestart -Lib -Start -Fast.
& "$PSScriptRoot\..\taskbar-dock\scripts\dev.ps1" -Crate $PSScriptRoot -Name taskbar-discord -Deploy "$PSScriptRoot\target\release\taskbar-discord.exe" -Log "$env:APPDATA\TaskbarDock\taskbar-discord.log" @args
