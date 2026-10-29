"""Phase 6 image conversion and inspection commands."""

from __future__ import annotations

import hashlib
import io as bytesio
import json
import logging
import math
import os
import re
import shutil
from datetime import datetime
from pathlib import Path

import img2pdf as img2pdf_lib
import pymupdf
from PIL import Image, ImageOps

import clierr
from .. import crypt, io, tools
from ..errors import PdfError
from ..pipeline import _signed, open_input, run_each
from ..registry import command

# Pillow's default cap (about 89 MP) is below the toolkit's 200 MP limit; align them.
MAX_PIXELS = 200_000_000
Image.MAX_IMAGE_PIXELS = MAX_PIXELS // 2
logging.getLogger("img2pdf").setLevel(logging.ERROR)

_IMAGE_EXTS = {".jpg", ".jpeg", ".jpe", ".png", ".tif", ".tiff", ".gif", ".webp", ".bmp", ".ico", ".tga", ".psd", ".eps", ".heic", ".heif", ".avif", ".svg", ".jp2", ".j2k"}
_LIB_FORMATS = {"JPEG", "MPO", "JPEG2000", "PNG", "TIFF", "GIF"}
_JPEG_FORMATS = {"JPEG", "MPO", "JPEG2000"}
_UNSUPPORTED_EXTS = {".dng", ".cr2", ".nef", ".jxl"}
_ROTATION = {1: 0, 3: 180, 6: 90, 8: 270}
_A4 = (595.276, 841.89)
_LETTER = (612, 792)


def _secret_env(context):
    """Password environment variables that no child process may inherit."""
    return tuple(getattr(context, "secret_env", ()))


def _tool_error(exc):
    return PdfError(exc.code, exc.what, exc.fix, detail=getattr(exc, "detail", None))


def _eps_bbox(path):
    with open(path, "rb") as stream:
        head = stream.read(65536)
        stream.seek(max(0, os.fstat(stream.fileno()).st_size - 65536))
        tail = stream.read()
    match = re.search(rb"%%BoundingBox:\s*(-?[\d.]+)\s+(-?[\d.]+)\s+(-?[\d.]+)\s+(-?[\d.]+)", head)
    if match is None and b"%%BoundingBox: (atend)" in head:
        match = re.search(rb"%%BoundingBox:\s*(-?[\d.]+)\s+(-?[\d.]+)\s+(-?[\d.]+)\s+(-?[\d.]+)", tail)
    if not match:
        raise PdfError("BAD_INPUT", "EPS has no resolvable BoundingBox", "add a numeric %%BoundingBox")
    try:
        box = tuple(float(v) for v in match.groups())
    except ValueError as exc:
        raise PdfError("BAD_INPUT", "EPS BoundingBox is invalid", "add a numeric %%BoundingBox") from exc
    if box[2] <= box[0] or box[3] <= box[1]:
        raise PdfError("BAD_INPUT", "EPS BoundingBox has no area", "add a numeric %%BoundingBox")
    return box


def _eps_dpi(path):
    """Raster resolution for an EPS: 300 dpi, lowered so the BoundingBox stays within the pixel limit."""
    x0, y0, x1, y1 = _eps_bbox(path)
    area = max(x1 - x0, 0) * max(y1 - y0, 0)
    dpi = min(300, int(72 * (MAX_PIXELS * 0.9 / area) ** 0.5))
    if dpi < 1:
        raise PdfError("TOO_BIG", "EPS page is too large to rasterize within the 200 MP limit", "choose a smaller image")
    return dpi


def _bounded_scale(rect, requested):
    width, height = math.ceil(rect.width * requested), math.ceil(rect.height * requested)
    if width * height <= MAX_PIXELS:
        return requested
    return requested * math.sqrt(MAX_PIXELS / (width * height)) * 0.999999


def _image(path, secret_env=()):
    path = Path(path).expanduser().resolve()
    if not path.is_file():
        raise PdfError("NOT_FOUND", f"image not found: {path}", "check the path")
    try:
        if path.suffix.lower() in {".heic", ".heif"}:
            import pillow_heif
            pillow_heif.register_heif_opener()
        if path.suffix.lower() == ".avif":
            import pillow_avif  # noqa: F401
        detected_eps = path.suffix.lower() == ".eps"
        if not detected_eps:
            with Image.open(path) as probe:
                detected_eps = probe.format == "EPS"
        if detected_eps:
            gs = tools.find_tool("ghostscript")
            if not gs:
                raise PdfError("NOT_FOUND", "Ghostscript is required to read EPS", "install Ghostscript")
            dpi = _eps_dpi(path)
            with io.temp_folder() as folder:
                raster = Path(folder) / "eps.png"
                bbox = _eps_bbox(path)
                width = max(1, round((bbox[2] - bbox[0]) * dpi / 72))
                height = max(1, round((bbox[3] - bbox[1]) * dpi / 72))
                try:
                    tools.run_tool([gs, "-q", "-dSAFER", "-dBATCH", "-dNOPAUSE", "-dEPSCrop", "-sDEVICE=pngalpha",
                                    "-dFIXEDMEDIA", f"-dDEVICEWIDTHPOINTS={bbox[2]-bbox[0]}",
                                    f"-dDEVICEHEIGHTPOINTS={bbox[3]-bbox[1]}", f"-g{width}x{height}",
                                    f"-r{dpi}", f"-sOutputFile={raster}", str(path)],
                                   timeout=30, cwd=folder, temp=folder, secret_env=secret_env)
                except clierr.Fail as exc:
                    raise _tool_error(exc)
                image = Image.open(bytesio.BytesIO(raster.read_bytes()))
                image.load()
                if image.width * image.height > MAX_PIXELS:
                    image.close()
                    raise PdfError("TOO_BIG", "decoded image exceeds the 200 MP limit", "choose a smaller image")
                return path, image
        image = Image.open(path)
        if image.width * image.height > MAX_PIXELS:
            image.close()
            raise PdfError("TOO_BIG", "decoded image exceeds the 200 MP limit", "choose a smaller image")
        try:
            image.load()
        except Exception:
            image.close()
            raise
        return path, image
    except PdfError:
        raise
    except Image.DecompressionBombError as exc:
        raise PdfError("TOO_BIG", "decoded image exceeds the 200 MP limit", "choose a smaller image", detail=str(exc))
    except Exception as exc:
        raise PdfError("BAD_INPUT", f"cannot read image: {path}", "choose a supported image file", detail=str(exc))


