from pathlib import Path

from ..registry import command
from .p3a_core import run_safely, text as run_text


def selftest_write(source, output):
    import pymupdf
    with pymupdf.open(source) as doc:
        Path(output).write_text(doc[0].get_text("text"), encoding="utf-8")


def add_arguments(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("--layout", action="store_true")
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")


@command("text", "3a", "pdf", "txt", "", False, "report")
def text(args, context):
    return run_safely(run_text, args, context)
