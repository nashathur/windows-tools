"""Display equations to LaTeX with Codex (ChatGPT plan, no Claude tokens), checked against the PDF text layer.

Run in the dev env, on chapter folders made by convert.py (book.py and convert.py call it themselves):
    conda run -n dev python C:\\Tools\\pdf2md\\eqread.py out\\ch02 [out\\ch03 ...]

Pass 1: a chapter's unread equation crops go onto numbered sheets (E1, E2, ... about 10 per image) and one Codex
run reads them all, given the PDF's text for each equation as a hint. Every reading is checked like qa.py checks a
display equation (symbols, bold, accents, number vs the PDF text) and with KaTeX. Pass 2 re-reads, with a stronger
model, the equations with any mismatch, told the first reading and what the check found; pass 3 (strongest effort)
takes what is still wrong. Where the PDF text can't be compared (scans, glyphs printed as other letters), passes
2 and 3 read blind and two readings must agree. The best reading is kept; what still fails is final too and gets
a "possible error, see PDF p. N" note in the chapter .md. Readings are saved after each pass, so an interrupted
run resumes where it stopped. Only the Codex runs go in parallel (PDFium is not thread-safe).
"""

import argparse
import hashlib
import json
import os
import re
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import threading

from PIL import Image, ImageDraw, ImageFont

from ..errors import PdfError
from .p8_io import atomic_write

from . import p8_qa as qa
from .p8_extract import load_eqs, render, save_eqs

ASK = r"C:\Tools\codex\ask.ps1"
PASSES = {  # ask.ps1 role and effort
    "pass1": ["-Role", "quick", "-Effort", "medium"],
    "pass2": ["-Role", "review", "-Effort", "high"],
    "pass3": ["-Role", "review", "-Effort", "xhigh"],
}
SHEET_MAX_H, SHEET_MAX_W, LABEL_W = 1600, 1500, 90
PARALLEL = 3  # Codex runs at once
CHECK = argparse.Namespace(max_symbol_diff=8)
ANSWER_LINE = re.compile(r"^\s*([ET])(\d+)\s*[\t:|]\s*(.+?)\s*$")
TAG = re.compile(r"\\tag\*?\{([^{}]*)\}")
# notation that two readings may differ in without disagreeing
SAME = [(re.compile(r"\\[lr]?Vert(?![A-Za-z])"), r"\\|"),
        (re.compile(r"\\(?:left|right|big[lr]?|Big[lr]?|bigg[lr]?|displaystyle|quad|qquad|[,;:!])(?![A-Za-z])"), ""), (re.compile(r"\\[dt]frac(?![A-Za-z])"), r"\\frac"),
        (re.compile(r"\\(?:boldsymbol|bm)(?![A-Za-z])"), r"\\mathbf"), (re.compile(r"\\varphi(?![A-Za-z])"), r"\\phi"),
        (re.compile(r"\\(?:begin|end)\{aligned\}|&"), ""), (re.compile(r"\s+"), ""), (re.compile(r"[.,;]+$"), "")]


# ---------------------------------------------------------------------------------------------------- sheets

