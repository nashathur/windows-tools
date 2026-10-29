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

from PIL import Image, ImageDraw, ImageFont

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "common"))
import clierr

import qa
from convert import load_eqs, render, save_eqs

ASK = r"C:\Tools\codex\ask.ps1"
PASSES = {  # ask.ps1 role and effort
    "pass1": ["-Role", "quick", "-Effort", "medium"],
    "pass2": ["-Role", "review", "-Effort", "high"],
    "pass3": ["-Role", "review", "-Effort", "xhigh"],
}
SHEET_MAX_H, SHEET_MAX_W, LABEL_W = 1600, 1500, 90
PARALLEL = 3  # Codex runs at once
CHECK = argparse.Namespace(max_symbol_diff=8)
ANSWER_LINE = re.compile(r"^\s*E(\d+)\s*[\t:|]\s*(.+?)\s*$")
TAG = re.compile(r"\\tag\*?\{([^{}]*)\}")
# notation that two readings may differ in without disagreeing
SAME = [(re.compile(r"\\[lr]?Vert(?![A-Za-z])"), r"\\|"),
        (re.compile(r"\\(?:left|right|big[lr]?|Big[lr]?|bigg[lr]?|displaystyle|quad|qquad|[,;:!])(?![A-Za-z])"), ""), (re.compile(r"\\[dt]frac(?![A-Za-z])"), r"\\frac"),
        (re.compile(r"\\(?:boldsymbol|bm)(?![A-Za-z])"), r"\\mathbf"), (re.compile(r"\\varphi(?![A-Za-z])"), r"\\phi"),
        (re.compile(r"\\(?:begin|end)\{aligned\}|&"), ""), (re.compile(r"\s+"), ""), (re.compile(r"[.,;]+$"), "")]


# ---------------------------------------------------------------------------------------------------- sheets

