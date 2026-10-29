import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
import sys
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))

from PIL import Image
import numpy as np
import pymupdf
from pdftools import cli
from pdftools.commands import p2_selftest
from tests.fixtures.p2 import build_fixtures


class Phase2CommandTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.fixtures = build_fixtures(self.root / "fixtures")

    def tearDown(self):
        self.tmp.cleanup()

    def run_cli(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with patch("clierr.LOG_ROOT", self.root / "logs"), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(["--json", *map(str, argv)])
        return code, json.loads(out.getvalue()) if out.getvalue().strip() else None, err.getvalue()

    def test_thumb_pdf_crop_image_and_dry_run(self):
        pdf_out = self.root / "thumb.pdf.png"
        code, record, _ = self.run_cli(["thumb", self.fixtures["equation"], "--page", "1",
                                        "--crop", "5", "5", "80", "80", "-o", pdf_out])
        self.assertEqual(code, 0)
        self.assertTrue(Path(record["path"]).is_file())
        with Image.open(record["path"]) as image:
            self.assertLessEqual(max(image.size), 800)
        source_image = self.fixtures["chart"].parent / "chart.png"
        image_out = self.root / "image.png"
        code, record, _ = self.run_cli(["thumb", source_image, "--crop", "0", "0", "40", "40", "-o", image_out])
        self.assertEqual(code, 0)
        self.assertTrue(Path(record["path"]).is_file())
        dry = self.root / "dry.png"
        code, record, _ = self.run_cli(["--dry-run", "thumb", source_image, "-o", dry])
        self.assertEqual((code, record["verdict"]), (0, "provisional"))
        self.assertFalse(dry.exists())

    def test_thumb_crop_is_scaled_to_max_side(self):
        output = self.root / "small-crop.png"
        code, record, _ = self.run_cli(["thumb", self.fixtures["equation"], "--crop", "5", "5", "25", "25",
                                        "--max-side", "800", "-o", output])
        self.assertEqual(code, 0, record)
        with Image.open(record["path"]) as image:
            self.assertGreaterEqual(max(image.size), 450)
            self.assertLessEqual(max(image.size), 800)

    def test_rotated_offset_crop_selects_displayed_mark(self):
        marked = self.root / "rotated-mark.pdf"
        doc = pymupdf.open(self.fixtures["rotated"])
        page = doc[0]
        mark = pymupdf.Rect(85, 216, 99, 231)
        page.draw_rect(mark, color=(1, 0, 0), fill=(1, 0, 0))
        doc.save(marked)
        doc.close()
        output = self.root / "rotated-crop.png"
        code, record, _ = self.run_cli(["thumb", marked, "--crop", "45", "42", "62", "60", "-o", output])
        self.assertEqual(code, 0, record)
        with Image.open(record["path"]) as image:
            pixels = np.asarray(image.convert("RGB"))
            red = (pixels[:, :, 0] > 200) & (pixels[:, :, 1] < 80) & (pixels[:, :, 2] < 80)
            self.assertGreater(int(red.sum()), 100)

    def test_thumb_job_uses_command_defaults(self):
        output = self.root / "thumb-job.png"
        job = self.root / "thumb.json"
        job.write_text(json.dumps({"version": 1, "command": "thumb", "input": str(self.fixtures["equation"]),
                                   "output": str(output)}), encoding="utf-8")
        code, record, _ = self.run_cli(["--job", job])
        self.assertEqual(code, 0, record)
        self.assertTrue(Path(record["path"]).is_file())

    def test_selftest_job_defaults_caps_false(self):
        job = self.root / "selftest.json"
        job.write_text(json.dumps({"version": 1, "command": "selftest"}), encoding="utf-8")
        caps = {"tools": {name: {"path": "present", "version": "ok"} for name in p2_selftest.TOOLS},
                "libraries": {name: "available" for name in p2_selftest.LIBRARIES}, "formats": cli.discover()}
        def writer(source, output):
            Path(output).write_bytes(b"selftest")
        out, err = io.StringIO(), io.StringIO()
        with patch("clierr.LOG_ROOT", self.root / "logs"), \
                patch.object(p2_selftest, "_capabilities", return_value=caps), \
                patch.object(p2_selftest, "_format_writer", return_value=writer), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(["--json", "--job", str(job)])
        record = json.loads(out.getvalue())
        self.assertEqual(code, 0)
        self.assertEqual(record["data"]["failures"], [])

    def test_selftest_quiet_and_caps(self):
        out, err = io.StringIO(), io.StringIO()
        with patch("clierr.LOG_ROOT", self.root / "logs"), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(["selftest"])
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue(), "")
        self.assertEqual(err.getvalue(), "")
        out, err = io.StringIO(), io.StringIO()
        with patch("clierr.LOG_ROOT", self.root / "logs"), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(["selftest", "--caps"])
        record = json.loads(out.getvalue())
        self.assertEqual(code, 0)
        self.assertIn("pymupdf", record["libraries"])
        self.assertNotEqual(record["tools"]["browser"]["path"], "absent")
        self.assertNotEqual(record["tools"]["ghostscript"]["path"], "absent")

    def test_selftest_finds_future_format_writer_in_command_module(self):
        def run(args, context):
            return None
        run.__module__ = p2_selftest.__name__
        writer = lambda source, output: output.write_bytes(b"format probe")
        with patch.object(p2_selftest, "commands_by_phase", return_value={
                "future": type("Spec", (), {"output_kind": "docx", "run": run})}), \
                patch.object(p2_selftest, "selftest_write", writer, create=True):
            self.assertIs(p2_selftest._format_writer("docx"), writer)

    def test_selftest_failures_are_warns_and_json_report_with_exit_one(self):
        caps = {"tools": {"qpdf": {"path": "absent", "version": "absent"}},
                "libraries": {"pymupdf": "absent: simulated"}, "formats": []}
        out, err = io.StringIO(), io.StringIO()
        with patch("clierr.LOG_ROOT", self.root / "logs"), \
                patch.object(p2_selftest, "_capabilities", return_value=caps), \
                patch.object(p2_selftest, "output_formats", return_value=[]), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(["--json", "selftest"])
        record = json.loads(out.getvalue())
        self.assertEqual(code, 1)
        self.assertEqual(record["type"], "report")
        self.assertTrue(record["data"]["failures"])
        self.assertEqual(len([line for line in err.getvalue().splitlines() if line.startswith("WARN pdf selftest ")]),
                         len(record["data"]["failures"]))
        self.assertNotIn("FAIL", out.getvalue())

    def test_version_probe_timeout_is_not_selftest_failure(self):
        from subprocess import TimeoutExpired
        context = type("Context", (), {"exit_code": 0})()
        args = type("Args", (), {"caps": False, "json": False})()
        with patch.object(p2_selftest, "TOOLS", {"fake": ["--version"]}), \
                patch("shutil.which", return_value="fake.exe"), \
                patch("subprocess.run", side_effect=TimeoutExpired("fake.exe", 4)), \
                patch.object(p2_selftest, "output_formats", return_value=[]), \
                contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            result = p2_selftest.selftest(args, context)
        self.assertIsNone(result)
        self.assertEqual(context.exit_code, 0)

    def test_bounded_json_report_can_be_forced_to_stdout(self):
        out = io.StringIO()
        record = {"type": "report", "command": "test", "input": "bounded.pdf", "verdict": "report",
                  "data": "\n".join(f"line {i} words" for i in range(201))}
        with contextlib.redirect_stdout(out):
            cli.emit(record, True)
        bounded = json.loads(out.getvalue())
        report_path = Path(bounded["path"])
        try:
            self.assertEqual(bounded["lines"], 201)
            self.assertEqual(bounded["words"], 603)
            self.assertEqual(len(bounded["preview"].splitlines()), 20)
            out = io.StringIO()
            full = {"type": "report", "command": "test", "input": "bounded.pdf", "verdict": "report",
                    "data": "\n".join(f"line {i} words" for i in range(201))}
            with contextlib.redirect_stdout(out):
                cli.emit(full, True, force_stdout=True)
            self.assertIn("data", json.loads(out.getvalue()))
        finally:
            report_path.unlink(missing_ok=True)

    def test_report_paths_and_pruning_share_all_file_types(self):
        import os
        from pdftools.cli import prune_reports, report_path
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            name = report_path(root, "thumb", "scan", ".png").name
            self.assertRegex(name, r"^\d{8}-\d{6}-\d{6}-\d+-thumb-scan\.png$")
            for index in range(55):
                path = root / f"{index}.{'png' if index % 2 else 'txt'}"
                path.write_bytes(b"x")
                os.utime(path, (index + 1, index + 1))
            prune_reports(root)
            self.assertEqual(len(list(root.iterdir())), 50)
            self.assertFalse((root / "0.txt").exists())


if __name__ == "__main__":
    unittest.main()
