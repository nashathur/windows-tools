"""Export converted Markdown as lean source files for a Claude project (the only use of these files).

Run in the dev env:
    conda run -n dev python C:\\Tools\\pdf2md\\export.py <book or document folder> <export folder>
        [--title "Cushman-Roisin & Beckers, Introduction to GFD"] [--skip index,references]

For each chapter (or the single document): drops image links (Claude can't see them; captions stay), HTML
leftovers (<sup>2</sup> -> ^2, <span>, <br>, <b>/<i>), ORCID links, review comments, extra blank lines; adds a one-line
source note with the PDF page range (so answers can point to the page). The Index is skipped by default (useless
to Claude); add references/front matter to --skip if the project doesn't need them. Prints an estimated token
count per file: upload only the chapters a project needs.
"""

import argparse
import glob
import json
import os
import re
import sys
from pathlib import Path

from ..errors import PdfError
from .p8_io import atomic_write
from .p8_qa import _fuzz

from . import p8_qa as qa
from .p8_fix import pattern
from .p8_post import tag_label

from ..errors import PdfError

CHARS_PER_TOKEN = 3.3  # rough: English prose ~3.5-4, LaTeX-heavy text ~3
MATH = re.compile(r"\$\$(.+?)\$\$|\$([^$\n]+)\$", re.S)
WRONG_EQUATION = re.compile(r"^(symbols|accents differ|equation number|KaTeX|equation block without LaTeX)")


def words_view(md):
    """The Markdown as words, math included the way a PDF text layer spells it (\\text{...} words, "Tpeak")."""
    def words(m):
        latex = m.group(1) or m.group(2)
        s = re.sub(r"[_{}]", "", re.sub(r"\\[A-Za-z]+", " ", qa.LATEX_TEXT_ARGS.sub(r"\1", latex)))
        return " " + " ".join(qa.LATEX_TEXT_ARGS.findall(latex)) + " " + " ".join(re.findall(r"[A-Za-z]+", s)) + " "
    return MATH.sub(words, md)


def skip_boxes(page, pdf, pi):
    out = []
    for b in qa.leaves(page):
        if b["block_type"] in qa.SKIP_FOR_LOST_TEXT:
            out.append(pdf.pdfium_box(pi, b["bbox"]))
    return out


def recover_lost_text(md, data, pdf):
    """Put PDF text that is missing from the Markdown back, from the PDF text layer (math then as Unicode), next to
    the paragraph it belongs to (found from the words just before or after it), marked with its page. Text whose
    place can't be found goes to a section at the end. Returns (md, words recovered)."""
    view = words_view(md)
    md_letters = re.sub(r"[^a-z]", "", re.sub(r"\$[^$]*\$|\\[A-Za-z]+", " ", md.lower()))
    md_tokens = qa.prose_words(md)  # positions in md
    joined, starts = "", []
    for w, s, e in md_tokens:
        starts.append((len(joined), e, s))
        joined += w
    inserts, leftovers, n_words = [], [], 0
    ocr_pages = set(data.get("ocr_pages", []))
    for page in data["pages"]:
        pi = qa.page_index(page["id"])
        if not pdf.has_text_layer(pi) or pi + 1 in ocr_pages:
            continue
        full = pdf.textpage(pi).get_text_range()
        toks, runs = qa.lost_runs(pdf, pi, view, skip_boxes(page, pdf, pi))
        for a, b in runs:
            raw = full[toks[a][1]:toks[b][2]]
            raw = re.sub(r"\s+", " ", re.sub(r"-[ \t]*\r?\n[ \t]*", "", raw)).strip()
            raw = re.sub(r"\bJMB (from|to)\s*\S?\s*", "", raw)  # C-R&B draft margin notes
            # Already there in another form (letters compared, math ignored)? Then it isn't lost.
            if _fuzz().partial_ratio(re.sub(r"[^a-z]", "", raw.lower()), md_letters) >= 85:
                continue
            text = f"{raw} *(recovered from PDF p. {pi + 1})*"
            n_words += b - a + 1
            where = None
            before = "".join(t[0] for t in toks[max(0, a - 6):a])
            after = "".join(t[0] for t in toks[b + 1:b + 7])
            if len(before) >= 15 and joined.count(before) == 1:
                k = joined.index(before) + len(before) - 1
                end_char = next(e for j, e, s in reversed(starts) if j <= k)
                para_end = md.find("\n\n", end_char)
                where = (len(md) if para_end == -1 else para_end, "\n\n" + text)
            elif len(after) >= 15 and joined.count(after) == 1:
                k = joined.index(after)
                start_char = next(s for j, e, s in reversed(starts) if j <= k)
                para_start = md.rfind("\n\n", 0, start_char)
                where = (0 if para_start == -1 else para_start, "\n\n" + text)
            if where:
                inserts.append(where)
            else:
                leftovers.append(text)
    for pos, text in sorted(inserts, key=lambda x: -x[0]):
        md = md[:pos] + text + md[pos:]
    if leftovers:
        md = md.rstrip() + "\n\n## Text recovered from the PDF (place not identified)\n\n" + "\n\n".join(leftovers) + "\n"
    return md, n_words


