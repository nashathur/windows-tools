# sysmon-temp

Windowless elevated helper for `taskbar-sysmon`. It reads AMD Ryzen Tctl through PawnIO and updates a shared memory block once per second.

## Shared memory contract

Mapping name: `Local\TaskbarSysmon.Temp`, 24 bytes, little endian:

| Offset | Type | Field |
|---:|---|---|
| 0 | `u32` | magic `0x504D5453` |
| 4 | `u32` | version `1` |
| 8 | `u32` | status: `0` ok, `1` PawnIO missing, `2` unsupported CPU, `3` read error |
| 12 | `i32` | temperature in centi-degrees Celsius |
| 16 | `u64` | Unix milliseconds: set to `0` before the fields change, written last |

A reader accepts a block only when Unix milliseconds is non-zero, recent (3 s), and equal before
and after reading the fields. It grants SYSTEM and Administrators full access and Authenticated
Users read access.

## PawnIO

PawnIO's driver (service `PawnIO`, demand start) only creates `\Device\PawnIO` while its setup-made
software device `ROOT\PAWNIO\0000` (hardware ID `Root\PawnIO`) exists; starting the service by hand
does nothing. `--once` failing with HRESULT `0x80070002` means that device is missing: rerun
PawnIO's setup (`PawnIO_setup.exe -install`, elevated). `0x80070005` means not elevated.

## Install and remove

Build `target\release\sysmon-temp.exe`, then run `install.ps1` in an elevated Windows PowerShell. It installs the binary under `C:\Program Files\TaskbarSysmon\` (admin-only, so the elevated exe can't be swapped by a normal process; its log `sysmon-temp.log` sits beside it) and registers the highest-privilege logon task "Taskbar sysmon temp". Run `uninstall.ps1` elevated to stop/remove the task and installed folder.

## Manual check

Run `target\release\sysmon-temp.exe --once` from a console. It prints one reading or a status and HRESULT/error, then exits with `0` for success or `1` for failure.
