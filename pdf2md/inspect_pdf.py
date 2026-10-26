"""Quick look at a PDF before converting: page count, text layer per sample page, and the outline (TOC).

Run in the pdfmath env:
    conda run -n pdfmath python C:\\Tools\\pdf2md\\inspect_pdf.py book.pdf [--toc-depth 2]
"""

import argparse
import logging
import os
import sys
from pathlib import Path

import pypdfium2 as pdfium

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "common"))
import clierr


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pdf")
    ap.add_argument("--toc-depth", type=int, default=2)
    args = ap.parse_args()

    if not os.path.isfile(args.pdf):
        raise clierr.Fail("NOT_FOUND", f"{args.pdf} not found", "check the PDF path")
    pdf = pdfium.PdfDocument(args.pdf)
    n = len(pdf)
    print(f"pages: {n}")
    print("metadata:", {k: v for k, v in pdf.get_metadata_dict().items() if v})
    w, h = pdf[0].get_size()
    print(f"page size: {w:.0f} x {h:.0f} pt")

    # Text layer: count pages with almost no extractable text (scans or figure-only pages).
    empty = []
    for i in range(n):
        if len(pdf[i].get_textpage().get_text_range().strip()) < 50:
            empty.append(i + 1)
    print(f"pages with <50 chars of text: {len(empty)}" + (f" -> {empty[:40]}{' …' if len(empty) > 40 else ''}" if empty else ""))
    for i in sorted({0, 2, 10, 40, n // 2, n - 5}):
        if 0 <= i < n:
            t = " ".join(pdf[i].get_textpage().get_text_range().split())
            print(f"  p{i + 1}: {t[:110]!r}")

    logging.getLogger("pypdfium2").setLevel(logging.ERROR)  # silence "max recursion depth reached" for deeper TOC levels
    toc = list(pdf.get_toc(max_depth=args.toc_depth))
    print(f"outline entries (depth <= {args.toc_depth}): {len(toc)}   [PDF page, 1-based; 0-indexed = minus 1]")
    for item in toc:
        dest = item.get_dest()
        page = dest.get_index() + 1 if dest is not None and dest.get_index() is not None else "?"
        print("  " * item.level + f"{item.get_title()}  -> p{page}")


if __name__ == "__main__":
    clierr.run("inspect_pdf", main)
