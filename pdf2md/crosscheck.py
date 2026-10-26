"""Second opinion from MinerU on the display equations QA flags, so fewer of them need a person (or Claude).

MinerU (separate `mineru` conda env, run as a subprocess) reads only the PDF pages that have flagged equations.
Each flagged equation is compared with MinerU's equation at the same place on the page:
  - MinerU reads the same symbols and accents      -> flag demoted to "lower confidence" (two OCR engines agree;
                                                      the PDF text layer is probably the odd one out)
  - MinerU differs and matches the PDF text exactly -> its reading (with Marker's equation number) is written to
                                                      qa_mineru_fixes.txt, in fix.py format
  - otherwise                                       -> MinerU's reading is shown next to the flag in qa_report.md
Equation numbers always come from Marker (MinerU is weaker at numbering).

MinerU's readings are cached in <chapter>/mineru.json, keyed by PDF page, so re-running QA (or reverify.py, which
only changes Marker's output) doesn't run MinerU again. MinerU needs the GPU that Marker uses: in a process that
loaded Marker, call release_gpu() first (book.py and reverify.py do this after their last chapter).

Run in the pdfmath env, on chapters QA has already checked:
    conda run -n pdfmath python C:\\Tools\\pdf2md\\crosscheck.py out_dir\\<chapter> [...]
    conda run -n pdfmath python C:\\Tools\\pdf2md\\crosscheck.py out_dir --all
"""

import argparse
import gc
import glob
import html
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import zipfile

import qa

MINERU_ENV = os.environ.get("PDF2MD_MINERU_ENV", "mineru")
CACHE = "mineru.json"
CHECKED_REASON = re.compile(r"^(symbols|accents differ|bold differs|KaTeX|no usable PDF text|box doesn't line up)")
TAG = re.compile(r"\\tag\*?\s*\{[^{}]*\}")
# MinerU sometimes writes a printed equation number as LaTeX: "(2. 1 \mathrm{a})"
MINERU_NUMBER = re.compile(r"\(\s*\d+\s*\.\s*\d+(?:\s*\d)*\s*(?:\\mathrm\s*\{\s*[a-z]\s*\}|[a-z])?\s*\)")


def mineru_clean(latex):
    return MINERU_NUMBER.sub(" ", TAG.sub(" ", latex)).strip()


def conda_exe():
    return (os.environ.get("CONDA_EXE") or shutil.which("conda")
            or os.path.expanduser(r"~\miniconda3\Scripts\conda.exe"))


def checked(issue):
    return issue["type"] == "Equation" and CHECKED_REASON.match(issue["reason"])


def release_gpu():
    """Free the GPU memory Marker holds in this process: its PyTorch models (once nothing refers to them) and the
    llama-server it started. Marker starts a new llama-server on its next run."""
    gc.collect()
    torch = sys.modules.get("torch")
    if torch is not None and torch.cuda.is_available():
        torch.cuda.empty_cache()
    spawn = sys.modules.get("surya.inference.backends.spawn")
    if spawn is None:
        return
    import psutil
    sentinel = spawn._read_sentinel("llamacpp")
    pid = sentinel and sentinel.get("pid")
    try:
        if pid and psutil.Process(pid).ppid() == os.getpid():  # only a server this process started
            spawn._stop_process(pid, "llamacpp")
            spawn._delete_sentinel("llamacpp")
    except psutil.NoSuchProcess:
        pass


# ------------------------------------------------------------------------------------------------ running MinerU

