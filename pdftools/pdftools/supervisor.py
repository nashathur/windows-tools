"""Standard-library/psutil timeout supervisor; intentionally independent of pdftools imports."""
import ctypes
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import queue
import threading
import time

import psutil


def _kill_tree(pid):
    try:
        parent = psutil.Process(pid)
        children = [child for child in parent.children(recursive=True) if child.pid != os.getpid()]
    except psutil.Error:
        children = []
    for child in reversed(children):
        try:
            child.kill()
        except psutil.Error:
            pass
    if children:
        psutil.wait_procs(children, timeout=1)
    try:
        if os.name == "nt":
            from ctypes import wintypes
            kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel32.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
            kernel32.OpenProcess.restype = wintypes.HANDLE
            kernel32.TerminateProcess.argtypes = (wintypes.HANDLE, wintypes.UINT)
            kernel32.TerminateProcess.restype = wintypes.BOOL
            kernel32.CloseHandle.argtypes = (wintypes.HANDLE,)
            kernel32.CloseHandle.restype = wintypes.BOOL
            handle = kernel32.OpenProcess(0x0001, False, pid)
            if handle:
                try:
                    if not kernel32.TerminateProcess(handle, 1):
                        psutil.Process(pid).kill()
                finally:
                    kernel32.CloseHandle(handle)
            else:
                psutil.Process(pid).kill()
        else:
            os.kill(pid, signal.SIGTERM)
    except (OSError, psutil.Error):
        pass
    try:
        psutil.Process(pid).wait(timeout=2)
    except psutil.Error:
        pass


def _cleanup(paths):
    deadline = time.monotonic() + 2
    remaining = set(paths)
    while remaining and time.monotonic() < deadline:
        for name in tuple(remaining):
            path = Path(name)
            try:
                if path.is_dir():
                    shutil.rmtree(path)
                else:
                    path.unlink(missing_ok=True)
                remaining.discard(name)
            except OSError:
                pass
        if remaining:
            time.sleep(0.05)


def _expire(pid, state, paths):
    _kill_tree(pid)
    _cleanup(paths)
    common = Path(__file__).resolve().parents[2] / "common"
    sys.path.insert(0, str(common))
    import clierr
    clierr.LOG_ROOT = Path(state["log_root"])
    command = state["command"]
    seconds = round(state["seconds"])
    what = f"{command} ran over {seconds} s"
    fix = "raise PDFTOOLS_TIME_LIMIT or split the input"
    log = clierr.write_log("pdftools", "TIMEOUT", what, fix, log_name=command,
                           sections={"registered paths": "\n".join(sorted(paths))})
    if state["json"]:
        print(json.dumps({"type": "error", "command": command, "code": "TIMEOUT",
                          "message": what, "fix": fix, "log": log, "problems": []},
                         separators=(",", ":")), flush=True)
    print(f"ERROR pdf {command} TIMEOUT: {what} | fix: {fix} | log: {log}", file=sys.stderr, flush=True)


def main():
    pid = int(sys.argv[1])
    state = {"seconds": 0, "command": "unknown", "json": False,
             "log_root": os.environ.get("CLIERR_LOG_ROOT", r"C:\Tools\logs")}
    paths = set()
    deadline = None
    paused_at = None
    messages = queue.Queue()
    def read_messages():
        for item in sys.stdin:
            messages.put(item)
        messages.put(None)
    threading.Thread(target=read_messages, daemon=True).start()
    while True:
        wait = None if deadline is None or paused_at is not None else max(0, deadline - time.monotonic())
        try:
            line = messages.get(timeout=wait)
        except queue.Empty:
            _expire(pid, state, paths)
            return 0
        if line is None:
            return 0
        try: message = json.loads(line)
        except Exception: continue
        op = message.get("op")
        if op == "arm":
            state.update(seconds=float(message["seconds"]), command=message["command"],
                         json=bool(message["json"]), log_root=message["log_root"])
            deadline = time.monotonic() + state["seconds"]
            paused_at = None
        elif op == "pause" and deadline is not None and paused_at is None:
            paused_at = time.monotonic()
        elif op == "resume" and paused_at is not None:
            deadline += time.monotonic() - paused_at
            paused_at = None
        elif op == "path":
            paths.add(message["path"])
        elif op == "unpath":
            paths.discard(message["path"])
        elif op == "disarm":
            return 0


if __name__ == "__main__":
    raise SystemExit(main())
