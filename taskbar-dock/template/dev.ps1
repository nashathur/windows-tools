# Build, test and optionally start this widget through the shared dev helper.
& "$PSScriptRoot\..\taskbar-dock\scripts\dev.ps1" -Crate $PSScriptRoot -Name "__NAME__" -Deploy "$PSScriptRoot\target\release\__NAME__.exe" -Log "$env:APPDATA\TaskbarDock\__NAME__.log" @args