def make_sheets(folder, eqs, prefix, tables=()):
    """Stack equation and table crops onto sheets; labels are independent E<n> and T<n> sequences."""
    font = ImageFont.load_default(size=30)
    for number, equation in enumerate(eqs, 1):
        equation["_label"] = number
    items = [("E", e, os.path.join(folder, e["crop"])) for e in eqs]
    items += [("T", t, t["image"]) for t in tables]
    imgs = {}
    for _, equation, path in items:
        with Image.open(path) as source:
            imgs[id(equation)] = source.convert("RGB")
    for i, im in imgs.items():
        if im.width > SHEET_MAX_W - LABEL_W:
            imgs[i] = im.resize((SHEET_MAX_W - LABEL_W, round(im.height * (SHEET_MAX_W - LABEL_W) / im.width)))
    groups, cur, cur_h = [], [], 0
    for kind, item, _ in items:
        h = imgs[id(item)].height + 24
        if cur and cur_h + h > SHEET_MAX_H:
            groups.append(cur); cur, cur_h = [], 0
        cur.append((kind, item)); cur_h += h
    if cur:
        groups.append(cur)
    os.makedirs(os.path.join(folder, "sheets"), exist_ok=True)
    paths = []
    for k, group in enumerate(groups, 1):
        W = LABEL_W + max(imgs[id(e)].width for _, e in group)
        sheet = Image.new("RGB", (W, sum(imgs[id(e)].height + 24 for _, e in group)), "white")
        draw, y = ImageDraw.Draw(sheet), 0
        for kind, e in group:
            im = imgs[id(e)]
            label = e.get("_label", e.get("label"))
            e["_label"] = label
            draw.line([(0, y + 2), (W, y + 2)], fill=(200, 200, 200), width=2)
            draw.text((6, y + 12 + im.height // 2 - 18), f"{kind}{label}", fill=(220, 0, 0), font=font)
            sheet.paste(im, (LABEL_W, y + 12))
            y += im.height + 24
        paths.append(os.path.join(folder, "sheets", f"{prefix}_{k}.png"))
        atomic_write(paths[-1], writer=lambda temp: sheet.save(temp, format="PNG"))
    for image in imgs.values():
        image.close()
    return paths


# ---------------------------------------------------------------------------------------------------- checking

def for_check(latex):
    """qa.py's checks read equation numbers as "(2.10)"; Codex may write \\tag{2.10}."""
    return TAG.sub(lambda m: rf" \quad ({m.group(1)})", latex)


def judge(pdf, e, latex):
    """The check of one reading: problems (worth a re-read and, if they stay, a note), minor (low-confidence
    differences, e.g. "probably a neighbouring line": worth a re-read only), checked (False when the PDF has no
    usable text to compare with)."""
    stats = {"equations_unverifiable": 0, "equations_compared": 0}
    blind = e["page"] in getattr(pdf, "ocr_pages", set())
    found = ([{"reason": "OCR page; equation cannot be verified against text", "visual": True,
               "confidence": "visual"}] if blind else
             qa.check_equation(pdf, e["page"] - 1, tuple(e["box"]), for_check(latex), "", CHECK, stats))
    visual = [f["reason"] for f in found if f.get("visual")]
    problems = [f["reason"] for f in found if f.get("confidence") == "high"]
    from ..mathcheck import validate
    katex = validate([{"id": "x", "latex": for_check(latex), "display": True}])
    if katex:
        problems.append(f"KaTeX: {katex['x'][:80]}")
    return {"latex": latex, "problems": problems, "minor": [f["reason"] for f in found if f.get("confidence") == "low"],
            "checked": not visual, "unchecked_reason": visual[0] if visual else None}


def same(a, b):
    """Two readings agree: equal LaTeX up to spacing and equivalent notation (structure still has to match)."""
    norm = lambda s: re.sub(r"\(\d+\.\d+[a-z]?\)", "", for_check(s or "").replace(r"\tag", ""))
    a, b = norm(a), norm(b)
    for pat, rep in SAME:
        a, b = pat.sub(rep, a), pat.sub(rep, b)
    return a == b


def pick(pdf, e):
    """Keep the best reading. A reading the PDF text can't line up with ranks below any reading it can check (a
    reading that drops half the equation is "unchecked", not better); then fewest problems, fewest minor
    differences; later readings win ties (they saw more)."""
    if not e["readings"]:
        e.update(latex=None, problems=["not read"], minor=[], checked=False, unchecked_reason="not read")
        return
    rank = lambda t: (not t["checked"] and bool(e["pdf_text"]), len(t["problems"]), len(t["minor"]))
    best = None
    for r in e["readings"]:
        trial = judge(pdf, e, r["latex"])
        if best is None or rank(trial) <= rank(best):
            best = trial
    e.update(best)


def doubtful(e):
    """Fails the check, or can't be checked and the last two readings disagree."""
    if e["problems"]:
        return True
    if e["checked"]:
        return False
    return len(e["readings"]) < 2 or not same(e["readings"][-1]["latex"], e["readings"][-2]["latex"])


def wanted(e, pass_name):
    """Does this equation go into this pass?"""
    if any(r["by"] == pass_name for r in e["readings"]):
        return False
    if pass_name == "pass1":
        return True
    if pass_name == "pass2":
        return doubtful(e) or bool(e["minor"])
    return doubtful(e)


# ---------------------------------------------------------------------------------------------------- Codex

def prompt_text(folder, sheets, eqs, pass_name, tables=()):
    lines = [
        "<task>",
        "Transcribe the display equations and flagged tables in these images. Each image (open it with your image tool) is a",
        "sheet of crops from a textbook page, each marked with a red label E<n> (equation) or T<n> (table) on its left.",
        "Sheets: " + ", ".join(os.path.relpath(p, folder) for p in sheets),
        "",
        "Rules:",
        "- Copy exactly what is printed: every symbol, sub/superscript, bold (\\mathbf{} for bold Latin letters,",
        "  \\boldsymbol{} for bold Greek), accent (\\tilde, \\bar, \\hat, \\dot, \\mathring differ), calligraphic letters.",
        "- Add nothing that isn't printed: no operator before an \\underbrace/\\overbrace whose content starts",
        "  with its own sign, no \\left/\\right pairs or brackets that aren't there.",
        "- Write each printed equation number as `\\quad (2.10)` at the end of the line it belongs to.",
        "- Several lines under one label: \\begin{aligned} ... \\end{aligned} with \\\\ between lines and & at the",
        "  alignment points; numbers then go at the end of their own line, inside aligned.",
        "- Leave out words of body text cut in at the edge of a crop; keep short words that belong to the equation",
        "  (\\text{for}, \\text{and}).",
        "- The PDF text below comes from the PDF's text layer: glyph order is scrambled and fractions are flattened,",
        "  but it shows which symbols, digits, bold letters and accent glyphs (˜ ¯ ˆ ˙) the equation really has.",
        "  Use it to settle doubtful glyphs; the image decides the structure.",
        "- The LaTeX must render in KaTeX.",
        "</task>",
        "",
        "<equations>",
    ]
    for e in eqs:
        pdf_hint = "(none: scanned page)" if e.get("blind") else (e["pdf_text"] or "(none: scanned page)")
        line = f"E{e['_label']} (p.{e['page']}): PDF text: {pdf_hint}"
        if e["bold"]:
            line += f" | bold in PDF: {e['bold']}"
        if pass_name != "pass1" and e.get("checked") and not e.get("blind"):
            line += f"\n    earlier reading: {e['latex']}\n    check found: {'; '.join(e['problems'] + e['minor'])}"
        lines.append(line)
    lines += ["</equations>"]
    if tables:
        lines += ["", "<tables>",
                  "Read each labelled table. Return one Markdown table per label on one line: rows separated by ` || `.",
                  "Preserve every cell and its order; do not add explanations."]
        for table in tables:
            shape = table.get("shape")
            hint = f" Expected source shape: {shape[0]} rows x {shape[1]} columns." if shape else ""
            lines.append(f"T{table['_label']} (p.{table['page']}):{hint}")
        lines += ["</tables>"]
    lines += ["", "<output_contract>",
              "One line per equation in label order: `E<n><TAB><latex>` (LaTeX on one line, no $ delimiters).",
              "One line per table in label order: `T<n><TAB><one Markdown table on one line, rows separated by ` || `>`.",
              "Nothing else: no commentary, no code fences.", "</output_contract>"]
    return "\n".join(lines) + "\n"


def ask_codex(folder, pass_name, eqs, tables=(), context=None):
    """One Codex run over equation and table crops. Returns equation ids and table indices separately."""
    sheets = make_sheets(folder, eqs, pass_name, tables)
    prompt = os.path.join(folder, "sheets", f"{pass_name}.md")
    atomic_write(prompt, prompt_text(folder, sheets, eqs, pass_name, tables))
    t0 = time.time()
    from .p8_codex import ask
    options = PASSES[pass_name]
    role = options[1]
    effort = options[3]
    answer = ask(prompt, folder, role, effort, sheets, **({"context": context} if context is not None else {}))
    by_label = {e["_label"]: ("E", e["id"]) for e in eqs}
    by_label.update({("T", t["_label"]): ("T", t["id"]) for t in tables})
    got = {by_label[key]: value for key, value in parse_answer(answer).items() if key in by_label}
    print(f"{os.path.basename(folder)} {pass_name}: {sum(k[0] == 'E' for k in got)}/{len(eqs)} equations, "
          f"{sum(k[0] == 'T' for k in got)}/{len(tables)} tables read on {len(sheets)} sheets "
          f"in {time.time() - t0:.0f} s", file=sys.stderr)
    return got


def parse_answer(text):
    out = {}
    for line in text.splitlines():
        m = ANSWER_LINE.match(line.strip("`"))
        if m:
            kind, n, value = m.groups()
            out[(kind, int(n)) if kind == "T" else int(n)] = value.strip().strip("$").strip()
    return out


# ---------------------------------------------------------------------------------------------------- run

def run(folders, source_paths=None, context=None):
    """Read the equations of these chapters. Returns {folder: number still doubtful}. Chapters with nothing to
    read are left untouched (their .md may carry fix.py edits)."""
    chapters = {}
    from . import p8_tables
    source_paths = source_paths or {}
    for f in folders:
        eqs = list(load_eqs(f).values())
        source_pdf = source_paths.get(f)
        if context is None:
            tables = p8_tables.pending(f, source_pdf) if source_pdf is not None else p8_tables.pending(f)
        else:
            tables = p8_tables.pending(f, source_pdf, context=context)
        if eqs or tables:
            with open(next(Path(f).glob("*.pages.json")), encoding="utf-8") as stream:
                page_data = json.load(stream)
            pdf = qa.Pdf(source_pdf or page_data["source_pdf"])
            pdf.ocr_pages = set(page_data.get("ocr_pages", []))
            for equation in eqs:
                equation["blind"] = equation["page"] in pdf.ocr_pages
            chapters[f] = (pdf, eqs, tables)
    changed = set()
    for pass_name in PASSES:
        jobs = {}
        for f, (pdf, eqs, tables) in chapters.items():
            for e in eqs:
                if "problems" not in e:
                    pick(pdf, e)
            todo = [e for e in eqs if wanted(e, pass_name)]
            table_todo = tables if pass_name == "pass1" else []
            if todo or table_todo:
                jobs[f] = (todo, table_todo)
        with ThreadPoolExecutor(PARALLEL) as pool:
            results = dict(zip(jobs, pool.map(lambda f: ask_codex(f, pass_name, *jobs[f], context=context), jobs)))
        for f, got in results.items():
            pdf, eqs, tables = chapters[f]
            eq_todo, table_todo = jobs[f]
            for e in eq_todo:
                if ("E", e["id"]) in got:
                    e["readings"].append({"by": pass_name, "latex": got[("E", e["id"])]})
                else:  # not answered: counts as this pass's attempt, so the next pass takes it
                    e["readings"].append({"by": pass_name, "latex": e.get("latex") or ""})
                pick(pdf, e)
            # Checkpoint equation work before any independent table review call;
            # that call can fail or be interrupted after this pass has succeeded.
            save_eqs(f, clean(eqs))
            if pass_name == "pass1" and table_todo:
                table_readings = {i: got[("T", i)] for i in (t["id"] for t in table_todo) if ("T", i) in got}
                if context is None:
                    p8_tables.apply_pass1(f, table_todo, table_readings)
                else:
                    p8_tables.apply_pass1(f, table_todo, table_readings, context=context)
            changed.add(f)
    left = {}
    for f, (pdf, eqs, tables) in chapters.items():
        for e in eqs:
            e["status"] = "doubtful" if doubtful(e) else "ok"
        save_eqs(f, clean(eqs))
        left[f] = sum(e["status"] == "doubtful" for e in eqs)
        if f in changed:
            render(f)
            print(f"  {os.path.basename(f)}: {len(eqs) - left[f]} of {len(eqs)} equations pass, {left[f]} marked "
                  f"as possible errors")
    for pdf, _, _ in chapters.values():
        pdf.close()
    return left


def clean(eqs):
    return [{k: v for k, v in e.items() if not k.startswith("_")} for e in eqs]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("folders", nargs="+", help="chapter folders made by convert.py (or book.py)")
    a = ap.parse_args()
    for f in a.folders:
        if not os.path.exists(os.path.join(f, "eqs.json")):
            PdfError("NOT_FOUND", f"eqs.json not found in {f}", "run convert.py or book.py first")
    run(a.folders)
    for f in a.folders:
        qa.run(f)


# Library module; command wrapper registered in p8_md.py.
