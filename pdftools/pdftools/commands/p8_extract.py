"""Convert a PDF (or a page range) to Markdown without a GPU: text, headings and tables from pymupdf4llm and the PDF's
own text layer (Tesseract on scanned pages), display equations cropped for eqread.py to turn into LaTeX.

Run in the dev env:
    conda run -n dev python C:\\Tools\\pdf2md\\convert.py book.pdf out_dir [--page-range 0-24] [--name ch01] [--no-read]

Writes to out_dir/<name>/:
    <name>.src.md      Markdown with one <!--eq:ID--> line per display equation (pymupdf4llm leaves them out)
    eqs.json           per equation: page, box, crop, and the PDF text inside the box (hint and referee for eqread.py)
    eqs/ID.png         crop of each equation, number included
    <name>.md          the finished Markdown (render(): equations filled in, headings and \\tag numbers tidied)
    <name>.blocks.json block types, text and boxes in the shape qa.py and export.py read
Then runs eqread.py (Codex reads the equations; --no-read to stop after extraction) and qa.py.
"""

import argparse
import hashlib
import html
import json
import math
import os
import re
import sys
import time
from pathlib import Path

from .. import coords
from ..errors import PdfError
from .p8_io import atomic_write

from . import p8_qa as qa
from .p8_post import clean_markdown

CROP_DPI = 200
EQ_LINE = re.compile(r"^<!--eq:(\w+)-->$", re.M)
# pymupdf-layout classes -> the Marker block types qa.py knows
BLOCK_TYPES = {"text": "Text", "title": "SectionHeader", "section-header": "SectionHeader", "list-item": "ListItem",
               "caption": "Caption", "footnote": "Footnote", "table": "Table", "picture": "Picture",
               "page-header": "PageHeader", "page-footer": "PageFooter", "formula": "Equation"}
DROP = {"picture", "page-header", "page-footer"}  # picture labels and running heads are never content
CAPTION_IN_PICTURE = re.compile(r"((?:Figure|Fig\.|Table)\s*\d+(?:\s*[-–.]\s*\d+)?\s*[:.]?\s.*?)(?:<!-- End of picture text -->|$)", re.S)


def parse_range(spec, n_pages):
    if not spec:
        return list(range(n_pages))
    pages = []
    for part in spec.split(","):
        a, _, b = part.partition("-")
        pages += range(int(a), int(b or a) + 1)
    if not pages or min(pages) < 0 or max(pages) >= n_pages:
        raise PdfError("BAD_ARG", f"page range {spec} outside 0-{n_pages - 1}", "use 0-indexed pages, e.g. 0-24")
    return pages


def merge_formulas(boxes):
    """Formula boxes that overlap (also a thin slice of one equation boxed on its own, C-R&B p. 84) become one
    box, so no equation is read twice."""
    out = []
    for b in boxes:
        prev = next((o for o in reversed(out) if o["class"] == "formula"), None) if b["class"] == "formula" else None
        p, q = (prev["bbox"], b["bbox"]) if prev else (None, None)
        if prev and min(p[3], q[3]) > max(p[1], q[1]) and min(p[2], q[2]) > max(p[0], q[0]):
            prev["bbox"] = (min(p[0], q[0]), min(p[1], q[1]), max(p[2], q[2]), max(p[3], q[3]))
            continue
        out.append(dict(b))
    return out


def _calibrate_crop(path, page, box):
    """Keep equation/table crops upright and downsample median ink-line height to about 24 px."""
    from PIL import Image
    import numpy as np
    with Image.open(path) as source:
        image = source.convert("RGB")
    doc_page = page
    x0, y0, x1, y1 = box
    directions = [line.get("dir", (1, 0)) for block in doc_page.get_text("dict")["blocks"]
                  for line in block.get("lines", [])
                  if line.get("bbox") and line["bbox"][0] < x1 and line["bbox"][2] > x0
                  and line["bbox"][1] < y1 and line["bbox"][3] > y0]
    # PDFium renders the crop with page rotation applied. Text directions are in the page's
    # unrotated coordinate system, so undo that orientation and then make the text horizontal.
    rotation = int(doc_page.rotation) % 360
    direction = directions[0] if directions else (1, 0)
    text_angle = round(math.degrees(math.atan2(direction[1], direction[0])))
    correction = (rotation - text_angle) % 360
    if correction:
        image = image.rotate(correction, expand=True)
    ink = np.asarray(image.convert("L")) < 160
    rows = np.flatnonzero(ink.any(axis=1))
    if len(rows):
        bands = np.split(rows, np.flatnonzero(np.diff(rows) > 1) + 1)
        heights = [len(band) for band in bands if len(band)]
        median = float(np.median(heights)) if heights else 0
        if median > 24:
            scale = 24 / median
            image = image.resize((max(1, round(image.width * scale)), max(1, round(image.height * scale))), Image.Resampling.LANCZOS)
        atomic_write(path, writer=lambda temp: image.save(temp, format="PNG"))


