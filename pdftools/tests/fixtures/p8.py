"""Deterministic Phase 8 PDF fixtures, created with PyMuPDF and Pillow only."""
from __future__ import annotations

from pathlib import Path

import pymupdf


def build(path):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    doc = pymupdf.open()
    # Front matter with enough content to survive the shared chapter planner's coverage rule.
    page = doc.new_page()
    page.insert_text((72, 72), "Introduction to a Sample Book", fontsize=20)
    for y in range(100, 260, 18):
        page.insert_text((72, y), "Front matter provides context for this illustrated sample book.", fontsize=10)
    page = doc.new_page()
    page.insert_text((72, 72), "Preface", fontsize=18)
    for y in range(110, 270, 20):
        page.insert_text((72, y), "The preface explains the subject and the organization of the book.", fontsize=10)

    page = doc.new_page()
    page.insert_text((72, 72), "Chapter One", fontsize=20)
    page.insert_text((90, 160), "The vector equation describes a simple physical process.", fontsize=11)
    page.insert_text((180, 240), "dX/dt = -alpha X", fontsize=20)
    page.insert_text((470, 240), "(1.1)", fontsize=11)
    page.insert_text((90, 290), "A bold vector follows: u = v + w.", fontsize=11)
    page.insert_text((72, 360), "| Quantity | Value |\n|---|---|\n| alpha | 2 |\n| beta | 3 |", fontsize=10)
    page.insert_text((72, 470), "Figure 1.1: A short illustrative caption.", fontsize=10)

    # A standalone, centered, numbered display equation gives the layout model its clearest fixture case.
    page = doc.new_page()
    page.insert_text((165, 400), "∫ x² dx = 1/3", fontsize=30)
    page.insert_text((485, 400), "(1.2)", fontsize=12)

    page = doc.new_page()
    page.insert_text((72, 72), "Part Title: Methods", fontsize=19)
    page.insert_text((72, 130), "This page introduces the methods used in the next chapter.", fontsize=11)

    page = doc.new_page()
    page.insert_text((72, 72), "Chapter Two", fontsize=20)
    page.insert_text((72, 160), "First column paragraph gives enough text to identify the column and preserve its words.", fontsize=10)
    page.insert_text((320, 160), "Second column paragraph also has enough words and should remain in its own column.", fontsize=10)
    page.insert_text((180, 260), "x + y = z", fontsize=22)
    page.insert_text((470, 260), "(2.1)", fontsize=11)

    page = doc.new_page()
    page.insert_text((72, 72), "Rotated equation", fontsize=16)
    page.insert_text((90, 200), "F = ma", fontsize=22)
    page.set_rotation(90)

    # A clear scanned page made from a rendered PyMuPDF page, embedded as pixels.
    source_page = doc.new_page(width=595, height=842)
    source_page.insert_text((72, 100), "Scanned table", fontsize=18)
    source_page.draw_rect(pymupdf.Rect(72, 140, 360, 250), width=1)
    source_page.draw_line((220, 140), (220, 250), width=1)
    source_page.draw_line((72, 175), (360, 175), width=1)
    source_page.draw_line((72, 212), (360, 212), width=1)
    for text, pos in (("Name", (90, 165)), ("Value", (240, 165)), ("alpha", (90, 200)),
                      ("2", (240, 200)), ("beta", (90, 238)), ("3", (240, 238))):
        source_page.insert_text(pos, text, fontsize=12)
    source_page.insert_text((150, 330), "q = r + s", fontsize=20)
    source_page.insert_text((470, 330), "(3.1)", fontsize=11)
    pix = source_page.get_pixmap(matrix=pymupdf.Matrix(1.5, 1.5), alpha=False)
    from PIL import Image
    image_path = path.with_suffix(".png")
    pix.save(image_path)
    pix = None
    scanned = doc.new_page(width=595, height=842)
    scanned.insert_image(scanned.rect, filename=str(image_path))
    doc.delete_page(doc.page_count - 2)

    typeset = pymupdf.open(Path(__file__).parent / "data" / "p8_typeset.pdf")
    doc.insert_pdf(typeset, start_at=doc.page_count)
    doc.insert_pdf(typeset, start_at=doc.page_count)
    doc[-1].set_rotation(90)
    typeset.close()
    # Chapter entries point one page early so repair is exercised.
    doc.set_toc([[1, "Chapter One", 2], [1, "Chapter Two", 5]])
    doc.set_metadata({"title": "Phase 8 Sample Book", "author": "pdftools tests"})
    doc.save(path)
    doc.close()
    image_path.unlink(missing_ok=True)
    return path


def build_fixtures(folder):
    folder = Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    return {"book": build(folder / "book.pdf")}


def build_typeset_variants(folder):
    """Make coordinate stress copies of the typeset equation page."""
    folder = Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    source = pymupdf.open(Path(__file__).parent / "data" / "p8_typeset.pdf")
    result = {}
    for name, keys in (
        ("userunit", {"UserUnit": "2"}),
        ("rotated_crop", {"CropBox": "[20 100 575 800]", "Rotate": "270"}),
    ):
        doc = pymupdf.open()
        doc.insert_pdf(source, from_page=0, to_page=0)
        page = doc[0]
        for key, value in keys.items():
            doc.xref_set_key(page.xref, key, value)
        path = folder / f"{name}.pdf"
        doc.save(path)
        doc.close()
        result[name] = path
    source.close()
    return result
