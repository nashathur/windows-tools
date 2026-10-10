"""Convert a whole book chapter by chapter: outline -> convert (models loaded once) -> QA -> 00_index.md.

Run in the pdfmath env:
    conda run -n pdfmath python C:\\Users\\thurs\\tools\\pdf2md\\book.py book.pdf out_dir --list
    conda run -n pdfmath python C:\\Users\\thurs\\tools\\pdf2md\\book.py book.pdf out_dir [--chapters 2,5-7]

Chapters come from the PDF outline: the level whose entries are closest to ~25 pages long (override with --level). A chapter
ends where the next entry at that level or above starts, so part-title pages are skipped. Without an outline,
use --chunk N to split into N-page pieces. Chapters already converted are skipped (resume after an
interruption); --force reconverts them. Always check the plan with --list first.
"""

import argparse
import json
import logging
import os
import re
import time

import pypdfium2 as pdfium

SECONDS_PER_PAGE = 5  # measured on the RTX 3070 (Chapter 2 of Cushman-Roisin & Beckers: 32 pages in 153 s)
TARGET_PAGES = 25  # typical chapter length, used to pick the outline level that holds the chapters


def outline(pdf):
    logging.getLogger("pypdfium2").setLevel(logging.ERROR)
    items = []
    for item in pdf.get_toc(max_depth=10):
        dest = item.get_dest()
        index = dest.get_index() if dest is not None else None
        if index is not None:
            items.append((item.level, item.get_title().strip(), index))
    return items


