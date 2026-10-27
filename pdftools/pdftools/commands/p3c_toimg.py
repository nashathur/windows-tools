from ..registry import command
from .p3c_helpers import add_toimg_arguments, toimg as _toimg
import pymupdf


def add_arguments(parser):
    add_toimg_arguments(parser)


@command("toimg", "3c", "pdf", "image", "", False, "report")
def toimg(args, context):
    return _toimg(args, context)


def selftest_write(source, output):
    output.mkdir(parents=True)
    with pymupdf.open(source) as doc:
        doc[0].get_pixmap().save(output / "page.png", output="png")
