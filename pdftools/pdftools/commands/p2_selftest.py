import importlib
import json
import shutil
import subprocess
import tempfile
from pathlib import Path

import pymupdf
from PIL import Image
import clierr

from .. import tools
from ..registry import command, commands_by_phase, output_formats

TOOLS = {  # keys in tools.TOOL_CANDIDATES resolve through tools.find_tool, the rest through PATH
    "ghostscript": ["-version"], "qpdf": ["--version"], "jbig2": ["--version"],
    "pngquant": ["--version"], "tesseract": ["--version"], "ocrmypdf": ["--version"],
    "img2pdf": ["--version"], "soffice": ["--version"], "pandoc": ["--version"],
    "node": ["--version"], "browser": ["--version"],
}
LIBRARIES = ("pymupdf", "pikepdf", "PIL", "numpy", "cv2", "pillow_heif", "pillow_avif", "pdf2docx", "docx")


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
    found = {}
    for name, args in TOOLS.items():
        path = tools.find_tool(name) if name in tools.TOOL_CANDIDATES else shutil.which(name)
        found[name] = {"path": path or "absent", "version": _version(path, args) if path else "absent"}
    libraries = {}
    for name in LIBRARIES:
        try:
            module = importlib.import_module(name)
            libraries[name] = getattr(module, "__version__", getattr(module, "VersionBind", "available"))
        except Exception as exc:
            libraries[name] = f"absent: {exc}"
    node_modules = Path(__file__).resolve().parents[2] / "node_modules"
    for name in ("puppeteer-core", "katex", "marked"):  # md2pdf's Node packages, next to the browser version
        try:
            libraries[name] = json.loads((node_modules / name / "package.json").read_text(encoding="utf-8"))["version"]
        except (OSError, ValueError, KeyError) as exc:
            libraries[name] = f"absent: {exc}"
    return {"tools": found, "libraries": libraries, "formats": output_formats()}


def _format_writer(kind):
    for spec in commands_by_phase().values():
        if spec.output_kind != kind:
            continue
        module = importlib.import_module(spec.run.__module__)
        writer = getattr(module, "selftest_write", None)
        if callable(writer):
            return writer
    return None


def _missing_library_failures(libraries):
    failures = []
    for name in LIBRARIES:
        if str(libraries.get(name, "")).startswith("absent"):
            package = "python-docx" if name == "docx" else name
            failures.append(f"library {package}: {libraries[name]}")
    for name in ("puppeteer-core", "katex", "marked"):
        if str(libraries.get(name, "")).startswith("absent"):
            failures.append(f"library {name}: {libraries[name]}")
    return failures


def _image_format_failures(pdf_path, folder):
    """Write page 1 in every shipped image format and reopen each file to confirm it is readable at the same size."""
    from types import SimpleNamespace
    from . import p3c_helpers
    options = SimpleNamespace(quality=None, lossless=False, tiff_compression=None)
    failures = []
    with pymupdf.open(pdf_path) as doc:
        for name, writer in p3c_helpers.WRITERS.items():
            path = folder / f"test-image.{writer['ext']}"
            try:
                if name == "svg":
                    path.write_text(doc[0].get_svg_image(), encoding="utf-8")
                    if "<svg" not in path.read_text(encoding="utf-8"):
                        raise ValueError("no <svg> element")
                    continue
                pix = doc[0].get_pixmap()
                image = Image.frombytes("RGB", (pix.width, pix.height), pix.samples)
                try:
                    writer["write"](image, path, options)
                finally:
                    image.close()
                problems = p3c_helpers._check_exports([(path, (pix.width, pix.height), 1)])
                if problems:
                    raise ValueError(problems[0][1])
            except Exception as exc:
                failures.append(f"image format {name}: {type(exc).__name__}: {exc}")
    return failures


@command("selftest", 2, "any", "report", "", False, "report")
def selftest(args, context):
    caps = _capabilities()
    if args.caps:
        return {"type": "report", "command": "selftest", "input": "capabilities", "verdict": "report", "data": caps}
    failures = []
    failures.extend(_missing_library_failures(caps["libraries"]))
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
                    problems = _image_format_failures(pdf_path, folder)
                    failures.extend(problems)
                    written.add("" if problems else output_kind)
                else:
                    writer = _format_writer(output_kind)
                    if writer is None:
                        failures.append(f"no selftest_write function for registered format {output_kind}")
                        continue
                    output_path = folder / f"test-write.{output_kind}"
                    try:
                        writer(pdf_path, output_path)
                    except Exception as exc:
                        failures.append(f"output format {output_kind}: {type(exc).__name__}: {exc}")
                        continue
                    written.add(output_kind if output_path.exists() else "")
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
