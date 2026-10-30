"""PDF compression with measured candidate selection."""
import os
import re
import shutil
import sys
import time
from pathlib import Path

import pymupdf

from .. import io
from ..check import OperationResult, Placement, compare
from ..errors import PdfError
from ..pipeline import announce_estimate, run_each, write_pdf
from ..registry import command
from ..tools import dev_tool_env, find_tool, run_tool


LEVELS = {"lossless": (None, None, None, None), "med": (.90, 225, 150, 80),
          "high": (.85, 165, 110, 65), "max": (.78, 110, 72, 50)}


def add_arguments_compress(parser):
    parser.add_argument("file", type=Path, nargs="?")
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--level", choices=tuple(LEVELS), default="med")
    group.add_argument("--target")
    parser.add_argument("--include-graphics", action="store_true")
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--each", type=Path)
    _password_args(parser)


def _password_args(parser):
    parser.add_argument("--password-file", type=Path); parser.add_argument("--password-env")
    parser.add_argument("--owner-password-file", type=Path); parser.add_argument("--owner-password-env")
    parser.add_argument("--ignore-restrictions", action="store_true"); parser.add_argument("--break-signature", action="store_true")


def _parse_target(value):
    match = re.fullmatch(r"\s*(\d+(?:\.\d+)?)\s*(KB|MB|GB)?\s*", str(value), re.I)
    if not match:
        raise PdfError("BAD_ARG", f"invalid target size: {value}", "use bytes or a size such as 10MB")
    number = float(match.group(1)); unit = (match.group(2) or "").upper()
    amount = int(number * (1024 ** {"": 0, "KB": 1, "MB": 2, "GB": 3}[unit]))
    if amount <= 0:
        raise PdfError("BAD_ARG", "target size must be positive", "give a positive byte size")
    return amount


def _policy(source, output, operation):
    return compare(source, output, "compress_lossy" if operation and operation.ssim_floor else "compress_lossless", operation)


def _merge_warnings(result,warnings):
    result["warnings"]=list(dict.fromkeys(result.get("warnings",[])+warnings))
    if result.get("verdict")=="ok" and result["warnings"]:result["verdict"]="warn"
    return result


def _features(doc):
    widgets = links = annots = signatures = False
    scanned = False
    for page in doc:
        widgets |= bool(list(page.widgets() or [])); links |= bool(page.get_links()); annots |= bool(list(page.annots() or []))
        images = page.get_image_info(xrefs=True); words = page.get_text("words")
        ocr_only = any(span.get("font", "").lower().startswith("glyphlessfont") or span.get("render_mode") == 3
                       for block in page.get_text("dict")["blocks"] if "lines" in block
                       for line in block["lines"] for span in line["spans"])
        scanned |= len(images)==1 and not page.get_drawings() and (not words or ocr_only)
    try: signatures = doc.get_sigflags() > 0
    except Exception: pass
    reasons = [name for name, val in (("forms", widgets), ("links", links), ("annotations", annots), ("signatures", signatures)) if val]
    return scanned, reasons


def _image_counts(doc, include_graphics):
    skipped = 0; total = 0; dimensions = []; seen = set()
    for page in doc:
        for row in page.get_images(full=True):
            xref=row[0]
            if not xref or xref in seen: continue
            seen.add(xref); total += 1
            try:
                bpc=row[4] if len(row)>4 else None
                cs_type,cs_value=doc.xref_get_key(xref,"ColorSpace")
                filter_type,filter_value=doc.xref_get_key(xref,"Filter")
                indexed="Indexed" in cs_value
                flate="FlateDecode" in filter_value
                if not include_graphics and ("JPXDecode" in filter_value or flate or bpc==1 or indexed):
                    skipped += 1
            except Exception: pass
    return total, skipped, dimensions