def mark_uncertain_equations(md, data, report):
    """After each display equation that QA still considers likely wrong, a short note for the reader (Claude)."""
    html = {}
    for page in data["pages"]:
        for b in qa.leaves(page):
            if b["block_type"] == "Equation":
                html[b["id"]] = b.get("html", "")
    marked = 0
    for issue in report["issues"]:
        if issue["type"] != "Equation" or issue.get("visual") or issue["confidence"] != "high" \
                or not WRONG_EQUATION.match(issue["reason"]) or issue["block"] not in html:
            continue
        latex = " ".join(m.strip() for m in re.findall(r"<math[^>]*>(.*?)</math>", html[issue["block"]] or "", re.S))
        latex = qa.html.unescape(latex)
        note = f" *(possible OCR error in this equation; see PDF p. {issue['page']})*"
        for form in dict.fromkeys([latex, tag_label(latex)]):
            matches = list(pattern(form).finditer(md)) if form.strip() else []
            if len(matches) == 1:
                end = md.find("$$", matches[0].end())
                if end != -1 and note not in md[end:end + len(note) + 5]:
                    md = md[:end + 2] + note + md[end + 2:]
                    marked += 1
                break
    return md, marked


def clean(md):
    md = re.sub(r"^[ \t]*!\[[^\]]*\]\([^)]*\)[ \t]*$\n?", "", md, flags=re.M)  # image-only lines
    md = re.sub(r"!\[[^\]]*\]\([^)]*\)", "", md)
    md = re.sub(r"\[([^\]]*)\]\(https?://orcid\.org/[^)]*\)", r"\1", md)
    md = re.sub(r"<!--.*?-->", "", md, flags=re.S)
    md = re.sub(r"<sup>\s*([^<]*?)\s*</sup>", lambda m: "^" + (m.group(1) if len(m.group(1)) <= 1 else f"({m.group(1)})"), md)
    md = re.sub(r"<sub>\s*([^<]*?)\s*</sub>", lambda m: "_" + (m.group(1) if len(m.group(1)) <= 1 else f"({m.group(1)})"), md)
    md = re.sub(r"</?span[^>]*>", "", md)
    md = re.sub(r"<br\s*/?>", " ", md)
    md = re.sub(r"</?b>", "**", md)
    md = re.sub(r"</?i>", "*", md)
    md = re.sub(r"[ \t]+$", "", md, flags=re.M)
    return re.sub(r"\n{3,}", "\n\n", md).strip() + "\n"


def chapters(folder):
    """[(md path, title, first page, last page)] from a book folder (00_index.md) or a single document folder."""
    out = []
    # a document folder from convert.py has subfolders too (crops/), so check the folder itself first
    single = glob.glob(os.path.join(folder, "*.blocks.json"))
    for sub in [folder] if single else sorted(f for f in glob.glob(os.path.join(folder, "*")) if os.path.isdir(f)):
        blocks = glob.glob(os.path.join(sub, "*.blocks.json"))
        # <name>.md beside <name>.blocks.json (the folder also holds <name>.src.md, qa_report.md, ...)
        mds = [blocks[0][: -len(".blocks.json")] + ".md"] if blocks else []
        if not mds or not os.path.isfile(mds[0]):
            continue
        with open(blocks[0], encoding="utf-8") as stream:
            data = json.load(stream)
        pages = [int(p["id"].split("/")[2]) + 1 for p in data["pages"]]
        name = os.path.basename(mds[0])[:-3]
        title = re.sub(r"^\d+_", "", name).replace("_", " ")
        out.append((mds[0], title, min(pages), max(pages), data["source_pdf"]))
    return out