def _exif_date(exif):
    """Raw EXIF capture date: DateTimeOriginal (Exif IFD), then DateTimeDigitized, then DateTime."""
    try:
        sub = exif.get_ifd(0x8769)
    except Exception:
        sub = {}
    return sub.get(36867) or sub.get(36868) or exif.get(36867) or exif.get(306)


def _exif_stamp(path):
    """Capture time as a timestamp, or None when the file has no readable EXIF date."""
    if Path(path).suffix.lower() in {".eps", ".svg"}:
        return None  # no EXIF; reading an EPS would launch Ghostscript just to sort
    try:
        _, image = _image(path)
    except PdfError:
        return None
    try:
        raw = _exif_date(image.getexif())
        return datetime.strptime(str(raw).strip()[:19], "%Y:%m:%d %H:%M:%S").timestamp() if raw else None
    except Exception:
        return None
    finally:
        image.close()


def _expand_inputs(value, sort="name", warnings=None, notes=None):
    path = Path(value).expanduser().resolve()
    if path.is_dir():
        files = [p for p in path.iterdir() if p.is_file() and p.suffix.lower() in _IMAGE_EXTS]
        skipped = sorted(p for p in path.iterdir() if p.is_file() and p.suffix.lower() in _UNSUPPORTED_EXTS)
        if skipped and not files:
            raise PdfError("UNSUPPORTED", f"only unsupported image formats found ({skipped[0].suffix.lower()})", "export the images to JPEG")
        if skipped and warnings is not None:
            warnings.append("SKIPPED_UNSUPPORTED")
            if notes is not None:
                notes["SKIPPED_UNSUPPORTED"] = f"{len(skipped)} unsupported RAW/JPEG XL file(s) in {path.name} were left out"
    elif path.is_file():
        files = [path]
    elif any(c in str(value) for c in "*?[]"):
        files = [p for p in path.parent.glob(path.name) if p.is_file()]
    else:
        files = [path]
    if not files:
        raise PdfError("NOT_FOUND", "no supported images found", "choose an image, folder or glob")
    def natural(p):
        return [(0, int(x)) if x.isdigit() else (1, x.casefold()) for x in re.split(r"(\d+)", p.name)]
    files.sort(key=natural)
    if sort == "date":
        files.sort(key=lambda p: p.stat().st_mtime)
    elif sort == "exif":
        stamps = {}
        for p in files:
            stamp = _exif_stamp(p)
            if stamp is None:
                stamp = p.stat().st_mtime
                if warnings is not None:
                    warnings.append("NO_EXIF_DATE")
            stamps[p] = stamp
        files.sort(key=stamps.get)
    return files


def add_arguments_img2pdf(parser):
    parser.add_argument("inputs", nargs="+", type=Path)
    parser.add_argument("-o", "--output")
    parser.add_argument("--page", choices=("auto", "fit", "a4", "letter"), default="auto")
    parser.add_argument("--fit", choices=("contain", "cover", "stretch"), default="contain")
    parser.add_argument("--orient", choices=("auto", "portrait", "landscape"), default="auto")
    parser.add_argument("--margin", default="10mm")
    parser.add_argument("--sort", choices=("name", "date", "exif"), default="name")
    parser.add_argument("--lossy", nargs="?", const=92, type=int)
    parser.add_argument("--flatten-alpha", action="store_true")
    parser.add_argument("--max-dpi", type=int)
    parser.add_argument("--each", action="store_true")
    parser.add_argument("--ocr", action="store_true")
    parser.add_argument("--toc", action="store_true")


def _margin(value):
    match = re.fullmatch(r"\s*(\d+(?:\.\d+)?)\s*(mm|in|pt)?\s*", str(value), re.I)
    if not match:
        raise PdfError("BAD_ARG", "--margin needs a length", "use e.g. 10mm, 0.4in or 28pt")
    v, unit = float(match.group(1)), (match.group(2) or "mm").lower()
    return v * {"mm": 72 / 25.4, "in": 72, "pt": 1}[unit]


def _validate_img2pdf(args):
    if args.lossy is not None and not 1 <= args.lossy <= 100:
        raise PdfError("BAD_ARG", "--lossy quality must be 1-100", "choose a JPEG quality")
    if args.max_dpi is not None and args.max_dpi < 1:
        raise PdfError("BAD_ARG", "--max-dpi must be positive", "choose a positive DPI")
    _margin(args.margin)


def _dpi(info, swap):
    value = info.get("dpi")
    try:
        x, y = float(value[0]), float(value[1])
    except (TypeError, ValueError, IndexError):
        return None
    if not (x > 0 and y > 0):
        return None
    return (y, x) if swap else (x, y)