@command("compress", "5", "pdf", "pdf", "_compressed", True, _policy)
def compress(args, context):
    if bool(getattr(args,"file",None)) == bool(getattr(args,"each",None)):
        raise PdfError("BAD_ARG","give exactly one of FILE and --each","provide a PDF file or --each folder/glob")
    if args.each:
        from .p5_batch import _glob_files
        return run_each(_glob_files(args.each), lambda file: compress(_per_file(args, file), context), "compress", context)
    path = Path(args.file).resolve()
    target_size = _parse_target(args.target) if args.target else None
    chosen_levels = ["lossless", "med", "high", "max"] if target_size else [args.level]
    before = path.stat().st_size
    warnings = []
    def writer(doc, staged):
        nonlocal warnings
        scanned, reasons = _features(doc)
        total_images, skipped_images, _ = _image_counts(doc, args.include_graphics)
        blocked = bool(reasons)
        passing = []
        font_count, unembedded_fonts = _font_counts(doc)
        font_note=f"{font_count} fonts subset"
        if unembedded_fonts:font_note+=f"; {unembedded_fonts} unembedded fonts could not be subset"
        source_copy = Path(context.temp) / "compress-original.pdf"
        doc.save(source_copy)
        source_dpi = _scanned_min_dpi(doc) if scanned else float("inf")
        gs = find_tool("ghostscript")
        ocrmypdf = _tool("ocrmypdf")
        if doc.page_count > 10:
            sample_path=Path(context.temp)/"compress-sample.pdf";sample_out=Path(context.temp)/"compress-sample-candidate.pdf"
            with pymupdf.open(source_copy) as sample, pymupdf.open() as first_two:
                first_two.insert_pdf(sample,from_page=0,to_page=1);first_two.save(sample_path)
            started=time.perf_counter()
            with pymupdf.open(sample_path) as sample:
                sample.subset_fonts()
                sample_level=chosen_levels[0];sample_ssim,sample_threshold,sample_dpi,sample_quality=LEVELS[sample_level]
                if sample_threshold is not None:
                    sample.rewrite_images(dpi_threshold=sample_threshold,
                        dpi_target=max(150,sample_dpi) if scanned and sample_level in {"med","high"} else sample_dpi,
                        quality=sample_quality,lossy=True,lossless=args.include_graphics,bitonal=args.include_graphics,color=True,gray=True)
                sample.ez_save(sample_out,garbage=4,deflate=True,use_objstms=True)
            seconds_per_page=(time.perf_counter()-started)/2
            planned=len(chosen_levels)*(1+int(bool(args.include_graphics and not blocked and gs))+int(bool(scanned and not blocked)))
            announce_estimate(doc.page_count,seconds_per_page*planned)
        for level in chosen_levels:
            ssim, threshold, dpi, quality = LEVELS[level]
            operation = OperationResult(pages=[[Placement(None, i)] for i in range(doc.page_count)], ssim_floor=ssim)
            pymu = Path(context.temp) / f"compress-{level}-pymupdf.pdf"
            with pymupdf.open(source_copy) as candidate:
                candidate.subset_fonts()
                if threshold is not None:
                    opts = dict(dpi_threshold=threshold, dpi_target=max(150,dpi) if scanned and level in {"med","high"} else dpi, quality=quality,
                                lossy=True, lossless=args.include_graphics, bitonal=args.include_graphics,
                                color=True, gray=True)
                    candidate.rewrite_images(**opts)
                for xref in _thumbnail_xrefs(candidate):
                    try: candidate.xref_set_key(xref, "Thumb", "null")
                    except Exception: pass
                candidate.ez_save(pymu, garbage=4, deflate=True, use_objstms=True)
            graphics_note=f"{skipped_images} graphics left alone (lossless)" if skipped_images else "0 graphics left alone"
            level_candidates=[(pymu, "pymupdf", operation, f"{total_images} images; {font_note}; {graphics_note}")]
            if args.include_graphics and level in {"high", "max"} and not blocked and gs:
                gs_out = Path(context.temp) / f"compress-{level}-gs.pdf"
                resolution = 150 if scanned and level == "high" else (110 if level == "high" else 72)
                argv = [gs,"-dSAFER","-dBATCH","-dNOPAUSE","-dQUIET","-sDEVICE=pdfwrite","-dCompatibilityLevel=1.7",
                        "-dAutoRotatePages=/None","-dDetectDuplicateImages=true","-dCompressFonts=true","-dSubsetFonts=true",
                        "-dColorConversionStrategy=/LeaveColorUnchanged","-dDownsampleColorImages=true","-dDownsampleGrayImages=true",
                        "-dDownsampleMonoImages=false",f"-dColorImageResolution={resolution}",f"-dGrayImageResolution={resolution}",
                        "-dColorImageDownsampleThreshold=1.5","-dGrayImageDownsampleThreshold=1.5","-dAutoFilterColorImages=true",
                        "-dAutoFilterGrayImages=true",f"-dNumRenderingThreads={max(1,os.cpu_count() or 1)}","-o",str(gs_out),str(source_copy)]
                run_tool(argv, timeout=max(120,30*doc.page_count), cwd=context.temp,
                         env=dev_tool_env(context.temp), temp=context.temp, secret_env=context.secret_env,
                         secret_values=context.secret_store.values if context.secret_store else ())
                level_candidates.append((gs_out,"ghostscript",operation,f"{total_images} images processed; {font_note}"))
            if scanned and not blocked:
                opt = 1 if level in {"lossless","med"} or not args.include_graphics else (2 if level == "high" else 3)
                scan_out = Path(context.temp) / f"compress-{level}-ocrmypdf.pdf"
                io.disk_budget([path], Path(context.temp) / "budget", context.temp, multiplier=20)
                argv = [ocrmypdf,"--ocr-engine","none","--skip-text","--output-type","pdf",f"-O{opt}","--jobs",str(max(1,os.cpu_count() or 1)),str(source_copy),str(scan_out)]
                run_tool(argv, timeout=max(120,60*doc.page_count), cwd=context.temp,
                         env=dev_tool_env(context.temp), temp=context.temp, secret_env=context.secret_env,
                         secret_values=context.secret_store.values if context.secret_store else ())
                level_candidates.append((scan_out,f"ocrmypdf -O{opt}",operation,f"scanned pages optimized ({opt}); {font_note}"))
            if blocked and (scanned or args.include_graphics and level in {"high","max"}):
                if "SKIPPED_GS" not in warnings:
                    reason=", ".join(reasons); warnings.append("SKIPPED_GS")
                    import clierr
                    clierr.warn("pdf compress","SKIPPED_GS",f"external compression skipped because the PDF contains {reason}","remove those features or review the PyMuPDF candidate")
            for candidate_path,method,op,changes in level_candidates:
                try:
                    with pymupdf.open(source_copy) as src, pymupdf.open(candidate_path) as out:
                        findings=compare(src,out,"compress_lossy" if op.ssim_floor else "compress_lossless",op)
                        # never below 150 dpi, nor below a scan that was already coarser than that
                        dpi_ok=not (scanned and level in {"med","high"}) or _scanned_dpi_ok(out,min(150,source_dpi)-1)
                    if not any(f.tier=="invariant" for f in findings) and dpi_ok:
                        passing.append((candidate_path.stat().st_size,candidate_path,method,op,changes))
                except Exception as exc:
                    import clierr
                    clierr.warn("pdf compress","CANDIDATE_FAILED",f"{method} candidate failed its check: {exc}","review the log and try another compression level")
                    clierr.write_log("pdf compress","CANDIDATE_FAILED",f"{method} candidate check raised","review the candidate exception",exc,
                                     secrets=context.secret_store.values if context.secret_store else (),log_name="compress")
            if passing:
                winner=min(passing,key=lambda row:row[0])
                if target_size and winner[0] <= target_size:
                    break
                if not target_size:
                    break
        source_bytes=source_copy.stat().st_size
        if not passing or min(row[0] for row in passing) >= source_bytes:
            return OperationResult(keep_input="NO_GAIN")
        if target_size and winner[0] > target_size:
            gap=winner[0]-target_size;warnings.append("TARGET_MISSED")
            import clierr
            clierr.warn("pdf compress","TARGET_MISSED",f"smallest passing output is {gap} bytes over target","use a larger target or --include-graphics")
        shutil.copy2(winner[1],staged)
        context.compress_details={"before":before,"method":winner[2],"changes":winner[4],"ssim":winner[3].ssim_floor}
        return winner[3]
    result=write_pdf(path,_spec(),args,context,writer)
    if result.get("path")==str(path):
        after=result.get("bytes",before)
        result.update(before=before,after=after,method="none",changes="no passing candidate was smaller")
        result["note"]=f"{before} → {after} bytes ({(before-after)*100/before if before else 0:.1f} %), none: no passing candidate was smaller"
        if warnings:_merge_warnings(result,warnings)
        return result
    if result.get("type")=="result" and warnings:_merge_warnings(result,warnings)
    if result.get("type")=="result" and hasattr(context,"compress_details"):
        d=context.compress_details; after=result.get("bytes",0); pct=(d["before"]-after)*100/d["before"] if d["before"] else 0
        result.update(d,after=after)
        result["note"]=f"{d['before']} → {after} bytes (-{pct:.1f} %), {d['method']}: {d['changes']}"
    return result


