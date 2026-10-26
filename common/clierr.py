"""One-line failures for CLIs Claude runs.

stderr gets `ERROR <tool> <CODE>: <what> | fix: <hint> | log: <path>` and the exit is non-zero;
the command line, traceback and external tools' output go to a log under
C:\\Tools\\logs\\<tool>\\ (last 50 kept). Warnings: `WARN <tool> <CODE>: <what> | fix: <hint>`.

    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "common"))
    import clierr
    ...
    if __name__ == "__main__":
        clierr.run("book", main)

Codes: NOT_FOUND, LOCKED, BAD_ARG, BAD_INPUT, TOOL_FAILED, TIMEOUT, INTERRUPTED, INTERNAL,
plus tool-specific ones.
"""
import argparse
import datetime
import os
import re
import subprocess
import sys
import traceback
from pathlib import Path

LOG_ROOT = Path(os.environ.get("CLIERR_LOG_ROOT", r"C:\Tools\logs"))
KEEP = 50
MAX_LOG = 200_000
TAIL = 30

_ANSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")
_RELEVANT = re.compile(r"error|fatal|failed|exception", re.I)


class Fail(Exception):
    """A failure with a code and a fix hint. `detail` (text) and `tool_run` (a CompletedProcess
    or CalledProcessError) go to the log only."""

    def __init__(self, code, what, fix, detail=None, tool_run=None):
        super().__init__(what)
        self.code, self.what, self.fix = code, what, fix
        self.detail, self.tool_run = detail, tool_run


def clean(text):
    """Text without colour codes and progress-bar redraws (keeps the last state of a \\r line)."""
    text = _ANSI.sub("", text or "")
    return "\n".join(line.rstrip("\r").rsplit("\r", 1)[-1] for line in text.split("\n"))


def relevant_line(text):
    """The single most telling line of a tool's stderr: the first mentioning
    error/fatal/failed/exception, else the last non-empty line."""
    lines = [l.strip() for l in clean(text).splitlines() if l.strip()]
    for line in lines:
        if _RELEVANT.search(line):
            return line[:300]
    return lines[-1][:300] if lines else "(no output)"


def run_tool(cmd, name=None, fix=None, timeout=None, **kwargs):
    """subprocess.run with captured text output; a non-zero exit raises Fail TOOL_FAILED quoting
    one stderr line, a timeout raises Fail TIMEOUT. Returns the CompletedProcess."""
    name = name or Path(str(cmd[0])).stem
    kwargs.setdefault("capture_output", True)
    kwargs.setdefault("text", True)
    kwargs.setdefault("errors", "replace")
    try:
        result = subprocess.run(cmd, timeout=timeout, **kwargs)
    except FileNotFoundError:
        raise Fail("NOT_FOUND", f"{name} not found ({cmd[0]})", fix or f"install {name} or fix its path")
    except subprocess.TimeoutExpired as e:
        raise Fail("TIMEOUT", f"{name} ran over {timeout} s", fix or "raise the timeout or shrink the input", tool_run=e)
    if result.returncode != 0:
        raise Fail("TOOL_FAILED", f"{name} exit {result.returncode}: {relevant_line(result.stderr or result.stdout)}",
                   fix or f"see the log's tool stderr section", tool_run=result)
    return result


def _one_line(text):
    return re.sub(r"\s*[\r\n]+\s*", " ", str(text)).strip()


def warn(tool, code, what, fix):
    print(f"WARN {tool} {code}: {_one_line(what)} | fix: {_one_line(fix)}", file=sys.stderr, flush=True)


def _classify(exc):
    """(code, what, fix) for an exception that isn't a Fail."""
    if isinstance(exc, FileNotFoundError):
        return "NOT_FOUND", f"{exc.filename or exc} not found", "check the path"
    if isinstance(exc, PermissionError):
        return "LOCKED", f"{exc.filename or exc} is locked or not writable", "close the program holding it, then rerun"
    if isinstance(exc, subprocess.CalledProcessError):
        name = Path(str(exc.cmd[0] if isinstance(exc.cmd, (list, tuple)) else exc.cmd)).stem
        return "TOOL_FAILED", f"{name} exit {exc.returncode}: {relevant_line(exc.stderr or exc.output)}", "see the log's tool stderr section"
    if isinstance(exc, subprocess.TimeoutExpired):
        return "TIMEOUT", f"command ran over {exc.timeout} s", "raise the timeout or shrink the input"
    if isinstance(exc, KeyboardInterrupt):
        return "INTERRUPTED", "stopped by Ctrl+C", "rerun"
    if isinstance(exc, SystemExit):
        return "BAD_INPUT", str(exc.code), "see the message"
    where = traceback.extract_tb(exc.__traceback__)[-1] if exc.__traceback__ else None
    at = f" at {Path(where.filename).name}:{where.lineno}" if where else ""
    return "INTERNAL", f"{type(exc).__name__}: {exc}{at}".replace("\n", " ")[:300], "a bug: read the log's traceback"


def _section(title, body):
    return f"== {title} ==\n{body.rstrip()}\n\n"


