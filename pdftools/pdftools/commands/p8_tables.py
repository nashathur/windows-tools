"""Read only tables whose extracted structure or OCR confidence needs a second look."""
from __future__ import annotations

import html
import hashlib
import json
import os
import re
import shutil
import unicodedata
from pathlib import Path

from . import p8_qa
from .p8_io import atomic_write

_PENDING = {}


def _plain(value):
    return re.sub(r"\s+", " ", html.unescape(re.sub(r"<[^>]+>", " ", value))).strip()


def _words(value):
    text = unicodedata.normalize("NFKC", _plain(value)).casefold().replace("−", "-").replace("–", "-")
    tokens = re.findall(r"(?<![\w])[+-]?\d+(?:[.,]\d+)*|[^\W\d_]+(?:[-'’][^\W\d_]+)*", text, re.UNICODE)
    return [token.replace("’", "'") for token in tokens]


def _coverage(source, target):
    from ..wordalign import align
    if not source:
        return 0.0
    matched, _ = align(source, target)
    return len(matched) / len(source)


def _agreement(first, second):
    return min(_coverage(first, second), _coverage(second, first))


def _confidence(image, temp, context=None):
    from .. import tools
    executable = shutil.which("tesseract")
    if not executable:
        return None
    def tsv_values(path, cell=False):
        args = [executable, str(path), "stdout"]
        if cell:
            args += ["--psm", "6"]
        args.append("tsv")
        result = tools.run_tool(args, timeout=120,
                                env=tools.dev_tool_env(temp), temp=temp,
                                secret_env=getattr(context, "secret_env", ()),
                                secret_values=context.secret_store.values if context and context.secret_store else ())
        values = []
        for line in result.stdout.splitlines()[1:]:
            columns = line.split("\t")
            if len(columns) >= 12 and columns[-1].strip() and columns[10].strip() != "-1":
                try:
                    values.append(float(columns[10]))
                except ValueError:
                    pass
        return values

    values = tsv_values(image)
    # Tesseract often treats a complete ruled grid as one connected component. Read its individual cells as well,
    # keeping the original pixels and collecting the same TSV word-confidence field from each cell crop.
    try:
        import cv2
        import numpy as np
        from PIL import Image
        with Image.open(image) as source:
            gray = np.asarray(source.convert("L"))
        _, binary = cv2.threshold(gray, 0, 255, cv2.THRESH_BINARY_INV | cv2.THRESH_OTSU)
        horizontal = cv2.morphologyEx(binary, cv2.MORPH_OPEN,
                                      cv2.getStructuringElement(cv2.MORPH_RECT, (max(8, binary.shape[1] // 12), 1)))
        vertical = cv2.morphologyEx(binary, cv2.MORPH_OPEN,
                                    cv2.getStructuringElement(cv2.MORPH_RECT, (1, max(8, binary.shape[0] // 12))))
        group = lambda xs: [int(np.mean(g)) for g in np.split(xs, np.flatnonzero(np.diff(xs) > 2) + 1) if len(g)]
        row_lines = group(np.flatnonzero(horizontal.any(axis=1)))
        col_lines = group(np.flatnonzero(vertical.any(axis=0)))
        if len(row_lines) >= 2 and len(col_lines) >= 2:
            cell_values, cell_number = [], 0
            for ya, yb in zip(row_lines, row_lines[1:]):
                for xa, xb in zip(col_lines, col_lines[1:]):
                    pad_x, pad_y = max(2, (xb - xa) // 20), max(2, (yb - ya) // 20)
                    cell = Image.fromarray(gray[max(0, ya + pad_y):max(ya + pad_y + 1, yb - pad_y),
                                                max(0, xa + pad_x):max(xa + pad_x + 1, xb - pad_x)])
                    cell_path = Path(temp) / f"tess-cell-{cell_number}.png"
                    cell_number += 1
                    cell.save(cell_path)
                    cell_values.extend(tsv_values(cell_path, cell=True))
            if cell_values:
                values = cell_values
    except Exception:
        pass
    return sum(values) / len(values) if values else None


def _ask(folder, image, prompt, role="quick", effort="medium", context=None):
    from .p8_codex import ask
    prompt_path = Path(folder) / "tables_read.md"
    atomic_write(prompt_path, prompt)
    answer = ask(prompt_path, folder, role, effort, [str(image)], context=context)
    match = re.search(r"(?m)^T\d+\s*\t\s*(.+?)\s*$", answer.strip())
    return _rows(match.group(1)) if match else ""


def _rows(value):
    """Convert the pass-1 one-line row separator into the normal Markdown table shape."""
    return "\n".join(part.strip() for part in re.split(r"\s*\|\|\s*", value.strip()) if part.strip())


def prepare(folder, source_pdf=None, context=None):
    """Crop flagged tables and measure OCR confidence before the shared pass-1 run."""
    folder = str(folder)
    blocks_path = next(Path(folder).glob("*.blocks.json"), None)
    pages_path = next(Path(folder).glob("*.pages.json"), None)
    if not blocks_path or not pages_path:
        _PENDING[folder] = []
        return []
    blocks_data = json.loads(blocks_path.read_text(encoding="utf-8"))
    page_data = json.loads(pages_path.read_text(encoding="utf-8"))
    pdf_path = source_pdf or page_data["source_pdf"]
    pdf = p8_qa.Pdf(pdf_path)
    scanned_pages = set(page_data.get("ocr_pages", []))
    tables, index = [], 0
    crop_dir = Path(folder) / "crops"
    crop_dir.mkdir(exist_ok=True)
    try:
        for page, source_page in zip(blocks_data["pages"], page_data["pages"]):
            pi = p8_qa.page_index(page["id"])
            page_blocks = {block.get("id"): block for block in source_page.get("children", [])}
            for block in page.get("children", []):
                if block.get("block_type") != "Table":
                    continue
                index += 1
                source = page_blocks.get(block.get("id"), block)
                box = pdf.pdfium_box(pi, block["bbox"])
                scanned = pi + 1 in scanned_pages
                shape = p8_qa._source_table_shape(pdf, pi, box, scanned)
                current = source.get("text", _plain(block.get("html", "")))
                current_shape = p8_qa._markdown_table_shape(current)
                image = crop_dir / f"table-{index}.png"
                pdf.crop(pi, box, 200, str(image), prose=False)
                from .p8_extract import _calibrate_crop
                _calibrate_crop(str(image), pdf.mupdf[pi], block["bbox"])
                confidence = _confidence(image, str(crop_dir), context=context) if scanned else 100.0
                if shape is None or (shape is not None and current_shape != shape) or (scanned and (confidence is None or confidence < 85)):
                    tables.append({"id": index, "label": index, "page": pi + 1, "box": box,
                                   "shape": shape, "current": current, "scanned": scanned,
                                   "image": str(image), "source_id": source.get("id"),
                                   "block_id": block.get("id"),
                                   "blocks_path": blocks_path, "pages_path": pages_path, "pdf_path": pdf_path})
    finally:
        pdf.close()
    _PENDING[folder] = tables
    return tables


def pending(folder, source_pdf=None, context=None):
    folder = str(folder)
    if source_pdf is not None:
        return prepare(folder, source_pdf, context=context)
    return _PENDING.get(folder) if folder in _PENDING else prepare(folder, context=context)


def apply_pass1(folder, tables, readings, context=None):
    """Accept shared pass-1 readings and request a second independent OCR-table reading as needed."""
    if not tables:
        return 0
    folder = str(folder)
    blocks_path, pages_path = tables[0]["blocks_path"], tables[0]["pages_path"]
    blocks_data = json.loads(blocks_path.read_text(encoding="utf-8"))
    page_data = json.loads(pages_path.read_text(encoding="utf-8"))
    md_path = blocks_path.with_name(blocks_path.name[:-len(".blocks.json")] + ".md")
    markdown = md_path.read_text(encoding="utf-8")
    src_path = blocks_path.with_name(blocks_path.name[:-len(".blocks.json")] + ".src.md")
    source_markdown = src_path.read_text(encoding="utf-8") if src_path.exists() else None
    readings_path = Path(folder) / "table_readings.json"
    if readings_path.is_file():
        table_readings = json.loads(readings_path.read_text(encoding="utf-8"))
    else:
        table_readings = {}
    pdf = p8_qa.Pdf(tables[0].get("pdf_path") or page_data["source_pdf"])
    changed = 0
    try:
        for table in tables:
            reading_key = table.get("block_id") or str(table["id"])
            saved = table_readings.setdefault(reading_key, {})
            crop_sha = hashlib.sha256(Path(table["image"]).read_bytes()).hexdigest()
            if saved.get("crop_sha") != crop_sha:
                saved.clear()
                saved["crop_sha"] = crop_sha
            first = _rows(readings.get(table["id"], "")) or saved.get("pass1", "")
            readings_for_table = [first]
            if first:
                saved["pass1"] = first
                atomic_write(readings_path, json.dumps(table_readings, ensure_ascii=False))
            if table["scanned"]:
                shape = table["shape"]
                prompt = ("Independently transcribe the table in the attached crop. Return a Markdown table on one line, "
                          "with rows separated by ` || `.\n"
                          f"PDF table shape: {shape[0]} rows x {shape[1]} columns.\nT{table['label']}\t") if shape else (
                          "Independently transcribe the table in the attached crop. Return a Markdown table on one line, "
                          "with rows separated by ` || `.\nT{table['label']}\t")
                second = saved.get("pass2") or _ask(folder, table["image"], prompt, "review", "high", context=context)
                readings_for_table.append(second)
                if second:
                    saved["pass2"] = second
                    atomic_write(readings_path, json.dumps(table_readings, ensure_ascii=False))
            shape = table["shape"]
            pi = table["page"] - 1
            source_words = _words(pdf.text_in_box(pi, table["box"]))
            accepted = None
            if table["scanned"]:
                first_shape = p8_qa._markdown_table_shape(readings_for_table[0])
                second_shape = p8_qa._markdown_table_shape(readings_for_table[1]) if len(readings_for_table) == 2 else None
                agree = (first_shape is not None and first_shape == second_shape
                         and (shape is None or first_shape == shape)
                         and _agreement(_words(readings_for_table[0]), _words(readings_for_table[1])) >= 0.95)
                if agree:
                    accepted = readings_for_table[0]
            else:
                for reading in readings_for_table:
                    if shape is not None and p8_qa._markdown_table_shape(reading) == shape:
                        if _coverage(source_words, _words(reading)) >= 0.98:
                            accepted = reading
            source = next((b for page in page_data["pages"] for b in page.get("children", [])
                           if b.get("id") == table["source_id"]), None)
            block = next((b for page in blocks_data["pages"] for b in page.get("children", [])
                          if b.get("id") == table["block_id"]), None)
            if source is None or block is None:
                continue
            if accepted:
                block.pop("table_garbled", None)
                block.pop("table_garbled_page", None)
                current = table["current"]
                if current:
                    markdown = markdown.replace(current, accepted, 1)
                    if source_markdown is not None:
                        source_markdown = source_markdown.replace(current, accepted, 1)
                source["text"] = accepted
                block["html"] = html.escape(accepted).replace("\n", "<br>")
            else:
                block["table_garbled"] = True
                block["table_garbled_page"] = pi + 1
                note = f"*(table may be garbled; see PDF p. {pi + 1})*"
                current = table["current"]
                if current and current in markdown:
                    markdown = markdown.replace(current, current + "\n\n" + note, 1)
                if current and source_markdown is not None and current in source_markdown:
                    source_markdown = source_markdown.replace(current, current + "\n\n" + note, 1)
            changed += 1
            saved["accepted"] = bool(accepted)
            saved["result"] = accepted or ""
            atomic_write(readings_path, json.dumps(table_readings, ensure_ascii=False))
            # Persist after each table so an interruption during a later reading cannot lose completed decisions.
            atomic_write(blocks_path, json.dumps(blocks_data, ensure_ascii=False))
            atomic_write(pages_path, json.dumps(page_data, ensure_ascii=False))
            atomic_write(md_path, markdown)
            if source_markdown is not None:
                atomic_write(src_path, source_markdown)
    finally:
        pdf.close()
    if changed:
        atomic_write(blocks_path, json.dumps(blocks_data, ensure_ascii=False))
        atomic_write(pages_path, json.dumps(page_data, ensure_ascii=False))
        atomic_write(md_path, markdown)
        if source_markdown is not None:
            atomic_write(src_path, source_markdown)
    _PENDING.pop(folder, None)
    return changed


def _read(folder):
    prepare(folder)
    return 0


def read(folders):
    return {folder: _read(folder) for folder in folders}
