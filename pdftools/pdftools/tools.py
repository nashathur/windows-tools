import os
import subprocess
import psutil
from pathlib import Path

import clierr


def run_tool(argv, *, timeout=None, cwd=None, env=None, temp=None, secret_env=(), secret_values=()):
    if not isinstance(argv, (list, tuple)):
        raise TypeError("tool arguments must be an argv list")
    if any(secret and any(str(secret) in str(arg) for arg in argv) for secret in secret_values):
        raise clierr.Fail("BAD_ARG", "a secret was included in an external tool argument", "pass secrets through a file or the tool's supported input channel")
    child_env = dict(os.environ if env is None else env)
    for name in secret_env:
        child_env.pop(name, None)
    if temp:
        child_env.update(TEMP=str(temp), TMP=str(temp))
    try:
        proc = subprocess.Popen(list(map(str, argv)), cwd=cwd or temp, env=child_env,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                encoding="utf-8", errors="replace")
    except FileNotFoundError:
        name = Path(str(argv[0])).stem
        raise clierr.Fail("NOT_FOUND", f"{name} not found ({argv[0]})", f"install {name} or fix its path")
    try:
        stdout, stderr = proc.communicate(timeout=timeout)
    except (KeyboardInterrupt, subprocess.TimeoutExpired) as exc:
        try:
            parent = psutil.Process(proc.pid)
            children = parent.children(recursive=True)
            for child in children:
                child.kill()
            parent.kill()
            psutil.wait_procs(children + [parent], timeout=3)
        except psutil.Error:
            pass
        try:
            proc.communicate(timeout=3)
        except Exception:
            pass
        if isinstance(exc, subprocess.TimeoutExpired):
            raise clierr.Fail("TIMEOUT", f"{Path(str(argv[0])).stem} ran over {timeout} s",
                              "raise the timeout or shrink the input", tool_run=exc)
        raise
    result = subprocess.CompletedProcess(argv, proc.returncode, stdout, stderr)
    if proc.returncode:
        name = Path(str(argv[0])).stem
        raise clierr.Fail("TOOL_FAILED", f"{name} exit {proc.returncode}: {clierr.relevant_line(stderr or stdout)}",
                          "see the log's tool stderr section", tool_run=result)
    return result
