import contextlib
import io
import os
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

import psutil

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))

from pdftools import cli


@unittest.skipUnless(os.environ.get("PDFTOOLS_BENCH") == "1", "set PDFTOOLS_BENCH=1 to run the 500-page benchmark")
class Phase3aBenchmarkTest(unittest.TestCase):
    def test_info_pages_and_text_stay_within_resource_limits(self):
        root = Path(__file__).resolve().parent / "fixtures" / "out" / "p2" / "benchmark-500.pdf"
        self.assertTrue(root.is_file(), "build fixtures with tests\\make_fixtures.py <folder> --bench first")
        process = psutil.Process()
        results = {}
        for command in ("info", "pages", "text"):
            peak = [process.memory_info().rss]
            done = threading.Event()

            def sample_rss():
                while not done.wait(0.01):
                    peak[0] = max(peak[0], process.memory_info().rss)

            sampler = threading.Thread(target=sample_rss, daemon=True)
            out, err = io.StringIO(), io.StringIO()
            sampler.start()
            started = time.perf_counter()
            try:
                with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                    code = cli.main(["--json", command, str(root)])
            finally:
                elapsed = time.perf_counter() - started
                done.set()
                sampler.join()
            peak[0] = max(peak[0], process.memory_info().rss)
            results[command] = (elapsed, peak[0] / (1024 * 1024))
            self.assertEqual(code, 0, f"{command} failed: {err.getvalue()}")
            self.assertLess(elapsed, 30, f"{command}: {elapsed:.2f}s")
            self.assertLess(peak[0], 1.5 * 1024 * 1024 * 1024, f"{command}: {peak[0] / (1024 * 1024):.1f} MiB")
        print("phase 3a benchmark: " + ", ".join(
            f"{name}={seconds:.2f}s/{memory:.1f}MiB peak RSS" for name, (seconds, memory) in results.items()))


if __name__ == "__main__":
    unittest.main()
