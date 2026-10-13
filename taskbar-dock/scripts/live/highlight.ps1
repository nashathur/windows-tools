# The Highlight box rows (Block's, every widget), changed through the app's own controls:
# Automatic / Chosen colour (a swatch, strength 3) / Off, with strip screenshots; the Colour row
# shows only in Chosen colour mode; the custom-colour pop-up opens and cancels (checked once).
# Puts the three settings back.
param([string[]]$Only = @())
. "$PSScriptRoot\common.ps1"
$keys = 'highlight', 'highlight_color', 'highlight_strength'
$started = -not (App-Running)
Check (Start-App) 'highlight/app-starts'
$dialogChecked = $false
foreach ($w in $(if ($Only) { $Only } else { $Widgets })) {
    $was = @{}; foreach ($k in $keys) { $was[$k] = Snapshot $w $k }
    $colourShown = { $r = App-Row "$w.highlight_color"; if ($null -eq $r -or $null -eq $r.PSObject.Properties['shown']) { $null } else { [bool]$r.shown } }
    try {
        [void](Ctl settings page $w); Start-Sleep -Milliseconds 600
        [void](Ctl settings row "$w.highlight" set auto); Start-Sleep -Milliseconds 600
        [void](Shot-Strip $w "highlight_${w}_auto")
        $s = & $colourShown
        if ($null -eq $s) { Note "highlight/$w/colour-row" 'the app reports no "shown" field (older app)' } else { Check (-not $s) "highlight/$w/colour-row-hidden-in-auto" }

        [void](Ctl settings row "$w.highlight" set custom); Start-Sleep -Milliseconds 600
        $s = & $colourShown
        if ($null -ne $s) { Check $s "highlight/$w/colour-row-shown-in-custom" }
        $sw = Ctl settings click "swatch:$w.highlight_color:E81123"
        if ($sw -and $sw.ok) { Pass "highlight/$w/swatch-click" } else { Note "highlight/$w/swatch-click" 'no swatch click (older app): set by row'; [void](Ctl settings row "$w.highlight_color" set '#E81123') }
        [void](Ctl settings row "$w.highlight_strength" set 3); Start-Sleep -Milliseconds 800
        $v = (Ctl $w get).value
        Check ($v.highlight -eq 'custom' -and "$($v.highlight_color)".TrimStart('#') -eq 'E81123' -and [double]$v.highlight_strength -eq 3) "highlight/$w/custom-saved" "$($v.highlight_color) x$($v.highlight_strength)"
        [void](Shot-Strip $w "highlight_${w}_red")

        if (-not $dialogChecked) {
            $dialogChecked = $true
            [void](Shot-App "highlight_${w}_app_custom")
            $cu = Ctl settings click "custom:$w.highlight_color"
            if ($cu -and $cu.ok) {
                Start-Sleep -Milliseconds 800
                $d = (Ctl settings state).dialog
                Check ($null -ne $d) 'highlight/custom-dialog-opens' "$($d | ConvertTo-Json -Compress)"
                [void](Shot-App "highlight_${w}_dialog")
                [void](Ctl settings click 'dialog:cancel'); Start-Sleep -Milliseconds 500
                Check ($null -eq (Ctl settings state).dialog) 'highlight/custom-dialog-cancels'
            } else { Note 'highlight/custom-dialog' 'no custom: click (older app)' }
        }

        [void](Ctl settings row "$w.highlight" set off); Start-Sleep -Milliseconds 600
        Check ((Ctl $w get highlight).value -eq 'off') "highlight/$w/off-saved"
        [void](Shot-Strip $w "highlight_${w}_off")
    } finally {
        foreach ($k in $keys) { [void](Ctl $w set $k ($was[$k] | ConvertTo-Json -Compress)) }
    }
    $ok = $true; foreach ($k in $keys) { if ("$((Ctl $w get $k).value)" -ne "$($was[$k])") { $ok = $false } }
    Check $ok "highlight/$w/restored"
}
if ($started) { Stop-App }
Note 'highlight/look' 'compare highlight_<widget>_auto / _red / _off.png (a hidden strip has none)'