def plan_chapters(pdf, level=None, chunk=None):
    n_pages = len(pdf)
    if chunk:
        return [(f"pages {s + 1}-{min(s + chunk, n_pages)}", s, min(s + chunk, n_pages) - 1)
                for s in range(0, n_pages, chunk)]
    items = outline(pdf)
    if not items:
        raise SystemExit("This PDF has no outline. Use --chunk N (e.g. --chunk 30) to split it into N-page pieces.")
    def chapters_at(level):
        out = []
        for i, (lvl, title, start) in enumerate(items):
            if lvl != level:
                continue
            nxt = [p for l2, _, p in items[i + 1:] if l2 <= level and p > start]
            end = (min(nxt) - 1) if nxt else n_pages - 1
            out.append((title, start, max(start, end)))
        return out

    if level is None:
        # Outline levels are parts / chapters / sections...: pick the level whose median entry length is closest
        # to a typical chapter (~25 pages), which is also a good size for one Marker run and one review.
        def median_length(lvl):
            lengths = sorted(e - s + 1 for _, s, e in chapters_at(lvl))
            return lengths[len(lengths) // 2]
        level = min(sorted({lvl for lvl, _, _ in items}), key=lambda lvl: abs(median_length(lvl) - TARGET_PAGES))
    chapters = fix_starts(pdf, chapters_at(level))
    return add_uncovered(pdf, chapters, items)


def page_text(pdf, i, n=None):
    text = pdf[i].get_textpage().get_text_range()
    return text[:n] if n else text


def letters(text):
    return re.sub(r"[^a-z]", "", text.lower())


def fix_starts(pdf, chapters):
    """Outline links are sometimes wrong (C-R&B: "Index" points at the References page). If a chapter's title is not
    near the top of its start page but is at the top of a later page in its range, start there instead."""
    fixed = []
    for title, start, end in chapters:
        key = letters(title)
        if key and key not in letters(page_text(pdf, start, 300)):
            for i in range(start + 1, end + 1):
                if letters(page_text(pdf, i, 80)).startswith(key):
                    print(f"  note: outline entry '{title}' points at p{start + 1}, but the title is on p{i + 1}")
                    start = i
                    break
        fixed.append((title, start, end))
    fixed.sort(key=lambda c: c[1])
    # A chapter ends before the next one starts (two entries pointing at the same page used to overlap).
    return [(t, s, min(e, fixed[k + 1][1] - 1) if k + 1 < len(fixed) else e) for k, (t, s, e) in enumerate(fixed)]


def add_uncovered(pdf, chapters, items, min_text_pages=2):
    """Pages outside every chapter (front matter, back matter) become their own entries when they hold real text;
    gaps of blank or part-title pages are skipped."""
    covered = set()
    for _, s, e in chapters:
        covered.update(range(s, e + 1))
    gaps, cur = [], []
    for i in range(len(pdf)):
        if i in covered:
            if cur:
                gaps.append(cur)
            cur = []
        else:
            cur.append(i)
    if cur:
        gaps.append(cur)
    out = list(chapters)
    for gap in gaps:
        if sum(len(page_text(pdf, i).strip()) >= 200 for i in gap) < min_text_pages:
            continue
        s, e = gap[0], gap[-1]
        named = [t for _, t, p in items if p == s]
        title = "Front matter" if s == 0 else (named[0] if named else f"Pages {s + 1}-{e + 1}")
        out.append((title, s, e))
    return sorted(out, key=lambda c: c[1])


def slug(text, n=40):
    return re.sub(r"[^a-z0-9]+", "_", text.lower()).strip("_")[:n].rstrip("_") or "part"


def select(spec, numbers):
    if not spec:
        return list(numbers)
    chosen = set()
    for part in spec.split(","):
        a, _, b = part.partition("-")
        chosen.update(range(int(a), int(b or a) + 1))
    return sorted(c for c in chosen if c in numbers)


def write_index(out_dir, book_title, rows):
    lines = [f"# {book_title}", "", "| # | Chapter | PDF pages | QA: likely / lower / visual | Files |", "|---|---|---|---|---|"]
    totals = [0, 0, 0]
    for n, title, start, end, folder, counts in rows:
        name = os.path.basename(folder)
        if counts:
            qa = f"{counts['high']} / {counts['low']} / {counts['visual']}"
            totals = [t + counts[k] for t, k in zip(totals, ("high", "low", "visual"))]
        else:
            qa = "not run"
        lines.append(f"| {n} | [{title}]({name}/{name}.md) | {start + 1}–{end + 1} | {qa} | "
                     f"[QA report]({name}/qa_report.md) |")
    lines += ["", f"QA total: {totals[0]} likely errors, {totals[1]} lower confidence, {totals[2]} equations to check visually."]
    with open(os.path.join(out_dir, "00_index.md"), "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("pdf")
    ap.add_argument("out_dir")
    ap.add_argument("--list", action="store_true", help="show the chapter plan and time estimate, convert nothing")
    ap.add_argument("--chapters", help='which chapters (numbers from --list), e.g. "2,5-7"')
    ap.add_argument("--level", type=int, help="outline level to treat as chapters (0 = top)")
    ap.add_argument("--chunk", type=int, help="no outline: split into pieces of this many pages")
    ap.add_argument("--force", action="store_true", help="reconvert chapters that already exist")
    ap.add_argument("--no-qa", action="store_true", help="skip qa.py")
    ap.add_argument("--no-mineru", action="store_true", help="skip the MinerU second opinion on flagged equations")
    args = ap.parse_args()

    pdf = pdfium.PdfDocument(args.pdf)
    chapters = plan_chapters(pdf, args.level, args.chunk)
    # Front matter is number 0, so chapter numbers match the book.
    first = 0 if chapters and chapters[0][0] == "Front matter" else 1
    chapters = {first + k: c for k, c in enumerate(chapters)}
    numbers = sorted(chapters)
    chosen = select(args.chapters, numbers)
    names = {n: f"{n:02d}_{slug(chapters[n][0])}" for n in numbers}

    total_pages = 0
    for n in numbers:
        title, start, end = chapters[n]
        exists = os.path.exists(os.path.join(args.out_dir, names[n], f"{names[n]}.blocks.json"))
        mark = "*" if n in chosen and (args.force or not exists) else ("done" if exists else " ")
        if n in chosen and (args.force or not exists):
            total_pages += end - start + 1
        print(f"{mark:>4} {n:3}. {title[:60]:60} PDF p{start + 1}-{end + 1} ({end - start + 1} pp)")
    print(f"\n* = will convert: {total_pages} pages, about {total_pages * SECONDS_PER_PAGE / 60:.0f} min on the GPU")
    if args.list:
        return

    # Import here so --list works instantly (loading Marker takes a while).
    from convert import convert
    from marker.models import create_model_dict
    import qa

    os.makedirs(args.out_dir, exist_ok=True)
    models = None
    rows, failures, checked = [], [], []
    t0 = time.time()
    for n in numbers:
        title, start, end = chapters[n]
        folder = os.path.join(args.out_dir, names[n])
        blocks = os.path.join(folder, f"{names[n]}.blocks.json")
        counts = None
        if n in chosen and (args.force or not os.path.exists(blocks)):
            if models is None:
                models = create_model_dict()
            print(f"\n[{n}/{len(chapters)}] {title}")
            try:
                convert(args.pdf, args.out_dir, f"{start}-{end}", names[n], models)
            except Exception as e:  # keep going; report at the end
                failures.append((n, title, repr(e)))
                print(f"  FAILED: {e!r}")
                continue
        if os.path.exists(blocks):
            report = os.path.join(folder, "qa_report.json")
            if n in chosen and not args.no_qa and (args.force or not os.path.exists(report) or
                                                    os.path.getmtime(report) < os.path.getmtime(blocks)):
                counts = qa.run(folder)
                checked.append(folder)
            elif os.path.exists(report):
                counts = json.load(open(report, encoding="utf-8")).get("counts")
            rows.append((n, title, start, end, folder, counts))

    # MinerU needs the GPU Marker was using, so it runs once, after the last chapter, on every chapter's flagged pages.
    if checked and not args.no_mineru:
        import crosscheck
        print(f"\nMinerU second opinion on flagged equations ({len(checked)} chapters)")
        models = None
        new_counts = crosscheck.run(checked, release=True)
        rows = [(n, title, s, e, folder, new_counts.get(folder, counts)) for n, title, s, e, folder, counts in rows]

    meta_title = pdf.get_metadata_dict().get("Title") or os.path.splitext(os.path.basename(args.pdf))[0]
    write_index(args.out_dir, meta_title, rows)
    print(f"\nDone in {(time.time() - t0) / 60:.1f} min. Index: {os.path.join(args.out_dir, '00_index.md')}")
    for n, title, err in failures:
        print(f"  Chapter {n} ({title}) failed: {err}")


if __name__ == "__main__":
    main()