def extract(pdf_path, pages):
    import pymupdf
    import pymupdf.layout  # noqa: F401  (switches pymupdf4llm to the layout model, which finds the formula boxes)
    import pymupdf4llm
    with pymupdf.open(pdf_path) as doc:
        rotations = {pi: doc[pi].rotation for pi in pages if doc[pi].rotation}
        for pi in rotations:
            doc[pi].set_rotation(0)
        chunks = pymupdf4llm.to_markdown(doc, pages=pages, page_chunks=True, write_images=False,
                                         show_progress=False, use_ocr=False)
        for pi, rotation in rotations.items():
            page = doc[pi]
            page.set_rotation(rotation)
            for chunk in chunks:
                if chunk["metadata"]["page_number"] - 1 != pi:
                    continue
                for box in chunk.get("page_boxes", []):
                    box["bbox"] = tuple(coords.to_displayed_points(page, box["bbox"]))
        return chunks


def convert(pdf_path, out_dir, page_range=None, name=None, read=True, ocr_pages=()):
    import pymupdf
    pdf = qa.Pdf(pdf_path)
    doc = pymupdf.open(pdf_path)
    try:
        return _convert(pdf_path, out_dir, page_range, name, read, ocr_pages, pdf, doc)
    finally:
        pdf.close()
        doc.close()


def _convert(pdf_path, out_dir, page_range, name, read, ocr_pages, pdf, doc):
    t0 = time.time()
    name = name or Path(pdf_path).stem
    folder = os.path.join(out_dir, name)
    os.makedirs(os.path.join(folder, "eqs"), exist_ok=True)
    chunks = extract(pdf_path, parse_range(page_range, len(pdf.doc)))

    parts, eqs, pages_json = [], [], []
    for chunk in chunks:
        pi = chunk["metadata"]["page_number"] - 1
        page_box = coords.to_displayed_points(doc[pi], coords.from_user_space(doc[pi], coords._pdf_box(doc[pi])))
        text, boxes = chunk["text"], merge_formulas(chunk["page_boxes"])
        page_md, blocks = [], []
        for b in boxes:
            cls, (x0, y0, x1, y1) = b["class"], b["bbox"]
            piece = text[b["pos"][0]:b["pos"][1]].strip()
            if cls == "formula":
                box = pdf.pdfium_box(pi, (x0, y0, x1, y1))
                raw = pdf.text_in_box(pi, box)
                words = [w for w in re.findall(r"[A-Za-z]{3,}", raw) if w not in qa.OPERATOR_NAMES]
                if len(words) >= 5 and "=" not in raw and not qa.eq_labels(raw):  # prose taken for math
                    cls, piece = "text", re.sub(r"\s+", " ", raw).strip()
            if cls == "picture":  # a caption set beside the figure ends up in the picture's text
                m = CAPTION_IN_PICTURE.search(piece)
                if m:
                    cls, piece = "caption", re.sub(r"<br>|\s+", " ", m.group(1)).strip()
            btype = BLOCK_TYPES.get(cls, "Text")
            block = {"id": f"/page/{pi}/{btype}/{b['index']}", "block_type": btype, "bbox": [x0, y0, x1, y1]}
            if cls == "formula":
                eid = f"p{pi + 1:03d}_{b['index']}"
                # Equation numbers sit flush right, sometimes outside the formula box, and on scans the box can miss
                # the start of the equation ("U ="): crop out to the edges of the text column the formula is in
                # (the crop then trims to the ink).
                column = [t["bbox"] for t in boxes if t["class"] == "text" and t["bbox"][0] < x1 and t["bbox"][2] > x0]
                left = min([x0] + [c[0] - 2 for c in column])
                right = max([x1] + [c[2] + 2 for c in column])
                box = pdf.pdfium_box(pi, (left, y0, right, y1))
                crop = f"eqs/{eid}.png"
                pdf.crop(pi, box, CROP_DPI, os.path.join(folder, crop), prose=False)
                _calibrate_crop(os.path.join(folder, crop), doc[pi], (x0, y0, x1, y1))
                raw = pdf.text_in_box(pi, box)
                eqs.append({"id": eid, "page": pi + 1, "block": block["id"], "box": [round(v, 2) for v in box],
                            "crop": crop, "pdf_text": re.sub(r"\s+", " ", raw).strip(),
                            "bold": "".join(pdf.bold_in_box(pi, box)), "labels": sorted(qa.eq_labels(raw)),
                            "crop_sha": hashlib.sha1(Path(folder, crop).read_bytes()).hexdigest(),
                            "readings": [], "latex": None})
                block["eq"] = eid
                page_md.append(f"<!--eq:{eid}-->")
            elif cls not in DROP and piece:
                page_md.append(piece)
                block["text"] = piece
            blocks.append(block)
        parts.append("\n\n".join(page_md))
        pages_json.append({"id": f"/page/{pi}/Page/0", "block_type": "Page",
                           "bbox": [0, 0, page_box.width, page_box.height], "children": blocks})

    atomic_write(os.path.join(folder, f"{name}.src.md"), "\n\n".join(parts) + "\n")
    atomic_write(os.path.join(folder, f"{name}.pages.json"), json.dumps(
        {"source_pdf": os.path.abspath(pdf_path), "pages": pages_json,
         "ocr_pages": sorted(int(page) + 1 for page in ocr_pages)}, ensure_ascii=False))
    old = load_eqs(folder)
    for e in eqs:  # re-extraction keeps readings already paid for
        o = old.get(e["id"])
        if o and (o["pdf_text"], o["box"], o.get("crop_sha")) == (e["pdf_text"], e["box"], e["crop_sha"]):
            e.update({k: v for k, v in o.items() if k not in e or k in ("readings", "latex")})
    save_eqs(folder, eqs)
    render(folder)
    print(f"{name}: {len(chunks)} pages, {len(eqs)} display equations cropped in {time.time() - t0:.0f} s")
    if read and eqs:
        import eqread
        eqread.run([folder])
    return folder


