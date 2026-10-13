# Compares live-check PNGs with the saved baseline using LockBits.
# Called by run.ps1 -Compare; example: Compare-LiveImages -CurrentDirectory .\run -BaselineDirectory .\baseline.
Add-Type -AssemblyName System.Drawing
function Compare-LiveImages([string]$CurrentDirectory, [string]$BaselineDirectory) {
    $results = New-Object System.Collections.ArrayList
    $currentFiles = @(Get-ChildItem -LiteralPath $CurrentDirectory -Filter '*.png' -File)
    $baselineFiles = @()
    if (Test-Path -LiteralPath $BaselineDirectory) { $baselineFiles = @(Get-ChildItem -LiteralPath $BaselineDirectory -Filter '*.png' -File) }
    foreach ($file in $currentFiles) {
        $baselinePath = Join-Path $BaselineDirectory $file.Name
        if (-not (Test-Path -LiteralPath $baselinePath)) {
            [void]$results.Add([pscustomobject]@{ Name = $file.Name; Percent = $null; Detail = 'only in current run' })
            continue
        }
        $oldImage = [Drawing.Image]::FromFile($baselinePath)
        $newImage = [Drawing.Image]::FromFile($file.FullName)
        try {
            if ($oldImage.Width -ne $newImage.Width -or $oldImage.Height -ne $newImage.Height) {
                $detail = "size changed $($oldImage.Width)x$($oldImage.Height) -> $($newImage.Width)x$($newImage.Height)"
                [void]$results.Add([pscustomobject]@{ Name = $file.Name; Percent = $null; Detail = $detail })
                continue
            }
            $rect = New-Object Drawing.Rectangle 0, 0, $newImage.Width, $newImage.Height
            $old32 = New-Object Drawing.Bitmap $oldImage.Width, $oldImage.Height, ([Drawing.Imaging.PixelFormat]::Format32bppArgb)
            $new32 = New-Object Drawing.Bitmap $newImage.Width, $newImage.Height, ([Drawing.Imaging.PixelFormat]::Format32bppArgb)
            $g = [Drawing.Graphics]::FromImage($old32); $g.DrawImageUnscaled($oldImage, 0, 0); $g.Dispose()
            $g = [Drawing.Graphics]::FromImage($new32); $g.DrawImageUnscaled($newImage, 0, 0); $g.Dispose()
            $oldData = $old32.LockBits($rect, [Drawing.Imaging.ImageLockMode]::ReadOnly, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
            $newData = $new32.LockBits($rect, [Drawing.Imaging.ImageLockMode]::ReadWrite, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
            $changed = 0
            try {
                $oldRow = New-Object byte[] ($newImage.Width * 4)
                $newRow = New-Object byte[] ($newImage.Width * 4)
                for ($y = 0; $y -lt $newImage.Height; $y++) {
                    [Runtime.InteropServices.Marshal]::Copy([IntPtr]::Add($oldData.Scan0, $y * $oldData.Stride), $oldRow, 0, $oldRow.Length)
                    [Runtime.InteropServices.Marshal]::Copy([IntPtr]::Add($newData.Scan0, $y * $newData.Stride), $newRow, 0, $newRow.Length)
                    for ($x = 0; $x -lt $newImage.Width; $x++) {
                        $i = $x * 4
                        if ([Math]::Abs([int]$oldRow[$i] - [int]$newRow[$i]) -gt 24 -or
                            [Math]::Abs([int]$oldRow[$i + 1] - [int]$newRow[$i + 1]) -gt 24 -or
                            [Math]::Abs([int]$oldRow[$i + 2] - [int]$newRow[$i + 2]) -gt 24 -or
                            [Math]::Abs([int]$oldRow[$i + 3] - [int]$newRow[$i + 3]) -gt 24) {
                            $changed++
                            $newRow[$i] = 255; $newRow[$i + 1] = 0; $newRow[$i + 2] = 255; $newRow[$i + 3] = 255
                        }
                    }
                    [Runtime.InteropServices.Marshal]::Copy($newRow, 0, [IntPtr]::Add($newData.Scan0, $y * $newData.Stride), $newRow.Length)
                }
            } finally {
                $new32.UnlockBits($newData); $old32.UnlockBits($oldData)
            }
            $percent = 100.0 * $changed / ($newImage.Width * $newImage.Height)
            $detail = 'changed {0:N1} %' -f $percent
            if ($percent -gt 0.5) { $new32.Save((Join-Path $CurrentDirectory ($file.BaseName + '.diff.png')), [Drawing.Imaging.ImageFormat]::Png) }
            [void]$results.Add([pscustomobject]@{ Name = $file.Name; Percent = $percent; Detail = $detail })
            $old32.Dispose(); $new32.Dispose()
        } finally { $oldImage.Dispose(); $newImage.Dispose() }
    }
    foreach ($file in $baselineFiles) {
        if (-not (Test-Path -LiteralPath (Join-Path $CurrentDirectory $file.Name))) {
            [void]$results.Add([pscustomobject]@{ Name = $file.Name; Percent = $null; Detail = 'only in baseline' })
        }
    }
    @($results)
}
