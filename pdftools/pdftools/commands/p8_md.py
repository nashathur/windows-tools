"""PDF to Markdown and the related chapter project commands."""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import re
import shutil
import sys
import time
from pathlib import Path

import pymupdf

from ..errors import PdfError
from .. import io
from ..chapters import plan as chapter_plan
from ..pages import parse_page_ranges
from ..pipeline import announce_estimate, open_input
from ..registry import command

SECONDS_PER_PAGE = 0.4
SECONDS_PER_PART = 300
PARALLEL = 3


def _workspace(context):
    """Create the run-scoped scratch folder used by encrypted copies and phase-5 OCR."""
    from .. import io
    temp = io.run_temp()
    previous = getattr(context, "temp", None)
    if context is not None:
        context.temp = temp
    return temp, previous


def _clear_workspace(context, temp, previous):
    shutil.rmtree(temp, ignore_errors=True)
    if context is not None:
        context.temp = previous
    from .. import watchdog
    watchdog.unregister_path(temp)


@contextlib.contextmanager
def _decrypted_copy(source, password, temp):
    """Yield (path, encrypted), keeping clear PDF bytes only in run temp and always removing them."""
    source = Path(source)
    target = Path(temp) / f"{source.stem}-decrypted.pdf"
    target.unlink(missing_ok=True)
    try:
        probe = pymupdf.open(source)
        encrypted = bool(probe.needs_pass or probe.is_encrypted)
        probe.close()
        with open_input(source, password) as doc:
            if encrypted:
                doc.save(target, encryption=pymupdf.PDF_ENCRYPT_NONE, garbage=3, deflate=True)
                yield target, True
            else:
                yield source, False
    finally:
        target.unlink(missing_ok=True)


def _folder_sources(folders, password, temp, stack):
    paths, encrypted = {}, False
    for folder in folders:
        pages_path = next(Path(folder).glob("*.pages.json"), None)
        if not pages_path:
            continue
        data = json.loads(pages_path.read_text(encoding="utf-8"))
        original = data.get("source_pdf")
        source = data.get("extraction_source_pdf") or original
        if not source:
            continue
        path, was_encrypted = stack.enter_context(_decrypted_copy(source, password, temp))
        paths[str(folder)] = path
        encrypted |= was_encrypted
        if original and Path(original).resolve() != Path(source).resolve():
            import pymupdf
            with pymupdf.open(original) as original_doc:
                encrypted |= bool(original_doc.needs_pass)
    return paths, encrypted


def _digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def _atomic(path, text):
    from .p8_io import atomic_write
    atomic_write(path, text)


def _slug(text, n=40):
    return re.sub(r"[^a-z0-9]+", "_", text.lower()).strip("_")[:n].rstrip("_") or "part"


def _selected(spec, values):
    if not spec:
        return list(values)
    chosen = set()
    try:
        for item in spec.split(","):
            a, sep, b = item.partition("-")
            chosen.update(range(int(a), int(b or a) + 1))
    except ValueError as exc:
        raise PdfError("BAD_ARG", "invalid part selection", "use --parts 2,5-7") from exc
    invalid = chosen - set(values)
    if invalid:
        raise PdfError("BAD_ARG", f"part number out of range: {min(invalid)}", "use numbers from md --list")
    return sorted(chosen)


def _parts(doc, args):
    if args.chunk is not None and args.chunk <= 0:
        raise PdfError("BAD_ARG", "chunk size must be greater than zero", "use --chunk N with N >= 1")
    if getattr(args, "pages", None):
        pages = parse_page_ranges(args.pages, doc.page_count)
        return [{"title": f"Pages {min(pages) + 1}-{max(pages) + 1}", "start": min(pages), "end": max(pages),
                 "pages": pages, "repaired_from": None}]
    if args.chunk is not None:
        return chapter_plan(doc, chunk=args.chunk, cover_all=False)["parts"]
    if args.level is None and (doc.page_count <= 40 or not doc.get_toc()):
        args.outline_notes = []
        return [{"title": doc.metadata.get("title") or "Document", "start": 0, "end": doc.page_count - 1,
                 "repaired_from": None}]
    plan = chapter_plan(doc, level=(None if args.level in (None, "auto") else args.level), cover_all=False)
    args.outline_notes = plan["notes"]
    return plan["parts"]


