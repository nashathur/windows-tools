import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parents[1]


class CliErrorTests(unittest.TestCase):
    def assert_cli_error(self, tool, *args):
        with tempfile.TemporaryDirectory() as log_root:
            env = os.environ.copy()
            env["CLIERR_LOG_ROOT"] = log_root
            result = subprocess.run(
                [sys.executable, str(SCRIPTS / f"{tool}.py"), *map(str, args)],
                capture_output=True,
                text=True,
                env=env,
            )

            self.assertNotEqual(result.returncode, 0, result.stdout)
            lines = result.stderr.splitlines()
            self.assertEqual(len(lines), 1, result.stderr)
            self.assertRegex(lines[0], rf"^ERROR {re.escape(tool)} [A-Z_]+: ")
            marker = " | log: "
            self.assertIn(marker, lines[0])
            log_path = lines[0].split(marker, 1)[1]
            self.assertTrue(Path(log_path).is_file(), log_path)

    def test_book(self):
        self.assert_cli_error("book", "missing.pdf", "out")

    def test_convert(self):
        self.assert_cli_error("convert", "missing.pdf", "out")

    def test_crosscheck(self):
        self.assert_cli_error("crosscheck", "missing_chapter")

    def test_export(self):
        self.assert_cli_error("export", "missing_folder", "out")

    def test_fix(self):
        self.assert_cli_error("fix", "missing_chapter", "missing_fixes.txt")

    def test_fix_unmatched_block(self):
        with tempfile.TemporaryDirectory() as root:
            chapter = Path(root) / "ch01"
            chapter.mkdir()
            (chapter / "ch01.md").write_text("Some text with x = 1.\n", encoding="utf-8")
            fixes = Path(root) / "fixes.txt"
            fixes.write_text("p3 wrong text\n- y = 2\n+ y = 3\n", encoding="utf-8")
            log_root = Path(root) / "logs"
            env = os.environ.copy()
            env["CLIERR_LOG_ROOT"] = str(log_root)
            result = subprocess.run(
                [sys.executable, str(SCRIPTS / "fix.py"), str(chapter), str(fixes)],
                capture_output=True,
                text=True,
                env=env,
            )
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("FAILED p3", result.stdout)
            lines = result.stderr.splitlines()
            self.assertEqual(len(lines), 1, result.stderr)
            self.assertTrue(lines[0].startswith("ERROR fix FIX_FAILED: "), lines[0])
            log_path = lines[0].split(" | log: ", 1)[1]
            self.assertTrue(Path(log_path).is_file(), log_path)

    def test_inspect_pdf(self):
        self.assert_cli_error("inspect_pdf", "missing.pdf")

    def test_postprocess(self):
        self.assert_cli_error("postprocess", "missing.md")

    def test_qa(self):
        self.assert_cli_error("qa", "missing_chapter")

    def test_reverify(self):
        self.assert_cli_error("reverify", "missing_chapter")


if __name__ == "__main__":
    unittest.main()
