# Build, test and restart the bus widget (built in rust\, runs from this folder). Switches: -NoTest -NoRestart -Lib -Start -Fast -NoSmoke.
& "$PSScriptRoot\..\taskbar-dock\scripts\dev.ps1" -Crate "$PSScriptRoot\rust" -Name buswidget -Deploy "$PSScriptRoot\buswidget.exe" -Log "$env:APPDATA\TaskbarDock\buswidget.log" @args
