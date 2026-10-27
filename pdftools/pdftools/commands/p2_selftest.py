import importlib
import shutil
import subprocess
import tempfile
from pathlib import Path

import pymupdf
import clierr

from ..registry import command, commands_by_phase, output_formats

TOOLS = {
    "gswin64c": ["-version"], "qpdf": ["--version"], "jbig2": ["--version"],
    "pngquant": ["--version"], "tesseract": ["--version"], "ocrmypdf": ["--version"],
    "img2pdf": ["--version"], "soffice": ["--version"], "pandoc": ["--version"],
    "node": ["--version"], "msedge": ["--version"],
}
TOOL_PATHS = {
    "soffice": Path(r"C:\Program Files\LibreOffice\program\soffice.com"),
    "msedge": Path(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"),
}
LIBRARIES = ("pymupdf", "pikepdf", "PIL", "numpy", "cv2", "pillow_heif", "pillow_avif")


def add_arguments(parser):
    parser.add_argument("--caps", action="store_true")


def _version(path, arguments):
    try:
        if Path(path).name.lower() == "msedge.exe":
            command = "(Get-Item -LiteralPath '" + str(path).replace("'", "''") + "').VersionInfo.ProductVersion"
            proc = subprocess.run(["powershell.exe", "-NoProfile", "-Command", command],
                                  capture_output=True, text=True, timeout=4)
            value = (proc.stdout or proc.stderr).strip().splitlines()
            return value[0] if value else f"exit {proc.returncode}"
        proc = subprocess.run([path, *arguments], capture_output=True, text=True, timeout=4)
        line = (proc.stdout or proc.stderr).strip().splitlines()
        return line[0] if line else f"exit {proc.returncode}"
    except Exception as exc:
        return f"version unavailable: {exc}"


def _capabilities():
    tools = {}
    for name, args in TOOLS.items():
        path = shutil.which(name) or (str(TOOL_PATHS[name]) if name in TOOL_PATHS and TOOL_PATHS[name].is_file() else None)
        tools[name] = {"path": path or "absent", "version": _version(path, args) if path else "absent"}
    libraries = {}
    for name in LIBRARIES:
        try:
            module = importlib.import_module(name)
            libraries[name] = getattr(module, "__version__", getattr(module, "VersionBind", "available"))
        except Exception as exc:
            libraries[name] = f"absent: {exc}"
    return {"tools": tools, "libraries": libraries, "formats": output_formats()}


def _format_writer(kind):
    for spec in commands_by_phase().values():
        if spec.output_kind != kind:
            continue
        module = importlib.import_module(spec.run.__module__)
        writer = getattr(module, "selftest_write", None)
        if callable(writer):
            return writer
    return None


@command("selftest", 2, "any", "report", "", False, "report")
def selftest(args, context):
    caps = _capabilities()
    if args.caps:
        return {"type": "report", "command": "selftest", "input": "capabilities", "verdict": "report", "data": caps}
    failures = []
    for name in LIBRARIES:
        if str(caps["libraries"].get(name, "")).startswith("absent"):
            failures.append(f"library {name}: {caps['libraries'][name]}")
    with tempfile.TemporaryDirectory(prefix="pdftools-selftest-") as folder:
        folder = Path(folder)
        pdf_path, png_path = folder / "test.pdf", folder / "test.png"
        try:
            doc = pymupdf.open()
            page = doc.new_page(width=100, height=100)
            page.insert_text((10, 30), "selftest")
            doc.save(pdf_path)
            doc.close()
            with pymupdf.open(pdf_path) as check_doc:
                pix = check_doc[0].get_pixmap()
                pix.save(png_path, output="png")
            written = set()
            for output_kind in output_formats():
                if output_kind == "pdf":
                    with pymupdf.open(pdf_path) as check_doc:
                        check_doc.save(folder / "test-write.pdf")
                    written.add(output_kind if (folder / "test-write.pdf").is_file() else "")
                elif output_kind == "image":
                    with pymupdf.open(pdf_path) as check_doc:
                        check_doc[0].get_pixmap().save(folder / "test-write.png", output="png")
                    written.add(output_kind if (folder / "test-write.png").is_file() else "")
                else:
                    writer = _format_writer(output_kind)
                    if writer is None:
                        failures.append(f"no selftest_write function for registered format {output_kind}")
                        continue
                    output_path = folder / f"test-write.{output_kind}"
                    writer(pdf_path, output_path)
                    written.add(output_kind if output_path.is_file() else "")
            if written != set(output_formats()):
                failures.append("one or more registered output formats could not be written")
        except Exception as exc:
            failures.append(f"render/write: {type(exc).__name__}: {exc}")
    for name, caps_for_tool in caps["tools"].items():
        if caps_for_tool["path"] == "absent":
            failures.append(f"tool {name}: absent")
    for message in failures:
        clierr.warn("pdf selftest", "SELFTEST_FAILED", message, "install or repair the listed dependency")
    context.exit_code = 1 if failures else 0
    if getattr(args, "json", False):
        return {"type": "report", "command": "selftest", "input": "selftest",
                "verdict": "report", "data": {"failures": failures}}
    return None
