import importlib.util
import tempfile
import unittest
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))
import pymupdf



def load_build_skill():
    path = ROOT / "tools" / "build_skill.py"
    spec = importlib.util.spec_from_file_location("build_skill", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.assemble


def load_make_fixtures():
    path = ROOT / "tests" / "make_fixtures.py"
    spec = importlib.util.spec_from_file_location("make_fixtures", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class BuilderTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.assemble = staticmethod(load_build_skill())

    def test_fixture_discovery_and_deterministic_content(self):
        make = load_make_fixtures()
        with tempfile.TemporaryDirectory() as temp:
            first = make.build_all(Path(temp) / "first")
            second = make.build_all(Path(temp) / "second")
            self.assertEqual(set(first), {"p1", "p2"})
            self.assertNotIn("benchmark", first["p2"])
            with pymupdf.open(first["p2"]["equation"]) as first_doc, \
                    pymupdf.open(second["p2"]["equation"]) as second_doc:
                self.assertEqual([page.get_text() for page in first_doc], [page.get_text() for page in second_doc])
            self.assertTrue(first["p2"]["recoverable_xref"].is_file())
            self.assertTrue(first["p2"]["widgets_no_appearance"].is_file())
            with pymupdf.open(first["p2"]["recoverable_xref"]) as recovered:
                self.assertGreaterEqual(recovered.page_count, 1)
            with self.assertRaises(Exception):
                pymupdf.open(first["p2"]["truncated"])

    def test_skill_phase_sort_and_missing_or_malformed_parts(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            (folder / "p0.md").write_text("---\nname: x\ndescription: y\n---\n# x", encoding="utf-8")
            for name in ("p1.md", "p2.md", "p3c.md", "p3a.md", "p3b.md", "p4.md", "p10.md"):
                (folder / name).write_text(f"## {name}\n", encoding="utf-8")
            combined = self.assemble(folder)
            self.assertLess(combined.index("p3a.md"), combined.index("p3b.md"))
            self.assertLess(combined.index("p3b.md"), combined.index("p3c.md"))
            self.assertLess(combined.index("p4.md"), combined.index("p10.md"))
            (folder / "p2.md").unlink()
            with self.assertRaises(Exception):
                self.assemble(folder)
            (folder / "p2.md").write_text("not a heading", encoding="utf-8")
            with self.assertRaises(Exception):
                self.assemble(folder)


if __name__ == "__main__":
    unittest.main()
