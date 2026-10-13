# Shared helpers for the live checks (dot-source it). Every check reports through Pass/Fail/Note
# and saves screenshots under $Live.Out; run.ps1 collects them into report.md.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
if (-not ('LiveW' -as [type])) {
Add-Type @'
using System; using System.Runtime.InteropServices;
public class LiveW {
 [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
 [DllImport("user32.dll")] public static extern IntPtr FindWindowEx(IntPtr p, IntPtr a, string c, string t);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint f);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int w, int ht, uint f);
 public struct R { public int L, T, Rt, B; } }
'@
}
[void][LiveW]::SetProcessDPIAware()

$Dock = Split-Path $PSScriptRoot -Parent | Split-Path -Parent
$Ctl = Join-Path $Dock 'dockctl.exe'
if (-not $global:Live) {
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $global:Live = @{ Out = "C:\Users\thurs\tools\_tmp\live\$stamp"; Results = New-Object System.Collections.ArrayList }
}
New-Item -ItemType Directory -Force $Live.Out | Out-Null

function Pass([string]$name, [string]$detail = '') { [void]$global:Live.Results.Add([pscustomobject]@{ R = 'PASS'; Name = $name; Detail = $detail }); "PASS $name $detail" }
function Fail([string]$name, [string]$detail = '') { [void]$global:Live.Results.Add([pscustomobject]@{ R = 'FAIL'; Name = $name; Detail = $detail }); "FAIL $name $detail" }
function Note([string]$name, [string]$detail = '') { [void]$global:Live.Results.Add([pscustomobject]@{ R = 'NOTE'; Name = $name; Detail = $detail }); "NOTE $name $detail" }
function Check([bool]$ok, [string]$name, [string]$detail = '') { if ($ok) { Pass $name $detail } else { Fail $name $detail } }

# dockctl as an object; $null when it fails (exit code non-zero or not JSON).
function Ctl {
    $out = & $Ctl @args 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) { return $null }
    try { return $out | ConvertFrom-Json } catch { return $null }
}
# dockctl's JSON whatever the exit code: `all ...` exits 1 when one widget fails but still
# reports the others.
function Ctl-Any {
    $out = & $Ctl @args 2>&1 | Out-String
    try { return $out | ConvertFrom-Json } catch { return $null }
}
# A setting's current value, or throws: a check snapshots before it changes anything, so it can
# always put the value back.
function Snapshot([string]$w, [string]$key, [switch]$Dock) {
    $r = if ($Dock) { Ctl $w get --dock $key } else { Ctl $w get $key }
    if (-not $r -or -not $r.ok -or $null -eq $r.PSObject.Properties['value']) { throw "could not read $w $key before changing it" }
    $r.value
}
function Wait-Until([scriptblock]$cond, [int]$ms = 5000) {
    $t0 = Get-Date
    do { if (& $cond) { return $true }; Start-Sleep -Milliseconds 200 } while (((Get-Date) - $t0).TotalMilliseconds -lt $ms)
    return $false
}

function Save-Rect($x, $y, $w, $h, [string]$name) {
    $b = New-Object Drawing.Bitmap ([int]$w), ([int]$h); $g = [Drawing.Graphics]::FromImage($b)
    $g.CopyFromScreen([int]$x, [int]$y, 0, 0, (New-Object Drawing.Size ([int]$w), ([int]$h))); $g.Dispose()
    $p = Join-Path $Live.Out "$name.png"; $b.Save($p, [Drawing.Imaging.ImageFormat]::Png); $b.Dispose(); $p
}
# The right half of the taskbar (where the widgets sit).
function Shot-Taskbar([string]$name) {
    $tb = [LiveW]::FindWindow('Shell_TrayWnd', [NullString]::Value); $r = New-Object LiveW+R; [void][LiveW]::GetWindowRect($tb, [ref]$r)
    $half = [int](($r.Rt - $r.L) / 2); Save-Rect ($r.L + $half) $r.T $half ($r.B - $r.T) $name
}
# One widget's strip (its rectangle from dockctl state), with a 4 px margin. $null when hidden.
function Shot-Strip([string]$w, [string]$name) {
    $s = (Ctl $w state).strip
    if (-not $s.shown -or -not $s.rect) { return $null }
    Save-Rect ($s.rect.x - 4) $s.rect.y ($s.rect.w + 8) $s.rect.h $name
}
# Block's standard menu rows, by id (the same in every widget; labels with "…" don't survive
# PowerShell 5.1's native argument passing).
$Row = @{ ShowOnTaskbar = 61444; Compact = 61440; StartWithWindows = 61441; EditSettings = 61442; Quit = 61443 }
# A widget's optional extra checks: scripts\live\extra\<widget>.ps1 -Check <check> (see run.ps1).
function Extra([string]$w, [string]$check) {
    $f = Join-Path $PSScriptRoot "extra\$w.ps1"
    if (Test-Path $f) { & $f -Check $check }
}
# Opens a widget's left-click panel, captures it, closes it. Returns the path or $null.
# Waits up to 2 s for it to report open (a widget re-laying out after a settings change can be
# late); tries once more and notes it when the first open didn't hold.
function Shot-Panel([string]$w, [string]$name) {
    foreach ($try in 1, 2) {
        [void](Ctl $w panel open)
        $p = $null
        if (Wait-Until { $script:sp = (Ctl $w state).panel; $script:sp.open -and $script:sp.rect } 2000) { $p = $script:sp }
        $path = $null
        if ($p) { Start-Sleep -Milliseconds 400; $path = Save-Rect $p.rect.x $p.rect.y $p.rect.w $p.rect.h $name }
        [void](Ctl $w panel close); Start-Sleep -Milliseconds 300
        if ($path) { if ($try -eq 2) { [void](Note "panel/$w" 'opened on the second try') }; return $path }
    }
    $null
}
function Shot-Menu([string]$w, [string]$name) {
    [void](Ctl $w menu open); Start-Sleep -Milliseconds 700
    $m = (Ctl $w state).menu
    $path = $null
    if ($m.rect) { $path = Save-Rect $m.rect.x $m.rect.y $m.rect.w $m.rect.h $name }
    [void](Ctl $w menu close); $path
}

