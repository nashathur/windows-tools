import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT)); sys.path.insert(0, str(ROOT.parent / "common"))
from pdftools import tools


class DevToolEnvTest(unittest.TestCase):
    def test_split_tessdata_is_joined_in_temp(self):
        with tempfile.TemporaryDirectory() as tmp:
            prefix, temp = Path(tmp) / "env", Path(tmp) / "run"
            (prefix / "share" / "tessdata").mkdir(parents=True); temp.mkdir()
            (prefix / "Library" / "share" / "tessdata" / "configs").mkdir(parents=True)
            (prefix / "Library" / "share" / "tessdata" / "configs" / "pdf").write_text("tessedit_create_pdf 1")
            for lang in ("eng", "ind", "osd", "deu"):
                (prefix / "share" / "tessdata" / f"{lang}.traineddata").write_bytes(lang.encode())
            with patch.object(sys, "prefix", str(prefix)), patch.object(tools, "UNPAPER_WINDOWS_DIR", Path(tmp) / "none"):
                env = tools.dev_tool_env(temp, "eng+ind", env={"PATH": "base"})
                again = tools.dev_tool_env(temp, "eng+ind", env={"PATH": "base"})  # second call reuses the folder
            joined = Path(env["TESSDATA_PREFIX"])
            self.assertEqual(again["TESSDATA_PREFIX"], str(joined))
            self.assertEqual(sorted(p.name for p in joined.iterdir()),
                             ["configs", "eng.traineddata", "ind.traineddata", "osd.traineddata"])
            self.assertTrue((joined / "configs" / "pdf").is_file())
            self.assertTrue(env["PATH"].endswith("base") and env["PATH"].startswith(str(prefix)))

    def test_joined_layout_needs_no_prefix(self):
        with tempfile.TemporaryDirectory() as tmp:
            prefix = Path(tmp)
            (prefix / "share" / "tessdata" / "configs").mkdir(parents=True)
            with patch.object(sys, "prefix", str(prefix)):
                env = tools.dev_tool_env(tmp, env={"PATH": "base"})
            self.assertNotIn("TESSDATA_PREFIX", env)

    def test_windows_unpaper_folder_goes_last_on_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp) / "ucrt64" / "bin"; folder.mkdir(parents=True); (folder / "unpaper.exe").write_bytes(b"")
            with patch.object(tools, "WINDOWS", True), patch.object(tools, "UNPAPER_WINDOWS_DIR", folder):
                env = tools.dev_tool_env(tmp, env={"PATH": "base"})
            self.assertTrue(env["PATH"].endswith("base" + __import__("os").pathsep + str(folder)))


if __name__ == "__main__":
    unittest.main()
