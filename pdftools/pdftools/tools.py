import os
import shutil
import subprocess
import sys
import psutil
from pathlib import Path

import clierr

# Each tool's candidates, first found wins: PATH names, and fixed Windows install paths (checked only on Windows).
TOOL_CANDIDATES = {
    "ghostscript": ("gswin64c", "gswin32c", "gs"),
    "soffice": (r"C:\Program Files\LibreOffice\program\soffice.com", "soffice"),
    "browser": ("msedge", r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
                "google-chrome", "chromium", "chromium-browser"),
}


WINDOWS = os.name == "nt"
_is_file = os.path.isfile
# unpaper (ocrmypdf --clean) on Windows comes from MSYS2; its folder goes last on the OCR tools' PATH only,
# so MSYS2's DLLs never shadow the conda env's.
UNPAPER_WINDOWS_DIR = Path(r"C:\msys64\ucrt64\bin")


def find_tool(tool):
    for candidate in TOOL_CANDIDATES[tool]:
        if candidate[1:3] == ":\\":
            if WINDOWS and _is_file(candidate):
                return candidate
        elif path := shutil.which(candidate):
            return path
    return None


def dev_tool_env(temp, langs="eng+ind", env=None):
    """Child environment for the dev env's OCR tools (ocrmypdf, tesseract, jbig2, pngquant): the env's tool folder
    first on PATH and, where the conda package splits tessdata (Windows: languages in share/tessdata, configs in
    Library/share/tessdata), a joined TESSDATA_PREFIX folder in temp holding the configs and the requested languages."""
    child = dict(os.environ if env is None else env)
    prefix = Path(sys.prefix)
    child["PATH"] = str(prefix / ("Library/bin" if WINDOWS else "bin")) + os.pathsep + child.get("PATH", "")
    if WINDOWS and (UNPAPER_WINDOWS_DIR / "unpaper.exe").is_file():
        child["PATH"] += os.pathsep + str(UNPAPER_WINDOWS_DIR)
    languages, configs = prefix / "share" / "tessdata", prefix / "Library" / "share" / "tessdata"
    if (languages / "configs").is_dir() or not (configs / "configs").is_dir():
        return child
    joined = Path(temp) / "tessdata"
    for name in ("configs", "tessconfigs"):
        if (configs / name).is_dir() and not (joined / name).exists():
            shutil.copytree(configs / name, joined / name)
    for lang in {*str(langs).split("+"), "osd"}:
        source, target = languages / f"{lang}.traineddata", joined / f"{lang}.traineddata"
        if source.is_file() and not target.exists():
            try:
                os.link(source, target)  # same drive: no copy of the (large) language model
            except OSError:
                shutil.copy2(source, target)
    child["TESSDATA_PREFIX"] = str(joined)
    return child


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
        if os.name != "nt" and Path(str(argv[0])).stem.lower() == "soffice":
            runtime = Path(temp) / "xdg-runtime"
            runtime.mkdir(exist_ok=True)
            try: runtime.chmod(0o700)
            except OSError: pass
            child_env["XDG_RUNTIME_DIR"] = str(runtime)
    try:
        proc = subprocess.Popen(list(map(str, argv)), cwd=cwd or temp, env=child_env,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                encoding="utf-8", errors="replace")
    except FileNotFoundError:
        name = Path(str(argv[0])).stem
        raise clierr.Fail("NOT_FOUND", f"{name} not found ({argv[0]})", f"install {name} or fix its path")
    try:
        from .watchdog import paused
        with paused():
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
