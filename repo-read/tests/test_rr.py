"""rr.py checks that need no network: python -m unittest discover -s repo-read/tests"""
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
import rr  # noqa: E402


class Units(unittest.TestCase):
    def test_slug(self):
        self.assertEqual(rr.slug("https://github.com/Stirling-Tools/Stirling-PDF"), "Stirling-Tools-Stirling-PDF")
        self.assertEqual(rr.slug("https://github.com/o/r.git"), "o-r")
        self.assertEqual(rr.slug("https://github.com/o/r/tree/main/app/core"), "o-r")
        with self.assertRaises(rr.clierr.Fail):
            rr.slug("not a url")

    def test_patterns(self):
        self.assertEqual(rr.split_patterns(["a,b", " c "]), ["a", "b", "c"])
        self.assertEqual(rr.split_patterns(["./README.md,*.gradle"]), ["/README.md", "*.gradle"])
        with self.assertRaises(rr.clierr.Fail):
            rr.split_patterns(["My Project/*"])

    def test_tokens(self):
        self.assertEqual(rr.token_count("Files analyzed: 3\nEstimated tokens: 12.3k"), 12300)
        self.assertEqual(rr.token_count("Estimated tokens: 950"), 950)
        self.assertIsNone(rr.token_count("nothing"))


class Cli(unittest.TestCase):
    def test_both_pattern_kinds_one_error_line(self):
        with tempfile.TemporaryDirectory() as d:
            env = {**os.environ, "RR_ROOT": d, "CLIERR_LOG_ROOT": str(Path(d) / "logs")}
            env.pop("RR_WIN_PREFIX", None)
            r = subprocess.run([sys.executable, str(HERE / "rr.py"), "ingest", "https://github.com/o/r", "-i", "a", "-e", "b"],
                               capture_output=True, text=True, env=env)
            lines = [l for l in r.stderr.splitlines() if l.strip()]
            self.assertEqual(r.returncode, 2, r.stderr)
            self.assertEqual(len(lines), 1, r.stderr)
            self.assertTrue(lines[0].startswith("ERROR rr BAD_ARG"), lines[0])
            self.assertEqual(len(list((Path(d) / "logs" / "rr").glob("*"))), 1)


if __name__ == "__main__":
    unittest.main()
