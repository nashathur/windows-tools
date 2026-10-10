# Download each site's own icon (apple-touch-icon / manifest icon / favicon) and measure its dominant brand colour.
param([string]$OutDir = "$PSScriptRoot\icons", [switch]$LibOnly)
Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force $OutDir | Out-Null
$ua = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:131.0) Gecko/20100101 Firefox/131.0'

function Dominant([string]$path) {
    $bmp = [System.Drawing.Bitmap]::FromFile($path)
    try {
        $step = [math]::Max(1, [int]($bmp.Width / 64))
        $bins = @{}; $total = 0; $colored = 0
        for ($y = 0; $y -lt $bmp.Height; $y += $step) { for ($x = 0; $x -lt $bmp.Width; $x += $step) {
            $p = $bmp.GetPixel($x, $y); if ($p.A -lt 200) { continue }; $total++
            $s = $p.GetSaturation(); $v = [math]::Max($p.R, [math]::Max($p.G, $p.B)) / 255
            if ($s -lt 0.3 -or $v -lt 0.25) { continue }; $colored++
            $b = [int]([math]::Floor($p.GetHue() / 10)) % 36
            if (-not $bins.ContainsKey($b)) { $bins[$b] = @(0.0, 0.0, 0.0, 0.0) }
            $w = $s * $v; $a = $bins[$b]; $a[0] += $w; $a[1] += $p.R * $w; $a[2] += $p.G * $w; $a[3] += $p.B * $w
        } }
        if ($colored -eq 0) { return "no colour (monochrome icon) size=$($bmp.Width)" }
        $top = $bins.GetEnumerator() | Sort-Object { $_.Value[0] } -Descending | Select-Object -First 1
        $a = $top.Value
        '#{0:X2}{1:X2}{2:X2}  coloured={3}% size={4}' -f [int]($a[1] / $a[0]), [int]($a[2] / $a[0]), [int]($a[3] / $a[0]), [int](100 * $colored / [math]::Max(1, $total)), $bmp.Width
    } finally { $bmp.Dispose() }
}

function IconUrls([string]$site) {
    $urls = @()
    try {
        $r = Invoke-WebRequest -UseBasicParsing -Uri $site -UserAgent $ua -TimeoutSec 20
        $base = $r.BaseResponse.ResponseUri
        foreach ($m in [regex]::Matches($r.Content, '<link[^>]+>')) {
            $tag = $m.Value
            if ($tag -match 'rel=["''][^"'']*(apple-touch-icon|icon)[^"'']*["'']' -and $tag -match 'href=["'']([^"'']+)') {
                $urls += ([Uri]::new($base, [System.Net.WebUtility]::HtmlDecode($Matches[1]))).AbsoluteUri }
            if ($tag -match 'rel=["'']manifest["'']' -and $tag -match 'href=["'']([^"'']+)') {
                try { $mu = [Uri]::new($base, $Matches[1]); $mj = (Invoke-WebRequest -UseBasicParsing -Uri $mu -UserAgent $ua -TimeoutSec 20).Content | ConvertFrom-Json
                      foreach ($i in $mj.icons) { $urls = @(([Uri]::new($mu, $i.src)).AbsoluteUri) + $urls } } catch {} }
        }
        $urls += ([Uri]::new($base, '/apple-touch-icon.png')).AbsoluteUri
    } catch { $urls += ([Uri]::new([Uri]$site, '/apple-touch-icon.png')).AbsoluteUri }
    $urls | Select-Object -Unique
}

if ($LibOnly) { return }
$list = [ordered]@{
    'JOOX' = 'https://www.joox.com/'; 'Disney+ Hotstar' = 'https://www.hotstar.com/'; 'Prime Video' = 'https://www.primevideo.com/'
    'Vidio' = 'https://www.vidio.com/'; 'Viu' = 'https://www.viu.com/'; 'WeTV' = 'https://wetv.vip/'; 'iQIYI' = 'https://www.iq.com/'
    'TikTok' = 'https://www.tiktok.com/'; 'Kick' = 'https://kick.com/'; 'Dailymotion' = 'https://www.dailymotion.com/'
}
foreach ($name in $list.Keys) {
    $done = $false; $i = 0
    foreach ($u in (IconUrls $list[$name])) {
        if ($u -match '\.svg(\?|$)') { continue }
        $file = Join-Path $OutDir (($name -replace '[^A-Za-z0-9]', '_') + "_$i" + '.img'); $i++
        try { Invoke-WebRequest -UseBasicParsing -Uri $u -UserAgent $ua -TimeoutSec 20 -OutFile $file
              "{0,-16} {1}`n                 from {2}" -f $name, (Dominant $file), $u; $done = $true; break } catch { }
    }
    if (-not $done) { "{0,-16} no usable icon found" -f $name }
}
