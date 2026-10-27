from pathlib import Path

from ..registry import command
from .p3a_core import open_file, run_safely


def add_arguments(parser):
    parser.add_argument("file", type=Path)


@command("open", "3a", "pdf", "report", "", False, "report")
def open_command(args, context):
    return run_safely(open_file, args, context)
