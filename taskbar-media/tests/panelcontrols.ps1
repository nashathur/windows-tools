# Opens the click panel (click on the strip's cover), presses its play/pause twice, drags its
# progress bar to the middle and back, closes it with Esc. Touches only the strip and its panel;
# reads what the media session reports after each step. 100% display scale assumed.
param([string]$App = 'SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify')
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class Q {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, IntPtr n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowExW(IntPtr p, IntPtr a, string c, IntPtr n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int x, int y, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint f, UIntPtr e);
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
function Sess { $mgr.GetSessions() | Where-Object { $_.SourceAppUserModelId -eq $App } | Select-Object -First 1 }
function State { $s = Sess; if (-not $s) { return 'session gone' }; $tl = $s.GetTimelineProperties()
  "status=$($s.GetPlaybackInfo().PlaybackStatus) pos=$([math]::Round($tl.Position.TotalSeconds,1))s of $([math]::Round($tl.EndTime.TotalSeconds,1))s" }
function Click($x, $y, $within) {
  if ($x -le $within.L -or $x -ge $within.R -or $y -le $within.T -or $y -ge $within.B) { throw "click $x,$y outside its window - stopping" }
  [void][Q]::SetCursorPos($x, $y); Start-Sleep -Milliseconds 120
  [Q]::mouse_event(0x2, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 60; [Q]::mouse_event(0x4, 0, 0, 0, [UIntPtr]::Zero) }

$strip = [Q]::FindWindowExW([Q]::FindWindowW('Shell_TrayWnd', [IntPtr]::Zero), [IntPtr]::Zero, 'TaskbarMediaStrip', [IntPtr]::Zero)
$sr = New-Object Q+RECT
if ($strip -eq [IntPtr]::Zero -or -not [Q]::IsWindowVisible($strip) -or -not [Q]::GetWindowRect($strip, [ref]$sr) -or ($sr.R - $sr.L) -lt 60) { 'Strip not found or hidden - stopping.'; return }
$saved = New-Object Q+POINT; [void][Q]::GetCursorPos([ref]$saved)
try {
  "start:              $(State)"
  Click ($sr.L + 20) ([int](($sr.T + $sr.B) / 2)) $sr
  Start-Sleep -Milliseconds 500
  $ph = [Q]::FindWindowW('TaskbarMediaPanel', [IntPtr]::Zero); $pr = New-Object Q+RECT
  if ($ph -eq [IntPtr]::Zero -or -not [Q]::GetWindowRect($ph, [ref]$pr) -or ($pr.R - $pr.L) -ne 360) { 'Click panel not found - stopping.'; return }
  # the click panel's layout at 100%: play/pause centred at (180, 168), the bar at y 126, x 16..344
  Click ($pr.L + 180) ($pr.T + 168) $pr; Start-Sleep -Milliseconds 800; "play/pause:         $(State)"
  Click ($pr.L + 180) ($pr.T + 168) $pr; Start-Sleep -Milliseconds 800; "play/pause again:   $(State)"
  $tl = (Sess).GetTimelineProperties(); $back = $tl.Position.TotalSeconds / [math]::Max(1, $tl.EndTime.TotalSeconds)
  Click ($pr.L + 180) ($pr.T + 126) $pr; Start-Sleep -Milliseconds 1200; "seek to the middle: $(State)"
  Click ($pr.L + 16 + [int](328 * $back)) ($pr.T + 126) $pr; Start-Sleep -Milliseconds 1200; "seek back:          $(State)"
} finally {
  [Q]::keybd_event(0x1B, 0, 0, [UIntPtr]::Zero); [Q]::keybd_event(0x1B, 0, 2, [UIntPtr]::Zero)
  [void][Q]::SetCursorPos($saved.X, $saved.Y)
}
