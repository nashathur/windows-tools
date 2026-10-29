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

    def test_eqread(self):
        self.assert_cli_error("eqread", "missing_chapter")

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



class EqreadTests(unittest.TestCase):
    def setUp(self):
        sys.path.insert(0, str(SCRIPTS))
        import eqread
        self.eqread = eqread

    def test_parse_answer(self):
        got = self.eqread.parse_answer("E1\t\\frac{d}{dt}\n`E2: $x = 1 \\quad (2.3)$`\nnoise\nE10 | y\n")
        self.assertEqual(got, {1: "\\frac{d}{dt}", 2: "x = 1 \\quad (2.3)", 10: "y"})

    def test_tag_read_as_number(self):
        self.assertEqual(self.eqread.for_check("x = 1 \\tag{2.3}"), "x = 1  \\quad (2.3)")

    def test_merge_formulas(self):
        import convert
        f = lambda i, y0, y1, cls="formula": {"index": i, "class": cls, "bbox": (100, y0, 500, y1)}
        got = convert.merge_formulas([f(0, 300, 322), f(1, 325, 333), f(2, 331, 346), f(3, 360, 380, "text"), f(4, 390, 400)])
        self.assertEqual([(b["index"], b["bbox"][1], b["bbox"][3]) for b in got],
                         [(0, 300, 322), (1, 325, 346), (3, 360, 380), (4, 390, 400)])

    def test_doubtful(self):
        d = self.eqread.doubtful
        r = lambda *xs: [{"by": by, "latex": x} for by, x in xs]
        self.assertTrue(d({"problems": ["symbols"], "checked": True, "readings": r(("pass1", "x"))}))
        self.assertFalse(d({"problems": [], "checked": True, "readings": r(("pass1", "x"))}))
        # no PDF text to check against: the last two readings must agree, structure included
        self.assertTrue(d({"problems": [], "checked": False, "readings": r(("pass1", "x"))}))
        self.assertTrue(d({"problems": [], "checked": False, "readings": r(("pass1", "\\frac{x}{y}"), ("pass2", "\\frac{y}{x}"))}))
        self.assertFalse(d({"problems": [], "checked": False, "readings": r(("pass1", "\\|x\\| \\quad (1.2)"),
                                                                              ("pass2", "\\lVert x\\rVert\\tag{1.2}"))}))

    def test_pass_selection(self):
        w = self.eqread.wanted
        r = lambda *bys: [{"by": by, "latex": "x"} for by in bys]
        ok = {"problems": [], "minor": [], "checked": True}
        self.assertTrue(w({**ok, "readings": []}, "pass1"))
        self.assertFalse(w({**ok, "readings": r("pass1")}, "pass2"))
        self.assertTrue(w({**ok, "minor": ["neighbour"], "readings": r("pass1")}, "pass2"))
        self.assertFalse(w({**ok, "minor": ["neighbour"], "readings": r("pass1", "pass2")}, "pass3"))
        self.assertTrue(w({**ok, "problems": ["symbols"], "readings": r("pass1", "pass2")}, "pass3"))


if __name__ == "__main__":
    unittest.main()
