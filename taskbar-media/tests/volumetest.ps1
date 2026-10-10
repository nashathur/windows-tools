# Opens the click panel (click on the strip's cover), screenshots it, sets the volume to 50 % and
# back to 100 % with its slider, mutes and unmutes with its speaker button, reading the Volume
# Mixer after each step; closes it with Esc. Touches only the strip and its panel. 100 % scale.
param([string]$Exe = 'firefox', [string]$Out = "$env:TEMP\tm_volume.png")
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class U {
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
function Mixer { (powershell -NoProfile -ExecutionPolicy Bypass -File "$PSScriptRoot\audiotest.ps1" | Select-String "^$Exe\s.*active " | Select-Object -First 1) -replace '\s+', ' ' -replace ' id=.*', '' }
function Click($x, $y, $w) {
  if ($x -le $w.L -or $x -ge $w.R -or $y -le $w.T -or $y -ge $w.B) { throw "click $x,$y outside its window - stopping" }
  [void][U]::SetCursorPos($x, $y); Start-Sleep -Milliseconds 120
  [U]::mouse_event(0x2, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 60; [U]::mouse_event(0x4, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 700 }

$strip = [U]::FindWindowExW([U]::FindWindowW('Shell_TrayWnd', [IntPtr]::Zero), [IntPtr]::Zero, 'TaskbarMediaStrip', [IntPtr]::Zero)
$sr = New-Object U+RECT
if ($strip -eq [IntPtr]::Zero -or -not [U]::IsWindowVisible($strip) -or -not [U]::GetWindowRect($strip, [ref]$sr) -or ($sr.R - $sr.L) -lt 60) { 'Strip not found or hidden - stopping.'; return }
$saved = New-Object U+POINT; [void][U]::GetCursorPos([ref]$saved)
try {
  "mixer at start: $(Mixer)"
  Click ($sr.L + 20) ([int](($sr.T + $sr.B) / 2)) $sr
  $ph = [U]::FindWindowW('TaskbarMediaPanel', [IntPtr]::Zero); $pr = New-Object U+RECT
  if ($ph -eq [IntPtr]::Zero -or -not [U]::GetWindowRect($ph, [ref]$pr) -or ($pr.R - $pr.L) -ne 360) { 'Click panel not found - stopping.'; return }
  Start-Sleep -Milliseconds 600
  $b = New-Object System.Drawing.Bitmap 360, 232; $g = [System.Drawing.Graphics]::FromImage($b); $g.CopyFromScreen($pr.L, $pr.T, 0, 0, $b.Size); $g.Dispose(); $b.Save($Out); $b.Dispose()
  # the volume row at 100 %: y 192..220; slider from x 58, 238 wide (to 296); speaker at x 16..48
  $y = $pr.T + 206; $sx = $pr.L + 58; $sw = 238
  Click ($sx + [int]($sw * 0.5)) $y $pr; "slider at 50 %:  $(Mixer)"
  Click ($sx + $sw + 4) $y $pr;          "slider at 100 %: $(Mixer)"
  Click ($pr.L + 32) $y $pr;             "speaker (mute):  $(Mixer)"
  Click ($pr.L + 32) $y $pr;             "speaker again:   $(Mixer)"
} finally {
  [U]::keybd_event(0x1B, 0, 0, [UIntPtr]::Zero); [U]::keybd_event(0x1B, 0, 2, [UIntPtr]::Zero)
  [void][U]::SetCursorPos($saved.X, $saved.Y)
}
