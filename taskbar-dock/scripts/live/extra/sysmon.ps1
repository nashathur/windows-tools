# System monitor-specific live checks, called by the shared live suite.
param([string]$Check)
. "$PSScriptRoot\..\common.ps1"
switch ($Check) {
    'idle-awake' {
        $wake = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        $fresh = Wait-Until {
            $script:result = Ctl sysmon do now
            $script:result -and $script:result.sampled_at_ms -ge $wake -and
                ([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() - $script:result.sampled_at_ms) -le 3000
        } 3000
        Check $fresh 'sysmon/idle-awake/fresh-sample' ($script:result | ConvertTo-Json -Compress)
    }
    'scale' {
        $result = Ctl sysmon do now
        $answers = $result -and $null -ne $result.sampled_at_ms -and
            $result.PSObject.Properties['speed'] -and $result.PSObject.Properties['history_len']
        Check $answers 'sysmon/scale/do-now' ($result | ConvertTo-Json -Compress)
    }
}
