from ..registry import command
from .p3c_helpers import add_zip_arguments, selftest_write, zip_command


def add_arguments(parser):
    add_zip_arguments(parser)


@command("zip", "3c", "any", "zip", "", False, "report")
def zip_files(args, context):
    return zip_command(args, context)
