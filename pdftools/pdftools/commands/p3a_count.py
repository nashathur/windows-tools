from pathlib import Path

from ..registry import command
from .p3a_core import count as run_count, run_safely


def add_arguments(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("--total", action="store_true")
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")


@command("count", "3a", "pdf", "report", "", False, "report")
def count(args, context):
    return run_safely(run_count, args, context)