def unique_path(path):
    """Choose path, then _2, _3, ... without replacing an existing export."""
    path = Path(path)
    if not path.exists():
        return path
    index = 2
    while True:
        candidate = path.with_name(f"{path.stem}_{index}{path.suffix}")
        if not candidate.exists():
            return candidate
        index += 1


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("folder", help="book folder from book.py, or a document folder from convert.py")
    ap.add_argument("out")
    ap.add_argument("--title", help="source name for the note at the top of each file (default: PDF title or file name)")
    ap.add_argument("--skip", default="index", help="comma-separated words; chapters whose name contains one are skipped")
    args = ap.parse_args()
    if not os.path.isdir(args.folder):
        raise PdfError("NOT_FOUND", f"folder not found: {args.folder}", "check the converted book or document folder")
    chapter_rows = chapters(args.folder)
    if not chapter_rows:
        raise PdfError("BAD_INPUT", f"no converted chapters found in {args.folder}",
                          "use a folder containing .blocks.json files and Markdown chapters")
    skip = [s.strip().lower() for s in args.skip.split(",") if s.strip()]
    destination = Path(args.out).resolve()
    source_folder = Path(args.folder).resolve()
    if destination == source_folder or destination in source_folder.parents or source_folder in destination.parents:
        raise PdfError("BAD_ARG", "export output overlaps the converted input folder", "choose an output folder outside the input tree")
    os.makedirs(destination, exist_ok=True)

    rows, total = [], 0
    for md_path, title, first, last, pdf in chapter_rows:
        if any(s in title.lower() for s in skip):
            print(f"  skipped: {title}")
            continue
        import pymupdf
        with pymupdf.open(pdf) as source_doc:
            meta = source_doc.metadata
        source = args.title or meta.get("title") or os.path.splitext(os.path.basename(pdf))[0]
        text = Path(md_path).read_text(encoding="utf-8")
        folder = os.path.dirname(md_path)
        blocks_data = json.loads(Path(glob.glob(os.path.join(folder, "*.blocks.json"))[0]).read_text(encoding="utf-8"))
        pages_data = json.loads(Path(glob.glob(os.path.join(folder, "*.pages.json"))[0]).read_text(encoding="utf-8"))
        # Block content drives uncertainty marking; pages.json carries OCR provenance used for recovery.
        blocks_data["ocr_pages"] = pages_data.get("ocr_pages", [])
        report_path = os.path.join(folder, "qa_report.json")
        recovered = marked = 0
        if "index" not in title.lower():
            with qa.Pdf(pdf) as source_pdf:
                text, recovered = recover_lost_text(text, data, source_pdf)
        if os.path.exists(report_path):
            report = json.loads(Path(report_path).read_text(encoding="utf-8"))
            text, marked = mark_uncertain_equations(text, blocks_data, report)
        text = clean(text)
        pages = f"p. {first}" if first == last else f"pp. {first}–{last}"
        note = f"> Source: {source}, PDF {pages}. Converted from the PDF locally; a few equations may contain OCR errors.\n\n"
        out_path = unique_path(destination / os.path.basename(md_path))
        atomic_write(out_path, note + text)
        before = os.path.getsize(md_path)
        tokens = round((len(note) + len(text)) / CHARS_PER_TOKEN)
        total += tokens
        rows.append((os.path.basename(md_path), tokens, recovered, marked))
    print(f"\n{'file':55} {'~tokens':>8} {'recovered words':>16} {'equations marked':>17}")
    for name, tokens, recovered, marked in rows:
        print(f"{name[:55]:55} {tokens:8,} {recovered:16} {marked:17}")
    print(f"{'total':60} {total:8,}   (estimate: characters / {CHARS_PER_TOKEN})\nWritten to {args.out}")


# Command wrapper registered in p8_md.py.
