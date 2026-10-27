from pathlib import Path

from ..registry import command
from .p3a_core import check_file, run_safely


def add_arguments(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("--against", type=Path)
    parser.add_argument("--map")
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")


@command("check", "3a", "pdf", "report", "", False, "report")
def check_command(args, context):
    if args.map and not args.against:
        from ..errors import PdfError
        raise PdfError("BAD_ARG", "--map requires --against", "use --map with check FILE --against ORIG")
    return run_safely(check_file, args, context)
