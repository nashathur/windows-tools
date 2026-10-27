from pathlib import Path

from ..registry import command
from .p3a_core import json_report, run_safely


def selftest_write(source, output):
    import pymupdf
    with pymupdf.open(source) as doc:
        Path(output).write_text(doc[0].get_text("json"), encoding="utf-8")


def add_arguments(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")


@command("json", "3a", "pdf", "json", "", False, "report")
def json_command(args, context):
    return run_safely(json_report, args, context)
