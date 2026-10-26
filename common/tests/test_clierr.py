"""clierr: every failure prints exactly one ERROR line and leaves its log.
Run: conda run -n dev python -m unittest discover -s C:\\Tools\\common\\tests"""
import os
import json
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest.mock import patch

import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import clierr

COMMON = Path(__file__).resolve().parents[1]


def run_script(body, *args, log_root):
    script = Path(log_root) / "tool.py"
    script.write_text(f"import sys\nsys.path.insert(0, {str(COMMON)!r})\nimport clierr\n" + textwrap.dedent(body))
    env = dict(os.environ, CLIERR_LOG_ROOT=str(log_root))
    return subprocess.run([sys.executable, str(script), *args], capture_output=True, text=True, env=env)


def error_lines(stderr):
    return [l for l in stderr.splitlines() if l.startswith("ERROR ")]


class ClierrTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def assert_one_error(self, result, code, exit_code=1):
        self.assertEqual(result.returncode, exit_code, result.stderr)
        lines = error_lines(result.stderr)
        self.assertEqual(len(lines), 1, result.stderr)
        self.assertEqual(result.stderr.strip(), lines[0])
        self.assertTrue(lines[0].startswith(f"ERROR demo {code}: "), lines[0])
        log = Path(lines[0].rsplit("| log: ", 1)[1])
        self.assertTrue(log.is_file(), log)
        return lines[0], log.read_text(encoding="utf-8")

    def test_uncaught_exception_is_internal_with_traceback_in_log(self):
        r = run_script("def main():\n    {}['x']\nclierr.run('demo', main)\n", log_root=self.root)
        line, log = self.assert_one_error(r, "INTERNAL")
        self.assertIn("== traceback ==", log)
        self.assertIn("== command ==", log)

    def test_fail_carries_code_and_fix(self):
        r = run_script("def main():\n    raise clierr.Fail('BAD_PAGE', 'page 12 of 10', 'use 1-10')\n"
                       "clierr.run('demo', main)\n", log_root=self.root)
        line, _ = self.assert_one_error(r, "BAD_PAGE")
        self.assertIn("page 12 of 10 | fix: use 1-10 | log: ", line)

    def test_argparse_error_is_bad_arg_exit_2(self):
        r = run_script("import argparse\ndef main():\n    p = argparse.ArgumentParser(prog='demo')\n"
                       "    p.add_argument('n', type=int)\n    p.parse_args()\nclierr.run('demo', main)\n",
                       "notanumber", log_root=self.root)
        self.assert_one_error(r, "BAD_ARG", exit_code=2)

    def test_tool_failure_quotes_one_line_and_logs_tail(self):
        r = run_script("def main():\n    clierr.run_tool([sys.executable, '-c', "
                       "\"import sys; print('\\\\x1b[31mnoise', file=sys.stderr); "
                       "print('fatal: disk full', file=sys.stderr); sys.exit(3)\"], name='helper')\n"
                       "clierr.run('demo', main)\n", log_root=self.root)
        line, log = self.assert_one_error(r, "TOOL_FAILED")
        self.assertIn("helper exit 3: fatal: disk full", line)
        self.assertIn("== tool stderr (last 30) ==", log)
        self.assertNotIn("\x1b", log)

    def test_success_prints_nothing_extra_and_exits_0(self):
        r = run_script("def main():\n    print('ok')\nclierr.run('demo', main)\n", log_root=self.root)
        self.assertEqual((r.returncode, r.stdout, r.stderr), (0, "ok\n", ""))

    def test_relevant_line_handles_crlf_and_progress(self):
        sys.path.insert(0, str(COMMON))
        import clierr
        self.assertEqual(clierr.relevant_line("10%\r50%\r\nfatal: x\r\n"), "fatal: x")
        self.assertEqual(clierr.clean("a\r\n\x1b[1mb\x1b[0m\r\n"), "a\nb\n")

    def test_same_second_failures_keep_separate_logs_and_one_line(self):
        r = run_script("def main():\n    for i in range(2):\n        clierr.fail('demo', clierr.Fail('X', 'two\\nlines', 'fix\\nthis'))\n"
                       "clierr.run('demo', main)\n", log_root=self.root)
        lines = error_lines(r.stderr)
        self.assertEqual(len(r.stderr.splitlines()), 2, r.stderr)
        self.assertTrue(all(" X: two lines | fix: fix this | log: " in l for l in lines), lines)
        logs = {l.rsplit("| log: ", 1)[1] for l in lines}
        self.assertEqual(len(logs), 2)
        self.assertTrue(all(Path(p).is_file() for p in logs))

    def test_keeps_last_50_logs(self):
        folder = self.root / "demo"
        folder.mkdir()
        for i in range(60):
            (folder / f"20000101-0000{i:02d}-1.log").write_text("old")
        r = run_script("def main():\n    raise ValueError('x')\nclierr.run('demo', main)\n", log_root=self.root)
        self.assert_one_error(r, "INTERNAL")
        self.assertEqual(len(list(folder.glob("*.log"))), 50)

    def test_secret_redaction_and_extra_sections(self):
        secret = 'q"\\é漢\nline'
        for spelling in (secret, repr(secret), json.dumps(secret), json.dumps(secret, ensure_ascii=False)):
            self.assertNotIn(spelling, clierr.redact(spelling, [secret]))
        with patch.object(clierr, "LOG_ROOT", self.root):
            log = clierr.write_log("demo", "X", secret, "fix", secrets=[secret],
                                   sections={"problems": secret, "library warnings": secret}, log_name="info")
        content = Path(log).read_text(encoding="utf-8")
        self.assertTrue(log.endswith("-info.log"), log)
        self.assertIn("== problems ==", content)
        self.assertIn("== library warnings ==", content)
        self.assertNotIn(secret, content)

    def test_tool_argv_is_redacted_before_windows_serialization(self):
        secret = 'q"\\quoted'
        run = subprocess.CalledProcessError(2, ["helper.exe", "--password", secret], stderr="")
        with patch.object(clierr, "LOG_ROOT", self.root), patch.object(sys, "argv", ["demo.py", "--password", secret]):
            log_path = clierr.write_log("demo", "X", "failed", "fix", exc=run, secrets=[secret])
        log = Path(log_path).read_text(encoding="utf-8")
        self.assertIn("== command ==", log)
        self.assertIn("== tool ==", log)
        self.assertNotIn(secret, log)
        self.assertNotIn(subprocess.list2cmdline([secret]), log)


if __name__ == "__main__":
    unittest.main()
