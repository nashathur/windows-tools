"""KaTeX validation shared by Markdown rendering and PDF-to-Markdown QA."""

import json
import os
import shutil
from pathlib import Path

from . import io, tools
from .errors import PdfError
from .commands.p6_convert import NODE_MODULES, ROOT


def validate(items, temp=None):
    if not items:
        return {}
    node = shutil.which("node")
    if not node:
        raise PdfError("NOT_FOUND", "Node.js is not installed", "install Node.js")
    with (io.temp_folder() if temp is None else _null_context(temp)) as work:
        work = Path(work)
        work.mkdir(parents=True, exist_ok=True)
        source = work / "math-validation.json"
        source.write_text(json.dumps(items, ensure_ascii=False), encoding="utf-8")
        try:
            result = tools.run_tool([node, str(ROOT / "md2pdf.js"), "--validate", str(source)], timeout=60,
                                    env={**os.environ, "NODE_PATH": os.environ.get("NODE_PATH", str(NODE_MODULES))},
                                    cwd=work, temp=work)
        except Exception as exc:
            if getattr(exc, "code", None) == "NOT_FOUND":
                raise PdfError("NOT_FOUND", "Node.js is not installed", "install Node.js")
            if isinstance(exc, PdfError):
                raise
            raise PdfError("TOOL_FAILED", f"KaTeX validation failed: {exc}", "check the Node.js installation", detail=str(exc))
        try:
            parsed = json.loads(result.stdout)
            return {item["id"]: item["error"] for item in parsed}
        except (ValueError, TypeError, KeyError) as exc:
            raise PdfError("TOOL_FAILED", "KaTeX validator returned invalid JSON", "check md2pdf.js", detail=str(exc))


class _null_context:
    def __init__(self, value): self.value = value
    def __enter__(self): return self.value
    def __exit__(self, *_): return False
