import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import pymupdf

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT)); sys.path.insert(0, str(ROOT.parent / "common"))

import clierr
from pdftools import cli
from pdftools.secrets import SecretStore

TARGET = "PRIVATE-TARGET-4711"


class SecretArgTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.pdf = self.root / "doc.pdf"
        doc = pymupdf.open(); doc.new_page().insert_text((50, 80), "plain text"); doc.save(self.pdf); doc.close()
        # annotate stands in for redact, so the test does not depend on phase 7.
        self.patches = [patch.dict("pdftools.cli.SECRET_ARGS", {"annotate": ("highlight",)}),
                        patch.object(clierr, "_REGISTERED", []), patch("clierr.LOG_ROOT", self.root / "logs")]
        for item in self.patches:
            item.start()

    def tearDown(self):
        for item in reversed(self.patches):
            item.stop()
        self.temp.cleanup()

    def run_cli(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with patch.object(sys, "argv", ["pdf.py", *argv]), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(argv)
        logs = "".join(path.read_text(encoding="utf-8") for path in (self.root / "logs").rglob("*.log"))
        return code, out.getvalue() + err.getvalue(), logs

    def test_parse_failure_does_not_log_secret_argument(self):
        for argv in (["annotate", str(self.pdf), "--highlight", TARGET, "--no-such-flag"],
                     ["annotate", str(self.pdf), f"--highlight={TARGET}", "--no-such-flag"]):
            code, output, logs = self.run_cli(argv)
            self.assertEqual(code, 2)
            self.assertTrue(logs)
            self.assertNotIn(TARGET, output + logs)

    def test_bad_job_does_not_log_secret_value(self):
        job = self.root / "job.json"
        job.write_text(json.dumps({"version": 1, "command": "annotate", "input": str(self.pdf),
                                   "highlight": [TARGET], "no_such_key": 1}), encoding="utf-8")
        code, output, logs = self.run_cli(["annotate", "--job", str(job)])
        self.assertEqual(code, 2)
        self.assertNotIn(TARGET, output + logs)

    def test_add_value_keeps_password(self):
        store = SecretStore()
        store.add("pw")
        store.add_value("target")
        self.assertEqual(store.password, "pw")
        self.assertEqual(store.values, ["pw", "target"])


if __name__ == "__main__":
    unittest.main()