def run_mineru(pdf_path, pages, log_path):
    """{page index: [{"bbox": [x0, y0, x1, y1] as page fractions from the top left, "latex": ...}]} for the pages."""
    out = tempfile.mkdtemp(prefix="mineru_")
    cmd = [conda_exe(), "run", "--no-capture-output", "-n", MINERU_ENV, "mineru-kit", "parse", pdf_path, "-o", out,
           "-p", ",".join(str(p + 1) for p in sorted(pages)), "-f", "zip", "--tier", "standard"]
    t0 = time.time()
    try:
        with open(log_path, "w", encoding="utf-8", errors="replace") as log:
            subprocess.run(cmd, stdout=log, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
        zips = glob.glob(os.path.join(out, "*.zip"))
        if not zips:
            print(f"  MinerU produced no output (see {log_path}); flags left as they are")
            return None
        doc = json.loads(zipfile.ZipFile(zips[0]).read("middle_json.json"))
    finally:
        shutil.rmtree(out, ignore_errors=True)
    found = {p: [] for p in pages}
    for page in doc["pages"]:
        if page["page_idx"] in found:
            found[page["page_idx"]] = [{"bbox": b["bbox"], "latex": b["content"]} for b in page["blocks"]
                                       if b["type"] == "equation" and isinstance(b.get("content"), str)]
    print(f"  MinerU read {len(pages)} pages in {time.time() - t0:.0f} s")
    return found


def load_cache(folder, source_pdf):
    path = os.path.join(folder, CACHE)
    if os.path.exists(path):
        cache = json.load(open(path, encoding="utf-8"))
        if cache.get("source_pdf") == source_pdf:
            return {int(k): v for k, v in cache["pages"].items()}
    return {}


def save_cache(folder, source_pdf, pages):
    with open(os.path.join(folder, CACHE), "w", encoding="utf-8") as f:
        json.dump({"source_pdf": source_pdf, "pages": {str(k): v for k, v in sorted(pages.items())}}, f,
                  ensure_ascii=False, indent=1)


def fetch(jobs):
    """jobs: [(folder, source_pdf, needed page indices)]. Runs MinerU once per PDF for all uncached pages and
    stores each chapter's pages in its cache."""
    by_pdf = {}
    for folder, source_pdf, pages in jobs:
        missing = set(pages) - set(load_cache(folder, source_pdf))
        if missing:
            by_pdf.setdefault(source_pdf, []).append((folder, missing))
    for source_pdf, parts in by_pdf.items():
        pages = set().union(*(m for _, m in parts))
        found = run_mineru(source_pdf, pages, os.path.join(parts[0][0], "mineru.log"))
        if found is None:
            continue
        for folder, missing in parts:
            cache = load_cache(folder, source_pdf)
            cache.update({p: found[p] for p in missing})
            save_cache(folder, source_pdf, cache)


# ------------------------------------------------------------------------------------------------ comparing

def marker_latex(data):
    out = {}
    for page in data["pages"]:
        for b in qa.leaves(page):
            if b["block_type"] == "Equation":
                out[b["id"]] = " ".join(html.unescape(inner).strip() for _, inner in qa.MATH.findall(b["html"] or ""))
    return out


def mineru_at(pdf, pi, box, equations):
    """MinerU equations at Marker's box: mostly inside it vertically (MinerU's boxes are page fractions)."""
    W, H = pdf.size(pi)
    left, top, right, bottom = box
    y0, y1, x0, x1 = (H - top) / H, (H - bottom) / H, left / W, right / W
    hits = []
    for e in equations:
        ex0, ey0, ex1, ey1 = e["bbox"]
        overlap = min(y1, ey1) - max(y0, ey0)
        if overlap > 0.5 * min(y1 - y0, ey1 - ey0) and min(x1, ex1) > max(x0, ex0):
            hits.append(e)
    return sorted(hits, key=lambda e: e["bbox"][1])


def reading(latex, bold):
    return (qa.symbols_from_latex(latex), qa.accents_from_latex(latex), qa.bold_from_latex(latex) if bold else [])


def judge(pdf, pi, box, marker, hits, bold):
    """Compare MinerU's reading with Marker's, and both with the PDF text layer."""
    theirs = " ".join(mineru_clean(e["latex"]) for e in hits)
    variants = list(qa.box_variants(box)) + list(qa.label_anchored_boxes(pdf, pi, box, qa.eq_labels(marker)))

    def dist(latex):
        """Symbols + accents (+ bold) differing from the PDF text in the best-fitting box, and PDF symbols there."""
        ours, ours_acc, ours_bold = reading(latex, bold)
        best = None
        for b in variants:
            raw = pdf.text_in_box(pi, b)
            pdf_syms = qa.symbols(qa.pdf_math_text(raw))
            d = sum(map(len, qa.multiset_diff(pdf_syms, ours, latex)))
            d += sum(map(len, qa.multiset_diff(qa.accents(raw), ours_acc)))
            if bold:
                d += sum(map(len, qa.multiset_diff(pdf.bold_in_box(pi, b), ours_bold)))
            if best is None or d < best[0]:
                best = (d, len(pdf_syms))
        return best

    if reading(marker, bold) == reading(theirs, bold):
        return "agree", theirs
    dk, n_pdf = dist(marker)
    dm, _ = dist(theirs)
    if n_pdf < 3:
        return "differs", theirs
    if dm == 0 and dk > 0:
        return "matches_pdf", theirs
    return ("closer" if dm < dk else "differs"), theirs


def with_marker_number(mineru, marker):
    """MinerU's LaTeX with Marker's equation number (None if Marker's block has several numbers)."""
    labels = [m.group(0) for m in qa.EQ_LABEL.finditer(marker)]
    if len(labels) > 1:
        return None
    new = qa.EQ_NUMBER.sub(" ", mineru_clean(mineru)).strip()
    return f"{new} \\quad {labels[0]}" if labels else new


NOTES = {"agree": "MinerU reads it the same",
         "matches_pdf": "MinerU's reading matches the PDF (fix in qa_mineru_fixes.txt)",
         "closer": "MinerU is closer to the PDF",
         "differs": "MinerU reads it differently",
         "none": "MinerU found no equation here"}


def annotate(folder, data, pdf, issues, fetch_missing=False):
    """Add MinerU's verdict to QA's equation flags (issue["mineru"]); demote flags MinerU agrees with. Uses the
    cache; with fetch_missing, runs MinerU first for flagged pages not in it. Safe to call again."""
    for i in issues:  # undo an earlier run, so the verdict is always from the current cache
        if i.pop("mineru", None) is not None and "qa_confidence" in i:
            qa_confidence = i.pop("qa_confidence")
            i.pop("confidence", None)
            if qa_confidence:
                i["confidence"] = qa_confidence
            if i.pop("qa_visual", False):
                i["visual"] = True
    todo = [i for i in issues if checked(i)]
    if not todo:
        return
    page_of = lambda i: i.get("pdf_index", i["page"] - 1)
    if fetch_missing:
        fetch([(folder, data["source_pdf"], {page_of(i) for i in todo})])
    cache = load_cache(folder, data["source_pdf"])
    latex = marker_latex(data)
    katex_ok, verdicts = {}, []
    for k, i in enumerate(todo):
        pi = page_of(i)
        if pi not in cache or not latex.get(i["block"]):
            continue
        marker = latex[i["block"]]
        hits = mineru_at(pdf, pi, i["box"], cache[pi])
        verdict, theirs = judge(pdf, pi, i["box"], marker, hits, i["reason"].startswith("bold")) if hits else ("none", "")
        verdicts.append((i, marker, verdict, theirs, len(hits)))
        if theirs:
            katex_ok[str(k)] = theirs
    errors = qa.validate_math([{"id": k, "latex": v, "display": True} for k, v in katex_ok.items()]) if katex_ok else {}
    valid = {v for k, v in katex_ok.items() if k not in errors}
    # one fix per block, and none where QA already has a mechanical (accent) fix for it
    fixed_blocks = {i["block"] for i in issues if i.get("fix")}
    for i, marker, verdict, theirs, n_hits in verdicts:
        note = {"verdict": verdict}
        if theirs and verdict != "agree":
            note["latex"] = theirs
        is_katex = i["reason"].startswith("KaTeX")
        if verdict == "agree" and not is_katex:
            i["qa_confidence"], i["confidence"] = i.get("confidence"), "low"
            if i.pop("visual", False):
                i["qa_visual"] = True
        if ((verdict == "matches_pdf" or verdict == "agree" and is_katex) and n_hits == 1 and theirs in valid
                and i["block"] not in fixed_blocks):
            new = with_marker_number(theirs, marker)
            if new:
                note["fix"] = [" ".join(marker.split()), new]
                note["verdict"] = "matches_pdf"
                fixed_blocks.add(i["block"])
        i["mineru"] = note


def note_text(issue):
    m = issue.get("mineru")
    if not m:
        return ""
    verdict = m["verdict"] if m["verdict"] != "matches_pdf" or m.get("fix") else "closer"
    text = NOTES[verdict]
    return text + (f": `{qa.short(m['latex'])}`" if m.get("latex") and verdict != "agree" else "")


# ------------------------------------------------------------------------------------------------ whole chapters

def run(folders, release=False):
    """Cross-check chapters QA has already checked (one MinerU run for all of them), then rewrite their reports."""
    if release:
        release_gpu()
    chapters = []
    for folder in folders:
        blocks = glob.glob(os.path.join(folder, "*.blocks.json"))
        report = os.path.join(folder, "qa_report.json")
        if blocks and os.path.exists(report):
            chapters.append((folder, blocks[0], json.load(open(blocks[0], encoding="utf-8")),
                             json.load(open(report, encoding="utf-8"))))
    fetch([(f, d["source_pdf"], {i["page"] - 1 for i in r["issues"] if checked(i)}) for f, _, d, r in chapters])
    all_counts = {}
    for folder, blocks, data, report in chapters:
        pdf = qa.Pdf(data["source_pdf"])
        issues = report["issues"]
        annotate(folder, data, pdf, issues)
        name = os.path.basename(blocks)[: -len(".blocks.json")]
        all_counts[folder] = qa.write_reports(folder, name, report["stats"], report["pages_without_text_layer"], issues)
        print(f"{name}: {summary(issues)}")
    return all_counts


def summary(issues):
    v = [i["mineru"]["verdict"] for i in issues if "mineru" in i]
    fixes = sum(1 for i in issues if i.get("mineru", {}).get("fix"))
    return (f"MinerU checked {len(v)} flagged equations: {v.count('agree')} agree with Marker (demoted), "
            f"{fixes} fixes matching the PDF, {len(v) - v.count('agree') - fixes} still to review")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("folders", nargs="+", help="chapter folders, or a book folder with --all")
    ap.add_argument("--all", action="store_true", help="every chapter folder inside the given book folder")
    args = ap.parse_args()
    folders = args.folders
    if args.all:
        folders = sorted(f for f in glob.glob(os.path.join(folders[0], "[0-9][0-9]_*")) if os.path.isdir(f))
    run(folders)


if __name__ == "__main__":
    main()
