# Rests the pointer on the media strip's cover (hover panel), then clicks the cover (the panel
# grows into the click panel), screenshots both, closes with Esc. Touches only the strip.
param([string]$OutDir = $env:TEMP)
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class P {
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
function Panel { $h = [P]::FindWindowW('TaskbarMediaPanel', [IntPtr]::Zero); if ($h -eq [IntPtr]::Zero) { return $null }; $r = New-Object P+RECT; [void][P]::GetWindowRect($h, [ref]$r); $r }
function Shot($r, $name) { $w = $r.R - $r.L; $hh = $r.B - $r.T; $b = New-Object System.Drawing.Bitmap $w, $hh; $g = [System.Drawing.Graphics]::FromImage($b); $g.CopyFromScreen($r.L, $r.T, 0, 0, $b.Size); $g.Dispose(); $p = Join-Path $OutDir $name; $b.Save($p); $b.Dispose(); $p }

$h = [P]::FindWindowExW([P]::FindWindowW('Shell_TrayWnd', [IntPtr]::Zero), [IntPtr]::Zero, 'TaskbarMediaStrip', [IntPtr]::Zero)
$r = New-Object P+RECT
if ($h -eq [IntPtr]::Zero -or -not [P]::IsWindowVisible($h) -or -not [P]::GetWindowRect($h, [ref]$r) -or ($r.R - $r.L) -lt 60) { 'Strip not found or hidden - stopping.'; return }
$x = $r.L + [int](($r.B - $r.T) * 20 / 48); $y = [int](($r.T + $r.B) / 2)
if ($x -le $r.L -or $x -ge $r.R) { 'Point outside the strip - stopping.'; return }
$saved = New-Object P+POINT; [void][P]::GetCursorPos([ref]$saved)

[void][P]::SetCursorPos($x, $y); Start-Sleep -Milliseconds 900
$p1 = Panel
if ($p1) { "hover panel: $($p1.L),$($p1.T) .. $($p1.R),$($p1.B)  ($($p1.R - $p1.L) x $($p1.B - $p1.T))"; Shot $p1 'tm_hover.png' } else { 'no hover panel' }

[P]::mouse_event(0x2, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40; [P]::mouse_event(0x4, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 600
$p2 = Panel
if ($p2) { "click panel: $($p2.L),$($p2.T) .. $($p2.R),$($p2.B)  ($($p2.R - $p2.L) x $($p2.B - $p2.T))"; Shot $p2 'tm_click.png' } else { 'no click panel' }

[P]::keybd_event(0x1B, 0, 0, [UIntPtr]::Zero); [P]::keybd_event(0x1B, 0, 2, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 300
"after Esc: panel " + $(if (Panel) { 'still open' } else { 'closed' })
[void][P]::SetCursorPos($saved.X, $saved.Y)
