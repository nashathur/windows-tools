"""Implementation helpers for the phase 3a read-only commands."""

from __future__ import annotations

import difflib
import hashlib
import json as json_module
import os
import re
from collections import Counter
from pathlib import Path

import pymupdf

from .. import check, coords
from ..errors import PdfError
from ..pipeline import open_input, write_non_pdf
from .. import crypt


def _path(args):
    path = Path(args.file).expanduser().resolve()
    if not path.is_file():
        if path.exists():
            raise PdfError("BAD_INPUT", f"not a PDF file: {path}", "choose a readable PDF file")
        raise PdfError("NOT_FOUND", f"PDF not found: {path}", "check the path")
    return path


def _open(path, context):
    return open_input(path, context.password or context.owner_password)


def _report(command, path, data, verdict="report"):
    return {"type": "report", "command": command, "input": str(path), "verdict": verdict, "data": data}


def run_safely(function, args, context):
    try:
        return function(args, context)
    except PdfError:
        raise
    except FileNotFoundError as exc:
        path = getattr(args, "file", "input")
        raise PdfError("NOT_FOUND", f"input not found: {path}", "check the path", detail=str(exc))
    except PermissionError as exc:
        path = getattr(args, "file", "input")
        raise PdfError("LOCKED", f"input is not readable: {path}", "close the program holding it, then rerun", detail=str(exc))
    except Exception as exc:
        path = getattr(args, "file", "input")
        raise PdfError("BAD_INPUT", f"cannot read PDF: {path}", "check that the PDF is valid", detail=str(exc))


def _save_or_report(command, path, args, context, content, suffix, kind, mime=None):
    output = getattr(args, "output", None)
    if output is None:
        encrypted = False
        try:
            import pikepdf
            with pikepdf.open(path, password=context.password or context.owner_password or "") as pdf:
                encrypted = bool(pdf.is_encrypted)
        except Exception:
            pass
        warnings = crypt.export_warnings(encrypted)
        if warnings:
            import clierr
            for warning in warnings:
                clierr.warn("pdf", warning, f"{command} exported encrypted input to stdout", "review the output")
        return _report(command, path, content, "warn" if warnings else "report")

    def writer(staged):
        staged.write_text(content, encoding="utf-8", newline="")

    path_out = Path(output)
    if path_out.is_dir() or str(output).endswith(("/", "\\")):
        path_out = path_out / f"{Path(path).stem}{suffix}"
        args.output = path_out
    try:
        return write_non_pdf(path, _spec(command), args, writer, kind=kind,
                             password=context.password or context.owner_password)
    except OSError as exc:
        raise PdfError("LOCKED", f"output is not writable: {path_out}", "choose a writable destination", detail=str(exc))


def _spec(command):
    from ..registry import get
    return get(command)


def pages(args, context):
    path = _path(args)
    rows = []
    with _open(path, context) as doc:
        for page in doc:
            text = page.get_text("text").strip()
            first = next((line.strip() for line in text.splitlines() if line.strip()), "")
            width, height = page.rect.width * coords.MM_PER_POINT, page.rect.height * coords.MM_PER_POINT
            name = _page_size_name(width, height)
            label = page.get_label() or str(page.number + 1)
            rows.append({"number": page.number + 1, "label": label,
                         "size_mm": [round(width, 2), round(height, 2)], "size_name": name,
                         "rotation": page.rotation, "blank": not page.get_text("words") and check.page_stats(page)["blank"], "first_line": first})
    return _report("pages", path, "\n".join(
        f"{r['number']} (label {r['label']}): {r['size_mm'][0]} × {r['size_mm'][1]} mm ({r['size_name']}), "
        f"rotation {r['rotation']}°, blank {'yes' if r['blank'] else 'no'}; {r['first_line']}" for r in rows))


def _page_size_name(width, height):
    known = ((210, 297, "A4"), (215.9, 279.4, "Letter"), (215.9, 355.6, "Legal"),
             (148, 210, "A5"), (297, 420, "A3"))
    for w, h, name in known:
        if min(abs(width - w) + abs(height - h), abs(width - h) + abs(height - w)) < 2:
            return name
    return "custom"


def text(args, context):
    path = _path(args)
    with _open(path, context) as doc:
        content = "\f".join(page.get_text("text", sort=bool(args.layout)) for page in doc)
    return _save_or_report("text", path, args, context, content, ".txt", "txt")