def _tool_sections(run):
    if run is None:
        return ""
    cmd = run.cmd if hasattr(run, "cmd") else run.args
    code = getattr(run, "returncode", "timeout")
    stderr = clean(run.stderr if isinstance(run.stderr, str) else (run.stderr or b"").decode("utf-8", "replace"))
    tail = "\n".join(stderr.splitlines()[-TAIL:])
    if isinstance(cmd, (list, tuple)):
        cmd = subprocess.list2cmdline(map(str, cmd))
    return (_section("tool", f"{cmd}\nexit {code}")
            + _section(f"tool stderr (last {TAIL})", tail or "(empty)"))


def write_log(tool, code, what, fix, exc=None, *, secrets=(), sections=None, log_name=None):
    """Write one log and keep the newest KEEP for this tool; returns its path (or a note if the
    log can't be written, so the ERROR line still prints)."""
    folder = LOG_ROOT / tool
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    suffix = f"-{log_name}" if log_name else ""
    path = folder / f"{stamp}-{os.getpid()}{suffix}.log"
    safe_argv = [redact(str(arg), secrets) for arg in sys.argv] if secrets else sys.argv
    text = (_section("command", subprocess.list2cmdline(safe_argv) + f"\ncwd {os.getcwd()}")
            + _section("error", f"{code}: {what}\nfix: {fix}"))
    if isinstance(exc, Fail) and exc.detail:
        text += _section("detail", clean(str(exc.detail)))
    tool_run = (getattr(exc, "tool_run", None) if isinstance(exc, Fail) else
                exc if isinstance(exc, (subprocess.CalledProcessError, subprocess.TimeoutExpired)) else None)
    if tool_run is not None and secrets:
        cmd = tool_run.cmd if hasattr(tool_run, "cmd") else tool_run.args
        safe_cmd = [redact(str(arg), secrets) for arg in cmd] if isinstance(cmd, (list, tuple)) else redact(str(cmd), secrets)
        try:
            tool_run = type("RedactedToolRun", (), {"cmd": safe_cmd, "returncode": getattr(tool_run, "returncode", "timeout"),
                                                     "stderr": getattr(tool_run, "stderr", None)})()
        except Exception:
            pass
    text += _tool_sections(tool_run)
    if exc is not None and not isinstance(exc, (Fail, KeyboardInterrupt)):
        text += _section("traceback", "".join(traceback.format_exception(type(exc), exc, exc.__traceback__)))
    for title, body in (sections or {}).items():
        text += _section(title, str(body))
    if secrets:
        text = redact(text, secrets)
    if len(text) > MAX_LOG:
        half = MAX_LOG // 2
        text = text[:half] + f"\n... {len(text) - MAX_LOG} characters cut ...\n" + text[-half:]
    try:
        folder.mkdir(parents=True, exist_ok=True)
        for n in range(2, 100):  # exclusive create: two failures in one second keep both logs
            try:
                with open(path, "x", encoding="utf-8") as f:
                    f.write(text)
                break
            except FileExistsError:
                path = folder / f"{stamp}-{os.getpid()}{suffix}-{n}.log"
    except OSError as e:
        return f"(log not written: {e})"
    for old in sorted(folder.glob("*.log"))[:-KEEP]:
        try:
            old.unlink(missing_ok=True)
        except OSError:  # a locked old log stays; the new one is already written
            pass
    return str(path)


def redact(text, secrets):
    """Redact raw, JSON-escaped and repr spellings of secrets from text."""
    import json
    candidates = set()
    for secret in secrets:
        if secret is None:
            continue
        value = str(secret)
        if value:
            for escaped in (json.dumps(value, ensure_ascii=False), json.dumps(value, ensure_ascii=True)):
                candidates.update((escaped, escaped[1:-1]))
            represented = repr(value)
            candidates.update((value, represented, represented[1:-1]))
    for value in sorted(candidates, key=len, reverse=True):
        text = text.replace(value, "***")
    return text


def fail(tool, exc, *, secrets=(), sections=None, display_tool=None, log_name=None):
    """Print the ERROR line for exc, write its log, and return the exit code (2 for BAD_ARG)."""
    if isinstance(exc, Fail):
        code, what, fix = exc.code, exc.what, exc.fix
    else:
        code, what, fix = _classify(exc)
    safe_what, safe_fix = redact(str(what), secrets), redact(str(fix), secrets)
    log = write_log(tool, code, safe_what, safe_fix, exc, secrets=secrets, sections=sections, log_name=log_name)
    print(f"ERROR {display_tool or tool} {code}: {_one_line(safe_what)} | fix: {_one_line(safe_fix)} | log: {log}", file=sys.stderr, flush=True)
    return 2 if code == "BAD_ARG" else 1


def _parser_error(self, message):
    raise Fail("BAD_ARG", message, f"run {self.prog} --help")


def run(tool, main, *, secrets=None, sections=None, display_tool=None):
    """Run main() so every failure ends as one ERROR line; argparse errors become BAD_ARG."""
    argparse.ArgumentParser.error = _parser_error
    try:
        result = main()
    except SystemExit as e:
        if e.code is None or isinstance(e.code, int):
            raise
        sys.exit(fail(tool, e, secrets=secrets or (), sections=sections, display_tool=display_tool))
    except BaseException as e:  # every failure, Ctrl+C included, ends as one line
        sys.exit(fail(tool, e, secrets=secrets or (), sections=sections, display_tool=display_tool))
    sys.exit(result if isinstance(result, int) else 0)
