# __TITLE__-specific live checks, called by the shared live suite.
param([string]$Check)
. "$PSScriptRoot\..\common.ps1"
switch ($Check) {
    'idle-awake' {
        $result = Ctl __SECTION__ do now
        Check ($null -ne $result -and $null -ne $result.counter) '__SECTION__/idle-awake/now' ($result | ConvertTo-Json -Compress)
    }
}