def add_arguments_md(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--parts")
    parser.add_argument("--level", type=lambda x: "auto" if x == "auto" else int(x))
    parser.add_argument("--chunk", type=int)
    parser.add_argument("--pages")
    parser.add_argument("--no-read", action="store_true")
    parser.add_argument("--no-qa", action="store_true")
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--lang", default="eng+ind")
    parser.add_argument("--clean", action="store_true", help="clean scanned pages during phase-5 OCR")
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")
    parser.add_argument("--owner-password-file", type=Path)
    parser.add_argument("--owner-password-env")


def _plan_for(doc, args):
    return _parts(doc, args)


def _plan_rows(parts, selected, folder, stem, force):
    has_front = bool(parts and parts[0]["title"].lower() == "front matter")
    first = 0 if has_front else 1
    rows = []
    for offset, part in enumerate(parts):
        number = first + offset
        name = f"{number:02d}_{_slug(part['title'])}"
        target = folder / name
        exists = (target / f"{name}.blocks.json").exists()
        pages = list(part.get("pages", range(part["start"], part["end"] + 1)))
        rows.append({"n": number, "title": part["title"], "start": part["start"], "end": part["end"],
                     "pages": pages, "name": name, "folder": target, "exists": exists,
                     "convert": number in selected and (force or not exists)})
    return rows


def _list_report(rows, chosen, force):
    out = []
    page_total = 0
    new = 0
    for row in rows:
        marker = "*" if row["convert"] else ("done" if (row["folder"] / f"{row['name']}.blocks.json").exists() else " ")
        count = row["end"] - row["start"] + 1
        out.append(f"{marker:>4} {row['n']:3}. {row['title'][:60]:60} PDF p{row['start']+1}-{row['end']+1} ({count} pp)")
        if row["convert"]:
            page_total += count
            new += 1
    seconds = (page_total * SECONDS_PER_PAGE + new * SECONDS_PER_PART / PARALLEL)
    out.append(f"\n* = will convert: {page_total} pages in {new} parts, about {seconds / 60:.0f} min (CPU + Codex)")
    return "\n".join(out)


def _index(folder, title, rows):
    lines = [f"# {title}", "", "| # | Chapter | PDF pages | QA: likely / lower / visual | Files |", "|---|---|---|---|---|"]
    total = [0, 0, 0]
    for row in rows:
        name = row["name"]
        qpath = row["folder"] / "qa_report.json"
        if qpath.exists():
            counts = json.loads(qpath.read_text(encoding="utf-8")).get("counts", {})
            qa = f"{counts.get('high', 0)} / {counts.get('low', 0)} / {counts.get('visual', 0)}"
            total = [a + counts.get(k, 0) for a, k in zip(total, ("high", "low", "visual"))]
        else:
            qa = "not run"
        lines.append(f"| {row['n']} | [{row['title']}]({name}/{name}.md) | {row['start']+1}–{row['end']+1} | {qa} | [QA report]({name}/qa_report.md) |")
    lines += ["", f"QA total: {total[0]} likely errors, {total[1]} lower confidence, {total[2]} equations to check visually."]
    _atomic(folder / "00_index.md", "\n".join(lines) + "\n")


def _run_ocr(source, destination, args, context):
    """Call Phase 5's registered OCR step in-process."""
    from .p5_ocr import add_arguments_ocr, ocr
    parser = argparse.ArgumentParser()
    add_arguments_ocr(parser)
    values = {action.dest: action.default for action in parser._actions if action.dest != "help"}
    values.update(file=source, each=None, output=destination, lang=args.lang, dry_run=False,
                  clean=getattr(args, "clean", False))
    values.setdefault("pdfa", False); values.setdefault("clean", False)
    return ocr(argparse.Namespace(**values), context)


def _convert_part(source, folder, row, read, ocr_pages=()):
    from .p8_extract import convert
    page_spec = ",".join(str(page) for page in row.get("pages", range(row["start"], row["end"] + 1)))
    with contextlib.redirect_stdout(sys.stderr):
        return convert(str(source), str(folder), page_spec, row["name"], read=False, ocr_pages=ocr_pages)


def _counts(folder):
    eqs_file = folder / "eqs.json"
    eqs = json.loads(eqs_file.read_text(encoding="utf-8")) if eqs_file.exists() else []
    report = folder / "qa_report.json"
    counts = json.loads(report.read_text(encoding="utf-8")).get("counts", {}) if report.exists() else {}
    table_flags = 0
    if report.exists():
        table_flags = sum(("table structure" in item.get("reason", "").lower()
                           and "not checkable" not in item.get("reason", "").lower())
                          or "table may be garbled" in item.get("reason", "").lower()
                          for item in json.loads(report.read_text(encoding="utf-8")).get("issues", []))
    return {"equations": len(eqs), "doubtful": sum(e.get("status") == "doubtful" for e in eqs),
            "unread": sum(not e.get("latex") for e in eqs), "qa_likely": counts.get("high", 0),
            "qa_lower": counts.get("low", 0), "qa_visual": counts.get("visual", 0), "tables_flagged": table_flags}


@command("md", 8, "pdf", "folder", "_md", False, "report")
def md(args, context):
    temp, previous = _workspace(context)
    try:
        source = Path(args.file).expanduser().resolve()
        password = context.password or context.owner_password or ""
        if args.dry_run:
            return _md_impl(args, context, source, False)
        with _decrypted_copy(source, password, temp) as (readable_source, encrypted):
            return _md_impl(args, context, readable_source, encrypted)
    finally:
        _clear_workspace(context, temp, previous)


def _md_impl(args, context, readable_source, encrypted):
    source = Path(args.file).expanduser().resolve()
    if not source.is_file():
        raise PdfError("NOT_FOUND", f"PDF not found: {source}", "check the PDF path")
    password = context.password or context.owner_password or ""
    try:
        with open_input(source, password) as doc:
            parts = _plan_for(doc, args)
            page_count = doc.page_count
            title = doc.metadata.get("title") or source.stem
            plan = _plan_rows(parts, [], Path(args.output or source.with_name(source.stem + "_md")), source.stem, args.force)
    except PdfError:
        raise
    except Exception as exc:
        raise PdfError("BAD_INPUT", f"cannot plan Markdown conversion: {exc}", "check the PDF and password", detail=str(exc))
    selected = _selected(args.parts, [r["n"] for r in plan])
    if args.list:
        listing = _list_report(_plan_rows(parts, selected, Path(args.output or source.with_name(source.stem + "_md")), source.stem, args.force), selected, args.force)
        if getattr(args, "outline_notes", None):
            listing += "\n\nOutline repair notes:\n" + "\n".join(f"  note: {note}" for note in args.outline_notes)
        return {"type": "report", "command": "md", "input": str(source), "verdict": "report",
                "data": listing}
    folder = Path(args.output).expanduser().resolve() if args.output else source.with_name(source.stem + "_md")
    if getattr(args, "dry_run", False):
        return {"type": "result", "command": "md", "path": str(folder), "kind": "folder", "pages": page_count,
                "bytes": 0, "verdict": "report", "provisional": True, "warnings": []}
    folder.mkdir(parents=True, exist_ok=True)
    manifest_path = folder / ".pdftools-md.json"
    digest = _digest(source)
    if manifest_path.exists():
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        if manifest.get("sha256") != digest or manifest.get("source") != str(source):
            raise PdfError("BAD_ARG", "Markdown output folder belongs to a different source PDF", "choose its original folder or a new -o folder")
    elif any(folder.iterdir()) and not any(folder.glob("*/.blocks.json")) and not any(folder.glob("*/*.blocks.json")):
        raise PdfError("BAD_ARG", "output folder has no pdftools/pdf2md conversion files", "choose an empty folder or a matching conversion folder")
    else:
        legacy_blocks = list(folder.glob("*/*.blocks.json"))
        for blocks_path in legacy_blocks:
            pages_path = blocks_path.with_name(blocks_path.name[:-len(".blocks.json")] + ".pages.json")
            if not pages_path.is_file():
                raise PdfError("BAD_ARG", f"legacy part {blocks_path.parent.name} has no source metadata",
                               "choose its original PDF or a new -o folder")
        for pages_path in folder.glob("*/*.pages.json"):
            try:
                legacy = json.loads(pages_path.read_text(encoding="utf-8"))
                recorded = Path(legacy["source_pdf"]).expanduser().resolve()
                recorded_digest = legacy.get("source_sha256") or legacy.get("sha256")
                if (recorded != source or not recorded.is_file() or _digest(recorded) != digest
                        or recorded_digest is not None and recorded_digest != digest):
                    raise ValueError("source path or content hash differs")
            except Exception as exc:
                raise PdfError("BAD_ARG", f"legacy part {pages_path.parent.name} does not match this source PDF",
                               "choose its original PDF or a new -o folder", detail=str(exc)) from exc
    _atomic(manifest_path, json.dumps({"source": str(source), "size": source.stat().st_size, "sha256": digest}, ensure_ascii=False, indent=2) + "\n")
    rows = _plan_rows(parts, selected, folder, source.stem, args.force)
    pages_to_convert = sum(len(row["pages"]) for row in rows if row["convert"])
    parts_to_convert = sum(row["convert"] for row in rows)
    if pages_to_convert:
        announce_estimate(pages_to_convert, SECONDS_PER_PAGE + (SECONDS_PER_PART / PARALLEL) / max(pages_to_convert, 1))
    failures = []
    ocr_warnings = set()
    ocr_low_pages = set()
    ocr_empty_pages = set()
    codex_unavailable = False
    for row in rows:
        if not row["convert"]:
            continue
        try:
            with open_input(source, password) as doc:
                no_text = [i for i in row["pages"] if not doc[i].get_text("text").strip()]
            extract_source = readable_source
            if no_text:
                # OCR output is retained and reused only while the source digest matches.
                ocr_pdf = folder / "source_ocr.pdf"
                marker = folder / ".source-ocr.json"
                if not (ocr_pdf.exists() and marker.exists() and json.loads(marker.read_text()).get("sha256") == digest):
                    ocr_pdf.unlink(missing_ok=True)
                    rec = _run_ocr(readable_source, ocr_pdf, args, context)
                    _atomic(marker, json.dumps({"sha256": digest, "warnings": rec.get("warnings", []),
                                                "page_measurements": rec.get("page_measurements", [])}))
                    ocr_warnings.update(rec.get("warnings", []))
                    ocr_low_pages.update(item["page"] for item in rec.get("page_measurements", [])
                                         if item.get("dpi") is not None and item["dpi"] < 150)
                    with open_input(ocr_pdf) as ocr_doc:
                        ocr_empty_pages.update(page + 1 for page in no_text
                                               if not ocr_doc[page].get_text("text").strip())
                    if "OCR_EMPTY" in ocr_warnings and not ocr_empty_pages:
                        ocr_empty_pages.update(page + 1 for page in no_text)
                    cached = json.loads(marker.read_text(encoding="utf-8"))
                    cached["empty_pages"] = sorted(ocr_empty_pages)
                    _atomic(marker, json.dumps(cached))
                else:
                    cached_ocr = json.loads(marker.read_text())
                    ocr_warnings.update(cached_ocr.get("warnings", []))
                    ocr_low_pages.update(item["page"] for item in cached_ocr.get("page_measurements", [])
                                         if item.get("dpi") is not None and item["dpi"] < 150)
                    if "OCR_LOW_QUALITY" in cached_ocr.get("warnings", []) and not ocr_low_pages:
                        ocr_low_pages.update(page + 1 for page in no_text)
                    ocr_empty_pages.update(cached_ocr.get("empty_pages", []))
                    if "OCR_EMPTY" in cached_ocr.get("warnings", []) and not ocr_empty_pages:
                        ocr_empty_pages.update(page + 1 for page in no_text)
                extract_source = ocr_pdf
            _convert_part(extract_source, folder, row, False, no_text)
            pages_json = row["folder"] / f"{row['name']}.pages.json"
            if pages_json.exists():
                persistent_extract_source = (source if Path(extract_source).resolve() == Path(readable_source).resolve()
                                             else extract_source)
                page_data = json.loads(pages_json.read_text(encoding="utf-8"))
                page_data["source_pdf"] = str(source)
                page_data["extraction_source_pdf"] = str(persistent_extract_source)
                page_data["ocr_quality"] = {str(page): {"low_quality": page in ocr_low_pages,
                                                         "empty": page in ocr_empty_pages,
                                                         "warnings": sorted(code for code, pages in (("OCR_LOW_QUALITY", ocr_low_pages), ("OCR_EMPTY", ocr_empty_pages))
                                                                            if code in ocr_warnings and page in pages)}
                                              for page in (i + 1 for i in row["pages"])}
                _atomic(pages_json, json.dumps(page_data, ensure_ascii=False))
                blocks_json = row["folder"] / f"{row['name']}.blocks.json"
                if blocks_json.exists():
                    blocks_data = json.loads(blocks_json.read_text(encoding="utf-8"))
                    blocks_data["source_pdf"] = str(source)
                    blocks_data["extraction_source_pdf"] = str(persistent_extract_source)
                    _atomic(blocks_json, json.dumps(blocks_data, ensure_ascii=False))
        except Exception as exc:
            failures.append({"part": row["n"], "title": row["title"], "error": str(exc)})
            print(f"ERROR pdf md PART_FAILED: part {row['n']} ({row['title']}): {exc}", file=sys.stderr)
    completed = [str(row["folder"]) for row in rows if row["n"] in selected and
                 (row["folder"] / f"{row['name']}.blocks.json").exists()]
    if not args.no_read and completed:
        try:
            from .p8_eqread import run as read_equations
            with contextlib.redirect_stdout(sys.stderr):
                read_equations(completed, source_paths={path: _part_source(Path(path), readable_source) for path in completed}, context=context)
        except PdfError as exc:
            if exc.code == "CODEX_UNAVAILABLE":
                codex_unavailable = True
            else:
                for row in rows:
                    if str(row["folder"]) in completed:
                        failures.append({"part": row["n"], "title": row["title"], "error": str(exc)})
        except Exception as exc:
            for row in rows:
                if str(row["folder"]) in completed:
                    failures.append({"part": row["n"], "title": row["title"], "error": str(exc)})
    # Reuse persisted OCR warnings on skipped parts, so resumed runs retain their original quality verdict.
    for row in rows:
        if row["n"] not in selected:
            continue
        pages_json = row["folder"] / f"{row['name']}.pages.json"
        if pages_json.exists():
            quality = json.loads(pages_json.read_text(encoding="utf-8")).get("ocr_quality", {})
            for page, info in quality.items():
                for code in info.get("warnings", []):
                    ocr_warnings.add(code)
                    if code == "OCR_LOW_QUALITY": ocr_low_pages.add(int(page))
                    if code == "OCR_EMPTY": ocr_empty_pages.add(int(page))
    if not args.no_qa:
        from .p8_qa import run as qa_run
        for folder_path in completed:
            try:
                with contextlib.redirect_stdout(sys.stderr):
                    qa_run(folder_path, source_pdf=_part_source(Path(folder_path), readable_source))
                _include_ocr_quality(folder_path)
            except Exception as exc:
                row = next(item for item in rows if str(item["folder"]) == folder_path)
                failures.append({"part": row["n"], "title": row["title"], "error": str(exc)})
    _index(folder, title, rows)
    entries = []
    warning_codes = []
    for row in rows:
        if row["n"] not in selected:
            continue
        counts = _counts(row["folder"])
        entry = {"n": row["n"], "title": row["title"], "pages": f"{row['start']+1}-{row['end']+1}",
                 "converted": bool(row["exists"] or row["convert"] and (row["folder"] / f"{row['name']}.blocks.json").exists()), **counts}
        part_warnings = [code for code, problem_pages in (("OCR_LOW_QUALITY", ocr_low_pages),
                                                            ("OCR_EMPTY", ocr_empty_pages))
                         if code in ocr_warnings and any(page + 1 in problem_pages for page in row["pages"])]
        if part_warnings: entry["warnings"] = part_warnings
        if any(f["part"] == row["n"] for f in failures):
            entry["failed"] = next(f["error"] for f in failures if f["part"] == row["n"])
        entries.append(entry)
        if counts["doubtful"]: warning_codes.append("EQ_DOUBTFUL")
        if counts["unread"]: warning_codes.append("EQ_UNREAD")
        if counts["qa_likely"]: warning_codes.append("QA_LIKELY")
        if counts["tables_flagged"]: warning_codes.append("TABLE_FLAGGED")
        if encrypted: entry["warnings"] = list(dict.fromkeys(entry.get("warnings", []) + ["DECRYPTED_EXPORT"]))
    warning_codes.extend(code for code in ("OCR_LOW_QUALITY", "OCR_EMPTY") if code in ocr_warnings)
    if codex_unavailable: warning_codes.append("CODEX_UNAVAILABLE")
    warnings = list(dict.fromkeys(warning_codes + (["DECRYPTED_EXPORT"] if encrypted else [])))
    total_eq = sum(e["equations"] for e in entries); doubtful = sum(e["doubtful"] for e in entries)
    likely = sum(e["qa_likely"] for e in entries)
    if failures: context.exit_code = 4
    return {"type": "result", "command": "md", "path": str(folder), "kind": "folder", "pages": page_count,
            "bytes": sum(p.stat().st_size for p in folder.rglob("*") if p.is_file()),
            "verdict": "warn" if warnings or failures else "ok", "warnings": warnings,
            "parts": entries, "note": f"{len(entries)} parts, {total_eq} equations ({doubtful} marked doubtful), QA likely {likely}",
            **({"failures": failures} if failures else {})}


def _folder_arguments(parser, with_fixes=False):
    parser.add_argument("folder", type=Path)
    if with_fixes:
        parser.add_argument("fixes", type=Path)


def _password_arguments(parser):
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")
    parser.add_argument("--owner-password-file", type=Path)
    parser.add_argument("--owner-password-env")


def add_arguments_mdread(parser):
    _folder_arguments(parser); _password_arguments(parser)
def add_arguments_mdqa(parser):
    _folder_arguments(parser); _password_arguments(parser)
def add_arguments_mdfix(parser):
    _folder_arguments(parser, True)
def add_arguments_mdexport(parser):
    parser.add_argument("folder", type=Path); parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--title"); parser.add_argument("--skip", default="index")
    _password_arguments(parser)


def _chapter_folders(folder):
    return sorted(p for p in folder.iterdir() if p.is_dir() and (p / "eqs.json").exists())


def _part_source(folder, fallback=None):
    pages = next(folder.glob("*.pages.json"), None)
    if pages:
        data = json.loads(pages.read_text(encoding="utf-8"))
        return data.get("extraction_source_pdf") or data.get("source_pdf") or fallback
    return fallback


def _include_ocr_quality(folder):
    pages_path = next(Path(folder).glob("*.pages.json"), None)
    report_path = Path(folder) / "qa_report.json"
    if not pages_path or not report_path.exists():
        return
    quality = json.loads(pages_path.read_text(encoding="utf-8")).get("ocr_quality", {})
    flagged = {page: item for page, item in quality.items() if item.get("warnings")}
    report = json.loads(report_path.read_text(encoding="utf-8"))
    report["ocr_quality"] = flagged
    _atomic(report_path, json.dumps(report, ensure_ascii=False, indent=1))
    md_path = Path(folder) / "qa_report.md"
    if md_path.exists():
        body = md_path.read_text(encoding="utf-8").split("\n## OCR quality\n", 1)[0].rstrip()
        if flagged:
            body += "\n\n## OCR quality\n" + "\n".join(
                f"- PDF page {page}: {', '.join(item['warnings'])}"
                for page, item in sorted(flagged.items(), key=lambda pair: int(pair[0])))
        _atomic(md_path, body + "\n")


def _ocr_quality_warnings(folder):
    pages_path = next(Path(folder).glob("*.pages.json"), None)
    if not pages_path:
        return []
    quality = json.loads(pages_path.read_text(encoding="utf-8")).get("ocr_quality", {})
    return sorted({code for item in quality.values() for code in item.get("warnings", [])})


@command("mdread", 8, "folder", "folder", "", False, "report")
def mdread(args, context):
    if not args.folder.is_dir(): raise PdfError("NOT_FOUND", f"Markdown folder not found: {args.folder}", "check the folder path")
    folders = _chapter_folders(args.folder)
    if not folders: raise PdfError("NOT_FOUND", "no converted parts found", "run pdf md first")
    if getattr(args, "dry_run", False):
        return {"type":"report","command":"mdread","input":str(args.folder),"verdict":"report",
                "data":f"Would read unread or doubtful equations and rerun QA for {len(folders)} parts."}
    temp, previous = _workspace(context)
    stack = contextlib.ExitStack()
    try:
        source_paths, encrypted = _folder_sources(folders, getattr(context, "owner_password", None) or
                                                   getattr(context, "password", None) or "", temp, stack)
        result = _mdread_impl(args, context, folders, source_paths)
        if encrypted:
            result["warnings"] = list(dict.fromkeys(result.get("warnings", []) + ["DECRYPTED_EXPORT"]))
            result["verdict"] = "warn"
        return result
    finally:
        stack.close()
        _clear_workspace(context, temp, previous)


def _mdread_impl(args, context, folders, source_paths):
    from .p8_eqread import run as read
    warnings = []
    try:
        with contextlib.redirect_stdout(sys.stderr):
            read([str(p) for p in folders], source_paths=source_paths, context=context)
    except PdfError as exc:
        if exc.code != "CODEX_UNAVAILABLE": raise
        warnings.append("CODEX_UNAVAILABLE")
    from .p8_qa import run as qa_run
    for path in folders:
        with contextlib.redirect_stdout(sys.stderr): qa_run(str(path), source_pdf=source_paths.get(str(path)) or _part_source(path))
        _include_ocr_quality(path)
        warnings.extend(_ocr_quality_warnings(path))
    warnings = list(dict.fromkeys(warnings))
    return {"type":"result","command":"mdread","path":str(args.folder),"kind":"folder","pages":None,
            "bytes":sum(file.stat().st_size for file in args.folder.rglob("*") if file.is_file()),
            "verdict":"warn" if warnings else "report","warnings":warnings}


@command("mdqa", 8, "folder", "folder", "", False, "report")
def mdqa(args, context):
    if not args.folder.is_dir(): raise PdfError("NOT_FOUND", f"Markdown folder not found: {args.folder}", "check the folder path")
    folders = _chapter_folders(args.folder)
    if not folders: raise PdfError("NOT_FOUND", "no converted parts found", "run pdf md first")
    if getattr(args, "dry_run", False):
        return {"type":"report","command":"mdqa","input":str(args.folder),"verdict":"report",
                "data":f"Would rerun QA for {len(folders)} parts and rewrite their QA reports."}
    temp, previous = _workspace(context)
    stack = contextlib.ExitStack()
    try:
        source_paths, encrypted = _folder_sources(folders, getattr(context, "owner_password", None) or
                                                   getattr(context, "password", None) or "", temp, stack)
        result = _mdqa_impl(args, context, folders, source_paths)
        if encrypted:
            result["warnings"] = list(dict.fromkeys(result.get("warnings", []) + ["DECRYPTED_EXPORT"]))
            result["verdict"] = "warn"
        return result
    finally:
        stack.close()
        _clear_workspace(context, temp, previous)


def _mdqa_impl(args, context, folders, source_paths):
    from .p8_qa import run as qa_run
    for path in folders:
        with contextlib.redirect_stdout(sys.stderr): qa_run(str(path), source_pdf=source_paths.get(str(path)) or _part_source(path))
        _include_ocr_quality(path)
    counts = [_counts(path) for path in folders]
    warnings = []
    if any(item["qa_likely"] for item in counts): warnings.append("QA_LIKELY")
    if any(item["tables_flagged"] for item in counts): warnings.append("TABLE_FLAGGED")
    warnings.extend(code for path in folders for code in _ocr_quality_warnings(path))
    warnings = list(dict.fromkeys(warnings))
    return {"type":"result","command":"mdqa","path":str(args.folder),"kind":"folder","pages":None,
            "bytes":sum(file.stat().st_size for file in args.folder.rglob("*") if file.is_file()),
            "verdict":"warn" if warnings else "ok","warnings":warnings}


@command("mdfix", 8, "folder", "folder", "", False, "report")
def mdfix(args, context):
    from .p8_fix import parse, apply_block, log_notes
    if not args.folder.is_dir(): raise PdfError("NOT_FOUND", f"Markdown folder not found: {args.folder}", "check the folder path")
    if not args.fixes.is_file(): raise PdfError("NOT_FOUND", f"fixes file not found: {args.fixes}", "check the fixes file path")
    block_files = sorted(args.folder.glob("*.blocks.json"))
    if not block_files: block_files = sorted(args.folder.glob("*/*.blocks.json"))
    md_files = [p.with_name(p.name[:-len(".blocks.json")] + ".md") for p in block_files]
    md_files = [p for p in md_files if p.is_file()]
    if not md_files: raise PdfError("NOT_FOUND", "Markdown file not found", "use a converted part folder")
    path = md_files[0]; fixes = parse(args.fixes.read_text(encoding="utf-8")); md = path.read_text(encoding="utf-8")
    applied, checked, failed = [], [], []
    for block in fixes:
        if block["ok"]: checked.append(block); continue
        new, error = apply_block(md, block)
        if error: failed.append((block, error))
        else: md = new; applied.append(block)
    if not args.dry_run:
        _atomic(path, md)
        log_notes(str(args.folder), applied, checked, failed)
    if failed: raise PdfError("FIX_FAILED", f"{len(failed)} fixes did not match", "review review_notes.md", detail="\n".join(e for _,e in failed))
    return {"type":"result","command":"mdfix","path":str(path),"kind":"markdown","pages":None,"bytes":path.stat().st_size if path.exists() else 0,"verdict":"report","warnings":[]}


@command("mdexport", 8, "folder", "folder", "_project", False, "report")
def mdexport(args, context):
    from .p8_export import chapters
    source = args.folder.resolve(); destination = args.output.resolve() if args.output else source.with_name(source.name + "_project")
    if not source.is_dir(): raise PdfError("NOT_FOUND", f"Markdown folder not found: {source}", "check the folder path")
    chapter_rows = chapters(str(source))
    if not chapter_rows: raise PdfError("BAD_INPUT", "no converted parts found", "use a folder containing .blocks.json files and Markdown chapters")
    if destination == source or destination in source.parents or source in destination.parents:
        raise PdfError("BAD_ARG", "export output overlaps the converted input folder", "choose an output folder outside the input tree")
    if destination.exists():
        base = destination; suffix = 2
        while destination.exists():
            destination = base.with_name(f"{base.name}_{suffix}"); suffix += 1
    skip = [word.strip().lower() for word in args.skip.split(",") if word.strip()]
    selected = [row for row in chapter_rows if not any(word in Path(row[0]).stem.lower() for word in skip)]
    if getattr(args, "dry_run", False):
        names = [Path(row[0]).name for row in selected]
        description = f"Would export {len(names)} chapter files to {destination}."
        if names:
            description += "\n" + "\n".join(f"  {name}" for name in names)
        return {"type":"report","command":"mdexport","input":str(source),"verdict":"report","data":description}
    temp, previous = _workspace(context)
    stack = contextlib.ExitStack()
    try:
        folders = list(dict.fromkeys(str(Path(row[0]).parent) for row in selected))
        source_paths, encrypted = _folder_sources(folders, getattr(context, "owner_password", None) or
                                                   getattr(context, "password", None) or "", temp, stack)
        result = _mdexport_impl(args, selected, destination, source_paths, skip)
        if encrypted:
            result["warnings"] = list(dict.fromkeys(result.get("warnings", []) + ["DECRYPTED_EXPORT"]))
            result["verdict"] = "warn"
        return result
    finally:
        stack.close()
        _clear_workspace(context, temp, previous)


def _mdexport_impl(args, selected, destination, source_paths, skip):
    from .p8_export import clean, mark_uncertain_equations, recover_lost_text
    destination.mkdir(parents=True, exist_ok=True)
    rows=[]; total=0
    for md_path, title, first, last, source_pdf in selected:
        folder = str(Path(md_path).parent)
        data = json.loads(Path(md_path.replace(".md", ".pages.json")).read_text(encoding="utf-8"))
        data["source_pdf"] = source_pdf
        from .p8_qa import Pdf
        with Pdf(source_paths.get(folder, data["source_pdf"])) as pdf:
            md = Path(md_path).read_text(encoding="utf-8")
            if not any(word in Path(md_path).stem.lower() for word in skip): md, recovered = recover_lost_text(md, data, pdf)
            else: recovered = 0
            blocks_path = Path(md_path.replace(".md", ".blocks.json"))
            blocks_data = json.loads(blocks_path.read_text(encoding="utf-8"))
            blocks_data["ocr_pages"] = data.get("ocr_pages", [])
            report_path=Path(folder)/"qa_report.json"; marked=0
            if report_path.exists(): md,marked=mark_uncertain_equations(md,blocks_data,json.loads(report_path.read_text(encoding="utf-8")))
        md=clean(md); pages=list(range(first, last + 1))
        extent=f"p. {min(pages)}" if pages and min(pages)==max(pages) else (f"pp. {min(pages)}–{max(pages)}" if pages else "")
        import pymupdf
        with pymupdf.open(source_paths.get(folder, data["source_pdf"])) as source_doc:
            metadata_title = source_doc.metadata.get("title")
        source_title = args.title or metadata_title or Path(data["source_pdf"]).stem
        note=f"> Source: {source_title}, PDF {extent}.\n\n"
        from .p8_export import unique_path
        out=unique_path(destination/Path(md_path).name); _atomic(out,note+md)
        tokens=round(len(note+md)/3.3); total+=tokens; rows.append({"file":out.name,"tokens":tokens,"recovered_words":recovered,"equations_marked":marked})
    return {"type":"result","command":"mdexport","path":str(destination),"kind":"folder","pages":None,"bytes":sum(p.stat().st_size for p in destination.iterdir() if p.is_file()),"verdict":"report","warnings":[],"files":rows,"tokens":total}
