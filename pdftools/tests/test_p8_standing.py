from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import pymupdf

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))

from tests.fixtures.p8 import build

PDF = ROOT / "pdf.py"
NODE_PATH = "/home/agent/src/windows-tools/pdftools/node_modules"


class P8StandingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def _cli(self, *args):
        env = os.environ.copy()
        env["NODE_PATH"] = NODE_PATH
        return subprocess.run([sys.executable, str(PDF), *map(str, args)], cwd=ROOT,
                              env=env, text=True, capture_output=True)

    def _prepared_book(self):
        source = build(self.root / "Bücher source with spaces.pdf")
        output = self.root / "Bücher output with spaces"
        result = self._cli("--json", "md", source, "-o", output, "--no-read", "--no-qa")
        self.assertIn(result.returncode, (0, 4), result.stdout + result.stderr)
        part = next(p for p in output.iterdir() if p.is_dir() and (p / "eqs.json").exists())
        # mdread can exercise its resume and QA paths without making a Codex call.
        (part / "eqs.json").write_text("[]", encoding="utf-8")
        blocks_path = next(part.glob("*.blocks.json"), None)
        if blocks_path:
            blocks = json.loads(blocks_path.read_text(encoding="utf-8"))
            for page in blocks.get("pages", []):
                page["children"] = [block for block in page.get("children", [])
                                    if block.get("block_type") != "Table"]
            blocks_path.write_text(json.dumps(blocks), encoding="utf-8")
        return source, output

    def test_dry_run_does_not_create_outputs_or_folders(self):
        source, converted = self._prepared_book()
        target = self.root / "dry run output"
        result = self._cli("--json", "--dry-run", "md", source, "-o", target)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(target.exists())
        fixes = self.root / "fixes.txt"; fixes.write_text("", encoding="utf-8")
        part = next(p for p in converted.iterdir() if p.is_dir() and (p / "eqs.json").exists())
        before = {p.relative_to(converted): p.read_bytes() for p in converted.rglob("*") if p.is_file()}
        result = self._cli("--json", "--dry-run", "mdfix", part, fixes)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        after = {p.relative_to(converted): p.read_bytes() for p in converted.rglob("*") if p.is_file()}
        self.assertEqual(before, after)
        self.assertTrue(converted.is_dir())

    def test_mdread_dry_run_does_not_rewrite_qa_outputs(self):
        # p8_md.mdread ignores args.dry_run and rewrites per-part QA reports.
        _, converted = self._prepared_book()
        part = next(p for p in converted.iterdir() if p.is_dir() and (p / "eqs.json").exists())
        before = {p.relative_to(converted): p.read_bytes() for p in converted.rglob("*") if p.is_file()}
        result = self._cli("--json", "--dry-run", "mdread", converted)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        after = {p.relative_to(converted): p.read_bytes() for p in converted.rglob("*") if p.is_file()}
        self.assertEqual(before, after)

    def test_mdqa_dry_run_does_not_rewrite_qa_outputs(self):
        # p8_md.mdqa ignores args.dry_run and rewrites per-part QA reports.
        _, converted = self._prepared_book()
        part = next(p for p in converted.iterdir() if p.is_dir() and (p / "eqs.json").exists())
        before = {p.relative_to(converted): p.read_bytes() for p in converted.rglob("*") if p.is_file()}
        result = self._cli("--json", "--dry-run", "mdqa", converted)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        after = {p.relative_to(converted): p.read_bytes() for p in converted.rglob("*") if p.is_file()}
        self.assertEqual(before, after)

    def test_mdexport_dry_run_does_not_create_project_folder(self):
        # p8_md.mdexport ignores args.dry_run and creates the export folder and files.
        _, converted = self._prepared_book()
        target = self.root / "dry export"
        result = self._cli("--json", "--dry-run", "mdexport", converted, "-o", target)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(target.exists())

    def test_json_output_is_one_parseable_json_line_for_each_command(self):
        source, converted = self._prepared_book()
        fixes = self.root / "fixes.txt"; fixes.write_text("", encoding="utf-8")
        part = next(p for p in converted.iterdir() if p.is_dir() and (p / "eqs.json").exists())
        cases = [("md", [source, "--list"]), ("mdread", [converted]), ("mdqa", [converted]),
                 ("mdfix", [part, fixes, "--dry-run"]),
                 ("mdexport", [converted, "-o", self.root / "json export"])]
        for command, args in cases:
            with self.subTest(command=command):
                result = self._cli("--json", command, *args)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                lines = result.stdout.splitlines()
                self.assertEqual(len(lines), 1, result.stdout)
                record = json.loads(lines[0])
                self.assertEqual(record["command"], command)

    def test_bad_arguments_emit_one_error_line_and_json_log_for_each_command(self):
        source = build(self.root / "bad args.pdf")
        cases = [("md", [self.root / "missing.pdf"]), ("mdread", [self.root / "missing folder"]),
                 ("mdqa", [self.root / "missing folder"]),
                 ("mdfix", [self.root / "missing folder", self.root / "fixes"]),
                 ("mdexport", [self.root / "missing folder"]),
                 ("md", [source, "--pages", "0"])]
        for command in ("md", "mdread", "mdqa", "mdfix", "mdexport"):
            cases.append((command, ["--unknown-p8-flag"]))
        for command, args in cases:
            with self.subTest(command=command, args=args):
                result = self._cli("--json", command, *args)
                self.assertNotEqual(result.returncode, 0)
                lines = result.stderr.splitlines()
                self.assertEqual(len(lines), 1, result.stderr)
                self.assertRegex(lines[0], rf"^ERROR pdf {command} [A-Z_]+:")
                self.assertIn("log:", lines[0])
                self.assertEqual(len(result.stdout.splitlines()), 1, result.stdout)
                record = json.loads(result.stdout)
                self.assertEqual(record["type"], "error")
                self.assertEqual(record["command"], command)
                self.assertTrue(record["log"])

    def test_md_leaves_source_pdf_sha256_unchanged(self):
        source = build(self.root / "hash source.pdf")
        before = hashlib.sha256(source.read_bytes()).hexdigest()
        result = self._cli("--json", "md", source, "-o", self.root / "hash output", "--no-read", "--no-qa")
        self.assertIn(result.returncode, (0, 4), result.stdout + result.stderr)
        after = hashlib.sha256(source.read_bytes()).hexdigest()
        self.assertEqual(before, after)

    def test_paths_with_spaces_and_non_ascii_work_across_commands(self):
        source, converted = self._prepared_book()
        self.assertTrue(source.is_file())
        self.assertTrue(converted.is_dir())
        fixes = self.root / "correcties ü fixes.txt"; fixes.write_text("", encoding="utf-8")
        part = next(p for p in converted.iterdir() if p.is_dir() and (p / "eqs.json").exists())
        result = self._cli("--json", "mdfix", part, fixes, "--dry-run")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        export = self.root / "uitvoer met ruimte ü"
        result = self._cli("--json", "mdexport", converted, "-o", export)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(export.is_dir())


if __name__ == "__main__":
    unittest.main()