def _place(args, wpx, hpx, dpi):
    """Page size and image rectangle in points for a displayed image of wpx x hpx pixels."""
    native = False
    if args.page == "auto":
        native_size = sorted((wpx / dpi[0] * 72, hpx / dpi[1] * 72)) if dpi else None
        if dpi and min(dpi) >= 150 and native_size[0] <= 842 and native_size[1] <= 1190:
            width, height = wpx / dpi[0] * 72, hpx / dpi[1] * 72
            native = True
        else:
            width, height = _A4
    elif args.page == "fit":
        width, height = wpx, hpx
    else:
        width, height = _A4 if args.page == "a4" else _LETTER
    if args.orient == "landscape" or (args.orient == "auto" and wpx > hpx and args.page != "fit"):
        width, height = max(width, height), min(width, height)
    elif args.orient == "portrait":
        width, height = min(width, height), max(width, height)
    margin = 0 if native else _margin(args.margin)
    box = pymupdf.Rect(margin, margin, width - margin, height - margin)
    if box.width <= 0 or box.height <= 0:
        raise PdfError("BAD_ARG", "--margin leaves no room on the page", "use a smaller --margin")
    if args.fit == "stretch":
        dest = box
    elif args.fit == "cover":
        factor = max(box.width / wpx, box.height / hpx)
        dest = pymupdf.Rect((width - wpx * factor) / 2, (height - hpx * factor) / 2,
                            (width + wpx * factor) / 2, (height + hpx * factor) / 2)
    else:
        factor = min(box.width / wpx, box.height / hpx)
        dest_w, dest_h = wpx * factor, hpx * factor
        x0, y0 = box.x0 + (box.width - dest_w) / 2, box.y0 + (box.height - dest_h) / 2
        dest = pymupdf.Rect(x0, y0, x0 + dest_w, y0 + dest_h)
    return width, height, dest


def _has_alpha(image):
    return "A" in image.getbands() or "transparency" in image.info


def _bit_depth(image):
    if image.mode.startswith("I;16") or image.mode in ("I", "F"):
        return 16
    stored = image.info.get("bit_depth")  # pillow-heif reports the file's depth; the decoded mode is already 8-bit
    if isinstance(stored, int) and stored > 0:
        return stored
    try:
        if image.format == "PNG" and getattr(image, "filename", None):
            with open(image.filename, "rb") as stream:
                return stream.read(26)[24]
        if image.format == "TIFF":
            depth = image.tag_v2.get(258, 8)
            return max(depth) if isinstance(depth, tuple) else depth
    except Exception:
        pass
    return 8


def _to_8bit(image):
    """High-bit-depth gray to 8-bit without clipping (16-bit keeps its top byte)."""
    import numpy as np
    arr = np.asarray(image)
    if image.mode.startswith("I;16"):
        out = (arr.astype(np.uint32) >> 8).astype(np.uint8)
    else:
        arr = arr.astype(np.float64)
        low, high = float(arr.min()), float(arr.max())
        if image.mode == "I" and low >= 0 and high <= 65535:
            out = (arr.astype(np.int64) >> 8).astype(np.uint8)
        elif image.mode == "F" and low >= 0 and high <= 1:
            out = np.rint(arr * 255).astype(np.uint8)
        else:
            out = np.rint((arr - low) * (255 / (high - low or 1))).astype(np.uint8)
    return Image.fromarray(out, "L")


def _prepare(original, args, warnings):
    """Decode the first frame into an upright 8-bit RGB/L(A) image; return (image, icc_profile)."""
    original.seek(0)
    icc = original.info.get("icc_profile")
    depth = _bit_depth(original)
    image = ImageOps.exif_transpose(original)
    image.load()
    if image.mode.startswith("I;16") or image.mode in ("I", "F"):
        image = _to_8bit(image)
    if depth > 8:
        warnings.append("BIT_DEPTH_REDUCED")
    if image.mode == "CMYK":
        from PIL import ImageCms
        try:
            if not icc:
                raise ValueError("no embedded profile")
            image = ImageCms.profileToProfile(image, ImageCms.ImageCmsProfile(bytesio.BytesIO(icc)),
                                              ImageCms.createProfile("sRGB"), outputMode="RGB")
        except Exception:
            image = image.convert("RGB")
            warnings.append("CMYK_APPROX")
        icc = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()
    if "transparency" in image.info and "A" not in image.getbands():
        image = image.convert("RGBA")
    if args.flatten_alpha and "A" in image.getbands():
        flat = Image.new("RGB", image.size, "white")
        flat.paste(image, mask=image.getchannel("A"))
        image = flat
    if image.mode not in ("1", "L", "LA", "RGB", "RGBA"):
        image = image.convert("RGBA" if "A" in image.getbands() else "RGB")
    if args.lossy is not None and image.mode in ("1", "L", "LA") and icc:
        # JPEG output below is RGB; do not attach a Gray profile to RGB samples.
        from PIL import ImageCms
        try:
            source_profile = ImageCms.ImageCmsProfile(bytesio.BytesIO(icc))
            target_profile = ImageCms.createProfile("sRGB")
            alpha = image.getchannel("A") if image.mode == "LA" else None
            gray = image.getchannel("L") if image.mode == "LA" else image
            image = ImageCms.profileToProfile(gray, source_profile, target_profile, outputMode="RGB")
            if alpha is not None:
                image.putalpha(alpha)
            icc = ImageCms.ImageCmsProfile(target_profile).tobytes()
        except Exception:
            image = image.convert("RGB")
            icc = None
    return image, icc


