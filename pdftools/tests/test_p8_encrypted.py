from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import pymupdf

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))
from .fixtures.p1 import build_fixtures as build_p1_fixtures

PDF = ROOT / "pdf.py"
NODE_PATH = "/home/agent/src/windows-tools/pdftools/node_modules"


class P8EncryptedTests(unittest.TestCase):
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

    def test_encrypted_typeset_book_exports_without_leaking_password(self):
        plain = ROOT / "tests" / "fixtures" / "data" / "p8_typeset.pdf"
        encrypted = self.root / "typeset-secret.pdf"
        with pymupdf.open(plain) as doc:
            doc.save(encrypted, encryption=pymupdf.PDF_ENCRYPT_AES_256,
                     user_pw="equation secret", owner_pw="owner secret")
        password = self.root / "password.txt"
        password.write_text("equation secret\n", encoding="utf-8")
        output = self.root / "book_md"

        result = self._cli("md", encrypted, "--password-file", password, "-o", output, "--no-read", "--no-qa")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        record = json.loads(result.stdout)
        self.assertIn("DECRYPTED_EXPORT", record["warnings"])
        self.assertNotIn("equation secret", result.stdout + result.stderr)
        self.assertFalse(any("decrypted" in path.name for path in output.rglob("*")))
        self.assertFalse(any(b"equation secret" in path.read_bytes() for path in output.rglob("*") if path.is_file()))
        self.assertTrue((output / ".pdftools-md.json").is_file())
        pages_data = json.loads(next(output.glob("*/*.pages.json")).read_text(encoding="utf-8"))
        blocks_data = json.loads(next(output.glob("*/*.blocks.json")).read_text(encoding="utf-8"))
        self.assertEqual(pages_data["extraction_source_pdf"], str(encrypted))
        self.assertEqual(blocks_data["extraction_source_pdf"], str(encrypted))

        fixtures = build_p1_fixtures(self.root / "fixtures")
        p1_password = self.root / "p1-password.txt"
        p1_password.write_text("user-pass\n", encoding="utf-8")
        small_output = self.root / "small_md"
        result = self._cli("md", fixtures["encrypted"], "--password-file", p1_password,
                           "-o", small_output, "--no-read", "--no-qa")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for command, extra in (("mdread", ()), ("mdqa", ()),
                               ("mdexport", ("-o", self.root / "project"))):
            result = self._cli(command, small_output, "--password-file", p1_password, *extra)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            record = json.loads(result.stdout)
            self.assertIn("DECRYPTED_EXPORT", record["warnings"])
            self.assertNotIn("user-pass", result.stdout + result.stderr)

    def test_wrong_password_is_reported_and_redacted(self):
        fixtures = build_p1_fixtures(self.root / "fixtures")
        password = self.root / "wrong.txt"
        password.write_text("wrong equation secret\n", encoding="utf-8")
        result = self._cli("md", fixtures["encrypted"], "--password-file", password, "--no-read")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("WRONG_PASSWORD", result.stderr)
        self.assertNotIn("wrong equation secret", result.stdout + result.stderr)
        match = re.search(r"log: (.+)$", result.stderr.strip())
        if match:
            log = Path(match.group(1))
            if log.is_file():
                self.assertNotIn("wrong equation secret", log.read_text(encoding="utf-8", errors="replace"))


if __name__ == "__main__":
    unittest.main()
