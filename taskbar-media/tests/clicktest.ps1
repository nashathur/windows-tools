# Clicks the media strip's buttons (and nothing else) and reads what the media session reports.
# Finds the strip first and stops if it isn't there or its rectangle looks wrong.
param([string]$App = '308046B0AF4A39CB', [string]$Buttons = 'right,right,middle,middle')

# one comma-separated string (a list passed through -File arrives as a single string)
$list = @($Buttons -split '[,\s]+' | Where-Object { $_ })
foreach ($b in $list) { if ($b -notin 'left', 'middle', 'right') { "Unknown button '$b' (use left, middle, right) - stopping."; return } }

Add-Type @'
using System; using System.Runtime.InteropServices;
public static class M {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, IntPtr n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowExW(IntPtr p, IntPtr a, string c, IntPtr n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int x, int y, uint d, UIntPtr e);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
}
'@

Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
function Await($op, [Type]$type) { $t = $asTask.MakeGenericMethod($type).Invoke($null, @($op)); $t.Wait(-1) | Out-Null; $t.Result }
$null = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]
$Mgr = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager]
$mgr = Await ($Mgr::RequestAsync()) $Mgr
function State { $s = $mgr.GetSessions() | Where-Object { $_.SourceAppUserModelId -eq $App } | Select-Object -First 1
    if (-not $s) { return 'session gone' }; $tl = $s.GetTimelineProperties()
    "status=$($s.GetPlaybackInfo().PlaybackStatus) pos=$([math]::Round($tl.Position.TotalSeconds,1))s end=$([math]::Round($tl.EndTime.TotalSeconds,1))s" }

$tb = [M]::FindWindowW('Shell_TrayWnd', [IntPtr]::Zero)
$h = [M]::FindWindowExW($tb, [IntPtr]::Zero, 'TaskbarMediaStrip', [IntPtr]::Zero)
$r = New-Object M+RECT
if ($h -eq [IntPtr]::Zero -or -not [M]::IsWindowVisible($h) -or -not [M]::GetWindowRect($h, [ref]$r)) { 'Strip not found or hidden - stopping.'; return }
$w = $r.R - $r.L; $hgt = $r.B - $r.T
if ($w -lt 60 -or $w -gt 600 -or $hgt -lt 30 -or $hgt -gt 120) { "Strip rectangle looks wrong ($($r.L),$($r.T),$($r.R),$($r.B)) - stopping."; return }
# buttons: the last 3 × 24 px before the 2 px right padding (100% scale)
$scale = $hgt / 48.0
$bw = [math]::Round(24 * $scale); $right = $r.R - [math]::Round(2 * $scale)
$x = @{ left = $right - 3 * $bw + $bw / 2; middle = $right - 2 * $bw + $bw / 2; right = $right - $bw / 2 }
$y = [int](($r.T + $r.B) / 2)
"Strip at $($r.L)..$($r.R); buttons centred at x $($x.left), $($x.middle), $($x.right)"

$saved = New-Object M+POINT; [void][M]::GetCursorPos([ref]$saved)
"start:        $(State)"
foreach ($b in $list) {
    $cx = [int]$x[$b]
    # never click anywhere but the strip
    if ($cx -le $r.L -or $cx -ge $r.R -or $y -le $r.T -or $y -ge $r.B) { "Click point $cx,$y is outside the strip - stopping."; break }
    [void][M]::SetCursorPos($cx, $y); Start-Sleep -Milliseconds 120
    [M]::mouse_event(0x2, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40; [M]::mouse_event(0x4, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 1500
    "after $($b.PadRight(7)): $(State)"
}
[void][M]::SetCursorPos($saved.X, $saved.Y)
