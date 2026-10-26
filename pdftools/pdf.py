import os
import sys
from pathlib import Path
import logging

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT.parent / "common"))
os.environ.setdefault("PYTHONUTF8", "1")
os.environ.setdefault("PYTHONIOENCODING", "utf-8")
for stream in (sys.stdout, sys.stderr):
    if hasattr(stream, "reconfigure"):
        stream.reconfigure(encoding="utf-8")

from pdftools.errors import library_warnings


class _Capture(logging.Handler):
    def emit(self, record):
        message = self.format(record)
        library_warnings.append(message)
        if "-v" in sys.argv:
            print(message, file=sys.stderr, flush=True)


logger = logging.getLogger("pymupdf")
logger.handlers.clear()
logger.addHandler(_Capture())
logger.propagate = False
logger.setLevel(logging.WARNING)
os.environ["PYMUPDF_MESSAGE"] = "logging:name=pymupdf,level=30"

import pymupdf

from pdftools.cli import main

if __name__ == "__main__":
    raise SystemExit(main())
