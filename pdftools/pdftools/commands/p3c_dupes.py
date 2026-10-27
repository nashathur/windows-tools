from ..registry import command
from .p3c_helpers import add_dupes_arguments, dupes as _dupes


def add_arguments(parser):
    add_dupes_arguments(parser)


@command("dupes", "3c", "folder", "report", "", False, "report")
def dupes(args, context):
    return _dupes(args, context)
