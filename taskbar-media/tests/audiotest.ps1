# Read-only: list Volume Mixer audio sessions (per app volume/mute) and the media sessions, to see if they can be matched.
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

[Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IMMDeviceEnumerator {
    int EnumAudioEndpoints(int dataFlow, int stateMask, out IMMDeviceCollection devices);
}
[Guid("0BD7A1BE-7A1A-44DB-8397-CC5392387B5E"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IMMDeviceCollection {
    int GetCount(out uint count);
    int Item(uint index, out IMMDevice device);
}
[Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IMMDevice {
    int Activate(ref Guid iid, int clsCtx, IntPtr p, [MarshalAs(UnmanagedType.IUnknown)] out object o);
    int OpenPropertyStore(int access, out IntPtr props);
    int GetId([MarshalAs(UnmanagedType.LPWStr)] out string id);
}
[Guid("77AA99A0-1BD6-484F-8BC7-2C654C9A9B6F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IAudioSessionManager2 {
    int GetAudioSessionControl(IntPtr a, int b, out IntPtr c);
    int GetSimpleAudioVolume(IntPtr a, int b, out IntPtr c);
    int GetSessionEnumerator(out IAudioSessionEnumerator e);
}
[Guid("E2F5BB11-0570-40CA-ACDD-3AA01277DEE8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IAudioSessionEnumerator {
    int GetCount(out int count);
    int GetSession(int index, out IAudioSessionControl2 session);
}
[Guid("bfb7ff88-7239-4fc9-8fa2-07c950be9c6d"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IAudioSessionControl2 {
    int GetState(out int state);
    int GetDisplayName([MarshalAs(UnmanagedType.LPWStr)] out string name);
    int SetDisplayName(string a, ref Guid b);
    int GetIconPath([MarshalAs(UnmanagedType.LPWStr)] out string path);
    int SetIconPath(string a, ref Guid b);
    int GetGroupingParam(out Guid g);
    int SetGroupingParam(ref Guid a, ref Guid b);
    int RegisterAudioSessionNotification(IntPtr a);
    int UnregisterAudioSessionNotification(IntPtr a);
    int GetSessionIdentifier([MarshalAs(UnmanagedType.LPWStr)] out string id);
    int GetSessionInstanceIdentifier([MarshalAs(UnmanagedType.LPWStr)] out string id);
    int GetProcessId(out uint pid);
}
[Guid("87CE5498-68D6-44E5-9215-6DA47EF883D8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface ISimpleAudioVolume {
    int SetMasterVolume(float level, ref Guid ctx);
    int GetMasterVolume(out float level);
    int SetMute(bool mute, ref Guid ctx);
    int GetMute(out bool mute);
}

public class AudioRow {
    public string Device; public uint Pid; public string State; public float Volume; public bool Muted; public string SessionId;
}

public static class Mixer {
    public static List<AudioRow> List() {
        var rows = new List<AudioRow>();
        var en = (IMMDeviceEnumerator)Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")));
        IMMDeviceCollection devs; en.EnumAudioEndpoints(0, 1, out devs);   // render, active
        uint n; devs.GetCount(out n);
        for (uint d = 0; d < n; d++) {
            IMMDevice dev; devs.Item(d, out dev);
            string devId; dev.GetId(out devId);
            Guid iid = typeof(IAudioSessionManager2).GUID; object o;
            dev.Activate(ref iid, 23, IntPtr.Zero, out o);
            IAudioSessionEnumerator se; ((IAudioSessionManager2)o).GetSessionEnumerator(out se);
            int count; se.GetCount(out count);
            for (int i = 0; i < count; i++) {
                IAudioSessionControl2 s; se.GetSession(i, out s);
                var r = new AudioRow { Device = devId.Substring(devId.Length - 8) };
                s.GetProcessId(out r.Pid);
                int st; s.GetState(out st); r.State = st == 1 ? "active" : st == 0 ? "inactive" : "expired";
                s.GetSessionIdentifier(out r.SessionId);
                var v = (ISimpleAudioVolume)s;
                v.GetMasterVolume(out r.Volume); v.GetMute(out r.Muted);
                rows.Add(r);
            }
        }
        return rows;
    }
}
'@

"--- Volume Mixer entries (all active output devices)"
foreach ($r in [Mixer]::List()) {
    $p = Get-Process -Id $r.Pid -ErrorAction SilentlyContinue
    $name = if ($r.Pid -eq 0) { 'System sounds' } elseif ($p) { $p.ProcessName } else { "pid $($r.Pid) (gone)" }
    $sid = $r.SessionId -replace '^.*\|', '' -replace '%b\{.*$', ''
    "{0,-14} pid={1,-6} {2,-8} vol={3,3}% mute={4,-5} device=..{5}  id={6}" -f $name, $r.Pid, $r.State, [math]::Round($r.Volume * 100), $r.Muted, $r.Device, $sid
}

"--- Media sessions"
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
$null = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]
$M = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager]
$t = $asTask.MakeGenericMethod($M).Invoke($null, @($M::RequestAsync())); $t.Wait(-1) | Out-Null
foreach ($s in $t.Result.GetSessions()) { "$($s.SourceAppUserModelId)  status=$($s.GetPlaybackInfo().PlaybackStatus)" }
