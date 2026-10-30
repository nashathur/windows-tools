"""Small PDFs for validator hardening attacks; generated directly with PyMuPDF/pikepdf."""
from pathlib import Path
import pymupdf


def build_fixtures(folder):
    folder = Path(folder); folder.mkdir(parents=True, exist_ok=True)
    source = folder / "source.pdf"
    doc = pymupdf.open()
    for label in ("exam- ple x - y", "", "second page"):
        page = doc.new_page(width=300, height=400)
        if label: page.insert_text((40, 80), label)
    doc[1].insert_text((40, 80), "value x", fontsize=11)
    doc[1].insert_text((78, 84), "2", fontsize=6)
    doc[1].insert_text((40, 120), " - 1", fontsize=11)
    doc.set_toc([[1, "Start", 1]])
    doc[0].insert_link({"kind": pymupdf.LINK_GOTO, "from": pymupdf.Rect(40, 100, 90, 115), "page": 1})
    widget = pymupdf.Widget(); widget.field_name = "answer"; widget.field_type = pymupdf.PDF_WIDGET_TYPE_TEXT
    widget.field_value = "42"; widget.rect = pymupdf.Rect(40, 130, 140, 150); doc[0].add_widget(widget)
    doc.save(source); doc.close()
    return {"source": source}