# ------------------------------------------------------------------------------------------ eqs.json and rendering

def load_eqs(folder):
    path = os.path.join(folder, "eqs.json")
    return {e["id"]: e for e in json.loads(Path(path).read_text(encoding="utf-8"))} if os.path.exists(path) else {}


def save_eqs(folder, eqs):
    atomic_write(os.path.join(folder, "eqs.json"), json.dumps(list(eqs), indent=1, ensure_ascii=False))


def fallback(e):
    """An equation nobody has read yet: the PDF's own text for it, marked, so nothing is lost."""
    return f"*(equation not read yet; PDF p. {e['page']} text: {e['pdf_text'] or 'none'})*"


def md_to_html(piece):
    """A block's Markdown as the HTML qa.py compares with the PDF text: emphasis marks dropped, <sup>/<sub>/<br>
    kept as tags, everything else escaped (a "<" in "a < b" must not read as a tag)."""
    t = html.escape(re.sub(r"\*\*|(?<!\w)[*_]|[*_](?!\w)", "", piece), quote=False)
    return re.sub(r"&lt;(/?(?:sup|sub|br)\s*/?)&gt;", r"<\1>", t)


def chapter_md(folder):
    """The rendered <name>.md of a chapter folder (other .md files there: <name>.src.md, qa_report.md, ...)."""
    blocks = next(Path(folder).glob("*.blocks.json"), None)
    return str(blocks)[: -len(".blocks.json")] + ".md" if blocks else None


def render(folder):
    """<name>.md and <name>.blocks.json from <name>.src.md, <name>.pages.json and eqs.json. Run again after every
    change to eqs.json; edits made to <name>.md by hand or fix.py are overwritten, so fix.py comes last."""
    src = next(Path(folder).glob("*.src.md"))
    name = src.name[: -len(".src.md")]
    eqs = load_eqs(folder)

    def fill(m):
        e = eqs.get(m.group(1))
        if not e:
            return ""
        if not e.get("latex"):
            return fallback(e)
        note = f" *(possible OCR error in this equation; see PDF p. {e['page']})*" if e.get("status") == "doubtful" else ""
        return f"$${e['latex']}$$" + note
    md = clean_markdown(EQ_LINE.sub(fill, src.read_text(encoding="utf-8")))
    atomic_write(Path(folder) / f"{name}.md", md)

    data = json.loads(Path(folder, f"{name}.pages.json").read_text(encoding="utf-8"))
    for page in data["pages"]:
        for b in page["children"]:
            e = eqs.get(b.pop("eq", None) or "")
            if e and e.get("latex"):
                b["html"] = f'<p><math display="block">{html.escape(e["latex"])}</math></p>'
            elif e:
                b["html"] = f"<p>{html.escape(e['pdf_text'])}</p>"
            else:
                b["html"] = f"<p>{md_to_html(b.pop('text', ''))}</p>"
    atomic_write(os.path.join(folder, f"{name}.blocks.json"), json.dumps(data, ensure_ascii=False))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("pdf")
    ap.add_argument("out_dir")
    ap.add_argument("--page-range", help='0-indexed pages, e.g. "0-24" or "0,5-10"')
    ap.add_argument("--name", help="output folder and file name (default: PDF name)")
    ap.add_argument("--no-read", action="store_true", help="extract and crop only; don't run eqread.py and qa.py")
    a = ap.parse_args()
    if not os.path.isfile(a.pdf):
        raise PdfError("NOT_FOUND", f"{a.pdf} not found", "check the PDF path")
    folder = convert(a.pdf, a.out_dir, a.page_range, a.name, read=not a.no_read)
    if not a.no_read:
        qa.run(folder)


# Library module, called by p8_md.py.
