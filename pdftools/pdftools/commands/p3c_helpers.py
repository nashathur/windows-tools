import hashlib
import math
import shutil
import zipfile
import argparse
from pathlib import Path

import pymupdf
from PIL import Image, ImageDraw

from .. import check, coords, crypt, io
from ..errors import PdfError
from ..pipeline import open_input, write_pdf, _signed
from ..check import normalize_words
import clierr


def add_toimg_arguments(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("--format", default="png")
    size = parser.add_mutually_exclusive_group()
    size.add_argument("--dpi", type=float, default=None)
    size.add_argument("--width", type=int)
    size.add_argument("--height", type=int)
    size.add_argument("--scale", type=float)
    parser.add_argument("--pages")
    parser.add_argument("--clip", type=_clip)
    parser.add_argument("--gray", action="store_true")
    parser.add_argument("--bw", action="store_true")
    parser.add_argument("--threshold", type=int, default=128)
    parser.add_argument("--alpha", action="store_true")
    parser.add_argument("--quality", type=int)
    parser.add_argument("--lossless", action="store_true")
    parser.add_argument("--single", action="store_true")
    parser.add_argument("--tiff-compression", choices=("lzw", "deflate", "group4"))
    parser.add_argument("--svg-text", action="store_true")
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--stitch", choices=("v", "h"))
    group.add_argument("--grid", type=_grid)
    parser.add_argument("--zip", action="store_true")
    # Keep the original spelling so io.resolve_output can recognize a trailing
    # separator for a directory that does not exist yet.
    parser.add_argument("-o", "--output")
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")


def _clip(value):
    try:
        values = tuple(float(x) for x in value.split(","))
        if len(values) != 4 or not all(math.isfinite(x) for x in values) or values[2] <= values[0] or values[3] <= values[1]:
            raise ValueError()
        return values
    except ValueError:
        raise PdfError("BAD_ARG", "--clip needs x0,y0,x1,y1 with positive area", "use displayed millimetres, for example 0,0,100,150")


def _grid(value):
    try:
        cols, rows = (int(x) for x in value.lower().split("x"))
        if cols < 1 or rows < 1:
            raise ValueError()
        return cols, rows
    except ValueError:
        raise PdfError("BAD_ARG", "--grid needs CxR positive integers", "use a value such as 3x2")


WRITERS = {
    "png": {"ext": "png", "write": lambda image, path, args: image.save(path, format="PNG"), "alpha": True, "quality": False},
    "jpeg": {"ext": "jpg", "write": lambda image, path, args: image.convert("RGB").save(path, format="JPEG", quality=args.quality or 90), "alpha": False, "quality": True},
    "webp": {"ext": "webp", "write": lambda image, path, args: image.save(path, format="WEBP", quality=args.quality or 80, lossless=args.lossless), "alpha": True, "quality": True},
    "tiff": {"ext": "tiff", "write": lambda image, path, args: image.save(path, format="TIFF", compression=_TIFF_COMPRESSION.get(args.tiff_compression, "tiff_lzw")), "alpha": True, "quality": False},
    "bmp": {"ext": "bmp", "write": lambda image, path, args: image.convert("RGB").save(path, format="BMP"), "alpha": False, "quality": False},
    "gif": {"ext": "gif", "write": lambda image, path, args: image.save(path, format="GIF"), "alpha": False, "quality": False},
    "avif": {"ext": "avif", "write": lambda image, path, args: _write_avif(image, path, args), "alpha": True, "quality": True, "lossless": False},
    "heic": {"ext": "heic", "write": lambda image, path, args: _write_heic(image, path, args), "alpha": False, "quality": True, "lossless": False},
    # SVG is vector: it is written from the page itself, not from a raster (see _emit_page).
    "svg": {"ext": "svg", "write": None, "alpha": False, "quality": False},
}
_TIFF_COMPRESSION = {"lzw": "tiff_lzw", "deflate": "tiff_adobe_deflate", "group4": "group4"}
_ALIASES = {"jpg": "jpeg", "tif": "tiff", "heif": "heic"}


def _write_avif(image, path, args):
    import pillow_avif  # plugin registration
    image.save(path, format="AVIF", quality=args.quality or 80)


def _write_heic(image, path, args):
    import pillow_heif
    pillow_heif.register_heif_opener()
    image.convert("RGB").save(path, format="HEIF", quality=args.quality or 80)


def _validate_toimg(args, fmt):
    supported = ", ".join(WRITERS)
    if fmt not in WRITERS:
        raise PdfError("BAD_ARG", f"unsupported image format {fmt}; supported formats: {supported}", f"choose {supported}")
    options = WRITERS[fmt]
    if args.alpha and not options["alpha"]:
        raise PdfError("BAD_ARG", f"--alpha is not supported for {fmt}", "use PNG")
    if args.quality is not None and not options["quality"]:
        raise PdfError("BAD_ARG", f"--quality is not supported for {fmt}", "choose a format with a quality setting")
    if args.lossless and (fmt not in {"webp"}):
        raise PdfError("BAD_ARG", f"--lossless is not supported for {fmt}", "use WebP")
    if args.single and fmt != "tiff":
        raise PdfError("BAD_ARG", "--single is supported only for TIFF", "choose --format tiff")
    if args.tiff_compression and fmt != "tiff":
        raise PdfError("BAD_ARG", "--tiff-compression is supported only for TIFF", "choose --format tiff")
    if args.tiff_compression == "group4" and not args.bw:
        raise PdfError("BAD_ARG", "TIFF group4 compression requires --bw", "add --bw")
    if fmt == "svg" and (args.clip or args.gray or args.bw or args.alpha or args.width or args.height or args.scale is not None or args.dpi is not None):
        raise PdfError("BAD_ARG", "SVG does not support raster options", "remove --clip, --gray, --bw, --alpha and size options")
    if fmt == "svg" and args.single:
        raise PdfError("BAD_ARG", "SVG does not support --single", "choose TIFF for multi-page output")
    if fmt == "svg" and (args.stitch or args.grid):
        raise PdfError("BAD_ARG", "SVG does not support --stitch or --grid", "choose PNG for a combined image")
    if args.svg_text and fmt != "svg":
        raise PdfError("BAD_ARG", "--svg-text is supported only for SVG", "choose --format svg")
    if args.single and (args.stitch or args.grid):
        raise PdfError("BAD_ARG", "--single, --stitch and --grid are mutually exclusive", "choose one of them")
    if args.dpi is not None and (not math.isfinite(args.dpi) or args.dpi <= 0) or args.scale is not None and (not math.isfinite(args.scale) or args.scale <= 0):
        raise PdfError("BAD_ARG", "render size must be positive", "use a positive dpi or scale")
    if args.width is not None and args.width < 1 or args.height is not None and args.height < 1:
        raise PdfError("BAD_ARG", "image dimensions must be positive", "use positive pixel dimensions")
    if args.quality is not None and not 1 <= args.quality <= 100:
        raise PdfError("BAD_ARG", "--quality must be 1-100", "choose a JPEG quality from 1 to 100")
    if not 0 <= args.threshold <= 255:
        raise PdfError("BAD_ARG", "--threshold must be 0-255", "choose a value from 0 to 255")
    if args.bw and args.gray:
        raise PdfError("BAD_ARG", "--bw and --gray are mutually exclusive", "choose one colour mode")
    if args.alpha and (args.bw or args.grid or args.stitch):
        raise PdfError("BAD_ARG", "--alpha cannot be combined with --bw, --grid or --stitch", "export individual PNG pages")


def _pixel_dimensions(rect, scale, cap):
    pixel_width, pixel_height = rect.width * scale, rect.height * scale
    if not math.isfinite(scale) or not math.isfinite(pixel_width) or not math.isfinite(pixel_height):
        raise PdfError("TOO_BIG", f"render exceeds the {cap} pixel limit", "lower --dpi, --scale, --width or --height")
    width, height = math.ceil(pixel_width), math.ceil(pixel_height)
    if width * height > cap:
        raise PdfError("TOO_BIG", f"render exceeds the {cap} pixel limit", "lower --dpi, --scale, --width or --height")
    return width, height


def _render(doc, page, args):
    rect = page.rect
    clip = None
    if args.clip:
        # get_pixmap clips the already rotated display list, so convert displayed
        # millimetres back to page space and then into displayed PyMuPDF space.
        clip = coords.display_to_pymupdf(page, pymupdf.Rect(args.clip)) * page.rotation_matrix
        clip &= rect
        if clip.is_empty:
            raise PdfError("BAD_ARG", "--clip does not intersect the page", "choose a displayed-mm box inside the page")
        rect = clip
    try:
        if args.width or args.height:
            scale = min(args.width / rect.width if args.width else float("inf"), args.height / rect.height if args.height else float("inf"))
        else:
            scale = args.scale if args.scale is not None else (args.dpi or 150) / 72
    except OverflowError as exc:
        raise PdfError("TOO_BIG", "render dimensions exceed the supported pixel limit", "lower the requested image size") from exc
    cap = 200_000_000 if args.stitch else 100_000_000
    _pixel_dimensions(rect, scale, cap)
    pix = page.get_pixmap(matrix=pymupdf.Matrix(scale, scale), clip=clip, alpha=args.alpha)
    image = Image.frombytes("RGBA" if pix.alpha else ("L" if pix.colorspace.n == 1 else "RGB"), (pix.width, pix.height), pix.samples)
    if args.gray or args.bw:
        if args.gray and args.alpha:
            image = image.convert("RGBA").convert("LA")
        else:
            image = image.convert("L")
        if args.bw:
            image = image.point(lambda x: 255 if x >= args.threshold else 0, mode="1")
    return image


def _output_folder(source, ext, output):
    """`<stem>_<ext>` beside the PDF, named after the written file extension (`doc_jpg`, not `doc_jpeg`)."""
    if output is None:
        return Path(source).with_name(f"{Path(source).stem}_{ext}")
    return io.resolve_output(source, ext, output, multi=True)


def _zip_paths(inputs, target):
    target = Path(target)
    target.parent.mkdir(parents=True, exist_ok=True)
    # Snapshot before the archive staging file exists. This also makes folder
    # inputs deterministic if the destination lives inside the source folder.
    snapshots = []
    for raw in inputs:
        path = Path(raw)
        files = [path] if path.is_file() else sorted(x for x in path.rglob("*") if x.is_file() and x.resolve() != target.resolve())
        snapshots.append((path, files))
    with io.temp_folder() as temp, io.Staging(target, temp) as stage:
        with zipfile.ZipFile(stage.path, "w", zipfile.ZIP_DEFLATED) as archive:
            members = set()
            for path, files in snapshots:
                for file in files:
                    # A single input file uses its own name; folder inputs keep
                    # their relative structure. Multiple files with equal names
                    # are placed under numbered top-level folders below.
                    if path.is_file():
                        member = file.name
                        if member in members:
                            parent = path.parent.name or "files"
                            member = str(Path(parent) / member)
                            index = 2
                            while member in members:
                                member = str(Path(f"{parent}_{index}") / file.name)
                                index += 1
                    else:
                        member = str(Path(path.name) / file.relative_to(path))
                        if member in members:
                            index = 2
                            while True:
                                candidate = str(Path(f"{path.name}_{index}") / file.relative_to(path))
                                if candidate not in members:
                                    member = candidate
                                    break
                                index += 1
                    archive.write(file, member)
                    members.add(member)
        return stage.publish()


def _check_exports(items):
    """Reopen every written raster with Pillow and compare its size (and TIFF frame count)."""
    try:
        import pillow_heif
        pillow_heif.register_heif_opener()
        import pillow_avif  # noqa: F401
    except Exception:
        pass
    problems = []
    for path, size, frames in items:
        # `size` is one (w, h) for every frame, or a list with one (w, h) per frame (pages of different sizes).
        expected = [tuple(s) for s in size] if isinstance(size[0], (tuple, list)) else [tuple(size)] * frames
        try:
            with Image.open(path) as written:
                ok = getattr(written, "n_frames", 1) == len(expected)
                for index in range(getattr(written, "n_frames", 1)):
                    written.seek(index)
                    written.load()
                    ok = ok and index < len(expected) and written.size == expected[index]
        except Exception:
            ok = False
        if not ok:
            problems.append(("EXPORT_FAILED", f"exported image failed verification: {Path(path).name}"))
    return problems


def _mark_unchecked(stage, target, context, problems):
    if not problems:
        return None
    stage.target = target.with_name(target.stem + "_UNCHECKED" + target.suffix)
    context.exit_code = 3
    log = clierr.write_log("pdf", "CHECK_FAILED", "toimg invariant check failed",
                           "review the exported images", sections={"findings": str(problems)}, log_name="toimg")
    for code, message in problems:
        clierr.warn("pdf toimg", code, message, "review the exported images")
    return log


def _emit_page(doc, n, fmt, path, args):
    """Write page n to path; return (size_px, frames) for rasters or None for SVG."""
    if fmt == "svg":
        text = doc[n].get_svg_image(text_as_path=not args.svg_text)
        Path(path).write_text(text, encoding="utf-8")
        return None
    image = _render(doc, doc[n], args)
    try:
        WRITERS[fmt]["write"](image, path, args)
        return image.size, 1
    finally:
        image.close()


def _write_tiff_single(doc, pages, path, args):
    from PIL.TiffImagePlugin import AppendingTiffWriter
    compression = _TIFF_COMPRESSION.get(args.tiff_compression, "tiff_lzw")
    sizes = []
    with AppendingTiffWriter(str(path), new=True) as writer:
        for n in pages:
            image = _render(doc, doc[n], args)
            try:
                image.save(writer, format="TIFF", compression=compression)
                writer.newFrame()
                sizes.append(image.size)
            finally:
                image.close()
    return sizes


def _result(final, kind, pages, warnings, verdict_ok, **extra):
    # "ok" only when the written content was reopened and matched; SVG is not checked.
    verdict = "warn" if warnings else ("ok" if verdict_ok else "report")
    return {"type": "result", "command": "toimg", "path": str(final), "kind": kind, "pages": pages,
            "bytes": (final.stat().st_size if final.is_file() else
                      sum(x.stat().st_size for x in final.iterdir() if x.is_file())),
            "verdict": verdict, "warnings": warnings, **extra}


def toimg(args, context):
    source = args.file.resolve()
    fmt = args.format.lower()
    fmt = _ALIASES.get(fmt, fmt)
    _validate_toimg(args, fmt)
    ext = WRITERS[fmt]["ext"]
    warnings = []
    try:
        import pikepdf
        with pikepdf.open(source, password=context.password or context.owner_password or "") as pdf:
            if pdf.is_encrypted:
                warnings.extend(crypt.export_warnings(True))
    except Exception:
        pass
    if fmt == "svg" and args.svg_text:
        warnings.append("SVG_TEXT_NEEDS_FONTS")
    with open_input(source, context.password or context.owner_password) as doc:
        from ..pages import parse_page_ranges
        pages = parse_page_ranges(args.pages, doc.page_count) if args.pages else list(range(doc.page_count))
        if _signed(source, context.password or context.owner_password):
            warnings.append("SIGNATURE_NOT_RETAINED")
        if args.zip and (len(pages) == 1 or args.stitch or args.grid or args.single):
            raise PdfError("BAD_ARG", "--zip requires a multi-file image export", "select multiple pages")
        # Estimate from page geometry before any raster is allocated, then check
        # both the destination and run-temp drives using the shared budget.
        dims = []
        for n in pages:
            if fmt == "svg":
                dims.append((0, 0))  # vector output: no raster is allocated, so no pixel limit applies
                continue
            page = doc[n]
            rect = page.rect
            if args.clip:
                clip = coords.display_to_pymupdf(page, pymupdf.Rect(args.clip)) * page.rotation_matrix
                rect = clip & rect
                if rect.is_empty:
                    raise PdfError("BAD_ARG", "--clip does not intersect the page", "choose a displayed-mm box inside the page")
            try:
                scale = (min(args.width / rect.width if args.width else float("inf"),
                             args.height / rect.height if args.height else float("inf"))
                         if args.width or args.height else args.scale if args.scale is not None else (args.dpi or 150) / 72)
            except OverflowError as exc:
                raise PdfError("TOO_BIG", "render dimensions exceed the supported pixel limit", "lower the requested image size") from exc
            cap = 200_000_000 if args.stitch else 100_000_000
            width, height = _pixel_dimensions(rect, scale, cap)
            dims.append((width, height))
        page_count = doc.page_count

    def finish(final, kind, count, checked, unchecked=False, log=None, expected=None, **extra):
        # An unchecked export is published under its _UNCHECKED name on purpose; that is not a name clash.
        if final != (expected or target_path[0]):
            warnings.append("NAME_TAKEN")
        for code in warnings:
            clierr.warn("pdf toimg", code, "image export completed with a warning", "review the exported image")
        record = _result(final, kind, count, warnings, checked, **extra)
        if unchecked:
            record["verdict"] = "unchecked"
            record["log"] = log
        return record

    target_path = [None]
    if args.single:
        target = target_path[0] = io.resolve_output(source, "toimg", args.output, suffix="_single", extension=".tiff")
        if args.dry_run:
            return {"type":"result","command":"toimg","path":str(target),"kind":"image","pages":len(pages),"bytes":0,"verdict":"report","provisional":True,"warnings":warnings}
        target.parent.mkdir(parents=True, exist_ok=True)
        with io.temp_folder() as temp:
            io.disk_budget([source], target, temp, estimate=sum(w * h * 4 for w, h in dims))
            with io.Staging(target, temp) as stage:
                with open_input(source, context.password or context.owner_password) as doc:
                    sizes = _write_tiff_single(doc, pages, stage.path, args)
                problems = _check_exports([(stage.path, sizes, len(sizes))])
                log = _mark_unchecked(stage, target, context, problems)
                final = stage.publish()
        return finish(final, "image", len(pages), True, bool(problems), log, expected=stage.target)
    if len(pages) == 1 and not (args.stitch or args.grid):
        target = target_path[0] = io.resolve_output(source, "toimg", args.output,
                                                    suffix=f"_p{pages[0] + 1:03d}", extension=f".{ext}")
        if args.dry_run:
            return {"type":"result","command":"toimg","path":str(target),"kind":"image","pages":1,"bytes":0,"verdict":"report","provisional":True,"warnings":warnings}
        target.parent.mkdir(parents=True, exist_ok=True)
        with io.temp_folder() as temp:
            io.disk_budget([source], target, temp, estimate=dims[0][0] * dims[0][1] * 4 or None)
            with io.Staging(target, temp) as stage:
                with open_input(source, context.password or context.owner_password) as doc:
                    done = _emit_page(doc, pages[0], fmt, stage.path, args)
                problems = _check_exports([(stage.path, done[0], 1)]) if done else []
                log = _mark_unchecked(stage, target, context, problems)
                final = stage.publish()
        return finish(final, "image", 1, bool(done), bool(problems), log, expected=stage.target)
    if args.stitch or args.grid:
        if args.grid:
            cols, rows = args.grid
            if len(pages) > cols * rows: raise PdfError("BAD_ARG", "grid is too small for selected pages", "increase --grid")
        if args.stitch == "v":
            canvas_size = (max(w for w, h in dims), sum(h for w, h in dims))
        elif args.stitch == "h":
            canvas_size = (sum(w for w, h in dims), max(h for w, h in dims))
        else:
            w, h = max(w for w, h in dims), max(h for w, h in dims)
            canvas_size = (cols * w, rows * (h + 24))
        cap = 200_000_000 if args.stitch else 100_000_000
        if canvas_size[0] * canvas_size[1] > cap:
            raise PdfError("TOO_BIG", f"combined image exceeds the {cap} pixel limit", "choose fewer pages or a smaller render size")
        suffix = "_stitch" if args.stitch else "_grid"
        target = target_path[0] = io.resolve_output(source, "toimg", args.output, suffix=suffix, extension=f".{ext}")
        if args.dry_run:
            return {"type":"result","command":"toimg","path":str(target),"kind":"image","pages":len(pages),"bytes":0,"verdict":"report","provisional":True,"page_map":[{"page":n+1,"path":str(target)} for n in pages],"warnings":warnings}
        target.parent.mkdir(parents=True, exist_ok=True)
        with io.temp_folder() as temp:
            estimate = canvas_size[0] * canvas_size[1] * 4
            io.disk_budget([source], target, temp, estimate=estimate)
            with open_input(source, context.password or context.owner_password) as doc:
                images = [(n + 1, _render(doc, doc[n], args)) for n in pages]
            if args.stitch == "v":
                canvas = Image.new(images[0][1].mode, canvas_size, "white")  # uncovered area is white, not black
                y = 0
                for _, image in images: canvas.paste(image, (0, y)); y += image.height
            elif args.stitch == "h":
                canvas = Image.new(images[0][1].mode, canvas_size, "white")
                x = 0
                for _, image in images: canvas.paste(image, (x, 0)); x += image.width
            else:
                w, h = max(i.width for _, i in images), max(i.height for _, i in images)
                canvas = Image.new("RGB", canvas_size, "white")
                draw = ImageDraw.Draw(canvas)
                for idx, (number, image) in enumerate(images):
                    x, y = (idx % cols) * w, (idx // cols) * (h + 24)
                    canvas.paste(image.convert("RGB"), (x, y + 24)); draw.text((x + 4, y + 4), str(number), fill="black")
            with io.Staging(target, temp) as stage:
                if fmt == "tiff" and args.tiff_compression == "group4":
                    canvas = canvas.convert("1")
                WRITERS[fmt]["write"](canvas, stage.path, args)
                problems = _check_exports([(stage.path, canvas.size, 1)])
                log = _mark_unchecked(stage, target, context, problems)
                final = stage.publish()
        return finish(final, "image", len(pages), True, bool(problems), log, expected=stage.target)
    folder = _output_folder(source, ext, args.output)
    target_path[0] = folder
    digits = max(3, len(str(page_count)))
    if args.dry_run:
        zip_target = folder.with_name(folder.name + ".zip") if args.zip else None
        return {"type":"result","command":"toimg","path":str(folder),"kind":"folder","pages":len(pages),"bytes":0,"verdict":"report","provisional":True,"page_map":[{"page":n+1,"path":str(folder / f"{source.stem}_p{n+1:0{digits}d}.{ext}")} for n in pages],"zip_path":str(zip_target) if zip_target else None,"warnings":warnings}
    folder.parent.mkdir(parents=True, exist_ok=True)
    with io.temp_folder() as temp:
        io.disk_budget([source], folder, temp, estimate=sum(w*h*4 for w, h in dims) or None)
        staged = io.create_staging(folder, temp); staged.mkdir()
        try:
            written = []
            with open_input(source, context.password or context.owner_password) as doc:
                for n in pages:
                    path = staged / f"{source.stem}_p{n+1:0{digits}d}.{ext}"
                    done = _emit_page(doc, n, fmt, path, args)
                    if done:
                        written.append((path, done[0], 1))
            problems = _check_exports(written)
            log = _mark_unchecked(type("Stage", (), {"target": folder})(), folder, context, problems)
            if problems:
                # Directory outputs use the same suffix protocol as file outputs.
                staged = staged.rename(staged.with_name(staged.name + "_UNCHECKED"))
                final = io.publish(staged, folder.with_name(folder.name + "_UNCHECKED"))
            else:
                final = io.publish(staged, folder)
        finally:
            if staged.exists(): shutil.rmtree(staged)
    if final != (folder.with_name(folder.name + "_UNCHECKED") if problems else folder): warnings.append("NAME_TAKEN")
    zip_final = None
    if args.zip:
        zpath = final.with_name(final.name + ".zip")
        try:
            zip_final = _zip_paths([final], zpath)
            if zip_final != zpath: warnings.append("NAME_TAKEN")
        except Exception as exc:
            warnings.append("ZIP_FAILED")
            clierr.warn("pdf toimg", "ZIP_FAILED", f"image folder published at {final}; ZIP creation failed: {exc}", "use the published image folder")
    for code in warnings:
        if code != "ZIP_FAILED": clierr.warn("pdf toimg", code, "image export completed with a warning", "review the exported images")
    record = _result(final, "folder", len(pages), warnings, fmt != "svg", **({"zip_path": str(zip_final)} if zip_final else {}))
    if problems:
        record["verdict"] = "unchecked"
        record["log"] = log
    return record


def add_zip_arguments(parser):
    parser.add_argument("inputs", nargs="+", type=Path)
    # Preserve a trailing separator so a not-yet-created output directory is
    # distinguishable from an exact archive filename.
    parser.add_argument("-o", "--output")


def zip_command(args, context):
    inputs = [p.resolve() for p in args.inputs]
    if any(not p.exists() for p in inputs): raise PdfError("NOT_FOUND", "input does not exist", "check the input paths")
    base = inputs[0]
    suffix = base.suffix
    target = io.resolve_output(base, "zip", args.output, suffix=suffix, extension=".zip")
    if args.dry_run: return {"type":"result","command":"zip","path":str(target),"kind":"zip","pages":None,"bytes":0,"verdict":"report","warnings":[]}
    target.parent.mkdir(parents=True, exist_ok=True)
    final = _zip_paths(inputs, target)
    warnings = ["NAME_TAKEN"] if final != target else []
    for code in warnings: clierr.warn("pdf zip", code, "archive published under an available name", "review the output path")
    return {"type":"result","command":"zip","path":str(final),"kind":"zip","pages":None,"bytes":final.stat().st_size,"verdict":"warn" if warnings else "ok","warnings":warnings}


def selftest_write(source, output):
    _zip_paths([Path(source)], Path(output))


def add_dupes_arguments(parser):
    parser.add_argument("folder", type=Path)
    parser.add_argument("--similar", action="store_true")
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")


def dupes(args, context):
    root = args.folder.resolve()
    if not root.is_dir(): raise PdfError("BAD_INPUT", f"not a folder: {root}", "choose a folder")
    groups = {}
    files = sorted(p for p in root.rglob("*") if p.is_file())
    for path in files:
        hasher = hashlib.sha256()
        with path.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                hasher.update(chunk)
        digest = hasher.hexdigest()
        groups.setdefault(digest, []).append(str(path))
    identical = [paths for paths in groups.values() if len(paths) > 1]
    candidates = []
    uninspected = []
    if args.similar:
        texts = {}
        for path in files:
            if path.suffix.lower() != ".pdf": continue
            try:
                with open_input(path, context.password or context.owner_password) as doc:
                    text = " ".join(normalize_words(" ".join(page.get_text() for page in doc).split())).casefold()
                    if text: texts.setdefault((len(doc), text), []).append(str(path))
            except Exception as exc:
                uninspected.append({"path": str(path), "reason": str(exc)})
        candidates = [paths for paths in texts.values() if len(paths) > 1]
    return {"type":"report","command":"dupes","input":str(root),"verdict":"report","data":{"identical":identical,"similar_candidates":candidates,"uninspected":uninspected}}


def add_compare_arguments(parser):
    parser.add_argument("a", type=Path); parser.add_argument("b", type=Path)
    parser.add_argument("-o", "--output")
    parser.add_argument("--break-signature", action="store_true")
    parser.add_argument("--ignore-restrictions", action="store_true")




def compare_command(args, context):
    from .p3a_core import check_file
    from ..registry import get

    original = args.a.expanduser().resolve()
    changed = args.b.expanduser().resolve()
    report = check_file(argparse.Namespace(file=changed, against=str(original), map=None,
                                           v=getattr(args, "verbose", False)), context)
    if getattr(args, "output", None) is None:
        default_name = io.resolve_output(original, "compare", suffix=f"_vs_{changed.stem}", extension=".pdf")
        target = changed.with_name(default_name.name)
        args.output = target
    else:
        target = io.resolve_output(original, "compare", args.output, suffix=f"_vs_{changed.stem}", extension=".pdf")
        args.output = target
    found_tiles = False

    def annotate(source_doc, staged_path):
        nonlocal found_tiles
        with open_input(original, context.password or context.owner_password) as original_doc:
            pairs = report["data"]["pairs"]
            for pair in pairs:
                source_index = pair["source_page"]
                output_index = pair["output_page"] - 1
                if source_index is None or not 1 <= pair["output_page"] <= len(source_doc):
                    continue
                source_page = original_doc[source_index - 1]
                output_page = source_doc[output_index]
                source_rgb = check.render_rgb(source_page, unrotated=True)
                output_rgb = check.render_rgb(output_page, unrotated=True)
                differences = check.tile_differences(source_rgb, output_rgb,
                                                     page_no=pair["output_page"], page=output_page)
                found_tiles = found_tiles or bool(differences)
                for difference in differences:
                    box = coords.display_to_pymupdf(output_page, pymupdf.Rect(difference["box_mm"]))
                    if not box.is_empty:
                        annot = output_page.add_rect_annot(box)
                        annot.set_colors(stroke=(1, 0, 0))
                        annot.set_border(width=1)
                        annot.update()
        source_doc.save(staged_path, garbage=3, deflate=True)

    artifact = write_pdf(changed, get("compare"), args, context, annotate)
    unmatched_or_structural = any(item.get("code") in {
        "UNMATCHED_OUTPUT", "UNMATCHED_SOURCE", "PAGE_SIZE_CHANGED"
    } for item in report["data"].get("findings", []))
    if report["verdict"] == "warn" or found_tiles or unmatched_or_structural:
        report["verdict"] = "warn"
        artifact["verdict"] = "warn"
    return [report, artifact]
