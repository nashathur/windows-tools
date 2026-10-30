import contextlib
import io
import json
import tempfile
import unittest
import sys
from pathlib import Path
from unittest.mock import patch

import pymupdf

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT)); sys.path.insert(0, str(ROOT.parent / "common"))

from pdftools.chapters import plan
from pdftools.commands.p3b_multi import _split_parts
from pdftools.errors import PdfError
from pdftools import cli
import clierr


def book(pages=70):
    doc = pymupdf.open()
    for i in range(pages):
        page = doc.new_page()
        if i in (0, 1, 2, 3, 32, 33, 58, 59, 60):
            label = ("Front matter " if i in (0, 1) else "Volume One " if i == 2 else
                     "Chapter Three " if i == 3 else "Chapter Four " if i == 33 else
                     "Volume Two " if i == 60 else "Chapter " + str(i) + " ")
            page.insert_text((50, 60), label)
            for line in range(20):
                page.insert_text((50, 75 + line * 11), "word " * 10)
    # The first chapter link is one page early; each level-1 part has one level-2 chapter.
    doc.set_toc([[1, "Volume One", 3], [2, "Chapter Three", 3],
                 [2, "Chapter Four", 33], [1, "Volume Two", 61]])
    return doc


class ChapterTests(unittest.TestCase):
    def test_plan_repairs_and_cover_all(self):
        doc = book()
        result = plan(doc, level=2, cover_all=True)
        self.assertTrue(result["notes"])
        pages = [p for part in result["parts"] for p in range(part["start"], part["end"] + 1)]
        self.assertEqual(pages, list(range(doc.page_count)))
        self.assertEqual(len(pages), len(set(pages)))
        # Chapter Four's outline page (index 32) is Chapter Three's last page: it stays there after the repair.
        parts = {part["title"]: (part["start"], part["end"]) for part in result["parts"]}
        self.assertEqual(parts["Front matter"], (0, 2))
        self.assertEqual(parts["Chapter Three"], (3, 32))
        self.assertEqual(parts["Chapter Four"][0], 33)
        self.assertEqual(plan(doc, level=2)["parts"][1]["end"], 32)

    def test_auto_and_no_outline(self):
        doc = book()
        automatic = plan(doc, level=None)
        self.assertEqual([part["title"] for part in automatic["parts"]],
                         ["Front matter", "Chapter Three", "Chapter Four"])
        empty = pymupdf.open(); empty.new_page()
        with self.assertRaises(PdfError) as caught: plan(empty)
        self.assertEqual(caught.exception.code, "BAD_INPUT")

    def test_split_parts_bookmarks(self):
        doc = book()
        args = type("Args", (), {"every": None, "ranges": None, "bookmarks": True, "level": "auto"})()
        parts = _split_parts(doc, args)
        self.assertEqual(sum(len(pages) for _, pages in parts), doc.page_count)
        self.assertTrue(args._chapter_notes)

    def test_split_command_auto_reports_repair_and_parts(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source, output = root / "outline.pdf", root / "parts"
            doc = book(); doc.save(source); doc.close()
            stdout, stderr = io.StringIO(), io.StringIO()
            with patch("clierr.LOG_ROOT", root / "logs"), contextlib.redirect_stdout(stdout), \
                    contextlib.redirect_stderr(stderr):
                code = cli.main(["--json", "split", str(source), "--bookmarks", "--level", "auto",
                                 "-o", str(output) + "/"])
            result = json.loads(stdout.getvalue())
            self.assertEqual(code, 0, result)
            self.assertNotEqual(result["verdict"], "unchecked")
            self.assertIn("OUTLINE_REPAIRED", result["warnings"])
            self.assertEqual(len(result["page_map"]), 3)

    def test_extract_keeps_chapter_whose_parent_is_dropped(self):
        # PyMuPDF select drops an outline entry whose parent points outside the kept pages.
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source, output = root / "outline.pdf", root / "tail.pdf"
            doc = book(); doc.save(source); doc.close()
            stdout = io.StringIO()
            with patch("clierr.LOG_ROOT", root / "logs"), contextlib.redirect_stdout(stdout), \
                    contextlib.redirect_stderr(io.StringIO()):
                code = cli.main(["--json", "extract", str(source), "--pages", "33-", "-o", str(output)])
            self.assertEqual(code, 0, stdout.getvalue())
            with pymupdf.open(output) as result:
                self.assertEqual([row[1] for row in result.get_toc()], ["Chapter Four", "Volume Two"])


if __name__ == "__main__": unittest.main()