def make_sheets(folder, eqs, prefix):
    """Stack crops under E-numbers onto images of at most SHEET_MAX_H px; sets e["_label"]. Returns sheet paths."""
    font = ImageFont.load_default(size=30)
    imgs = {e["id"]: Image.open(os.path.join(folder, e["crop"])).convert("RGB") for e in eqs}
    for i, im in imgs.items():
        if im.width > SHEET_MAX_W - LABEL_W:
            imgs[i] = im.resize((SHEET_MAX_W - LABEL_W, round(im.height * (SHEET_MAX_W - LABEL_W) / im.width)))
    groups, cur, cur_h = [], [], 0
    for e in eqs:
        h = imgs[e["id"]].height + 24
        if cur and cur_h + h > SHEET_MAX_H:
            groups.append(cur); cur, cur_h = [], 0
        cur.append(e); cur_h += h
    if cur:
        groups.append(cur)
    os.makedirs(os.path.join(folder, "sheets"), exist_ok=True)
    paths, n = [], 0
    for k, group in enumerate(groups, 1):
        W = LABEL_W + max(imgs[e["id"]].width for e in group)
        sheet = Image.new("RGB", (W, sum(imgs[e["id"]].height + 24 for e in group)), "white")
        draw, y = ImageDraw.Draw(sheet), 0
        for e in group:
            im, n = imgs[e["id"]], n + 1
            e["_label"] = n
            draw.line([(0, y + 2), (W, y + 2)], fill=(200, 200, 200), width=2)
            draw.text((6, y + 12 + im.height // 2 - 18), f"E{n}", fill=(220, 0, 0), font=font)
            sheet.paste(im, (LABEL_W, y + 12))
            y += im.height + 24
        paths.append(os.path.join(folder, "sheets", f"{prefix}_{k}.png"))
        sheet.save(paths[-1])
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
    found = qa.check_equation(pdf, e["page"] - 1, tuple(e["box"]), for_check(latex), "", CHECK, stats)
    visual = [f["reason"] for f in found if f.get("visual")]
    problems = [f["reason"] for f in found if f.get("confidence") == "high"]
    katex = qa.validate_math([{"id": "x", "latex": for_check(latex), "display": True}])
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

def prompt_text(folder, sheets, eqs, pass_name):
    lines = [
        "<task>",
        "Transcribe the display equations in these images into LaTeX. Each image (open it with your image tool) is a",
        "sheet of equations cropped from a textbook page, each marked with a red label E<n> on its left.",
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
        line = f"E{e['_label']} (p.{e['page']}): PDF text: {e['pdf_text'] or '(none: scanned page)'}"
        if e["bold"]:
            line += f" | bold in PDF: {e['bold']}"
        if pass_name != "pass1" and e.get("checked"):  # unchecked ones are read blind: agreement is the test
            line += f"\n    earlier reading: {e['latex']}\n    check found: {'; '.join(e['problems'] + e['minor'])}"
        lines.append(line)
    lines += ["</equations>", "", "<output_contract>",
              "One line per equation, in label order: `E<n><TAB><latex>` (LaTeX on one line, no $ delimiters).",
              "Nothing else: no commentary, no code fences.", "</output_contract>"]
    return "\n".join(lines) + "\n"


def ask_codex(folder, pass_name, eqs):
    """One Codex run over eqs (no PDFium here: this runs in a worker thread). Returns {eq id: latex}."""
    sheets = make_sheets(folder, eqs, pass_name)
    prompt = os.path.join(folder, "sheets", f"{pass_name}.md")
    with open(prompt, "w", encoding="utf-8") as f:
        f.write(prompt_text(folder, sheets, eqs, pass_name))
    t0 = time.time()
    clierr.run_tool(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", ASK, "-PromptFile", prompt,
                     "-Dir", os.path.abspath(folder), *PASSES[pass_name]], name="codex", timeout=3600,
                    fix=f"see {prompt}.log; rerun on the folder (readings already saved are kept)")
    by_label = {e["_label"]: e["id"] for e in eqs}
    got = {by_label[n]: latex for n, latex in parse_answer(open(prompt + ".answer.md", encoding="utf-8").read()).items()
           if n in by_label}
    print(f"  {os.path.basename(folder)} {pass_name}: {len(got)}/{len(eqs)} equations read on {len(sheets)} sheets "
          f"in {time.time() - t0:.0f} s")
    return got


def parse_answer(text):
    out = {}
    for line in text.splitlines():
        m = ANSWER_LINE.match(line.strip("`"))
        if m:
            out[int(m.group(1))] = m.group(2).strip().strip("$").strip()
    return out


# ---------------------------------------------------------------------------------------------------- run

def run(folders):
    """Read the equations of these chapters. Returns {folder: number still doubtful}. Chapters with nothing to
    read are left untouched (their .md may carry fix.py edits)."""
    chapters = {}
    for f in folders:
        eqs = list(load_eqs(f).values())
        if eqs:
            pdf = qa.Pdf(json.load(open(next(Path(f).glob("*.pages.json")), encoding="utf-8"))["source_pdf"])
            chapters[f] = (pdf, eqs)
    changed = set()
    for pass_name in PASSES:
        jobs = {}
        for f, (pdf, eqs) in chapters.items():
            for e in eqs:
                if "problems" not in e:
                    pick(pdf, e)
            todo = [e for e in eqs if wanted(e, pass_name)]
            if todo:
                jobs[f] = todo
        with ThreadPoolExecutor(PARALLEL) as pool:
            results = dict(zip(jobs, pool.map(lambda f: ask_codex(f, pass_name, jobs[f]), jobs)))
        for f, got in results.items():
            pdf, eqs = chapters[f]
            for e in jobs[f]:
                if e["id"] in got:
                    e["readings"].append({"by": pass_name, "latex": got[e["id"]]})
                else:  # not answered: counts as this pass's attempt, so the next pass takes it
                    e["readings"].append({"by": pass_name, "latex": e.get("latex") or ""})
                pick(pdf, e)
            save_eqs(f, clean(eqs))
            changed.add(f)
    left = {}
    for f, (pdf, eqs) in chapters.items():
        for e in eqs:
            e["status"] = "doubtful" if doubtful(e) else "ok"
        save_eqs(f, clean(eqs))
        left[f] = sum(e["status"] == "doubtful" for e in eqs)
        if f in changed:
            render(f)
            print(f"  {os.path.basename(f)}: {len(eqs) - left[f]} of {len(eqs)} equations pass, {left[f]} marked "
                  f"as possible errors")
    return left


def clean(eqs):
    return [{k: v for k, v in e.items() if not k.startswith("_")} for e in eqs]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("folders", nargs="+", help="chapter folders made by convert.py (or book.py)")
    a = ap.parse_args()
    for f in a.folders:
        if not os.path.exists(os.path.join(f, "eqs.json")):
            raise clierr.Fail("NOT_FOUND", f"eqs.json not found in {f}", "run convert.py or book.py first")
    run(a.folders)
    for f in a.folders:
        qa.run(f)


if __name__ == "__main__":
    clierr.run("eqread", main)
