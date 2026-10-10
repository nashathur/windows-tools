"""Export converted Markdown as lean source files for a Claude project (the only use of these files).

Run in the pdfmath env:
    conda run -n pdfmath python C:\\Users\\thurs\\tools\\pdf2md\\export.py <book or document folder> <export folder>
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

import pypdfium2 as pdfium
from rapidfuzz import fuzz

import qa
from fix import pattern
from postprocess import tag_label

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
    W, H = pdf.size(pi)
    sx, sy = W / page["bbox"][2], H / page["bbox"][3]
    out = []
    for b in qa.leaves(page):
        if b["block_type"] in qa.SKIP_FOR_LOST_TEXT:
            x0, y0, x1, y1 = b["bbox"]
            out.append((x0 * sx, H - y0 * sy, x1 * sx, H - y1 * sy))
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
    for page in data["pages"]:
        pi = qa.page_index(page["id"])
        if not pdf.has_text_layer(pi):
            continue
        full = pdf.textpage(pi).get_text_range()
        toks, runs = qa.lost_runs(pdf, pi, view, skip_boxes(page, pdf, pi))
        for a, b in runs:
            raw = full[toks[a][1]:toks[b][2]]
            raw = re.sub(r"\s+", " ", re.sub(r"-[ \t]*\r?\n[ \t]*", "", raw)).strip()
            raw = re.sub(r"\bJMB (from|to)\s*\S?\s*", "", raw)  # C-R&B draft margin notes
            # Already there in another form (letters compared, math ignored)? Then it isn't lost.
            if fuzz.partial_ratio(re.sub(r"[^a-z]", "", raw.lower()), md_letters) >= 85:
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
                html[b["id"]] = b["html"]
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
    for sub in sorted(f for f in glob.glob(os.path.join(folder, "*")) if os.path.isdir(f)) or [folder]:
        mds = [p for p in glob.glob(os.path.join(sub, "*.md")) if os.path.basename(p) not in ("qa_report.md", "review_notes.md")]
        blocks = glob.glob(os.path.join(sub, "*.blocks.json"))
        if not mds or not blocks:
            continue
        data = json.load(open(blocks[0], encoding="utf-8"))
        pages = [int(p["id"].split("/")[2]) + 1 for p in data["pages"]]
        name = os.path.basename(mds[0])[:-3]
        title = re.sub(r"^\d+_", "", name).replace("_", " ")
        out.append((mds[0], title, min(pages), max(pages), data["source_pdf"]))
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("folder", help="book folder from book.py, or a document folder from convert.py")
    ap.add_argument("out")
    ap.add_argument("--title", help="source name for the note at the top of each file (default: PDF title or file name)")
    ap.add_argument("--skip", default="index", help="comma-separated words; chapters whose name contains one are skipped")
    args = ap.parse_args()
    skip = [s.strip().lower() for s in args.skip.split(",") if s.strip()]
    os.makedirs(args.out, exist_ok=True)

    rows, total = [], 0
    for md_path, title, first, last, pdf in chapters(args.folder):
        if any(s in title.lower() for s in skip):
            print(f"  skipped: {title}")
            continue
        source = args.title
        if not source:
            meta = pdfium.PdfDocument(pdf).get_metadata_dict()
            source = meta.get("Title") or os.path.splitext(os.path.basename(pdf))[0]
        text = open(md_path, encoding="utf-8").read()
        folder = os.path.dirname(md_path)
        data = json.load(open(glob.glob(os.path.join(folder, "*.blocks.json"))[0], encoding="utf-8"))
        report_path = os.path.join(folder, "qa_report.json")
        recovered = marked = 0
        if "index" not in title.lower():
            text, recovered = recover_lost_text(text, data, qa.Pdf(pdf))
        if os.path.exists(report_path):
            text, marked = mark_uncertain_equations(text, data, json.load(open(report_path, encoding="utf-8")))
        text = clean(text)
        pages = f"p. {first}" if first == last else f"pp. {first}–{last}"
        note = f"> Source: {source}, PDF {pages}. Converted from the PDF locally; a few equations may contain OCR errors.\n\n"
        out_path = os.path.join(args.out, os.path.basename(md_path))
        with open(out_path, "w", encoding="utf-8") as f:
            f.write(note + text)
        before = os.path.getsize(md_path)
        tokens = round((len(note) + len(text)) / CHARS_PER_TOKEN)
        total += tokens
        rows.append((os.path.basename(md_path), tokens, recovered, marked))
    print(f"\n{'file':55} {'~tokens':>8} {'recovered words':>16} {'equations marked':>17}")
    for name, tokens, recovered, marked in rows:
        print(f"{name[:55]:55} {tokens:8,} {recovered:16} {marked:17}")
    print(f"{'total':60} {total:8,}   (estimate: characters / {CHARS_PER_TOKEN})\nWritten to {args.out}")


if __name__ == "__main__":
    main()