def count(args, context):
    path = _path(args)
    totals = Counter()
    rows = []
    with _open(path, context) as doc:
        for page in doc:
            text = page.get_text("text")
            words = text.split()
            blocks = [block[4] for block in page.get_text("blocks") if len(block) > 4 and str(block[4]).strip()]
            values = {"words": len(words), "characters": len(text),
                      "characters_without_spaces": sum(not ch.isspace() for ch in text),
                      "paragraphs": len(blocks), "lines": len(text.splitlines()),
                      "no_text_layer": not bool(text.strip())}
            rows.append(values)
            totals.update({key: val for key, val in values.items() if isinstance(val, int) and not isinstance(val, bool)})
    if args.total:
        data = {"pages_without_text_layer": sum(row["no_text_layer"] for row in rows), **dict(totals)}
        return _report("count", path, data)
    lines = [f"page {i + 1}: words {r['words']}, characters {r['characters']} "
             f"({r['characters_without_spaces']} without spaces), paragraphs {r['paragraphs']}, "
             f"lines {r['lines']}" + (", no text layer" if r["no_text_layer"] else "")
             for i, r in enumerate(rows)]
    lines.append("total: " + ", ".join(f"{key} {value}" for key, value in totals.items()))
    return _report("count", path, "\n".join(lines))


def json_report(args, context):
    path = _path(args)
    with _open(path, context) as doc:
        pages_data = []
        for page in doc:
            raw = page.get_text("rawdict")
            blocks, lines, spans = [], [], []
            for block in raw.get("blocks", []):
                if block.get("type") != 0:
                    continue
                block_lines = []
                for line in block.get("lines", []):
                    text_spans = []
                    for span in line.get("spans", []):
                        chars = span.get("chars", [])
                        value = "".join(ch.get("c", "") for ch in chars)
                        item = {"text": value, "font": span.get("font"), "size": span.get("size"),
                                "bbox_pymupdf": list(span.get("bbox", [])),
                                "bbox_mm": _bbox_mm(page, span.get("bbox", []))}
                        spans.append(item)
                        text_spans.append(item)
                    row = {"spans": text_spans}
                    block_lines.append(row)
                    lines.append(row)
                blocks.append({"lines": block_lines})
            pages_data.append({"number": page.number + 1, "blocks": blocks, "lines": lines, "spans": spans})
        result = {"pages": pages_data, "metadata": doc.metadata}
    content = json_module.dumps(result, ensure_ascii=False, indent=2)
    return _save_or_report("json", path, args, context, content, ".json", "json")


def _bbox_mm(page, bbox):
    if len(bbox) != 4:
        return []
    rect = coords.pymupdf_to_display(page, pymupdf.Rect(bbox))
    return [round(v, 3) for v in rect]


def html(args, context):
    path = _path(args)
    with _open(path, context) as doc:
        content = "\n".join(page.get_text("xhtml") for page in doc)
    return _save_or_report("html", path, args, context, content, ".html", "html")


