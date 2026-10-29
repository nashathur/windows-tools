"""Apply verify.py's equation corrections (numbers, accents, several readings, duplicates) to chapter folders that
were converted earlier, without converting again (Marker's OCR varies between runs, so a new conversion would
also change everything else). Updates <name>.blocks.json and <name>.md in place, then re-runs qa.py.

Run in the pdfmath env:
    conda run -n pdfmath python C:\\Tools\\pdf2md\\reverify.py out\\ch02 [out\\ch05 ...]
    conda run -n pdfmath python C:\\Tools\\pdf2md\\reverify.py out --all

An equation is changed in the Markdown where its old LaTeX (or its equation number) can be found there exactly
once; others are listed as not applied (blocks.json and the Markdown then still agree: both keep the old form).
"""

import argparse
import glob
import json
import os
import re
import sys
import time
from pathlib import Path

import convert  # noqa: F401  (applies the Windows fix for Surya's process handling)
import qa
import verify
from fix import pattern
from postprocess import tag_label, validate_math  # noqa: F401

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "common"))
import clierr

SPAN = re.compile(r'<span id="[^"]*"></span>')
DISPLAY = re.compile(r"\$\$(.+?)\$\$", re.S)


def equations_from_json(data, pdf):
    eqs, leaves = [], {}
    for page in data["pages"]:
        pi = qa.page_index(page["id"])
        if not pdf.has_text_layer(pi):
            continue
        W, H = pdf.size(pi)
        sx, sy = W / page["bbox"][2], H / page["bbox"][3]

        def walk(block, parent):
            if block.get("children"):
                for child in block["children"]:
                    walk(child, block)
            elif block["block_type"] == "Equation":
                x0, y0, x1, y1 = block["bbox"]
                h = SPAN.sub("", block["html"] or "")
                has_math = "<math" in h
                eqs.append({"id": block["id"], "pi": pi, "box": (x0 * sx, H - y0 * sy, x1 * sx, H - y1 * sy),
                            "html": h if has_math else None, "text": "" if has_math else verify.outside_math(h)})
                leaves[block["id"]] = (block, parent, h)
        walk(page, None)
    return eqs, leaves


def label_units(md, label):
    """Spans in the Markdown carrying an equation number: display blocks with \\tag{N} or (N), and lines (outside
    display math) ending in (N)."""
    bare = label.strip("()")
    units = [(m.start(), m.end()) for m in DISPLAY.finditer(md)
             if f"\\tag{{{bare}}}" in m.group(1) or label in m.group(1)]
    inside = [(m.start(), m.end()) for m in DISPLAY.finditer(md)]
    for m in re.finditer(r"^.*" + re.escape(label) + r"\s*$", md, re.M):
        if not any(a <= m.start() < b for a, b in inside):
            units.append((m.start(), m.end()))
    return units


def update_markdown(md, old_html, old_text, new_html):
    """Returns (new_md, how) or (None, reason)."""
    old_latex = verify.latex_of(old_html) if old_html else ""
    new_latex = verify.latex_of(new_html) if new_html else None
    replacement = "" if new_latex is None else "$$" + tag_label(new_latex) + "$$"
    # A. the old LaTeX itself, once
    if old_latex:
        for form in dict.fromkeys([old_latex, tag_label(old_latex)]):
            matches = list(pattern(form).finditer(md))
            if len(matches) == 1:
                m = matches[0]
                start = md.rfind("$$", 0, m.start())
                end = md.find("$$", m.end())
                opening = start != -1 and md[:start].count("$$") % 2 == 0
                if opening and end != -1 and "\n\n" not in md[start:m.start()] + md[m.end():end]:
                    return md[:start] + replacement + md[end + 2:], "latex"
    # B. by equation number(s): every unit carrying one of them, each number found exactly once
    labels = qa.eq_labels(old_latex + " " + old_text + " " + (new_latex or ""))
    if not labels:
        return None, "no unique match and no equation number"
    units = []
    for label in labels:
        found = label_units(md, label)
        if len(found) != 1:
            return None, f"{label} found {len(found)} times"
        units.append(found[0])
    units = sorted(set(units))
    for a, b in reversed(units[1:]):
        md = md[:a] + md[b:]
    a, b = units[0]
    return md[:a] + replacement + md[b:], "number"


def reverify(folder, models, equation_processor):
    blocks_path = glob.glob(os.path.join(folder, "*.blocks.json"))[0]
    name = os.path.basename(blocks_path)[: -len(".blocks.json")]
    md_path = os.path.join(folder, f"{name}.md")
    data = json.load(open(blocks_path, encoding="utf-8"))
    md = open(md_path, encoding="utf-8").read()
    pdf = qa.Pdf(data["source_pdf"])
    eqs, leaves = equations_from_json(data, pdf)
    t0 = time.time()
    changes, stats = verify.correct_equations(pdf, eqs, models["recognition_model"], equation_processor)
    by_id = {e["id"]: e for e in eqs}
    applied, skipped = 0, []
    for block_id, new_html in changes.items():
        e = by_id[block_id]
        new_md, how = update_markdown(md, e["html"], e["text"], new_html)
        if new_md is None:
            skipped.append(f"p{e['pi'] + 1} {block_id}: {how}")
            continue
        md = new_md
        applied += 1
        block, parent, _ = leaves[block_id]
        if new_html is None:
            parent["children"] = [c for c in parent["children"] if c is not block]
        else:
            block["html"] = f'<p block-type="Equation">{new_html}</p>'
    with open(md_path, "w", encoding="utf-8") as f:
        f.write(md)
    with open(blocks_path, "w", encoding="utf-8") as f:
        json.dump(data, f)
    print(f"{name}: {dict(stats)}; {applied} changes applied, {len(skipped)} not applied ({time.time() - t0:.0f} s)")
    for s in skipped:
        print("   not applied:", s)
    return qa.run(folder)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("folders", nargs="+", help="chapter folders, or a book folder with --all")
    ap.add_argument("--all", action="store_true", help="every chapter folder inside the given book folder")
    ap.add_argument("--no-mineru", action="store_true", help="skip the MinerU second opinion on flagged equations")
    args = ap.parse_args()
    folders = args.folders
    if args.all:
        if not os.path.isdir(folders[0]):
            raise clierr.Fail("NOT_FOUND", f"book folder not found: {folders[0]}", "check the book folder path")
        folders = sorted(f for f in glob.glob(os.path.join(folders[0], "[0-9][0-9]_*")) if os.path.isdir(f))
        if not folders:
            raise clierr.Fail("BAD_INPUT", f"no chapter folders found in {args.folders[0]}",
                              "use a book folder containing numbered chapter folders")
    missing = [folder for folder in folders if not os.path.isdir(folder)]
    if missing:
        raise clierr.Fail("NOT_FOUND", f"chapter folder not found: {missing[0]}", "check the chapter folder path")
    from marker.models import create_model_dict
    from marker.processors.equation import EquationProcessor
    models = create_model_dict()
    equation_processor = EquationProcessor(models["recognition_model"])
    for folder in folders:
        reverify(folder, models, equation_processor)
    if not args.no_mineru:  # after Marker is done with the GPU: one MinerU run for all chapters
        import crosscheck
        models = equation_processor = None
        crosscheck.run(folders, release=True)


if __name__ == "__main__":
    clierr.run("reverify", main)
