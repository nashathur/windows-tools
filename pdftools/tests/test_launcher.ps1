$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$conda = 'C:\Users\thurs\miniconda3\Scripts\conda.exe'
$oldLogRoot = $env:CLIERR_LOG_ROOT
$oldEncoding = [Console]::OutputEncoding
$oldOutputEncoding = $OutputEncoding
$scratch = Join-Path $PSScriptRoot '_launcher-check'
if (Test-Path -LiteralPath $scratch) { Remove-Item -LiteralPath $scratch -Recurse -Force }
New-Item -ItemType Directory -Path $scratch | Out-Null
try {
    $fixtureDir = Join-Path $scratch ("space {0} 'quoted" -f [char]0x00e9)
    & $conda run -n dev python (Join-Path $PSScriptRoot 'fixtures\p1.py') $fixtureDir
    if ($LASTEXITCODE -ne 0) { throw "fixture builder exited $LASTEXITCODE" }

    $launcherText = Get-Content -LiteralPath (Join-Path $root 'pdf.cmd') -Raw
    $launcherText = $launcherText.Replace('C:\Tools\pdftools', $root)
    $launcher = Join-Path $scratch 'pdf.cmd'
    Set-Content -LiteralPath $launcher -Value $launcherText -Encoding ASCII
    $env:CLIERR_LOG_ROOT = Join-Path $scratch 'logs'

    [Console]::OutputEncoding = [Text.Encoding]::UTF8
    $OutputEncoding = [Text.Encoding]::UTF8
    $plain = Join-Path $fixtureDir 'plain.pdf'
    $output = & $launcher info $plain
    if ($LASTEXITCODE -ne 0 -or $output -notmatch '2 pages') { throw "plain info launcher failed: $LASTEXITCODE $output" }

    $asciiPdf = Join-Path $scratch 'plain.pdf'
    Copy-Item -LiteralPath $plain -Destination $asciiPdf
    $job = @{ version = 1; command = 'info'; input = $asciiPdf } | ConvertTo-Json -Compress
    $output = $job | & $launcher --json --job -
    if ($LASTEXITCODE -ne 0) { throw "--job - exited $LASTEXITCODE" }
    $record = $output | ConvertFrom-Json
    if ($record.type -ne 'report' -or $record.data.pages -ne 2) { throw 'stdin JSON job did not produce the expected report' }

    $unicodeDir = Join-Path $scratch ('input ' + [char]0x00e9)
    New-Item -ItemType Directory -Path $unicodeDir | Out-Null
    $unicodePdf = Join-Path $unicodeDir 'plain.pdf'
    Copy-Item -LiteralPath $plain -Destination $unicodePdf
    $unicodeJob = @{ version = 1; command = 'info'; input = $unicodePdf } | ConvertTo-Json -Compress
    $output = $unicodeJob | & $launcher --json --job -
    if ($LASTEXITCODE -ne 0) { throw "non-ASCII --job - exited $LASTEXITCODE" }
    $record = $output | ConvertFrom-Json
    if ($record.type -ne 'report' -or $record.input -ne $unicodePdf) { throw 'non-ASCII stdin JSON path was not preserved' }

    $specialDir = Join-Path $scratch 'special % ^ & !'
    New-Item -ItemType Directory -Path $specialDir | Out-Null
    $specialPdf = Join-Path $specialDir 'input.pdf'
    Copy-Item -LiteralPath $plain -Destination $specialPdf
    $specialJob = Join-Path $scratch 'special-job.json'
    @{ version = 1; command = 'info'; input = $specialPdf } | ConvertTo-Json -Compress | Set-Content -LiteralPath $specialJob -Encoding UTF8
    $output = & $launcher --json --job $specialJob
    if ($LASTEXITCODE -ne 0 -or ($output | ConvertFrom-Json).data.pages -ne 2) { throw "special path JSON job failed: $LASTEXITCODE" }

    $passwordFile = Join-Path $scratch 'pw.txt'
    Set-Content -LiteralPath $passwordFile -Value 'user-pass' -Encoding ASCII
    $output = & $launcher info (Join-Path $fixtureDir 'encrypted.pdf') --password-file $passwordFile
    if ($LASTEXITCODE -ne 0) { throw "--password-file exited $LASTEXITCODE" }

    $stub = Join-Path $scratch 'stub'
    New-Item -ItemType Directory -Path $stub | Out-Null
    Set-Content -LiteralPath (Join-Path $stub 'pdf.py') -Value 'import sys; sys.exit(int(sys.argv[1]))' -Encoding ASCII
    $stubLauncher = $launcherText.Replace($root, $stub)
    Set-Content -LiteralPath $launcher -Value $stubLauncher -Encoding ASCII
    foreach ($code in 0..4) {
        $process = New-Object System.Diagnostics.Process
        $process.StartInfo = New-Object System.Diagnostics.ProcessStartInfo
        $process.StartInfo.FileName = $env:ComSpec
        $process.StartInfo.Arguments = '/d /c call "' + $launcher + '" ' + $code
        $process.StartInfo.UseShellExecute = $false
        $process.Start() | Out-Null
        $process.WaitForExit()
        if ($process.ExitCode -ne $code) { throw "launcher exit $code became $($process.ExitCode)" }
        $process.Dispose()
    }
    'PASS PowerShell 5.1 launcher tests'
}
finally {
    Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
    $env:CLIERR_LOG_ROOT = $oldLogRoot
    [Console]::OutputEncoding = $oldEncoding
    $OutputEncoding = $oldOutputEncoding
}
