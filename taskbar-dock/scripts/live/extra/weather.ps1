# Weather's own live checks, called by the shared checks at named points (common.ps1 `Extra`).
param([string]$Check)
. "$PSScriptRoot\..\common.ps1"
switch ($Check) {
    'idle-awake' {
        $fresh = Wait-Until { $n = Ctl weather do now; $n -and $null -ne $n.age_s -and $n.age_s -lt 30 } 15000
        Check $fresh 'idle/weather/fetches-at-once' "age_s=$((Ctl weather do now).age_s)"
    }
}
