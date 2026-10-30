import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT)); sys.path.insert(0, str(ROOT.parent / "common"))

from pdftools import tools


class ToolRunnerTests(unittest.TestCase):
    def test_input_reaches_stdin(self):
        result = tools.run_tool([sys.executable, "-c", "import sys; print(sys.stdin.read().upper())"],
                                input="prompt text", timeout=30)
        self.assertEqual(result.stdout.strip(), "PROMPT TEXT")

    def test_discovery_does_not_import_heavy_libraries(self):
        code = ("import sys; sys.path[:0] = [%r, %r]; from pdftools.registry import discover; discover(); "
                "print(sorted(m for m in ('pymupdf4llm', 'pymupdf.layout', 'pypdfium2') if m in sys.modules))"
                % (str(ROOT), str(ROOT.parent / "common")))
        result = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, timeout=120)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "[]")


if __name__ == "__main__":
    unittest.main()