def _insert_pillow(image, icc, args, dest, page):
    """Embed a prepared image: PNG (lossless) or JPEG plus a separate soft mask (--lossy)."""
    mask = None
    stream = bytesio.BytesIO()
    if args.lossy is not None:
        if "A" in image.getbands():
            alpha = bytesio.BytesIO()
            image.getchannel("A").save(alpha, "PNG")
            mask = alpha.getvalue()
        image.convert("RGB").save(stream, "JPEG", quality=args.lossy, icc_profile=icc)
    else:
        image.save(stream, "PNG", icc_profile=icc)
    page.insert_image(dest, stream=stream.getvalue(), mask=mask, keep_proportion=False)


def _image_pdf(source, staged, args):
    """Write a one-page PDF for source; return the embedded pixel size, or None for SVG."""
    if source.suffix.lower() == ".svg":
        try:
            with pymupdf.open(source) as doc:
                pdf_bytes = doc.convert_to_pdf()
            with pymupdf.open("pdf", pdf_bytes) as vector_pdf:
                rect = vector_pdf[0].rect
                width, height, dest = _place(args, rect.width, rect.height, (72, 72))
                placed = pymupdf.open()
                try:
                    page = placed.new_page(width=width, height=height)
                    page.show_pdf_page(dest, vector_pdf, 0, keep_proportion=args.fit != "stretch")
                    placed.save(staged, garbage=3, deflate=True)
                finally:
                    placed.close()
            return None
        except Exception as exc:
            raise PdfError("BAD_INPUT", f"cannot read SVG: {source}", "check the SVG file", detail=str(exc))
    if source.suffix.lower() in _UNSUPPORTED_EXTS:
        raise PdfError("UNSUPPORTED", f"unsupported image format: {source.suffix.lower()}", "export the image to JPEG")
    warnings = args._warnings
    path, original = _image(source, getattr(args, "_secret_env", ()))
    try:
        original.seek(0)
        fmt = original.format
        orientation = original.getexif().get(274, 1)
        swap = orientation in (5, 6, 7, 8)
        wpx, hpx = (original.height, original.width) if swap else original.size
        width, height, dest = _place(args, wpx, hpx, _dpi(original.info, swap))
        target = (wpx, hpx)
        if args.max_dpi and not (fmt in _JPEG_FORMATS and orientation in _ROTATION):
            # --max-dpi applies on the Pillow route only; JPEG bytes that img2pdf embeds stay untouched.
            target = (max(1, round(wpx * min(1, args.max_dpi / (wpx / dest.width * 72)))),
                      max(1, round(hpx * min(1, args.max_dpi / (hpx / dest.height * 72)))))
        alpha = _has_alpha(original)
        lossy_recode = args.lossy is not None and fmt not in _JPEG_FORMATS
        library = (fmt in _LIB_FORMATS and path.suffix.lower() != ".eps" and orientation in _ROTATION and target == (wpx, hpx) and not lossy_recode
                   and not (args.flatten_alpha and alpha))
        if library:
            rotation = _ROTATION[orientation]
            def layout(_w, _h, _ndpi):
                # img2pdf lays out the stored (unrotated) image and rotates the page afterwards.
                if rotation in (90, 270):
                    return height, width, dest.height, dest.width
                return width, height, dest.width, dest.height
            try:
                with open(path, "rb") as stream:
                    data = img2pdf_lib.convert(stream, layout_fun=layout, first_frame_only=True, nodate=True)
                Path(staged).write_bytes(data)
                return original.size
            except (img2pdf_lib.AlphaChannelError, img2pdf_lib.ExifOrientationError, img2pdf_lib.ImageOpenError,
                    img2pdf_lib.JpegColorspaceError, img2pdf_lib.UnsupportedColorspaceError,
                    ValueError, NotImplementedError, TypeError, OSError):
                pass  # a documented refusal: fall back to the lossless Pillow route
        image, icc = _prepare(original, args, warnings)
        if image.size != (wpx, hpx):
            raise PdfError("INTERNAL", f"decoded size {image.size} differs from the probed size {(wpx, hpx)}", "report this image")
        if target != image.size:
            image = image.resize(target, Image.Resampling.LANCZOS)
        doc = pymupdf.open()
        try:
            page = doc.new_page(width=width, height=height)
            _insert_pillow(image, icc, args, dest, page)
            doc.save(staged, garbage=3, deflate=True)
        finally:
            doc.close()
        size = image.size
        image.close()
        return size
    finally:
        original.close()


def _check_pdf(path, expected):
    """Independent check: one page per input and each page's image has the expected pixel size.

    An SVG page (expected size None) has no raster to measure: it is rendered and must show something.
    Returns the 1-based numbers of SVG pages that render blank."""
    blank = []
    with pymupdf.open(path) as doc:
        if doc.page_count != len(expected):
            raise PdfError("CHECK_FAILED", f"output has {doc.page_count} pages for {len(expected)} images", "rerun, or report the input")
        for index, size in enumerate(expected):
            if size is None:
                rect = doc[index].rect
                scale = _bounded_scale(rect, 36 / 72)
                samples = doc[index].get_pixmap(matrix=pymupdf.Matrix(scale, scale), colorspace=pymupdf.csGRAY, alpha=False).samples
                if min(samples) >= 250:
                    blank.append(index + 1)
                continue
            found = [(i[2], i[3]) for i in doc[index].get_images(full=True)]
            if tuple(size) not in found:
                raise PdfError("CHECK_FAILED", f"page {index + 1} embeds {found}, expected {tuple(size)} pixels", "rerun, or report the input")
    return blank


def _mark_pdf_unchecked(stage, target, context, problem):
    if problem is None:
        return None
    stage.target = target.with_name(target.stem + "_UNCHECKED" + target.suffix)
    context.exit_code = 3
    log = clierr.write_log("pdf", "CHECK_FAILED", "img2pdf invariant check failed",
                           "review the converted document against its source", sections={"detail": problem.what},
                           log_name="img2pdf")
    clierr.warn("pdf img2pdf", "EXPORT_FAILED", problem.what, "review the converted document")
    return log


