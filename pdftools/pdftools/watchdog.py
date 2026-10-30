"""Parent-side protocol for the wall-clock supervisor process."""
import json
import os
import subprocess
import sys
import threading
from pathlib import Path

_lock = threading.RLock()
_process = None
_stream = None
_armed = False
_command = "unknown"
_json_output = False
_limit = 0
_pause_count = 0
_SUPERVISOR_ENV = {"PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "TEMP", "TMP", "TMPDIR", "HOME", "USERPROFILE",
                   "LOCALAPPDATA", "APPDATA", "CONDA_PREFIX", "PYTHONUTF8", "PYTHONIOENCODING", "LANG", "LC_ALL",
                   "CLIERR_LOG_ROOT", "PDFTOOLS_TIME_LIMIT"}


def base_limit():
    try:
        return max(1, int(os.environ.get("PDFTOOLS_TIME_LIMIT", "600")))
    except ValueError:
        return 600


def _send(message):
    global _process, _stream
    if _stream is None:
        return
    try:
        _stream.write(json.dumps(message, separators=(",", ":")) + "\n")
        _stream.flush()
    except (BrokenPipeError, OSError, ValueError):
        _process = _stream = None


def arm(command="unknown", json_output=False, seconds=None):
    global _process, _stream, _armed, _command, _json_output, _limit, _pause_count
    with _lock:
        if _process is None or _process.poll() is not None:
            module = Path(__file__).with_name("supervisor.py")
            # Only what Python and the log need: password environment variables never reach a child (PLAN Secrets).
            env = {key: value for key, value in os.environ.items() if key.upper() in _SUPERVISOR_ENV}
            _process = subprocess.Popen([sys.executable, str(module), str(os.getpid())],
                                        stdin=subprocess.PIPE, stdout=None, stderr=None, env=env,
                                        text=True, encoding="utf-8", bufsize=1)
            _stream = _process.stdin
        _command = str(command)
        _json_output = bool(json_output)
        _limit = float(base_limit() if seconds is None else seconds)
        _pause_count = 0
        _armed = True
        try:
            import clierr
            log_root = str(clierr.LOG_ROOT)
        except Exception:
            log_root = os.environ.get("CLIERR_LOG_ROOT", r"C:\Tools\logs")
        _send({"op": "arm", "seconds": _limit, "command": _command,
               "json": _json_output, "log_root": log_root})


def rearm(page_count):
    with _lock:
        if not _armed:
            return
        arm(_command, _json_output, base_limit() + 20 * max(0, int(page_count)))


def is_armed():
    return _armed


def register_path(path):
    with _lock:
        if _armed:
            _send({"op": "path", "path": str(Path(path).resolve())})


def unregister_path(path):
    with _lock:
        if _armed:
            _send({"op": "unpath", "path": str(Path(path).resolve())})


def disarm():
    global _process, _stream, _armed, _pause_count
    with _lock:
        process, stream = _process, _stream
        _armed = False
        _pause_count = 0
        if process is None:
            return
        _send({"op": "disarm"})
        try:
            stream.close()
        except (AttributeError, OSError):
            pass
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=2)
        _process = _stream = None


class paused:
    def __enter__(self):
        global _pause_count
        with _lock:
            self.active = _armed
            if self.active:
                _pause_count += 1
                if _pause_count == 1:
                    _send({"op": "pause"})
        return self

    def __exit__(self, *_):
        global _pause_count
        if self.active:
            with _lock:
                _pause_count = max(0, _pause_count - 1)
                if _armed and _pause_count == 0:
                    _send({"op": "resume"})
