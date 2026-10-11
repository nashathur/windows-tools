# Build, test and restart the media widget. Switches: -NoTest -NoRestart -Lib -Start -Fast -NoSmoke (see the shared script).
& "$PSScriptRoot\..\taskbar-dock\scripts\dev.ps1" -Crate $PSScriptRoot -Name taskbar-media -Deploy "$PSScriptRoot\taskbar-media.exe" -Log "$PSScriptRoot\taskbar-media.log" @args