def size(args, context):
    path = _path(args)
    import pikepdf
    with _open(path, context), pikepdf.open(path, password=context.password or context.owner_password or "") as pdf:
        objects = {}
        def register(obj, kind, page_xrefs=None):
            if not isinstance(obj, (pikepdf.Stream, pikepdf.Dictionary)):
                return None
            xref = obj.objgen[0]
            if not xref:
                return None
            if page_xrefs is not None:
                page_xrefs.add(xref)
            if xref not in objects:
                objects[xref] = {"xref": xref, "kind": kind,
                                 "bytes": len(obj.read_raw_bytes()) if isinstance(obj, pikepdf.Stream) else 0}
            elif objects[xref]["kind"] == "content streams" and kind != "content streams":
                objects[xref]["kind"] = kind
            return xref

        def font_streams(font, page_xrefs):
            for candidate in [font, *list(font.get("/DescendantFonts", []))]:
                descriptor = candidate.get("/FontDescriptor")
                if isinstance(descriptor, pikepdf.Dictionary):
                    for key in ("/FontFile", "/FontFile2", "/FontFile3"):
                        stream = descriptor.get(key)
                        if isinstance(stream, pikepdf.Stream):
                            register(stream, "fonts", page_xrefs)

        def resources(resource_dict, page_xrefs, seen):
            if not isinstance(resource_dict, pikepdf.Dictionary):
                return
            fonts = resource_dict.get("/Font", {})
            if isinstance(fonts, pikepdf.Dictionary):
                for font in fonts.values():
                    font_streams(font, page_xrefs)
            xobjects = resource_dict.get("/XObject", {})
            if not isinstance(xobjects, pikepdf.Dictionary):
                return
            for xobj in xobjects.values():
                if not isinstance(xobj, pikepdf.Stream):
                    continue
                subtype = str(xobj.get("/Subtype", ""))
                xref = register(xobj, "images" if subtype == "/Image" else "content streams", page_xrefs)
                if xref in seen:
                    continue
                seen.add(xref)
                smask = xobj.get("/SMask")
                if isinstance(smask, pikepdf.Stream):
                    register(smask, "images", page_xrefs)
                if subtype == "/Form":
                    resources(xobj.get("/Resources", {}), page_xrefs, seen)

        page_usage = []
        for page_number, page_obj in enumerate(pdf.pages, 1):
            page_xrefs = set()
            resources(page_obj.obj.get("/Resources", {}), page_xrefs, set())
            contents = page_obj.obj.get("/Contents")
            for stream in contents if isinstance(contents, pikepdf.Array) else [contents]:
                if isinstance(stream, pikepdf.Stream):
                    register(stream, "content streams", page_xrefs)
            page_usage.append({"page": page_number, "xrefs": sorted(page_xrefs)})
        attachments = []
        for name, attachment in pdf.attachments.items():
            spec = attachment.obj
            ef = spec.get("/EF", {})
            stream = ef.get("/UF") or ef.get("/F")
            xref = register(stream, "attachments") if isinstance(stream, pikepdf.Stream) else None
            attachments.append({"name": name, "xref": xref, "bytes": objects[xref]["bytes"] if xref else 0})
        info = pdf.trailer.get("/Info")
        metadata_bytes = 0
        if isinstance(info, pikepdf.Dictionary) and info.objgen[0]:
            info_bytes = len(bytes(info.unparse()))
            objects.setdefault(info.objgen[0], {"xref": info.objgen[0], "kind": "metadata", "bytes": info_bytes})
            metadata_bytes += info_bytes
        metadata = pdf.Root.get("/Metadata")
        if isinstance(metadata, pikepdf.Stream):
            xref = register(metadata, "metadata")
            metadata_bytes += objects[xref]["bytes"]
        categories = {}
        for item in objects.values():
            categories[item["kind"]] = categories.get(item["kind"], 0) + item["bytes"]
        return _report("size", path, {"objects": sorted(objects.values(), key=lambda item: (item["kind"], item["xref"])),
                                      "page_usage": page_usage, "attachments": attachments,
                                      "metadata_bytes": metadata_bytes,
                                      "bytes_by_kind": categories, "total_bytes": path.stat().st_size})


def check_file(args, context):
    path = _path(args)
    if not args.against:
        with _open(path, context) as doc:
            return _report("check", path, check.inspect(doc))
    origin = Path(args.against).expanduser().resolve()
    if not origin.is_file():
        raise PdfError("NOT_FOUND", f"PDF not found: {origin}", "check the --against path")
    with _open(path, context) as output_doc, _open(origin, context) as source_doc:
        pairs, ambiguous = _page_pairs(output_doc, source_doc, args.map)
        operation = check.OperationResult(pages=[[
            check.Placement(str(origin), source_page if source_page is not None else "generated",
                            rotation=source_doc[source_page].rotation if source_page is not None else None)
        ] for source_page in pairs])
        findings = check.compare(source_doc, output_doc, operation=operation,
                                 source_docs={str(origin): source_doc})
        for output_index, source_index in enumerate(pairs):
            if output_index in ambiguous:
                findings.append(check.Finding("report", "AMBIGUOUS_MATCH", "output page has multiple equally good origin matches",
                                              output_index + 1, {"source_candidates": [page + 1 for page in ambiguous[output_index]]}))
            if source_index is None:
                findings.append(check.Finding("report", "UNMATCHED_OUTPUT", "output page did not match an origin page", output_index + 1))
            else:
                a, b = source_doc[source_index], output_doc[output_index]
                if abs(a.rect.width - b.rect.width) > 0.01 or abs(a.rect.height - b.rect.height) > 0.01:
                    findings.append(check.Finding("report", "PAGE_SIZE_CHANGED", "matched page sizes differ",
                                                  output_index + 1, {"source": list(a.rect), "output": list(b.rect)}))
        matched_sources = {page for page in pairs if page is not None}
        for source_index in range(len(source_doc)):
            if source_index not in matched_sources:
                findings.append(check.Finding("report", "UNMATCHED_SOURCE", "origin page did not match an output page", source_index + 1))
        pre_existing = [item for item in findings if item.tier == "pre_existing"]
        visible_findings = [item for item in findings if item.tier != "pre_existing"]
        if getattr(args, "v", False):
            visible_findings.extend(pre_existing)
        data = {"against": str(origin), "pairs": [
            {"output_page": i + 1, "source_page": (j + 1) if j is not None else None} for i, j in enumerate(pairs)
        ], "ambiguous": {str(i + 1): [page + 1 for page in choices] for i, choices in ambiguous.items()},
            "findings": [vars(item) for item in visible_findings]}
        if pre_existing:
            data["pre_existing_count"] = len(pre_existing)
        return _report("check", path, data,
                       verdict="warn" if any(item.tier in {"difference", "new_warning", "invariant"} for item in findings) else "report")