@command("img2pdf", 6, "image", "pdf", "", False, "report")
def img2pdf(args, context):
    if not hasattr(args, "_warnings"):
        args._warnings = []
    _validate_img2pdf(args)
    args._secret_env = _secret_env(context)
    args._notes = {}
    files = []
    for raw in args.inputs:
        files.extend(_expand_inputs(raw, args.sort, args._warnings, args._notes))
    output_src = files[0]
    output_text = str(args.output) if args.output is not None else ""
    output_is_dir = bool(args.output and (Path(args.output).is_dir() or output_text.endswith(("/", "\\"))))
    if args.each:
        if args.output and not output_is_dir and len(files) > 1:
            raise PdfError("BAD_ARG", "--each needs an output directory for multiple inputs", "end -o with a directory separator")
        if output_is_dir and not args.dry_run:
            Path(args.output).mkdir(parents=True, exist_ok=True)
        def one(source):
            target = (Path(args.output) / f"{source.stem}.pdf") if output_is_dir else io.resolve_output(source, "img2pdf", args.output,
                                                                                                         suffix="", extension=".pdf")
            local = argparse_namespace(args, output=str(target), _warnings=[], each=False)
            return _img2pdf_one(source, local, context)
        each = run_each(files, one, "img2pdf", context)
        if args._warnings and isinstance(each, list) and each and each[-1].get("type") == "summary":
            each[-1]["warnings"] = list(dict.fromkeys(each[-1].get("warnings", []) + args._warnings))
            for code in args._warnings:
                clierr.warn("pdf img2pdf", code, args._notes.get(code, "some inputs were skipped"), "review the inputs")
        return each
    target = io.resolve_output(output_src, "img2pdf", args.output, suffix="", extension=".pdf")
    if len(files) > 1 and args.output is None:
        # One folder argument names the PDF after the folder; otherwise after the first image.
        only = Path(args.inputs[0]).expanduser().resolve() if len(args.inputs) == 1 else None
        name = only.name if only is not None and only.is_dir() else files[0].stem
        target = files[0].parent / (name + "_images.pdf")
    if output_is_dir:
        folder_name = Path(args.inputs[0]).name if len(args.inputs) == 1 and Path(args.inputs[0]).is_dir() else files[0].stem
        target = Path(args.output) / (folder_name + "_images.pdf")
    target = target.resolve()
    if args.dry_run:
        return {"type": "result", "command": "img2pdf", "path": str(target), "kind": "pdf", "pages": len(files),
                "bytes": 0, "verdict": "report", "provisional": True, "warnings": list(args._warnings)}
    target.parent.mkdir(parents=True, exist_ok=True)
    io.sweep_stale(target.parent)
    with io.temp_folder() as temp, io.Staging(target, temp) as stage:
        io.disk_budget(files, target, temp, multiplier=20 if args.ocr else 3)
        combined = pymupdf.open()
        expected = []
        for source in files:
            sub = temp / f"image-{len(combined)}.pdf"
            expected.append(_image_pdf(source, sub, args))
            with pymupdf.open(sub) as doc:
                combined.insert_pdf(doc)
        if args.toc:
            combined.set_toc([[1, source.stem, index + 1] for index, source in enumerate(files)])
        combined.save(stage.path, garbage=3, deflate=True)
        combined.close()
        if args.ocr:
            ocrmypdf = shutil.which("ocrmypdf")
            if not ocrmypdf:
                raise PdfError("NOT_FOUND", "ocrmypdf is not installed", "install ocrmypdf")
            ocr_path = temp / "ocr.pdf"
            try:
                tools.run_tool([ocrmypdf, "--skip-text", "--output-type", "pdf", "--jobs", str(os.cpu_count() or 1),
                                "-l", "eng+ind", str(stage.path), str(ocr_path)],
                               timeout=max(120, 60 * len(files)), cwd=temp, temp=temp, secret_env=args._secret_env)
            except clierr.Fail as exc:
                raise _tool_error(exc)
            stage.path.unlink(missing_ok=True)
            shutil.move(ocr_path, stage.path)
        # The check runs on the file that will be published (after OCR, when requested).
        invariant, blank = None, []
        try:
            blank = _check_pdf(stage.path, expected)
        except PdfError as exc:
            invariant = exc
        if blank:
            args._warnings.append("SVG_BLANK")
        if args.ocr and invariant is None:
            with pymupdf.open(stage.path) as checked:
                if any(not page.get_text("text").strip() for page in checked):
                    args._warnings.append("OCR_EMPTY")
        log = _mark_pdf_unchecked(stage, target, context, invariant)
        final = stage.publish()
    if final != stage.target:
        args._warnings.append("NAME_TAKEN")
    warnings = list(dict.fromkeys(args._warnings))
    for warning in warnings:
        clierr.warn("pdf img2pdf", warning, args._notes.get(warning, "image conversion required a documented adjustment"), "review the output")
    record = {"type": "result", "command": "img2pdf", "path": str(final), "kind": "pdf", "pages": len(files), "bytes": final.stat().st_size,
              "verdict": "unchecked" if invariant else "warn" if warnings else "ok", "warnings": warnings}
    if invariant:
        record["log"] = log
    return record


def argparse_namespace(args, **updates):
    import argparse
    data = vars(args).copy(); data.update(updates)
    return argparse.Namespace(**data)


def _img2pdf_one(source, args, context):
    # Route through the same implementation as the multi-image case.
    return img2pdf(argparse_namespace(args, inputs=[source], each=False), context)


