import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
import contextlib
import io
import psutil

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT)); sys.path.insert(0, str(ROOT.parent / "common"))
from pdftools.registry import command, get
from pdftools import cli


def add_arguments(parser): parser.add_argument("file", type=Path)


@command("pH-sleep", "2", "pdf", "pdf", "_sleep", True, "report")
def sleep_command(args, context):
    time.sleep(20)


@command("pH-gil-hang", "2", "pdf", "pdf", "_gil", True, "report")
def gil_command(args, context):
    import re
    re.match(r"(a+)+$", "a" * 40 + "!")


@command("pH-staging-hang", "2", "pdf", "pdf", "_stage", True, "report")
def staging_command(args, context):
    from pdftools import io as pdf_io
    with pdf_io.temp_folder() as folder:
        stage = pdf_io.Staging(Path(args.file).with_name("staged.pdf"), folder)
        with stage:
            with open(stage.path, "w", encoding="utf-8") as handle:
                handle.write("held open")
                handle.flush()
                time.sleep(20)


@command("pH-check-hang", "2", "pdf", "pdf", "_hang", True, "placements")
def hang_command(args, context):
    from pdftools.pipeline import write_pdf
    from unittest.mock import patch
    def writer(doc, target): doc.save(target)
    from pdftools import watchdog
    with patch("pdftools.pipeline.compare", side_effect=lambda *a, **k: time.sleep(20)), \
            patch("pdftools.watchdog.rearm", side_effect=lambda n: watchdog.arm("pH-check-hang", True, 2)):
        return write_pdf(args.file, get("pH-check-hang"), args, context, writer)


@command("pH-tool-wait", "2", "pdf", "pdf", "_tool", True, "report")
def tool_wait(args, context):
    from pdftools.tools import run_tool
    run_tool([sys.executable, "-c", "import time; time.sleep(3)"])
    return {"type": "report", "command": "pH-tool-wait", "verdict": "report"}


class WatchdogTest(unittest.TestCase):
    def run_child(self, command_name, input_file, root):
        script = ("import sys; sys.path.insert(0, %r); sys.path.insert(0, %r); "
                  "import tests.test_pH_watchdog; from pdftools.cli import main; "
                  "raise SystemExit(main(['--json', %r, %r]))") % (str(ROOT), str(ROOT.parent / "common"), command_name, str(input_file))
        temp = root / "tmp"; temp.mkdir(exist_ok=True)
        env = dict(os.environ, PDFTOOLS_TIME_LIMIT="2", CLIERR_LOG_ROOT=str(root / "logs"),
                   TMP=str(temp), TEMP=str(temp), TMPDIR=str(temp))
        started = time.monotonic()
        proc = subprocess.run([sys.executable, "-c", script], capture_output=True, text=True, timeout=15, env=env)
        return proc, time.monotonic() - started

    def test_command_and_checker_expiry_are_bounded_and_clean(self):
        for command_name in ("pH-sleep", "pH-gil-hang", "pH-staging-hang", "pH-check-hang"):
            with self.subTest(command=command_name), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); source = root / "empty.pdf"
                doc = __import__("pymupdf").open(); doc.new_page(); doc.save(source); doc.close()
                temp_root = root / "tmp" / "pdftools"
                before_temp = set(temp_root.iterdir()) if temp_root.exists() else set()
                proc, elapsed = self.run_child(command_name, source, root)
                self.assertEqual(proc.returncode, 1, (proc.stdout, proc.stderr))
                self.assertLess(elapsed, 15)
                self.assertEqual(sum(line.startswith("ERROR pdf ") and " TIMEOUT:" in line for line in proc.stderr.splitlines()), 1, proc.stderr)
                rows = [json.loads(line) for line in proc.stdout.splitlines() if line.strip()]
                self.assertEqual(rows[0]["code"], "TIMEOUT")
                self.assertTrue(list((root / "logs").rglob("*.log")))
                self.assertEqual(list(root.glob(".*.part.pdf")), [])
                after_temp = set(temp_root.iterdir()) if temp_root.exists() else set()
                self.assertEqual(after_temp - before_temp, set())
                if command_name == "pH-staging-hang":
                    self.assertFalse(list((root / "tmp").rglob("staged.pdf")))

    def test_external_tool_runtime_pauses_watchdog(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); source = root / "source.pdf"
            doc = __import__("pymupdf").open(); doc.new_page(); doc.save(source); doc.close()
            proc, elapsed = self.run_child("pH-tool-wait", source, root)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertGreaterEqual(elapsed, 2.8)

    def test_supervisor_environment_holds_no_password_variable(self):
        from unittest.mock import MagicMock, patch
        from pdftools import watchdog
        fake = MagicMock(); fake.poll.return_value = None
        with patch.dict(os.environ, {"PH_SECRET_PASSWORD": "hunter2"}), \
                patch.object(watchdog, "_process", None), patch.object(watchdog, "_stream", None), \
                patch.object(watchdog.subprocess, "Popen", return_value=fake) as popen:
            watchdog.arm("pH-env", False)
            watchdog.disarm()
        env = popen.call_args.kwargs["env"]
        self.assertNotIn("PH_SECRET_PASSWORD", env)
        self.assertNotIn("hunter2", "".join(env.values()))
        self.assertIn("PATH", {key.upper() for key in env})

    def test_repeated_main_calls_leave_no_supervisor(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "source.pdf"
            doc = __import__("pymupdf").open(); doc.new_page(); doc.save(source); doc.close()
            for _ in range(2):
                with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                    self.assertEqual(cli.main(["--json", "info", str(source)]), 0)
                children = psutil.Process().children(recursive=True)
                self.assertFalse([child for child in children if any("supervisor.py" in part for part in child.cmdline())])


if __name__ == "__main__": unittest.main()
