# Right-clicks the media strip's cover (the menu panel opens), screenshots the panel, clicks
# "Compact mode" twice (on and back off) checking the strip's width each time, closes it with
# Esc. Touches only the strip and its panel. 100 % display scale assumed.
param([string]$Out = "$env:TEMP\tm_menu.png")
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class R {
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
function StripRect { $h = [R]::FindWindowExW([R]::FindWindowW('Shell_TrayWnd', [IntPtr]::Zero), [IntPtr]::Zero, 'TaskbarMediaStrip', [IntPtr]::Zero); $r = New-Object R+RECT; if ($h -eq [IntPtr]::Zero -or -not [R]::IsWindowVisible($h) -or -not [R]::GetWindowRect($h, [ref]$r)) { return $null }; $r }
function Click($x, $y, $w, [uint32]$down = 2, [uint32]$up = 4) {
  if ($x -le $w.L -or $x -ge $w.R -or $y -le $w.T -or $y -ge $w.B) { throw "click $x,$y outside its window - stopping" }
  [void][R]::SetCursorPos($x, $y); Start-Sleep -Milliseconds 120
  [R]::mouse_event($down, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 50; [R]::mouse_event($up, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 600 }

$sr = StripRect
if (-not $sr -or ($sr.R - $sr.L) -lt 60) { 'Strip not found or hidden - stopping.'; return }
$saved = New-Object R+POINT; [void][R]::GetCursorPos([ref]$saved)
try {
  "strip width: $($sr.R - $sr.L)"
  Click ($sr.L + 20) ([int](($sr.T + $sr.B) / 2)) $sr 8 16   # right click
  $ph = [R]::FindWindowW('TaskbarMediaPanel', [IntPtr]::Zero); $pr = New-Object R+RECT
  if ($ph -eq [IntPtr]::Zero -or -not [R]::GetWindowRect($ph, [ref]$pr) -or ($pr.R - $pr.L) -ne 300) { 'Menu panel not found - stopping.'; return }
  "menu panel: $($pr.R - $pr.L) x $($pr.B - $pr.T)"
  $b = New-Object System.Drawing.Bitmap 300, ($pr.B - $pr.T); $g = [System.Drawing.Graphics]::FromImage($b); $g.CopyFromScreen($pr.L, $pr.T, 0, 0, $b.Size); $g.Dispose(); $b.Save($Out); $b.Dispose()
  # "Compact mode" row at 100 %: y 138..170
  Click ($pr.L + 150) ($pr.T + 154) $pr; $s1 = StripRect; "compact on:  strip width $($s1.R - $s1.L)"
  Click ($pr.L + 150) ($pr.T + 154) $pr; $s2 = StripRect; "compact off: strip width $($s2.R - $s2.L)"
} finally {
  [R]::keybd_event(0x1B, 0, 0, [UIntPtr]::Zero); [R]::keybd_event(0x1B, 0, 2, [UIntPtr]::Zero)
  [void][R]::SetCursorPos($saved.X, $saved.Y)
}
Start-Sleep -Milliseconds 300
"after Esc: panel " + $(if ([R]::FindWindowW('TaskbarMediaPanel', [IntPtr]::Zero) -ne [IntPtr]::Zero) { 'still open' } else { 'closed' })
