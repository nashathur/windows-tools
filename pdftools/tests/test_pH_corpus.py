import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
import sys
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests")); sys.path.insert(0, str(ROOT))
import corpus
from corpus import categorize, compare_baselines, digest, fetch


class CorpusTest(unittest.TestCase):
    def test_categories_and_baseline_regression_rules(self):
        self.assertEqual(categorize(0, '{"type":"result","verdict":"warn"}\n'), "warn")
        self.assertEqual(categorize(1, '{"type":"error","code":"BAD_INPUT"}\n'), "error:BAD_INPUT")
        self.assertEqual(categorize(1, "", "Traceback (most recent call last)"), "crash")
        old = {"a": "ok", "b": "error:BAD_INPUT", "c": "unchecked"}
        new = {"a": "warn", "b": "error:INTERNAL", "c": "ok", "d": "ok"}
        worse, changed, improved = compare_baselines(old, new)
        self.assertEqual({row[0] for row in worse}, {"a", "b"})
        self.assertEqual(changed, [("d", "ok")])
        self.assertIn(("c", "unchecked", "ok"), improved)
        self.assertEqual(categorize(0, '{"type":"report","verdict":"report"}\n'), "report")
        self.assertEqual(categorize(3, '{"type":"result","verdict":"unchecked"}\n'), "unchecked")
        self.assertEqual(categorize(1, '{"type":"error","code":"BAD_INPUT"}\n'), "error:BAD_INPUT")
        self.assertEqual(categorize(0, '{"type":"report","verdict":"report"}\n', "tool said TIMEOUT once"), "report")
        self.assertEqual(categorize(-9, '{"type":"result","verdict":"ok"}\n'), "crash")
        self.assertEqual(categorize(4, '{"type":"result","verdict":"ok"}\n'), "crash")

    def test_standalone_entry_point_loads_common_before_discovery(self):
        # independent of whether this machine has fetched the public corpus
        env = dict(os.environ, PDFTOOLS_CORPUS_PUBLIC=str(Path(tempfile.gettempdir()) / "pdftools-no-manifest.json"))
        proc = subprocess.run([sys.executable, str(Path(__file__).with_name("corpus.py")), "sweep"],
                              capture_output=True, text=True, timeout=30, env=env)
        self.assertEqual(proc.returncode, 1, proc.stderr)
        self.assertIn("public corpus manifest is missing", proc.stderr)
        self.assertNotIn("No module named 'clierr'", proc.stderr)

    def test_sweep_rejects_a_bad_public_file_before_starting_children(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); manifest = root / "public.json"
            manifest.write_text(json.dumps({"commit": "abc", "files": [
                {"path": "missing.pdf", "sha256": "0" * 64, "bytes": 10}]}), encoding="utf-8")
            with patch.object(corpus, "PUBLIC", manifest), patch.object(corpus, "CORPUS", root / "corpus"), \
                    patch.object(corpus, "BASELINE", root / "baseline.json"), \
                    patch("subprocess.Popen", side_effect=AssertionError("child must not start")):
                with self.assertRaisesRegex(ValueError, "digest mismatch or missing file"):
                    corpus.sweep()

    def test_fetch_local_repo_and_verify_manifest_digests(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); repo = root / "repo"; (repo / "test/pdfs").mkdir(parents=True)
            import subprocess
            subprocess.run(["git", "init", str(repo)], check=True, capture_output=True)
            subprocess.run(["git", "-C", str(repo), "config", "user.email", "test@example.invalid"], check=True)
            subprocess.run(["git", "-C", str(repo), "config", "user.name", "Test"], check=True)
            (repo / "test/pdfs/a.pdf").write_bytes(b"%PDF sample")
            subprocess.run(["git", "-C", str(repo), "add", "test/pdfs/a.pdf"], check=True)
            subprocess.run(["git", "-C", str(repo), "commit", "-m", "fixture"], check=True, capture_output=True)
            commit = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
            manifest = root / "manifest.json"; destination = root / "clone"
            result = fetch(commit, url=str(repo), destination=destination, manifest=manifest)
            self.assertEqual(result["files"][0]["sha256"], digest(destination / "test/pdfs/a.pdf"))
            manifest_bytes = manifest.read_bytes()
            (destination / "test/pdfs/a.pdf").write_bytes(b"tampered")
            fetch(None, url=str(repo), destination=destination, manifest=manifest)
            self.assertEqual(manifest.read_bytes(), manifest_bytes)
            self.assertEqual(digest(destination / "test/pdfs/a.pdf"), result["files"][0]["sha256"])


if __name__ == "__main__": unittest.main()
