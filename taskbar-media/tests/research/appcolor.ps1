# Dominant colour of Disney+'s site icon and of local desktop app icons.
. "$PSScriptRoot\iconcolor.ps1" -LibOnly
$ua = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:131.0) Gecko/20100101 Firefox/131.0'
$OutDir = "$PSScriptRoot\icons"

$n = 0
foreach ($u in (IconUrls 'https://www.disneyplus.com/')) {
    if ($u -match '\.svg') { continue }
    $f = "$OutDir\disney_$n.img"; $n++
    try { Invoke-WebRequest -UseBasicParsing -Uri $u -UserAgent $ua -TimeoutSec 20 -OutFile $f; "Disney+  $(Dominant $f)  from $u" } catch {}
}

$apps = [ordered]@{
    'Microsoft Edge' = "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"
    'Windows Media Player Legacy' = "$env:ProgramFiles\Windows Media Player\wmplayer.exe"
    'Firefox' = "$env:ProgramFiles\Mozilla Firefox\firefox.exe"
}
foreach ($name in $apps.Keys) {
    $p = $apps[$name]
    if (-not (Test-Path $p)) { "$name  not installed"; continue }
    $f = "$OutDir\" + ($name -replace '\W', '_') + '.png'
    $bmp = ([System.Drawing.Icon]::ExtractAssociatedIcon($p)).ToBitmap()
    $bmp.Save($f, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
    "$name  $(Dominant $f)  (local exe icon)"
}

$pk = Get-AppxPackage -Name Microsoft.ZuneMusic -ErrorAction SilentlyContinue
if ($pk) {
    $logo = Get-ChildItem $pk.InstallLocation -Recurse -Filter '*.png' -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match 'AppList.*targetsize-256' } | Select-Object -First 1
    if ($logo) { "Media Player (new)  $(Dominant $logo.FullName)  ($($logo.Name))" }
} else { "Media Player (new)  not installed" }
