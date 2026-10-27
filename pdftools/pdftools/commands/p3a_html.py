from pathlib import Path

from ..registry import command
from .p3a_core import html as run_html, run_safely


def selftest_write(source, output):
    import pymupdf
    with pymupdf.open(source) as doc:
        Path(output).write_text(doc[0].get_text("xhtml"), encoding="utf-8")


def add_arguments(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")


@command("html", "3a", "pdf", "html", "", False, "report")
def html(args, context):
    return run_safely(run_html, args, context)
