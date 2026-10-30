from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import pymupdf

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))

from tests.fixtures.p8 import build
from pdftools.commands import p8_export, p8_qa


class ExportTests(unittest.TestCase):
    def test_recovered_text_is_inserted_beside_its_paragraph_with_pdf_page(self):
        full = ("The introduction explains the basic context. A missing sentence contains useful recovered details. "
                "The following discussion continues with additional background information.")
        words = p8_qa.prose_words(full)
        lost = [i for i, (word, _, _) in enumerate(words) if word in {"missing", "sentence", "contains", "useful", "recovered", "details"}]

        class TextPage:
            def get_text_range(self):
                return full

        class Pdf:
            def has_text_layer(self, pi): return True
            def textpage(self, pi): return TextPage()
            def size(self, pi): return (600, 800)

        data = {"pages": [{"id": "/page/4/Page/0", "block_type": "Page", "bbox": [0, 0, 600, 800], "children": []}]}
        md = "The introduction explains the basic context.\n\nThe following discussion continues with additional background information."
        with patch.object(p8_qa, "lost_runs", return_value=(words, [(lost[0], lost[-1])])):
            got, count = p8_export.recover_lost_text(md, data, Pdf())
        self.assertEqual(count, 6)
        self.assertIn("The introduction explains the basic context.\n\nmissing sentence", got)
        self.assertIn("recovered details *(recovered from PDF p. 5)*", got)
        self.assertLess(got.index("recovered from PDF"), got.index("The following discussion"))

    def test_unlocated_recovered_text_goes_in_closing_section(self):
        full = "An entirely absent passage contains seven distinct recoverable prose words here."
        words = p8_qa.prose_words(full)

        class TextPage:
            def get_text_range(self): return full

        class Pdf:
            def has_text_layer(self, pi): return True
            def textpage(self, pi): return TextPage()
            def size(self, pi): return (600, 800)

        data = {"pages": [{"id": "/page/1/Page/0", "block_type": "Page", "bbox": [0, 0, 600, 800], "children": []}]}
        with patch.object(p8_qa, "lost_runs", return_value=(words, [(0, len(words) - 1)])):
            got, count = p8_export.recover_lost_text("A completely unrelated chapter paragraph.", data, Pdf())
        self.assertEqual(count, len(words))
        self.assertIn("## Text recovered from the PDF (place not identified)", got)
        self.assertTrue(got.rstrip().endswith("*(recovered from PDF p. 2)*"))

    def test_two_column_text_in_markdown_order_is_not_recovered(self):
        with tempfile.TemporaryDirectory() as td:
            pdf_path = build(Path(td) / "book.pdf")
            pdf = p8_qa.Pdf(str(pdf_path))
            try:
                full = pdf.textpage(5).get_text_range()
                # The page's two prose columns appear in reverse order in Markdown.
                md = ("Second column paragraph also has enough words and should remain in its own column.\n\n"
                      "First column paragraph gives enough text to identify the column and preserve its words.")
                data = {"pages": [{"id": "/page/5/Page/0", "block_type": "Page", "bbox": [0, 0, 595, 842], "children": []}]}
                got, count = p8_export.recover_lost_text(md, data, pdf)
            finally:
                pdf.close()
            self.assertEqual(count, 0, full)
            self.assertEqual(got, md)

    def test_equation_qa_note_is_added_once_and_rerun_does_not_duplicate(self):
        data = {"pages": [{"children": [{"id": "/page/0/Equation/0", "block_type": "Equation",
                                           "html": "<math>x = y</math>"}]}]}
        report = {"issues": [{"type": "Equation", "visual": False, "confidence": "high", "reason": "symbols differ",
                               "block": "/page/0/Equation/0", "page": 1}]}
        md = "$$\nx = y\n$$\n\nText follows."
        once, marked = p8_export.mark_uncertain_equations(md, data, report)
        twice, marked_again = p8_export.mark_uncertain_equations(once, data, report)
        self.assertEqual(marked, 1)
        self.assertEqual(marked_again, 0)
        self.assertEqual(twice.count("possible OCR error in this equation"), 1)

    def test_mdexport_marks_equations_from_blocks_and_honors_title_precedence(self):
        import argparse
        from pdftools.commands import p8_md
        with tempfile.TemporaryDirectory() as td:
            base = Path(td); root = base / "book"; root.mkdir()
            source = root / "source-file.pdf"
            doc = pymupdf.open(); doc.new_page(); doc.set_metadata({"title": "Metadata Title"}); doc.save(source); doc.close()
            part = root / "01_chapter"; part.mkdir()
            equation_id = "/page/0/Equation/0"
            block = {"id": equation_id, "block_type": "Equation", "bbox": [40, 40, 200, 80],
                     "html": "<math>x = 1</math>"}
            page = {"id": "/page/0/Page/0", "block_type": "Page", "bbox": [0, 0, 595, 842],
                    "children": [{"id": equation_id, "block_type": "Equation", "bbox": block["bbox"], "eq": "e1"}]}
            (part / "01_chapter.md").write_text("$$\nx = 1\n$$\n", encoding="utf-8")
            (part / "01_chapter.blocks.json").write_text(json.dumps({"source_pdf": str(source), "pages": [
                {**page, "children": [block]}]}), encoding="utf-8")
            (part / "01_chapter.pages.json").write_text(json.dumps({"source_pdf": str(source), "ocr_pages": [],
                                                                       "pages": [page]}), encoding="utf-8")
            (part / "qa_report.json").write_text(json.dumps({"issues": [{"type": "Equation", "visual": False,
                "confidence": "high", "reason": "symbols differ", "block": equation_id, "page": 1}]}), encoding="utf-8")
            (part / "eqs.json").write_text("[]", encoding="utf-8")
            class FakePdf:
                def __enter__(self): return self
                def __exit__(self, *args): pass
            with patch.object(p8_qa, "Pdf", return_value=FakePdf()), \
                 patch.object(p8_export, "recover_lost_text", side_effect=lambda md, data, pdf: (md, 0)):
                selected = p8_export.chapters(str(root))
                result = p8_md._mdexport_impl(argparse.Namespace(title=None), selected, base / "default",
                                             {str(part): str(source)}, [])
                default_text = (base / "default" / "01_chapter.md").read_text(encoding="utf-8")
                self.assertIn("possible OCR error in this equation", default_text)
                self.assertTrue(default_text.startswith("> Source: Metadata Title"))
                p8_md._mdexport_impl(argparse.Namespace(title="Author, Book"), selected, base / "override",
                                     {str(part): str(source)}, [])
                override_text = (base / "override" / "01_chapter.md").read_text(encoding="utf-8")
                self.assertTrue(override_text.startswith("> Source: Author, Book"))
                with pymupdf.open(source) as changed:
                    changed.set_metadata({"title": ""})
                    changed.save(base / "source-no-title.pdf")
                (part / "01_chapter.blocks.json").write_text(
                    json.dumps({"source_pdf": str(base / "source-no-title.pdf"), "pages": [{**page, "children": [block]}]}),
                    encoding="utf-8")
                (part / "01_chapter.pages.json").write_text(
                    json.dumps({"source_pdf": str(base / "source-no-title.pdf"), "ocr_pages": [], "pages": [page]}),
                    encoding="utf-8")
                selected = p8_export.chapters(str(root))
                p8_md._mdexport_impl(argparse.Namespace(title=None), selected, base / "filename",
                                     {str(part): str(base / "source-no-title.pdf")}, [])
                filename_text = (base / "filename" / "01_chapter.md").read_text(encoding="utf-8")
                self.assertTrue(filename_text.startswith("> Source: source-no-title"))

    def test_clean_drops_images_html_comments_and_orcid_links(self):
        raw = ("![diagram](images/diagram.png)\n\nText <sup>2</sup> and <sub>long</sub> "
               "<span class=\"x\">span</span><br><b>bold</b><i>italics</i> "
               "[Author](https://orcid.org/0000-0000-0000-0000) <!--review-->\n")
        got = p8_export.clean(raw)
        self.assertNotIn("images/diagram.png", got)
        self.assertNotIn("<", got)
        self.assertNotIn("orcid.org", got)
        self.assertNotIn("review", got)
        self.assertIn("Text ^2 and _(long) span **bold***italics* Author", got)

    def test_export_record_has_source_pages_and_per_file_token_estimate(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td) / "book"
            root.mkdir()
            source_pdf = root / "source.pdf"
            doc = pymupdf.open(); doc.new_page(); doc.save(source_pdf); doc.close()
            part = root / "02_chapter_one"; part.mkdir()
            (part / "02_chapter_one.md").write_text("Chapter body.\n", encoding="utf-8")
            data = {"source_pdf": str(source_pdf), "pages": [{"id": "/page/3/Page/0", "bbox": [0, 0, 612, 792], "children": []}]}
            (part / "02_chapter_one.blocks.json").write_text(json.dumps({"pages": data["pages"], "source_pdf": str(source_pdf)}), encoding="utf-8")
            (part / "02_chapter_one.pages.json").write_text(json.dumps(data), encoding="utf-8")
            (part / "eqs.json").write_text("[]", encoding="utf-8")
            out = root.parent / "project"
            with patch.object(p8_qa, "Pdf") as pdf_ctor, patch.object(p8_export, "recover_lost_text", return_value=("Chapter body.\n", 0)):
                result = __import__("pdftools.commands.p8_md", fromlist=["mdexport"]).mdexport(
                    __import__("argparse").Namespace(folder=root, output=out, skip="index", title=None), None)
            pdf_ctor.assert_called_once()
            self.assertEqual([row[1] for row in p8_export.chapters(str(root))], ["chapter one"])
            exported = (out / "02_chapter_one.md").read_text(encoding="utf-8")
            self.assertIn("PDF p. 4", exported)
            self.assertEqual(result["files"][0]["file"], "02_chapter_one.md")
            self.assertGreater(result["files"][0]["tokens"], 0)
            self.assertEqual(result["tokens"], result["files"][0]["tokens"])

    def test_default_skip_matches_index_within_chapter_name(self):
        # p8_md.mdexport skips only an exact basename, not names containing the default skip word as pdf2md does.
        with tempfile.TemporaryDirectory() as td:
            root = Path(td) / "book"; root.mkdir()
            pdf = root / "source.pdf"
            doc = pymupdf.open(); doc.new_page(); doc.save(pdf); doc.close()
            part = root / "01_index_of_terms"; part.mkdir()
            (part / "01_index_of_terms.md").write_text("index body", encoding="utf-8")
            page = {"id": "/page/0/Page/0", "block_type": "Page", "bbox": [0, 0, 600, 800], "children": []}
            common = {"source_pdf": str(pdf), "pages": [page]}
            (part / "01_index_of_terms.blocks.json").write_text(json.dumps(common), encoding="utf-8")
            (part / "01_index_of_terms.pages.json").write_text(json.dumps(common), encoding="utf-8")
            from pdftools.commands.p8_md import mdexport
            with patch.object(p8_qa, "Pdf"):
                result = mdexport(__import__("argparse").Namespace(folder=root, output=root.parent / "out", skip="index", title=None), None)
            self.assertEqual(result["files"], [])


if __name__ == "__main__":
    unittest.main()
