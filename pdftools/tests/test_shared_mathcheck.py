import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT)); sys.path.insert(0, str(ROOT.parent / "common"))

from pdftools import mathcheck


class MathCheckTests(unittest.TestCase):
    def test_empty_does_not_start_node(self):
        with patch("pdftools.mathcheck.tools.run_tool") as run:
            self.assertEqual(mathcheck.validate([]), {})
            run.assert_not_called()

    def test_validation_preserves_ids_and_display_mode(self):
        items = [{"id": "inline-good", "latex": r"\frac{a}{b}", "display": False},
                 {"id": "inline-bad", "latex": r"\frac{a}{", "display": False},
                 {"id": "display-bad", "latex": "x^", "display": True},
                 {"id": "tag", "latex": r"x=1\tag{2.1}", "display": True}]
        self.assertEqual(set(mathcheck.validate(items)), {"inline-bad", "display-bad"})


if __name__ == "__main__": unittest.main()