def _page_pairs(output_doc, source_doc, manual):
    pairs = [None] * len(output_doc)
    used = set()
    ambiguous = {}
    if manual:
        for item in manual.split(","):
            match = re.fullmatch(r"\s*(\d+)\s*=\s*(\d+)\s*", item)
            if not match:
                raise PdfError("BAD_ARG", f"invalid --map pair: {item}", "use output_page=source_page, for example 1=1,2=3")
            output_page, source_page = (int(value) for value in match.groups())
            if not 1 <= output_page <= len(output_doc) or not 1 <= source_page <= len(source_doc):
                raise PdfError("BAD_PAGE", "--map page is outside the document", "choose existing 1-based page numbers")
            if pairs[output_page - 1] is not None or source_page - 1 in used:
                raise PdfError("BAD_ARG", "--map pages must be unique", "map each output and source page at most once")
            pairs[output_page - 1] = source_page - 1
            used.add(source_page - 1)
        return pairs, ambiguous
    source_text = [_normalized_text(page) for page in source_doc]
    output_text = [_normalized_text(page) for page in output_doc]
    for oi, text in enumerate(output_text):
        if not text:
            continue
        candidates = sorted(((difflib.SequenceMatcher(None, text, other, autojunk=False).ratio(), si)
                             for si, other in enumerate(source_text) if other and si not in used), reverse=True)
        if candidates:
            score, si = candidates[0]
            if score >= check.TEXT_SIMILARITY_MIN:
                tied = [candidate for candidate_score, candidate in candidates
                        if score - candidate_score <= 1e-9 and candidate_score >= check.TEXT_SIMILARITY_MIN]
                if len(tied) > 1:
                    ambiguous[oi] = tied
                else:
                    pairs[oi] = si
                    used.add(si)
    remaining_output = [i for i, text in enumerate(output_text) if not text and pairs[i] is None]
    remaining_source = [i for i, text in enumerate(source_text) if not text and i not in used]
    source_hashes = {i: _difference_hash(source_doc[i]) for i in remaining_source}
    for oi in remaining_output:
        output_hash = _difference_hash(output_doc[oi])
        if not source_hashes:
            break
        distances = {si: (output_hash ^ value).bit_count() for si, value in source_hashes.items()}
        best_distance = min(distances.values())
        tied = [si for si, distance in distances.items() if distance == best_distance]
        if len(tied) > 1:
            ambiguous[oi] = tied
            continue
        si = tied[0]
        pairs[oi] = si
        used.add(si)
        del source_hashes[si]
    return pairs, ambiguous


def _normalized_text(page):
    words = page.get_text("words")
    return " ".join(word[4].casefold() for word in words)


def _difference_hash(page):
    pix = page.get_pixmap(matrix=pymupdf.Matrix(32 / max(page.rect.width, 1), 32 / max(page.rect.height, 1)),
                          colorspace=pymupdf.csGRAY, alpha=False)
    samples = pix.samples
    width, height = pix.width, pix.height
    bits = 0
    for row in range(8):
        y = min(height - 1, round(row * (height - 1) / 7))
        row = samples[y * width:(y + 1) * width]
        columns = [round(column * (width - 1) / 8) for column in range(9)]
        for left, right in zip(columns, columns[1:]):
            bits = (bits << 1) | int(row[left] > row[right])
    return bits


def open_file(args, context):
    path = _path(args)
    try:
        os.startfile(str(path))
    except Exception as exc:
        raise PdfError("TOOL_FAILED", f"could not open PDF viewer: {path}", "check the default PDF viewer", detail=str(exc))
    return _report("open", path, f"opened {path}")

