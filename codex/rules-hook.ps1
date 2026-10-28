# Codex PreToolUse hook (~\.codex\hooks.json, matcher Bash|PowerShell): refuses shell commands that break
# the user's standing rules. A guardrail on top of the sandbox, not a full boundary: it reads the command text.
# Input: one JSON object on stdin (tool_input.command, cwd). Block: exit 2, reason on stderr (Codex shows it
# to the model). Test: tests\rules-hook.ps1.
$ErrorActionPreference = 'Stop'
try {
    [Console]::InputEncoding = New-Object Text.UTF8Encoding $false
    $in = [Console]::In.ReadToEnd() | ConvertFrom-Json
} catch {
    [Console]::Error.WriteLine('WARN rules-hook BAD_INPUT: unreadable hook input, command allowed | fix: check the PreToolUse input format')
    exit 0
}
$cmd = @($in.tool_input.command) -join ' '
$cwd = ("$($in.cwd)" -replace '/', '\').TrimEnd('\')

# Up to the end of one statement: the rest of a command, before ; & | or a new line.
$s = '[^;&|\r\n]*'
$rules = @(
    @("\btaskkill(\.exe)?\b$s\s/im\b", 'no blanket kills by name; stop only a process you started, by its PID (taskkill /PID <pid>)'),
    @("\b(stop-process|spps|kill)\b$s\s-(n|na|nam|name|processname)\b", 'no blanket kills by name; stop only a process you started, by its PID (Stop-Process -Id <pid>)'),
    @('\b(get-process|gps)\b(?![^|]*\s-id\b)[^|;&\r\n]*\|\s*(stop-process|spps|kill)\b', 'no blanket kills by name; stop only a process you started, by its PID (Stop-Process -Id <pid>)'),
    @('\b(killall|pkill)\b', 'no blanket kills by name; stop only a process you started, by its PID'),
    @("\bwmic\b$s\bprocess\b$s\b(delete|terminate)\b", 'no blanket kills by name; stop only a process you started, by its PID'),
    @('appdata[\\/]+local[\\/]+programs[\\/]+python', 'never the system Python; run Python as: conda run -n <env> python ...'),
    @('(miniconda3|anaconda3)[\\/]+(envs[\\/]+[^\\/\s"'']+[\\/]+)?pythonw?(\.exe)?\b', 'never call an env''s python.exe directly (wrong DLLs); use: conda run -n <env> python ...'),
    @('(?:^|[;&|(\r\n]|-command\s|\s-c\s|/c\s)\s*&?\s*["'']?(?:[^\s"'';&|]*[\\/])?(python[0-9.]*|py|pip[0-9.]*)(\.exe)?["'']?(?=\s|$|[";&|)''])', 'Python and pip run only through conda: conda run -n <env> python ... (the env the prompt names)'),
    @("\bconda(\.exe)?\b(?=$s(-n|--name)[\s=]+base\b)(?=$s\b(install|uninstall|remove)\b)", 'never install into conda base; use the env the prompt names'),
    @("\bconda(\.exe)?\s+install\b(?!$s\s(-n|--name|-p|--prefix)[\s=])", 'conda install needs -n <env> (never base)'),
    @('\bnet(\.exe)?\s+(user|accounts|localgroup)\b', 'no Windows account or password changes'),
    @('\b(new|set|remove|enable|disable|rename)-localuser\b|\b(add|remove)-localgroupmember\b|\bunlock-adaccount\b', 'no Windows account changes'),
    @('\b(secpol|secedit|gpedit|auditpol|bcdedit)\b', 'no Windows security or policy changes'),
    @('\b(set|add|remove)-mppreference\b', 'no Windows Defender changes'),
    @('\bset-executionpolicy\b', 'no Windows policy changes'),
    @("\bnetsh\b$s\bfirewall\b", 'no firewall changes'),
    @("\breg(\.exe)?\s+(add|delete|import|restore|load)\b$s\bhk(lm|ey_local_machine)\b", 'no machine-wide registry changes (HKLM)'),
    @("\b(new|set|remove|rename|clear)-item(property)?\b$s(hklm:|hkey_local_machine)", 'no machine-wide registry changes (HKLM)'),
    @('\bgit(\.exe)?((\s+-C\s+\S+|\s+-c\s+\S+|\s+--[\w-]+(=\S+)?))*\s+(push|stash)\b', 'no git push or git stash; Claude handles publishing and other sessions'' work'),
    @("\bgit(\.exe)?((\s+-C\s+\S+|\s+-c\s+\S+|\s+--[\w-]+(=\S+)?))*\s+reset\b$s\s--hard\b", 'no git reset --hard; it discards uncommitted work')
)

function Block([string]$Reason) {
    [Console]::Error.WriteLine("Blocked by the user's rules: $Reason")
    exit 2
}

foreach ($r in $rules) { if ($cmd -match "(?i)$($r[0])") { Block $r[1] } }

# Recursive deletes: every target must be a plain path inside the working folder.
foreach ($stmt in $cmd -split '&&|\|\||;|\r?\n') {
    if ($stmt -notmatch '(?i)(^|[\s"''(&|])(remove-item|rm|ri|rmdir|rd|del|erase)(\.exe)?(?=\s)') { continue }
    if ($stmt -notmatch '(?i)\s-(recurse|r|rf|fr|rfv)\b|\s/s\b') { continue }
    foreach ($t in ($stmt -replace '["'']', ' ') -split '\s+') {
        # Skip flags, redirections (2>$null), constants and the shell itself.
        if (-not $t -or $t -match '^-|^\d*>|(?i)^\$(true|false|null)$|(?i)\.exe$' -or $t -match '^/[a-z?]$') { continue }
        $p = $t -replace '^/([a-z])/', '$1:/' -replace '/', '\'
        if ($p -match '[$%~]' -or $p -match '(^|\\)\.\.(\\|$)') { Block "recursive delete of '$t': name a literal path inside $cwd" }
        if ($p -match '^([a-z]:|\\)' -and -not ($p.TrimEnd('\') -eq $cwd -or $p.StartsWith("$cwd\", 'OrdinalIgnoreCase'))) {
            Block "recursive delete outside the working folder ($t); only paths inside $cwd"
        }
    }
}
exit 0
