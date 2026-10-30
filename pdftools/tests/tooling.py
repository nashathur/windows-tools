"""Shared external-tool availability and version accounting for integration tests."""
import importlib.metadata
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest import mock
from pdftools import tools as pdf_tools

TOOL_USAGE = {}
_TOOL_INFO = {}


def tool_info(name):
    if name in _TOOL_INFO:
        return _TOOL_INFO[name]
    if name == "pdf2docx":
        try:
            result = (name, importlib.metadata.version(name))
        except importlib.metadata.PackageNotFoundError:
            result = (None, "pdf2docx is not installed")
        _TOOL_INFO[name] = result
        return result
    folders = ((Path(sys.prefix) / "Library" / "bin", Path(sys.prefix) / "Scripts") if os.name == "nt"
               else (Path(sys.prefix) / "bin",))
    with mock.patch.dict(os.environ, {"PATH": os.pathsep.join([os.environ.get("PATH", ""), *map(str, folders)])}):
        path = pdf_tools.find_tool(name) if name in pdf_tools.TOOL_CANDIDATES else shutil.which(name)
    if not path:
        result = (None, f"{name} executable is absent")
        _TOOL_INFO[name] = result
        return result
    from pdftools.commands.p2_selftest import _version
    version_args = {"ghostscript": ["-version"]}.get(name, ["--version"])
    version = _version(path, version_args)
    probe_error = None
    if name == "node":
        try:
            toolkit = Path(__file__).resolve().parents[1]
            child_env = dict(os.environ, NODE_PATH=str(toolkit / "node_modules"))
            check = subprocess.run([path, "-e", "require.resolve('marked');require.resolve('katex');require.resolve('puppeteer-core')"],
                                   capture_output=True, text=True, timeout=8, env=child_env)
            if check.returncode:
                probe_error = (check.stderr or check.stdout or "Node package resolution failed").strip()
        except Exception as exc:
            probe_error = str(exc)
    try:
        if name == "soffice":
            with tempfile.TemporaryDirectory(prefix="pdftools-soffice-probe-") as folder:
                folder = Path(folder); source = folder / "probe.rtf"; source.write_text(r"{\rtf1\ansi probe}", encoding="ascii")
                profile = (folder / "profile").as_uri()
                converted = subprocess.run([path, "--headless", f"-env:UserInstallation={profile}",
                                            "--convert-to", "pdf", "--outdir", str(folder), str(source)],
                                           capture_output=True, text=True, timeout=15)
                if converted.returncode or not (folder / "probe.pdf").is_file():
                    probe_error = (f"exit {converted.returncode}; stdout: {(converted.stdout or '').strip()}; "
                                   f"stderr: {(converted.stderr or '').strip()}; "
                                   f"files: {[p.name for p in folder.iterdir()]}")
    except Exception as exc:
        probe_error = str(exc)
    result = (path, f"{version}; probe failed: {probe_error}" if probe_error else version)
    _TOOL_INFO[name] = result
    return result


def require_tool(testcase, name, reason):
    path, version = tool_info(name)
    if path and version and "probe failed:" in version:
        testcase.fail(f"tool {name} functional probe failed: {version}")
    if not path:
        missing_reason = f"{reason} ({version or 'executable is absent'})"
        if os.environ.get("PDFTOOLS_REQUIRE_TOOLS") == "1":
            testcase.fail(f"required tool {name} is missing or unusable: {missing_reason}")
        print(f"SKIP {name}: {missing_reason}", file=sys.stderr)
        testcase.skipTest(missing_reason)
    usage = TOOL_USAGE.setdefault(name, {"version": version, "tests": set()})
    usage["tests"].add(testcase.id())
    return path, version
