from pathlib import Path

import pymupdf
from PIL import Image

from .. import coords
from ..errors import PdfError
from ..pipeline import open_input, write_non_pdf
from ..cli import prune_reports, report_path
from ..registry import command


def add_arguments(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("--page", type=int, default=1)
    parser.add_argument("--max-side", type=int, default=800)
    parser.add_argument("--crop", nargs=4, type=float, metavar=("X0", "Y0", "X1", "Y1"))
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")


@command("thumb", 2, "any", "image", "_thumb", False, "report")
def thumb(args, context):
    path = args.file.resolve()
    if args.max_side < 1:
        raise PdfError("BAD_ARG", "--max-side must be positive", "use a positive pixel count")
    reports = Path(__file__).resolve().parents[2] / "reports"
    extension = ".png"
    reports.mkdir(parents=True, exist_ok=True)
    target = args.output or report_path(reports, "thumb", path.stem, extension)
    args.output = target
    is_pdf = path.suffix.lower() == ".pdf"
    if is_pdf:
        with open_input(path, context.password or context.owner_password) as doc:
            if not 1 <= args.page <= doc.page_count:
                raise PdfError("BAD_PAGE", f"page {args.page} is outside 1-{doc.page_count}", "choose an existing page")
            page = doc[args.page - 1]
            if args.crop:
                factor = coords.MM_PER_POINT
                clip = pymupdf.Rect(*(value / factor for value in args.crop))
            else:
                clip = None
            dimensions = clip or page.rect
            scale = min(args.max_side / max(dimensions.width, dimensions.height), 600 / 72)
            pix = page.get_pixmap(matrix=pymupdf.Matrix(scale, scale),
                                  clip=clip, alpha=False)
            image = Image.frombytes("RGB", (pix.width, pix.height), pix.samples)
    else:
        try:
            import pillow_heif
            pillow_heif.register_heif_opener()
            import pillow_avif
        except ImportError:
            pass
        image = Image.open(path).convert("RGB")
        if args.crop:
            image = image.crop(tuple(round(x) for x in args.crop))
    image.thumbnail((args.max_side, args.max_side), Image.Resampling.LANCZOS)
    def save(staged):
        image.save(staged, format="PNG")
    result = write_non_pdf(path, _spec(), args, save, kind="image", password=context.password or context.owner_password)
    prune_reports(reports)
    return result


def _spec():
    from ..registry import get
    return get("thumb")
