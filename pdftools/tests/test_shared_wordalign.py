import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT)); sys.path.insert(0, str(ROOT.parent / "common"))

from pdftools.wordalign import align, missing_runs, norm_word, prose_words


class WordAlignTests(unittest.TestCase):
    def test_identical_reordered_missing_and_url_split(self):
        self.assertEqual(align(["a", "b"], ["a", "b"])[0], {0, 1})
        self.assertEqual(align(["alpha", "beta", "gamma"], ["gamma", "alpha", "beta"])[0], {0, 1})
        self.assertEqual(align(["before", "missing", "after"], ["before", "after"])[0], {0, 2})
        self.assertEqual(align(["before", "httpslongdomainname", "after"],
                               ["before", "https", "long", "domain", "name", "after"])[0], {0, 1, 2})
        self.assertEqual(norm_word("Foo—Bar"), "foobar")

    def test_missing_runs(self):
        sentence = "alpha bravo charlie delta echo foxtrot golf hotel india"
        tokens = [(word, start, end) for word, start, end in prose_words(sentence)]
        self.assertEqual(missing_runs(tokens, "unrelated text"), [(0, 8)])
        self.assertEqual(missing_runs(tokens[:2], "unrelated text"), [])

    def test_two_column_order_is_not_missing(self):
        text = "alpha bravo charlie delta echo foxtrot golf hotel india"
        tokens = [(word, start, end) for word, start, end in prose_words(text)]
        self.assertEqual(missing_runs(tokens, "alpha bravo charlie golf hotel india delta echo foxtrot"), [])


if __name__ == "__main__": unittest.main()