def _thumbnail_xrefs(doc):
    for xref in range(1,doc.xref_length()):
        typ,key=doc.xref_get_key(xref,"Thumb")
        if typ!="null": yield xref


def _font_counts(doc):
    fonts=set();unembedded=set()
    for page_number in range(doc.page_count):
        for row in doc.get_page_fonts(page_number,full=True):
            xref=row[0]
            if not xref or xref in fonts:continue
            fonts.add(xref)
            try:
                _,_,_,data=doc.extract_font(xref)
                if not data:unembedded.add(xref)
            except Exception:unembedded.add(xref)
    return len(fonts),len(unembedded)


def _tool(name):
    return shutil.which(name) or str(Path(sys.prefix)/("Library/bin" if os.name=="nt" else "bin")/name)


def _scanned_dpi_ok(doc, floor=150):
    return _scanned_min_dpi(doc) >= floor


def _scanned_min_dpi(doc):
    """Lowest effective dpi of the images on scanned pages (inf when there are none)."""
    lowest=float("inf")
    for page in doc:
        images=page.get_image_info(xrefs=True);words=page.get_text("words")
        ocr_only=any(span.get("font","").lower().startswith("glyphlessfont") or span.get("render_mode")==3
                     for block in page.get_text("dict")["blocks"] if "lines" in block
                     for line in block["lines"] for span in line["spans"])
        if not (len(images)==1 and not page.get_drawings() and (not words or ocr_only)):continue
        for info in images:
            bbox=info.get("bbox")
            if not bbox:continue
            rect=pymupdf.Rect(bbox)
            if rect.width<=0 or rect.height<=0:continue
            lowest=min(lowest,72*info.get("width",0)/rect.width,72*info.get("height",0)/rect.height)
    return lowest


def _spec():
    from ..registry import get
    return get("compress")


def _per_file(args,file):
    import argparse
    values=vars(args).copy();values["file"]=file;values["each"]=None
    return argparse.Namespace(**values)
