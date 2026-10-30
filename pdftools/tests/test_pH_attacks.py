import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
import sys
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT)); sys.path.insert(0, str(ROOT.parent / "common"))
import pymupdf
from pdftools import cli
from pdftools.check import Features, OperationResult, Placement, compare, feature_records, normalize_words
from pdftools.pipeline import write_pdf
from pdftools.registry import command, get
from pdftools.check import _record_equal
from tests.fixtures.pH import build_fixtures


def add_arguments(parser):
    parser.add_argument("file", type=Path); parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--kind", choices=("bookmark", "link", "widget"), default="bookmark")


def _raw_chars(page):
    return [char["c"] for block in page.get_text("rawdict")["blocks"]
            for line in block.get("lines", []) for span in line.get("spans", [])
            for char in span.get("chars", [])]


def _glyph_rect(path, page_no, character, occurrence=0):
    with pymupdf.open(path) as doc:
        page = doc[page_no]
        boxes = [char["bbox"] for block in page.get_text("rawdict")["blocks"]
                 for line in block.get("lines", []) for span in line.get("spans", [])
                 for char in span.get("chars", []) if char["c"] == character]
    box = pymupdf.Rect(boxes[occurrence])
    return pymupdf.Rect(box.x0 + 0.05, box.y0 + 0.05, box.x1 - 0.05, box.y1 - 0.05)


def _attack(args, context, kind):
    source = Path(args.file)
    def writer(doc, staged):
        if kind == "truncated":
            staged.write_bytes(b"%PDF-1.7 broken")
        else:
            if kind == "blank":
                page = doc[0]; page.add_redact_annot(page.rect, fill=(1, 1, 1)); page.apply_redactions()
            if kind == "wrong":
                doc.delete_page(0); doc.insert_page(0, text="wrong page")
            if kind == "minus":
                page = doc[0]; page.add_redact_annot(_glyph_rect(source, 0, "-", 1), fill=False); page.apply_redactions()
            if kind == "subscript":
                page = doc[1]; page.add_redact_annot(_glyph_rect(source, 1, "2"), fill=False); page.apply_redactions()
            doc.save(staged)
    op = OperationResult(pages=[[Placement(str(source), i)] for i in range(3)])
    return write_pdf(source, get("pH-" + kind), args, context, writer, operation=op)


def _feature_attack(args, context):
    source = Path(args.file)
    with pymupdf.open(source) as doc:
        records = feature_records(doc)
    def writer(doc, staged):
        if args.kind == "bookmark": doc.set_toc([[1, "Start", 2]])
        elif args.kind == "link":
            link = doc[0].get_links()[0]; link["page"] = 2; doc[0].update_link(link)
        else:
            page = doc[0]; widget = next(page.widgets()); widget.field_value = "43"; widget.update()
        doc.save(staged)
    op = OperationResult(pages=[[Placement(str(source), i)] for i in range(len(records["bookmarks"])*0 + 3)],
                         features=Features(records=records))
    return write_pdf(source, get("pH-feature"), args, context, writer, operation=op)


for _name in ("truncated", "blank", "wrong", "minus", "subscript"):
    command("pH-" + _name, "2", "pdf", "pdf", "_" + _name, True, "placements")(
        lambda args, context, kind=_name: _attack(args, context, kind))

command("pH-feature", "2", "pdf", "pdf", "_feature", True, "placements")(_feature_attack)


@command("pH-extractor", "2", "pdf", "pdf", "_extractor", True, "placements")
def extractor_attack(args, context):
    source = Path(args.file)
    def writer(doc, staged): doc.save(staged)
    op = OperationResult(pages=[[Placement(str(source), i)] for i in range(3)])
    return write_pdf(source, get("pH-extractor"), args, context, writer, operation=op)


class AttacksTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.root = Path(self.tmp.name)
        self.source = build_fixtures(self.root / "fixtures")["source"]
    def tearDown(self): self.tmp.cleanup()

    def test_minus_and_hyphenation_normalization(self):
        self.assertEqual(normalize_words(["exam-", "ple", "x", "-", "y"]), ("example", "x", "-", "y"))
        self.assertIn("x", normalize_words(["x", "-", "y"]))

    def test_bad_writer_candidates_are_never_ok(self):
        for kind in ("truncated", "blank", "wrong", "minus", "subscript"):
            with self.subTest(kind=kind):
                out, err = io.StringIO(), io.StringIO()
                out, err = io.StringIO(), io.StringIO()
                with patch("clierr.LOG_ROOT", self.root / "logs"), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                    code = cli.main(["--json", "pH-" + kind, str(self.source), "-o", str(self.root / (kind + ".pdf"))])
                record = json.loads(out.getvalue())
                if kind == "truncated":
                    self.assertEqual(code, 1, (kind, record, err.getvalue()))
                    self.assertEqual(record["code"], "BAD_INPUT", (record, err.getvalue(), Path(record["log"]).read_text()))
                    self.assertFalse((self.root / (kind + "_UNCHECKED.pdf")).exists())
                else:
                    self.assertEqual(code, 3, (kind, record, err.getvalue()))
                    self.assertEqual(record["verdict"], "unchecked")
                    self.assertTrue(Path(record["path"]).stem.endswith("_UNCHECKED"))
                    self.assertNotIn("measurement_log", record)
                    self.assertTrue(Path(record["log"]).is_file())
                    measurement_rows = json.loads(Path(record["log"]).read_text().split("== measurements ==\n", 1)[1].split("\n\n", 1)[0])
                    self.assertEqual([row["page"] for row in measurement_rows], [1, 2, 3])

    def test_minus_and_subscript_attacks_remove_only_the_selected_glyph(self):
        original = __import__("pdftools.pipeline", fromlist=["compare"]).compare
        for kind, page_no, char, occurrence in (("minus", 0, "-", 1), ("subscript", 1, "2", 0)):
            with self.subTest(kind=kind):
                out, err = io.StringIO(), io.StringIO()
                def inspect(source, output, *args, _page_no=page_no, _char=char, _occurrence=occurrence, **kwargs):
                    before, after = _raw_chars(source[_page_no]), _raw_chars(output[_page_no])
                    index = [n for n, value in enumerate(before) if value == _char][_occurrence]
                    expected = before[:index] + before[index + 1:]
                    self.assertEqual([char for char in after if not char.isspace()],
                                     [char for char in expected if not char.isspace()], (before, after, index))
                    return original(source, output, *args, **kwargs)
                with patch("pdftools.pipeline.compare", side_effect=inspect), patch("clierr.LOG_ROOT", self.root / "logs"), \
                        contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                    code = cli.main(["--json", "pH-" + kind, str(self.source), "-o", str(self.root / (kind + "-assert.pdf"))])
                self.assertEqual(code, 3, (kind, code, out.getvalue(), err.getvalue(),
                                           Path(json.loads(out.getvalue())["log"]).read_text() if out.getvalue() else ""))

    def test_unreadable_page_is_invariant(self):
        with pymupdf.open(self.source) as source, pymupdf.open(self.source) as output:
            original = pymupdf.Page.get_text
            def broken(page, *args, **kwargs):
                if str(page.parent.this) == str(output.this): raise RuntimeError("broken extractor")
                return original(page, *args, **kwargs)
            with patch.object(pymupdf.Page, "get_text", broken):
                findings = compare(source, output, "placements", OperationResult(pages=[[Placement(None, 0)]]))
        self.assertTrue(any(f.code == "PAGE_UNREADABLE" and f.tier == "invariant" for f in findings), findings)

    def test_source_non_words_extractor_failure_is_unreadable(self):
        with pymupdf.open(self.source) as source, pymupdf.open(self.source) as output:
            original = pymupdf.Page.get_text
            def broken(page, option="text", *args, **kwargs):
                if str(page.parent.this) == str(source.this) and option == "dict":
                    raise RuntimeError("source dict extractor failed")
                return original(page, option, *args, **kwargs)
            with patch.object(pymupdf.Page, "get_text", broken):
                findings = compare(source, output, "placements", OperationResult(pages=[[Placement(None, 0)]]))
        self.assertIn("PAGE_UNREADABLE", [finding.code for finding in findings])
        self.assertNotIn("TEXT_CHANGED", [finding.code for finding in findings])

    def test_pipeline_publishes_unreadable_extractor_result_as_unchecked(self):
        original = pymupdf.Page.get_text
        def broken(page, *args, **kwargs):
            if str(page.parent.name) != str(self.source.resolve()): raise RuntimeError("broken extractor")
            return original(page, *args, **kwargs)
        out, err = io.StringIO(), io.StringIO()
        with patch.object(pymupdf.Page, "get_text", broken), patch("clierr.LOG_ROOT", self.root / "logs"), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(["--json", "pH-extractor", str(self.source), "-o", str(self.root / "extractor.pdf")])
        record = json.loads(out.getvalue())
        self.assertEqual(code, 3, (record, err.getvalue()))
        self.assertEqual(record["verdict"], "unchecked")
        self.assertTrue(Path(record["path"]).stem.endswith("_UNCHECKED"))
        self.assertIn("PAGE_UNREADABLE", [finding["code"] for finding in record["findings"]])

    def test_feature_record_mismatch_is_invariant(self):
        with pymupdf.open(self.source) as source, pymupdf.open(self.source) as output:
            op = OperationResult(pages=[[Placement(None, i)] for i in range(3)],
                                 features=Features(records={"bookmarks": [{"title": "Start", "level": 1, "page": 2}]}))
            findings = compare(source, output, "placements", op)
        self.assertIn("FEATURE_RECORD_MISMATCH", [f.code for f in findings])

    def test_wrong_semantic_records_exit_three(self):
        for kind in ("bookmark", "link", "widget"):
            with self.subTest(kind=kind):
                out, err = io.StringIO(), io.StringIO()
                with patch("clierr.LOG_ROOT", self.root / "logs"), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                    code = cli.main(["--json", "pH-feature", str(self.source), "--kind", kind,
                                     "-o", str(self.root / (kind + ".pdf"))])
                record = json.loads(out.getvalue())
                self.assertEqual(code, 3, (record, err.getvalue()))
                self.assertEqual(record["verdict"], "unchecked")
                self.assertIn("FEATURE_RECORD_MISMATCH", [f["code"] for f in record["findings"]])

    def test_numbers_detects_writer_retargeting_a_link(self):
        original_save = pymupdf.Document.save
        def retarget(doc, path, *args, **kwargs):
            if doc.page_count and doc[0].get_links():
                link = doc[0].get_links()[0]
                link["page"] = 0 if link.get("page") != 0 else 1
                doc[0].update_link(link)
            return original_save(doc, path, *args, **kwargs)
        out, err = io.StringIO(), io.StringIO()
        with patch.object(pymupdf.Document, "save", retarget), patch("clierr.LOG_ROOT", self.root / "logs"), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(["--json", "numbers", str(self.source), "-o", str(self.root / "numbered.pdf")])
        record = json.loads(out.getvalue())
        self.assertEqual(code, 3, (record, err.getvalue()))
        self.assertIn("FEATURE_RECORD_MISMATCH", [row["code"] for row in record["findings"]])
