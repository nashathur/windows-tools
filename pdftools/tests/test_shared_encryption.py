import contextlib
import io
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import pikepdf

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT)); sys.path.insert(0, str(ROOT.parent / "common"))

from pdftools import cli
from tests.fixtures.p1 import build_fixtures


class ReencryptionTests(unittest.TestCase):
    def test_owner_and_user_password_keep_user_password_and_permissions(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source = build_fixtures(root)["encrypted"]
            out = root / "reencrypted.pdf"
            env = {"T_USER": "user-pass", "T_OWNER": "owner-pass"}
            stdout = io.StringIO()
            with patch.dict(os.environ, env), patch("clierr.LOG_ROOT", root / "logs"), \
                    contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(io.StringIO()):
                code = cli.main(["--json", "rotate", str(source), "--pages", "1", "--by", "90",
                                 "--password-env", "T_USER", "--owner-password-env", "T_OWNER", "-o", str(out)])
            self.assertEqual(code, 0, stdout.getvalue())
            out = Path(json.loads(stdout.getvalue())["path"])
            with pikepdf.open(source, password="user-pass") as original, pikepdf.open(out, password="user-pass") as result:
                self.assertTrue(result.is_encrypted)
                self.assertEqual(result.encryption.P, original.encryption.P)
            with pikepdf.open(out, password="owner-pass") as result:
                self.assertTrue(result.owner_password_matched)


if __name__ == "__main__":
    unittest.main()
