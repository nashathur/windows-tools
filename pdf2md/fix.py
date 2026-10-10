"""Apply review corrections to a chapter's Markdown from a short fixes file, and log them in review_notes.md.

Run in the pdfmath env:
    conda run -n pdfmath python C:\\Users\\thurs\\tools\\pdf2md\\fix.py out\\ch02 fixes.txt [--dry-run]

Fixes file: blocks separated by blank lines. Each block starts with a header line, then edit lines:

    p56 (2.3b) cos -> sin
    - y = -X \\cos \\Omega t
    + y = -X \\sin \\Omega t

    p57 (2.8a) second line duplicated by Marker
    -- \\underbrace{(X \\cos \\Omega t + Y \\sin \\Omega t)}_V

    ok p65 (2.20b) flag came from the line above

  "- old" + "+ new"  replace old with new (old must occur exactly once; spacing differences are ignored)
  "-- old"           delete the whole line containing old (e.g. a duplicated equation)
  "ok pNN note"      no change; logged as checked (several "ok" lines may follow each other)
A block is applied only if all its edits match; otherwise it is skipped and reported.
Each change gets <!-- reconstructed: verify against p.NN (note) --> in the Markdown.
"""

import argparse
import datetime
import glob
import os
import re

from postprocess import tag_label


def parse(text):
    # Every header line ("pNN …" or "ok pNN …") starts a new block, blank line or not.
    text = re.sub(r"\n(?=(?:ok\s+)?p\d+\b)", "\n\n", text.strip(), flags=re.I)
    blocks = []
    for chunk in re.split(r"\n\s*\n", text):
        lines = [l.rstrip() for l in chunk.splitlines() if l.strip()]
        if not lines:
            continue
        header = lines[0]
        ok = header.lower().startswith("ok ")
        m = re.match(r"(?:ok\s+)?p(\d+)\s*(.*)", header, re.I)
        if not m:
            raise SystemExit(f"Bad header (expected 'pNN note' or 'ok pNN note'): {header}")
        edits, i = [], 1
        while i < len(lines):
            line = lines[i]
            if line.startswith("-- "):
                edits.append(("delete", line[3:], None))
            elif line.startswith("- ") and i + 1 < len(lines) and lines[i + 1].startswith("+ "):
                edits.append(("replace", line[2:], lines[i + 1][2:]))
                i += 1
            elif line.startswith("- ") and i + 1 < len(lines) and lines[i + 1].strip() == "+":
                edits.append(("replace", line[2:], ""))
                i += 1
            else:
                raise SystemExit(f"Bad edit line under '{header}': {line}")
            i += 1
        blocks.append({"page": int(m.group(1)), "note": m.group(2).strip(), "ok": ok, "edits": edits})
    return blocks


def pattern(old):
    """Whitespace-insensitive regex for an old fragment: any run of spaces may be 0+ spaces in the file."""
    parts = [re.escape(p) for p in old.split()]
    return re.compile(r"\s*".join(parts))


def apply_block(md, block):
    """Returns (new_md, error). Edits are applied in order; the block fails as a whole."""
    comment_kind = "removed duplicate" if all(k == "delete" for k, _, _ in block["edits"]) else "reconstructed"
    comment = f"<!-- {comment_kind}: verify against p.{block['page']}" + (f" ({block['note']})" if block["note"] else "") + " -->"
    first_line = None
    for kind, old, new in block["edits"]:
        matches = list(pattern(old).finditer(md))
        if not matches and tag_label(old) != old:
            # QA reports equations as Marker wrote them ("\quad (2.31)"); the Markdown has "\tag{2.31}".
            old, new = tag_label(old), tag_label(new) if new else new
            matches = list(pattern(old).finditer(md))
        if len(matches) != 1:
            return None, f"'{old[:60]}' found {len(matches)} times (need exactly 1)"
        m = matches[0]
        line_start = md.rfind("\n", 0, m.start()) + 1
        if kind == "replace":
            md = md[:m.start()] + new + md[m.end():]
        else:
            line_end = md.find("\n", m.end())
            line_end = len(md) if line_end == -1 else line_end + 1
            md = md[:line_start] + md[line_end:]
        first_line = line_start if first_line is None else min(first_line, line_start)
    # Comment on its own line before the change; at the end of the line for table rows (keeps the table intact).
    line_end = md.find("\n", first_line)
    line = md[first_line:line_end if line_end != -1 else len(md)]
    if line.lstrip().startswith("|"):
        md = md[:first_line] + line + " " + comment + md[first_line + len(line):]
    else:
        md = md[:first_line] + comment + "\n" + md[first_line:]
    return md, None


def log_notes(folder, applied, checked, failed):
    path = os.path.join(folder, "review_notes.md")
    new = not os.path.exists(path)
    with open(path, "a", encoding="utf-8") as f:
        if new:
            f.write(f"# Review notes: {os.path.basename(os.path.normpath(folder))}\n\n"
                    "Every change is marked in the Markdown with `<!-- reconstructed/removed duplicate: verify against p.N -->`.\n")
        f.write(f"\n## Review {datetime.datetime.now():%Y-%m-%d %H:%M}\n\n")
        if applied:
            f.write("| Page | Note | Change |\n|---|---|---|\n")
            for b in applied:
                changes = "; ".join(f"`{o}` → `{n}`" if k == "replace" else f"deleted line with `{o}`"
                                    for k, o, n in b["edits"])
                f.write(f"| {b['page']} | {b['note']} | {changes.replace('|', '\\|')} |\n")
        if checked:
            f.write("\nChecked, no change: " + "; ".join(f"p.{b['page']} {b['note']}" for b in checked) + "\n")
        if failed:
            f.write("\nNot applied (fix by hand): " + "; ".join(f"p.{b['page']} {b['note']}: {err}" for b, err in failed) + "\n")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("folder", help="chapter folder (contains one .md file)")
    ap.add_argument("fixes", help="fixes file")
    ap.add_argument("--dry-run", action="store_true", help="check that every edit matches, change nothing")
    args = ap.parse_args()

    md_path = [p for p in glob.glob(os.path.join(args.folder, "*.md"))
               if os.path.basename(p) not in ("qa_report.md", "review_notes.md")][0]
    with open(md_path, encoding="utf-8") as f:
        md = f.read()
    with open(args.fixes, encoding="utf-8-sig") as f:  # -sig: Notepad/PowerShell may add a byte-order mark
        blocks = parse(f.read())

    applied, checked, failed = [], [], []
    for block in blocks:
        if block["ok"]:
            checked.append(block)
            continue
        new_md, err = apply_block(md, block)
        if err:
            failed.append((block, err))
        else:
            md = new_md
            applied.append(block)

    print(f"{'Would apply' if args.dry_run else 'Applied'} {len(applied)} fixes, {len(checked)} checked OK, {len(failed)} failed")
    for block, err in failed:
        print(f"  FAILED p{block['page']} {block['note']}: {err}")
    if not args.dry_run:
        with open(md_path, "w", encoding="utf-8") as f:
            f.write(md)
        log_notes(args.folder, applied, checked, failed)


if __name__ == "__main__":
    main()
