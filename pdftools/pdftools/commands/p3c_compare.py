from ..registry import command
from .p3c_helpers import add_compare_arguments, compare_command


def add_arguments(parser):
    add_compare_arguments(parser)


@command("compare", "3c", "pdf", "pdf", "_compare", True, "report")
def compare(args, context):
    return compare_command(args, context)
