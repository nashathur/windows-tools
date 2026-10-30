from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
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
from pdftools.commands import p8_eqread, p8_md, p8_qa
from pdftools.cli import Context

PDF = ROOT / "pdf.py"
NODE_PATH = "/home/agent/src/windows-tools/pdftools/node_modules"


class P8OcrTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def _cli(self, *args):
        env = os.environ.copy()
        env["NODE_PATH"] = NODE_PATH
        return subprocess.run([sys.executable, str(PDF), "--json", *map(str, args)], cwd=ROOT,
                              env=env, text=True, capture_output=True)

    def test_scanned_page_ocr_is_cached_and_low_dpi_is_reported_per_part(self):
        source = build(self.root / "scan.pdf")
        output = self.root / "scan_md"
        calls = []

        def fake_ocr(input_path, destination, args, context):
            calls.append((Path(input_path), Path(destination), args.clean))
            shutil.copyfile(input_path, destination)
            return {"warnings": ["OCR_LOW_QUALITY"], "page_measurements": [{"page": 8, "dpi": 100}]}

        with patch.object(p8_md, "_run_ocr", side_effect=fake_ocr):
            first = p8_md.md(self._md_args(source, output), Context())
            self.assertIn("OCR_LOW_QUALITY", first["warnings"])
            self.assertTrue(any("OCR_LOW_QUALITY" in part.get("warnings", []) for part in first["parts"]))
            self.assertEqual(len(calls), 1)

            resumed = p8_md.md(self._md_args(source, output), Context())
            self.assertIn("OCR_LOW_QUALITY", resumed["warnings"])
            self.assertTrue(any(part.get("warnings") == ["OCR_LOW_QUALITY"] for part in resumed["parts"]))
            self.assertEqual(len(calls), 1)

            part_folder = next(output.glob("*/eqs.json")).parent
            (part_folder / "eqs.json").write_text("[]", encoding="utf-8")
            source_paths = {str(part_folder): str(output / "source_ocr.pdf")}
            from types import SimpleNamespace
            def write_qa(path, source_pdf=None):
                (Path(path) / "qa_report.json").write_text(json.dumps({"issues": []}), encoding="utf-8")
                (Path(path) / "qa_report.md").write_text("# QA report\n", encoding="utf-8")
                return {}
            with patch.object(p8_qa, "run", side_effect=write_qa):
                read_result = p8_md._mdread_impl(SimpleNamespace(folder=output), Context(), [part_folder], source_paths)
                self.assertEqual(read_result["verdict"], "warn")
                self.assertIn("OCR_LOW_QUALITY", read_result["warnings"])
                report = json.loads((part_folder / "qa_report.json").read_text(encoding="utf-8"))
                self.assertIn("OCR_LOW_QUALITY", [code for item in report["ocr_quality"].values()
                                                   for code in item["warnings"]])
                qa_result = p8_md._mdqa_impl(SimpleNamespace(folder=output), Context(), [part_folder], source_paths)
                self.assertEqual(qa_result["verdict"], "warn")
                self.assertIn("OCR_LOW_QUALITY", qa_result["warnings"])
            report = json.loads((part_folder / "qa_report.json").read_text(encoding="utf-8"))
            self.assertIn("OCR_LOW_QUALITY", [code for item in report["ocr_quality"].values()
                                               for code in item["warnings"]])

            second = p8_md.md(self._md_args(source, output, "--force"), Context())
            self.assertIn("OCR_LOW_QUALITY", second["warnings"])
            self.assertEqual(len(calls), 1)

            with pymupdf.open(source) as doc:
                doc.set_metadata({"title": "Changed scan source"})
                changed = self.root / "changed.pdf"
                doc.save(changed)
            changed.replace(source)
            manifest = json.loads((output / ".pdftools-md.json").read_text(encoding="utf-8"))
            manifest["sha256"] = hashlib.sha256(source.read_bytes()).hexdigest()
            manifest["size"] = source.stat().st_size
            (output / ".pdftools-md.json").write_text(json.dumps(manifest), encoding="utf-8")
            p8_md.md(self._md_args(source, output, "--force"), Context())
            self.assertEqual(len(calls), 2)

    def test_ocr_equations_are_judged_blind_and_need_two_agreeing_reads(self):
        pdf = type("Pdf", (), {"ocr_pages": {3}})()
        equation = {"page": 3, "box": [0, 0, 10, 10], "pdf_text": "misleading text"}
        with patch("pdftools.mathcheck.validate", return_value={}), \
             patch.object(p8_qa, "check_equation", side_effect=AssertionError("OCR equation was compared")):
            judged = p8_eqread.judge(pdf, equation, "x = 1")
        self.assertFalse(judged["checked"])
        self.assertTrue(judged["unchecked_reason"])
        state = {"problems": [], "checked": False,
                 "readings": [{"by": "pass1", "latex": "x = 1"}, {"by": "pass2", "latex": "x = 1"}]}
        self.assertFalse(p8_eqread.doubtful(state))
        state["readings"][-1]["latex"] = "x = 2"
        self.assertTrue(p8_eqread.doubtful(state))
        equation.update(blind=True, _label=1, id="p3_1", bold="")
        prompt = p8_eqread.prompt_text(".", [], [equation], "pass1")
        self.assertIn("PDF text: (none: scanned page)", prompt)
        self.assertNotIn("misleading text", prompt)

    def test_md_passes_clean_option_to_phase_five_ocr(self):
        source = build(self.root / "clean.pdf")
        parser, args = self._parsed_md(source, self.root / "clean_md", "--clean")
        self.assertTrue(args.clean)
        from pdftools.commands.p8_md import _run_ocr
        with patch("pdftools.commands.p5_ocr.ocr", return_value={"warnings": []}) as ocr:
            _run_ocr(source, self.root / "ocr.pdf", args, Context())
        self.assertTrue(ocr.call_args.args[0].clean)

    def test_ocr_equation_crops_keep_published_render_pixels_with_and_without_clean(self):
        from PIL import Image
        from pdftools.commands import p8_extract
        source = ROOT / "tests" / "fixtures" / "data" / "p8_typeset.pdf"
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            plain_page = root / "plain-page.pdf"
            clean_page = root / "clean-page.pdf"
            shutil.copyfile(source, plain_page)
            shutil.copyfile(source, clean_page)
            plain_folder = Path(p8_extract.convert(str(plain_page), str(root), "0-0", "plain", read=False,
                                                   ocr_pages=[0]))
            clean_folder = Path(p8_extract.convert(str(clean_page), str(root), "0-0", "clean", read=False,
                                                   ocr_pages=[0]))
            plain_eqs = json.loads((plain_folder / "eqs.json").read_text(encoding="utf-8"))
            clean_eqs = json.loads((clean_folder / "eqs.json").read_text(encoding="utf-8"))
            self.assertGreaterEqual(len(plain_eqs), 3)
            for before, after in zip(plain_eqs, clean_eqs):
                with Image.open(plain_folder / before["crop"]) as a, Image.open(clean_folder / after["crop"]) as b:
                    self.assertEqual(a.size, b.size)
                    self.assertEqual(list(a.getdata()), list(b.getdata()))
                    self.assertGreater(len(set(a.getdata())), 2, "crop appears thresholded")

    def _parsed_md(self, source, output, *flags):
        import argparse
        parser = argparse.ArgumentParser()
        p8_md.add_arguments_md(parser)
        args = parser.parse_args([str(source), "-o", str(output), *flags])
        args.dry_run = False
        return parser, args

    def _md_args(self, source, output, *flags):
        return self._parsed_md(source, output, "--no-read", "--no-qa", *flags)[1]


if __name__ == "__main__":
    unittest.main()
