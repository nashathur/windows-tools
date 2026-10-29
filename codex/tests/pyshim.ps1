# pyshim\sitecustomize.py: with CODEX_CONSULT a tempfile folder inherits its parent's ACL; without it, Python's own owner-only ACL stays.
$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) "ask-pyshim-$PID"
[void](New-Item -ItemType Directory -Force $root)
$conda = 'C:\Users\thurs\miniconda3\Scripts\conda.exe'
$savedPath, $savedConsult = $env:PYTHONPATH, $env:CODEX_CONSULT
$env:PYTHONPATH = Join-Path $PSScriptRoot '..\pyshim'
$failed = 0

function Run-Case([string]$Name, [string]$Consult, [bool]$Inherits) {
    if ($Consult) { $env:CODEX_CONSULT = $Consult } else { Remove-Item Env:CODEX_CONSULT -ErrorAction SilentlyContinue }
    $dir = (& $conda run -n dev python -c "import tempfile; print(tempfile.mkdtemp(dir=r'$root'))").Trim()
    $acl = (& icacls.exe $dir) -join "`n"
    if ($acl.Contains('(I)') -ne $Inherits) { $script:failed++; "FAIL ${Name}: inherited ACEs expected $Inherits`n$acl" }
}

Run-Case 'codex' '1' $true
Run-Case 'user' '' $false

$env:PYTHONPATH, $env:CODEX_CONSULT = $savedPath, $savedConsult
Remove-Item -LiteralPath $root -Recurse -Force
if ($failed) { exit 1 }
'pyshim ok'