# The settings app: started as at sign-in (one-off scheduled task: real %APPDATA%).
function App-Running { $null -ne (Ctl settings state) }
function Start-App([string]$AppArgs = '') {
    if (App-Running) { return $true }
    $exe = Join-Path $Dock 'taskbar-dock-settings.exe'; $task = "LiveCheckApp_$PID"
    $action = if ($AppArgs) { New-ScheduledTaskAction -Execute $exe -Argument $AppArgs -WorkingDirectory $Dock } else { New-ScheduledTaskAction -Execute $exe -WorkingDirectory $Dock }
    Register-ScheduledTask -TaskName $task -Action $action -Force | Out-Null
    Start-ScheduledTask -TaskName $task
    $ok = Wait-Until { App-Running } 10000
    Unregister-ScheduledTask -TaskName $task -Confirm:$false -ErrorAction SilentlyContinue
    $ok
}
function Stop-App { if (App-Running) { [void](Ctl settings quit); [void](Wait-Until { -not (Get-Process taskbar-dock-settings -ErrorAction SilentlyContinue) } 5000) } }
# The app's window through PrintWindow (works while covered).
function Shot-App([string]$name) {
    $h = [LiveW]::FindWindow([NullString]::Value, 'Taskbar Dock')
    if ($h -eq [IntPtr]::Zero) { return $null }
    $r = New-Object LiveW+R; [void][LiveW]::GetWindowRect($h, [ref]$r)
    $b = New-Object Drawing.Bitmap ($r.Rt - $r.L), ($r.B - $r.T); $g = [Drawing.Graphics]::FromImage($b)
    $dc = $g.GetHdc(); [void][LiveW]::PrintWindow($h, $dc, 2); $g.ReleaseHdc($dc); $g.Dispose()
    $p = Join-Path $Live.Out "$name.png"; $b.Save($p, [Drawing.Imaging.ImageFormat]::Png); $b.Dispose(); $p
}
# Resizes the app's own window (e.g. tall, to capture a whole page without scrolling).
function Size-App([int]$w, [int]$h) {
    $hw = [LiveW]::FindWindow([NullString]::Value, 'Taskbar Dock')
    if ($hw -eq [IntPtr]::Zero) { return $false }
    [LiveW]::SetWindowPos($hw, [IntPtr]::Zero, 40, 0, $w, $h, 0x0014)   # NOZORDER | NOACTIVATE
}
# Scrolls the app's page to a vertical percentage (0 top, 100 bottom) through UI Automation.
function Scroll-App([double]$percent) {
    Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
    $A = [Windows.Automation.AutomationElement]
    $win = $A::RootElement.FindFirst([Windows.Automation.TreeScope]::Children, (New-Object Windows.Automation.PropertyCondition $A::NameProperty, 'Taskbar Dock'))
    if (-not $win) { return $false }
    $cond = New-Object Windows.Automation.PropertyCondition $A::IsScrollPatternAvailableProperty, $true
    foreach ($e in $win.FindAll([Windows.Automation.TreeScope]::Descendants, $cond)) {
        $sp = $e.GetCurrentPattern([Windows.Automation.ScrollPattern]::Pattern)
        if ($sp.Current.VerticallyScrollable) { $sp.SetScrollPercent(-1, $percent); Start-Sleep -Milliseconds 400; return $true }
    }
    $false
}
function App-Row([string]$id) { (Ctl settings state).rows | Where-Object { $_.id -eq $id } | Select-Object -First 1 }

# Every running widget (dockctl all ping): a new widget is checked as soon as it runs. -Widgets on
# run.ps1 narrows it.
if (-not $Live.Widgets) {
    $p = Ctl-Any all ping
    $Live.Widgets = @(if ($p) { $p.PSObject.Properties | Where-Object { $_.Value.ok } | ForEach-Object Name | Sort-Object })
    if ($p) { $p.PSObject.Properties | Where-Object { -not $_.Value.ok -and -not $_.Value.absent } | ForEach-Object { [void](Fail "widgets/$($_.Name)" "not answering: $($_.Value.error)") } }
}
$Widgets = $Live.Widgets
