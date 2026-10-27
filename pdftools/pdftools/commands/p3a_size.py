from pathlib import Path

from ..registry import command
from .p3a_core import run_safely, size as run_size


def add_arguments(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")


@command("size", "3a", "pdf", "report", "", False, "report")
def size(args, context):
    return run_safely(run_size, args, context)