def add_arguments_images(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("-o", "--output")
    parser.add_argument("--min-px", type=int, default=0)
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")


def _pixmap_image(pix):
    """Pixmap to an RGB or L Pillow image (alpha dropped, CMYK converted)."""
    if pix.alpha:
        pix = pymupdf.Pixmap(pix, 0)
    if pix.colorspace is not None and pix.colorspace.n == 4:
        pix = pymupdf.Pixmap(pymupdf.csRGB, pix)
    return Image.frombytes("L" if pix.n == 1 else "RGB", (pix.width, pix.height), pix.samples)


def _save_decoded(doc, xref, smask, path):
    """Decode through PyMuPDF; merge the soft mask (resampled to the base size) when present."""
    image = _pixmap_image(pymupdf.Pixmap(doc, xref))
    if smask:
        mask = _pixmap_image(pymupdf.Pixmap(doc, smask)).convert("L")
        if mask.size != image.size:
            mask = mask.resize(image.size, Image.Resampling.BILINEAR)
        image = image.convert("RGB")
        image.putalpha(mask)
    image.save(path, "PNG")
    image.close()


@command("images", 6, "pdf", "folder", "", False, "report")
def images(args, context):
    source = Path(args.file).expanduser().resolve()
    if not source.is_file(): raise PdfError("NOT_FOUND", f"PDF not found: {source}", "check the path")
    if args.min_px < 0: raise PdfError("BAD_ARG", "--min-px must be nonnegative", "choose 0 or a positive pixel count")
    # The target is a folder: `<stem>_images` beside the PDF, never `<stem>_images.pdf`.
    target = source.with_name(source.stem + "_images") if args.output is None else io.resolve_output(source, "images", args.output, multi=True)
    password = context.password or context.owner_password
    warnings = []
    try:
        import pikepdf
        with pikepdf.open(source, password=password or "") as pdf:
            warnings.extend(crypt.export_warnings(pdf.is_encrypted))
    except Exception:
        pass
    if _signed(source, password):
        warnings.append("SIGNATURE_NOT_RETAINED")
    if args.dry_run:
        return {"type": "result", "command": "images", "path": str(target), "kind": "folder", "pages": None, "bytes": 0, "verdict": "report", "provisional": True, "warnings": warnings}
    target.parent.mkdir(parents=True, exist_ok=True)
    io.sweep_stale(target.parent)
    with io.temp_folder() as temp:
        staged = io.create_staging(target, temp); staged.mkdir(parents=True)
        try:
            found, seen, skipped, digests, too_big = {}, set(), set(), set(), set()
            with open_input(source, password) as doc:
                for page_no, page in enumerate(doc, 1):
                    page_xrefs = page.get_images(full=True)
                    preflight = {}
                    for info in page_xrefs:
                        xref = info[0]
                        try:
                            width = int(doc.xref_get_key(xref, "Width")[1])
                            height = int(doc.xref_get_key(xref, "Height")[1])
                        except (TypeError, ValueError):
                            width = height = 0
                        preflight[xref] = (width, height)
                    for info in page_xrefs:
                        xref, smask = info[0], info[1]
                        if xref in seen or xref in skipped: continue
                        try:
                            width, height = preflight.get(xref, (0, 0))
                            if width * height > MAX_PIXELS:
                                warnings.append("SKIPPED_TOO_BIG"); skipped.add(xref); too_big.add(xref); continue
                            if smask:
                                try:
                                    mw = int(doc.xref_get_key(smask, "Width")[1]); mh = int(doc.xref_get_key(smask, "Height")[1])
                                except (TypeError, ValueError):
                                    mw = mh = 0
                                if mw * mh > MAX_PIXELS:
                                    warnings.append("SKIPPED_TOO_BIG"); skipped.add(xref); too_big.add(xref); continue
                            extracted = doc.extract_image(xref)
                            width, height = extracted["width"], extracted["height"]
                            if width * height < args.min_px:
                                skipped.add(xref); continue
                            ext = extracted.get("ext", "png").lower()
                            number = len(found) + 1
                            if smask or ext in {"jb2", "jpx"}:  # PyMuPDF already hands CCITT/raw/Flate data back as PNG
                                if width * height > MAX_PIXELS:
                                    warnings.append("SKIPPED_TOO_BIG"); skipped.add(xref); too_big.add(xref); continue
                                filename = f"image_{number:03d}.png"
                                _save_decoded(doc, xref, smask, staged / filename)
                                if not smask: warnings.append("CONVERTED_TO_PNG")
                            else:
                                filename = f"image_{number:03d}.{ext}"
                                (staged / filename).write_bytes(extracted["image"])
                        except Exception as exc:
                            clierr.warn("pdf images", "IMAGE_FAILED", f"image {xref} on page {page_no} could not be extracted: {exc}", "extract it another way")
                            warnings.append("IMAGE_FAILED"); skipped.add(xref); continue
                        seen.add(xref)
                        found[xref] = {"xref": xref, "file": filename, "page": page_no, "width": width, "height": height}
                    # Inline images have xref 0 in get_image_info; XObjects are already written above.
                    # get_image_info(xrefs=True) builds a Pixmap for every XObject on the page, so a page that
                    # holds an oversized one (declared size, checked above) is not asked at all.
                    if any(preflight.get(i[0], (0, 0))[0] * preflight.get(i[0], (0, 0))[1] > MAX_PIXELS or i[0] in too_big
                           for i in page_xrefs):
                        continue
                    image_info = page.get_image_info()
                    if any((i.get("width") or 0) * (i.get("height") or 0) > MAX_PIXELS for i in image_info):
                        warnings.append("SKIPPED_TOO_BIG")
                        continue
                    image_info = page.get_image_info(xrefs=True)
                    inline = {tuple(round(v, 1) for v in i["bbox"]) for i in image_info if i["xref"] == 0}
                    for block in page.get_text("dict").get("blocks", []):
                        if block.get("type") != 1 or not block.get("image"): continue
                        if block.get("xref", 0): continue
                        if tuple(round(v, 1) for v in block["bbox"]) not in inline: continue
                        if (block.get("width") or 0) * (block.get("height") or 0) < args.min_px: continue
                        digest = hashlib.sha256(block["image"]).hexdigest()
                        if digest in digests: continue
                        digests.add(digest)
                        filename = f"inline_{len(found)+1:03d}.{block.get('ext', 'png')}"
                        (staged / filename).write_bytes(block["image"])
                        found[f"inline-{len(found)}"] = {"file": filename, "page": page_no,
                            "width": block.get("width"), "height": block.get("height"), "digest": digest}
            unreadable = 0
            for row in found.values():
                try:
                    with Image.open(staged / row["file"]) as written:
                        written.load()
                        if written.size != (row["width"], row["height"]): unreadable += 1
                except Exception:
                    unreadable += 1
            if unreadable:
                clierr.warn("pdf images", "UNVERIFIED_IMAGE", f"{unreadable} extracted image(s) did not reopen with the expected size", "open them to check")
                warnings.append("UNVERIFIED_IMAGE")
            if not found:
                warnings.append("NO_IMAGES")
            (staged / "images.json").write_text(json.dumps(list(found.values()), indent=2), encoding="utf-8")
            final = io.publish(staged, target)
            if final != target: warnings.append("NAME_TAKEN")
        finally:
            if staged.exists(): shutil.rmtree(staged, ignore_errors=True)
    warnings = list(dict.fromkeys(warnings))
    for code in warnings:
        if code not in {"IMAGE_FAILED", "UNVERIFIED_IMAGE"}:
            clierr.warn("pdf images", code, "image extraction required a documented adjustment", "review the output")
    return {"type": "result", "command": "images", "path": str(final), "kind": "folder", "pages": None,
            "bytes": sum(p.stat().st_size for p in final.iterdir() if p.is_file()), "verdict": "warn" if warnings else "ok", "warnings": warnings}


def add_arguments_imgtext(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("--lang", default="eng+ind")
    parser.add_argument("-o", "--output")


_TESSERACT_READS = {".png", ".jpg", ".jpeg", ".tif", ".tiff", ".bmp"}


def _tesseract_input(image, temp):
    """A PNG Tesseract can read from any image Pillow decoded: upright, 8-bit, transparency on white."""
    image = ImageOps.exif_transpose(image)
    image.load()
    if image.mode.startswith("I;16") or image.mode in ("I", "F"):
        image = _to_8bit(image)
    if "A" in image.getbands() or "transparency" in image.info:
        rgba = image.convert("RGBA")
        image = Image.new("RGB", rgba.size, "white")
        image.paste(rgba, mask=rgba.getchannel("A"))
    elif image.mode not in ("1", "L", "RGB"):
        image = image.convert("RGB")
    out = Path(temp) / "ocr-input.png"
    image.save(out)
    return out


@command("imgtext", 6, "image", "report", "", False, "report")
def imgtext(args, context):
    secret = _secret_env(context)
    file = Path(args.file).expanduser().resolve()
    svg = file.suffix.lower() == ".svg"
    if getattr(args, "dry_run", False):
        if not file.is_file(): raise PdfError("NOT_FOUND", f"image not found: {file}", "check the path")
        target = io.resolve_output(file, "imgtext", args.output, suffix="", extension=".txt") if args.output is not None else None
        data = {"provisional": True}
        if target is not None: data["output"] = str(target.resolve())
        return {"type": "report", "command": "imgtext", "input": str(file), "verdict": "report", "data": data}
    if svg:
        if not file.is_file(): raise PdfError("NOT_FOUND", f"image not found: {file}", "check the path")
        path = file
    else:
        path, image = _image(file, secret)
        if path.suffix.lower() in _TESSERACT_READS:
            image.close(); image = None
    tess = shutil.which("tesseract")
    if not tess: raise PdfError("NOT_FOUND", "tesseract is not installed", "install Tesseract OCR")
    try:
        with io.temp_folder() as temp:
            source = path
            if svg:
                source = Path(temp) / "ocr-input.png"
                try:
                    with pymupdf.open(path) as vector:
                        rect = vector[0].rect
                        scale = _bounded_scale(rect, 200 / 72)
                        vector[0].get_pixmap(matrix=pymupdf.Matrix(scale, scale)).save(source)
                except Exception as exc:
                    raise PdfError("BAD_INPUT", f"cannot read SVG: {path}", "check the SVG file", detail=str(exc))
            elif image is not None:
                # Tesseract reads only the common raster formats; hand it a PNG made from what Pillow decoded.
                try:
                    source = _tesseract_input(image, temp)
                finally:
                    image.close()
            # tessedit_create_tsv works without the "tsv" config file, which the conda package keeps elsewhere.
            result = tools.run_tool([tess, str(source), "stdout", "-l", args.lang, "-c", "tessedit_create_tsv=1"],
                                    timeout=120, cwd=temp, temp=temp, secret_env=secret)
    except clierr.Fail as exc:
        raise _tool_error(exc)
    lines = result.stdout.splitlines()
    rows = [line.split("\t") for line in lines if line]
    words, conf = [], []
    for row in rows:
        if len(row) >= 12 and row[11].strip():
            try: value = float(row[10])
            except ValueError: continue
            if value < 0: continue
            words.append(row[11]); conf.append(value)
    text = " ".join(words)
    data = {"text": text, "mean_confidence": round(sum(conf) / len(conf), 2) if conf else None,
            "words": len(words), "language": args.lang}
    if args.output is not None:
        target = io.resolve_output(path, "imgtext", args.output, suffix="", extension=".txt")
        target.parent.mkdir(parents=True, exist_ok=True)
        with io.temp_folder() as temp, io.Staging(target, temp) as stage:
            stage.path.write_text(text, encoding="utf-8")
            data["output"] = str(stage.publish())
    return {"type": "report", "command": "imgtext", "input": str(path), "verdict": "report", "data": data}


def add_arguments_imginfo(parser):
    parser.add_argument("file", type=Path)


@command("imginfo", 6, "image", "report", "", False, "report")
def imginfo(args, context):
    path, image = _image(args.file, _secret_env(context))
    try:
        image.load()
        info, exif = image.info, image.getexif()
        try:
            dpi = [float(v) for v in info["dpi"]]  # TIFF gives IFDRational, which JSON cannot carry
        except (KeyError, TypeError, ValueError):
            dpi = None
        hist = image.convert("L").histogram()
        pixels = image.width * image.height
        blank = max(hist) / max(pixels, 1) >= .999
        result = {"path": str(path), "width": image.width, "height": image.height,
                  "size": f"{image.width}x{image.height}", "dpi": dpi,
                  "mode": image.mode, "bit_depth": _bit_depth(image),
                  "icc_profile": bool(info.get("icc_profile")), "colour": "gray" if image.mode in ("1", "L", "LA", "I", "F") or image.mode.startswith("I;16") else "colour",
                  "blank": bool(blank), "exif_date": _exif_date(exif),
                  "camera": exif.get(272), "orientation": exif.get(274)}
    finally: image.close()
    return {"type": "report", "command": "imginfo", "input": str(path), "verdict": "report", "data": result}


def add_arguments_imgdiff(parser):
    parser.add_argument("a", type=Path)
    parser.add_argument("b", type=Path)
    parser.add_argument("--resize", action="store_true")


def _high_bit(image):
    return image.mode.startswith("I;16") or image.mode in ("I", "F")


def _rgba_tile(tile):
    """RGBA for comparing; 16-bit gray keeps its top byte instead of clipping at 255."""
    if not _high_bit(tile):
        return tile.convert("RGBA")
    import numpy as np
    arr = np.asarray(tile)
    if tile.mode == "F":
        gray = np.clip(np.rint(arr * 255), 0, 255)
    else:
        gray = np.clip(arr.astype(np.int64) >> 8, 0, 255)
    return Image.fromarray(gray.astype(np.uint8), "L").convert("RGBA")


@command("imgdiff", 6, "image", "report", "", False, "report")
def imgdiff(args, context):
    import numpy as np
    secret = _secret_env(context)
    pa, a = _image(args.a, secret); pb, b = _image(args.b, secret)
    try:
        findings = []
        if a.size != b.size:
            findings.append({"code": "SIZE_CHANGED", "a": list(a.size), "b": list(b.size)})
            if args.resize:
                if _high_bit(b):
                    b = _to_8bit(b)
                b = b.convert("RGBA" if _has_alpha(b) else "RGB").resize(a.size, Image.Resampling.LANCZOS)
        if a.size == b.size:
            w, h = a.size
            tile = max(1, round(10 * 96 / 25.4))
            # One tile row at a time keeps memory bounded for very large images.
            for top in range(0, h, tile):
                bottom = min(h, top + tile)
                x = np.asarray(_rgba_tile(a.crop((0, top, w, bottom))), dtype=np.int16)
                y = np.asarray(_rgba_tile(b.crop((0, top, w, bottom))), dtype=np.int16)
                for left in range(0, w, tile):
                    delta = np.any(np.abs(x[:, left:left+tile] - y[:, left:left+tile]) > 8, axis=2)
                    pct = float(delta.mean()) if delta.size else 0
                    if pct > .01: findings.append({"code": "TILE_CHANGED", "box_px": [left, top, min(w, left+tile), bottom], "changed_fraction": round(pct, 4)})
        return {"type": "report", "command": "imgdiff", "input": f"{pa} vs {pb}",
                "verdict": "warn" if findings else "report", "data": {"a": str(pa), "b": str(pb), "findings": findings}}
    finally:
        a.close(); b.close()


def selftest_write(source, output):
    """Extract an embedded image through the real `images` command; `output` becomes the exported folder."""
    from types import SimpleNamespace
    output = Path(output)
    work = output.parent / "images-selftest"
    work.mkdir()
    sample = work / "sample.pdf"
    buffer = bytesio.BytesIO()
    Image.new("RGB", (12, 8), "red").save(buffer, "PNG")
    with pymupdf.open() as doc:
        doc.new_page(width=100, height=100).insert_image(pymupdf.Rect(10, 10, 60, 50), stream=buffer.getvalue())
        doc.save(sample)
    context = SimpleNamespace(password=None, owner_password=None, secret_env=(), exit_code=0)
    record = images(SimpleNamespace(file=sample, output=str(work / "out") + os.sep, min_px=0, dry_run=False), context)
    folder = Path(record["path"])
    pngs = sorted(folder.glob("*.png"))
    if record["verdict"] != "ok" or len(pngs) != 1:
        raise RuntimeError(f"images extracted {len(pngs)} PNG file(s), verdict {record['verdict']}")
    with Image.open(pngs[0]) as written:
        if written.size != (12, 8):
            raise RuntimeError(f"extracted image is {written.size}, expected (12, 8)")
    folder.rename(output)
