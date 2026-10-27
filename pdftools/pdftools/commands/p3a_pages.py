from pathlib import Path

from ..registry import command
from .p3a_core import pages as run_pages, run_safely


def add_arguments(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")


@command("pages", "3a", "pdf", "report", "", False, "report")
def pages(args, context):
    return run_safely(run_pages, args, context)
