import json
import multiprocessing
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))

import clierr
import pymupdf
from pdftools import coords, crypt, io, pages
from pdftools.cli import Parser, build_parser, load_job
from pdftools.registry import Command, discover
from pdftools.errors import BadArg, PdfError
from pdftools.secrets import SecretStore
from pdftools.tools import run_tool
from tests.fixtures.p1 import build_fixtures


def _publish_process(staged, target, ready):
    ready.wait(5)
    io.publish(staged, target)


class ContractsTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.fixtures = build_fixtures(self.root)

    def tearDown(self):
        self.tmp.cleanup()

    def test_parser_rejects_abbreviation_and_badarg(self):
        parser = build_parser()
        self.assertFalse(parser.allow_abbrev)
        with self.assertRaises(BadArg):
            parser.parse_args(["inf", str(self.fixtures["plain"])])
        with self.assertRaises(BadArg):
            parser.parse_args(["info", str(self.fixtures["plain"]), "--passw", "x"])
        parsed = parser.parse_args(["info", str(self.fixtures["plain"]), "--json"])
        self.assertTrue(parsed.json)

    def test_job_version_unknown_key_path_base_and_multistep(self):
        job = self.root / "jobs" / "task.json"
        job.parent.mkdir()
        (job.parent / "input.pdf").write_bytes(self.fixtures["plain"].read_bytes())
        job.write_text(json.dumps({"version": 1, "command": "info", "input": "input.pdf"}), encoding="utf-8")
        commands = discover()
        args, _ = load_job(job, commands)
        self.assertEqual(args.file, (job.parent / "input.pdf").resolve())
        for payload in ({"version": 2, "command": "info", "file": "x"},
                        {"version": 1, "command": "info", "mystery": 1},
                        {"version": 1, "command": "info", "steps": [{"cmd": "info"}, {"cmd": "info"}]},
                        {"version": 1, "command": "info"}):
            job.write_text(json.dumps(payload), encoding="utf-8")
            with self.assertRaises(BadArg):
                load_job(job, commands)
        # several steps without a command go to batch, which receives them raw
        fake = Command("batch", "5", "pdf", "pdf", "", True, "report",
                       lambda p: (p.add_argument("file", nargs="?", type=Path), p.add_argument("pipeline", nargs="?")), None)
        steps = [{"cmd": "rotate", "by": 90}, {"cmd": "numbers"}]
        job.write_text(json.dumps({"version": 1, "input": "input.pdf", "steps": steps}), encoding="utf-8")
        args, _ = load_job(job, dict(commands, batch=fake))
        self.assertEqual((args.command, args.steps, args.file), ("batch", steps, (job.parent / "input.pdf").resolve()))
        malformed = {"version": 1, "command": [], "input": [], "output": {},
                     "password_file": [], "password_env": {}, "owner_password_file": 1,
                     "owner_password_env": [], "steps": [{}, 7]}
        job.write_text(json.dumps(malformed), encoding="utf-8")
        with self.assertRaises(BadArg) as caught:
            load_job(job, commands)
        self.assertEqual(len(caught.exception.problems), 8)
        self.assertTrue(all("must be" in problem for problem in caught.exception.problems))

    def test_naming_and_collision_safe_publish(self):
        src = self.root / "folder with spaces" / "résumé.pdf"
        src.parent.mkdir()
        src.write_bytes(b"input")
        name = io.resolve_output(src, "info", suffix="_info")
        self.assertEqual(name.name, "résumé_info.pdf")
        target = self.root / "result.pdf"
        target.write_bytes(b"first")
        staged = self.root / ".result.1-12345678.part.pdf"
        staged.write_bytes(b"second")
        self.assertEqual(io.publish(staged, target).name, "result_2.pdf")
        self.assertEqual(target.read_bytes(), b"first")

    def test_two_processes_racing_for_one_name(self):
        target = self.root / "race.pdf"
        stages = []
        for i in range(2):
            stage = self.root / f".race.{os.getpid()}-{i:08x}.part.pdf"
            stage.write_text(str(i))
            stages.append(stage)
        ready = multiprocessing.Event()
        processes = [multiprocessing.Process(target=_publish_process, args=(stage, target, ready)) for stage in stages]
        for process in processes:
            process.start()
        ready.set()
        for process in processes:
            process.join(10)
            self.assertEqual(process.exitcode, 0)
        self.assertEqual({p.name for p in self.root.glob("race*.pdf")}, {"race.pdf", "race_2.pdf"})

    def test_locked_target_retries_then_publishes(self):
        staged, target = self.root / "stage", self.root / "out"
        staged.write_text("x")
        real = io.rename_exclusive
        calls = []
        def rename(a, b):
            calls.append(1)
            if len(calls) == 1:
                raise PermissionError
            return real(a, b)
        with patch.object(io, "rename_exclusive", rename):
            io.publish(staged, target, sleep=lambda _: None)
        self.assertEqual(len(calls), 2)

    def test_posix_publish_never_replaces(self):
        target = self.root / "keep.pdf"
        target.write_text("existing")
        for name, link in (("a", io.os.link), ("b", PermissionError)):  # hard link, then the no-hard-link fallback
            staged = self.root / f"stage-{name}"
            staged.write_text(name)
            with patch.object(io, "RENAME_REPLACES", True), patch.object(io, "_renameat2", None), patch.object(io.os, "link", side_effect=link):
                published = io.publish(staged, target)
            self.assertNotEqual(published, target)
            self.assertEqual(published.read_text(), name)
            self.assertFalse(staged.exists())
        self.assertEqual(target.read_text(), "existing")
        with patch.object(io, "RENAME_REPLACES", True), self.assertRaises(FileExistsError):
            io.rename_exclusive(published, target)

    def test_trailing_separator_is_a_folder_marker_for_every_output_type(self):
        import contextlib
        import io as io_module
        from pdftools import cli
        self.assertEqual(io.folder_marker(Path("out"), "out\\"), "out" + os.sep)
        self.assertEqual(io.folder_marker("out\\", "out\\"), "out" + os.sep)
        self.assertEqual(io.folder_marker(Path("out.pdf"), "out.pdf"), Path("out.pdf"))
        self.assertIsNone(io.folder_marker(None, None))
        self.assertEqual(cli._raw_output(["organize", "a.pdf", "1", "-o", "x\\", "--output=y/"]), "y/")
        job = self.root / "job.json"
        job.write_text(json.dumps({"version": 1, "command": "organize", "input": str(self.fixtures["plain"]),
                                   "recipe": "1", "output": str(self.root / "job-out") + "\\"}), encoding="utf-8")
        # organize declares -o as type=Path, which drops the separator; both markers must still mean "folder".
        for folder, argv in ((self.root / "argv-out", ["organize", str(self.fixtures["plain"]), "1", "-o", str(self.root / "argv-out") + "\\"]),
                             (self.root / "slash-out", ["organize", str(self.fixtures["plain"]), "1", "-o", str(self.root / "slash-out") + "/"]),
                             (self.root / "job-out", ["--job", str(job)])):
            out = io_module.StringIO()
            with patch.object(clierr, "LOG_ROOT", self.root / "logs"), contextlib.redirect_stdout(out), \
                    contextlib.redirect_stderr(io_module.StringIO()):
                code = cli.main(["--json", *argv])
            record = json.loads(out.getvalue().splitlines()[0])
            self.assertEqual(code, 0, record)
            self.assertEqual(Path(record["path"]), folder / "plain_organized.pdf")

    def test_find_tool_order_and_windows_only_fixed_paths(self):
        from pdftools import tools
        on_path = {"gs": "/usr/bin/gs", "google-chrome": "/usr/bin/google-chrome", "soffice": "/usr/bin/soffice"}
        with patch("shutil.which", side_effect=on_path.get), patch.object(tools, "WINDOWS", False):
            self.assertEqual(tools.find_tool("ghostscript"), "/usr/bin/gs")
            self.assertEqual(tools.find_tool("browser"), "/usr/bin/google-chrome")
            self.assertEqual(tools.find_tool("soffice"), "/usr/bin/soffice")
        with patch("shutil.which", side_effect=lambda name: None), patch.object(tools, "WINDOWS", True), \
                patch.object(tools, "_is_file", lambda path: path.endswith("soffice.com")):
            self.assertEqual(tools.find_tool("soffice"), r"C:\Program Files\LibreOffice\program\soffice.com")
            self.assertIsNone(tools.find_tool("browser"))

    def test_staging_path_length_and_copy_publish(self):
        temp = self.root / "t"
        temp.mkdir()
        # 235 characters: the target fits under 240, its in-place staging name does not (any temp root length).
        target = self.root / ("x" * (235 - len(str(self.root)) - 1 - len(".pdf")) + ".pdf")
        self.assertEqual(len(str(target)), 235)
        stage = io.create_staging(target, temp=temp, pid=1, token="12345678")
        self.assertEqual(stage.parent, temp)
        self.assertLessEqual(len(str(stage)), 240)
        stage.write_bytes(b"data")
        copied_to = []
        copy2 = io.shutil.copy2
        def capture_copy(source, destination):
            copied_to.append(Path(destination))
            return copy2(source, destination)
        with patch.object(io.shutil, "copy2", capture_copy):
            published = io.publish(stage, target)
        self.assertEqual(published.read_bytes(), b"data")
        self.assertLessEqual(len(copied_to[0].name), 30)
        self.assertTrue(copied_to[0].name.endswith(".part.pdf"))
        with self.assertRaises(PdfError) as caught:
            io.create_staging(self.root / ("x" * 240), temp=temp)
        self.assertEqual(caught.exception.code, "BAD_ARG")

    def test_staging_context_cleans_failed_stage(self):
        with io.Staging(self.root / "out.pdf") as stage:
            stage.path.write_bytes(b"partial")
            path = stage.path
        self.assertFalse(path.exists())

    def test_directory_publish_across_folders_copies_tree(self):
        staged = self.root / "staged-folder"
        staged.mkdir()
        (staged / "inside.txt").write_text("payload", encoding="utf-8")
        target = self.root / "destination" / "published-folder"
        target.parent.mkdir()
        published = io.publish(staged, target)
        self.assertEqual((published / "inside.txt").read_text(encoding="utf-8"), "payload")
        self.assertFalse(staged.exists())

    def test_temp_folder_is_removed(self):
        with io.temp_folder() as folder:
            self.assertTrue(folder.is_dir())
        self.assertFalse(folder.exists())

    def test_locked_target_eventually_errors(self):
        staged = self.root / "stage"
        staged.write_text("x")
        with patch.object(io, "rename_exclusive", side_effect=PermissionError), patch.object(io.time, "monotonic", side_effect=[0, 0, 0, 3]):
            with self.assertRaises(PdfError) as caught:
                io.publish(staged, self.root / "out", retry_seconds=2, sleep=lambda _: None)
        self.assertEqual(caught.exception.code, "LOCKED")

    def test_stale_stage_sweep(self):
        stale = self.root / ".out.99999999-deadbeef.part.pdf"
        stale.write_text("x")
        os.utime(stale, (0, 0))
        fresh = self.root / ".out.99999999-deadbeef.part2.pdf"
        fresh.write_text("x")
        self.assertIn(stale, io.sweep_stale(self.root, now=5000))
        self.assertTrue(fresh.exists())

    def test_stale_run_temp_sweep(self):
        parent = self.root / "pdftools-temp"
        parent.mkdir()
        stale = parent / "99999999-deadbeef"
        stale.mkdir()
        os.utime(stale, (0, 0))
        self.assertIn(stale, io.sweep_temp(now=5000, parent=parent))

    def test_sweeps_skip_a_leftover_that_cannot_be_deleted(self):
        parent = self.root / "pdftools-temp"
        parent.mkdir()
        held = parent / "99999999-deadbeef"
        held.mkdir()
        os.utime(held, (0, 0))
        part = self.root / ".out.99999999-deadbeef.part.pdf"
        part.mkdir()
        os.utime(part, (0, 0))
        with patch.object(io.shutil, "rmtree", side_effect=PermissionError("in use")):
            self.assertEqual(io.sweep_temp(now=5000, parent=parent), [])
            self.assertEqual(io.sweep_stale(self.root, now=5000), [])
        self.assertTrue(held.exists())

    def test_disk_budget_checks_both_destinations(self):
        source = self.root / "source.pdf"
        source.write_bytes(b"1234")
        usage = type("Usage", (), {"free": 10})()
        with patch.object(io.shutil, "disk_usage", return_value=usage):
            with self.assertRaises(PdfError) as caught:
                io.disk_budget([source], self.root, self.root, multiplier=3)
        self.assertEqual(caught.exception.code, "TOO_BIG")

    def test_secret_spellings_are_redacted(self):
        secret = 'q"\\é漢\nline'
        redacted = clierr.redact(secret + " | " + json.dumps(secret) + " | " + repr(secret), [secret])
        self.assertNotIn("é漢", redacted)
        self.assertNotIn(json.dumps(secret), redacted)
        self.assertNotIn(json.dumps(secret, ensure_ascii=False), redacted)
        self.assertNotIn(repr(secret), redacted)
        from pdftools.secrets import strip_one_newline
        self.assertEqual(strip_one_newline("embedded\n\n"), "embedded\n")

    def test_job_secrets_are_read_before_validation(self):
        secret = 'q"\\é漢\nline'
        secret_file = self.root / "pw.txt"
        secret_file.write_bytes((secret + "\r\n").encode("utf-8"))
        job_path = self.root / "bad-job.json"
        job_path.write_text(json.dumps({"version": 9, "command": "info", "password_file": "pw.txt", "bad": secret}), encoding="utf-8")
        store = SecretStore()
        with self.assertRaises(BadArg):
            load_job(job_path, discover(), secret_store=store)
        self.assertEqual(store.password, secret)
        self.assertNotIn(secret, store.redact(json.dumps({"bad": secret})))
        with self.assertRaises(BadArg):
            store.extract(["--password", secret])
        self.assertNotIn(secret, store.redact(secret))
        owner_file = self.root / "owner.txt"
        owner_file.write_text("owner-secret\n", encoding="utf-8")
        job_path.write_text(json.dumps({"version": 1, "command": "info", "input": "input.pdf",
            "owner_password_file": "owner.txt", "owner_password_env": "PDF_TEST_OWNER"}), encoding="utf-8")
        with patch.dict(os.environ, {"PDF_TEST_OWNER": "owner-from-env"}):
            owner_store = SecretStore()
            load_job(job_path, discover(), secret_store=owner_store)
        self.assertEqual(owner_store.owner_password, "owner-from-env")
        self.assertIn("owner-secret", owner_store.values)
        self.assertEqual(owner_store.env_names, ["PDF_TEST_OWNER"])

    def test_secret_sources_file_env_and_stdin(self):
        store = SecretStore()
        secret_file = self.root / "secret.txt"
        secret_file.write_bytes(b"from-file\r\n")
        clean, value = store.extract(["--password-file", str(secret_file), "info"])
        self.assertEqual((clean, value), (["info"], "from-file"))
        with patch.dict(os.environ, {"PDF_TEST_PASSWORD": "from-env"}):
            env_store = SecretStore()
            _, value = env_store.extract(["--password-env", "PDF_TEST_PASSWORD", "info"])
        self.assertEqual(value, "from-env")
        self.assertEqual(env_store.env_names, ["PDF_TEST_PASSWORD"])
        equal_store = SecretStore()
        with patch.dict(os.environ, {"PDF_TEST_PASSWORD": "from-env"}):
            clean, value = equal_store.extract(["--password-env=PDF_TEST_PASSWORD", "info"])
        self.assertEqual(clean, ["info"])
        self.assertEqual(value, "from-env")
        self.assertEqual(equal_store.env_names, ["PDF_TEST_PASSWORD"])
        password_store = SecretStore()
        clean, value = password_store.extract([f"--password-file={secret_file}", "info"])
        self.assertEqual((clean, value), (["info"], "from-file"))
        owner_file = self.root / "owner.txt"
        owner_file.write_text("owner-file\n", encoding="utf-8")
        owner_store = SecretStore()
        clean, value = owner_store.extract([f"--owner-password-file={owner_file}", "info"])
        self.assertEqual((clean, value, owner_store.owner_password), (["info"], None, "owner-file"))
        with patch.dict(os.environ, {"PDF_TEST_OWNER": "owner-env"}):
            owner_env = SecretStore()
            owner_env.extract(["--owner-password-env=PDF_TEST_OWNER", "info"])
        self.assertEqual(owner_env.owner_password, "owner-env")
        self.assertEqual(owner_env.env_names, ["PDF_TEST_OWNER"])
        for args in (("--password=hidden",), ("--owner-password=hidden",), ("--owner-password", "hidden")):
            refused_store = SecretStore()
            with self.assertRaises(BadArg):
                refused_store.extract([*args, "info"])
            self.assertEqual(refused_store.values, ["hidden"])
        stdin_store = SecretStore()
        with self.assertRaises(BadArg):
            stdin_store.extract(["--password", "-", "info"])
        self.assertEqual(stdin_store.values, [])

    def test_encryption_decision_matrix(self):
        encrypted = crypt.Encryption(True, "u", "o", 4, False)
        self.assertEqual(crypt.read_decision(encrypted), "open-with-password")
        with self.assertRaisesRegex(ValueError, "secret_store is required"):
            crypt.write_decision(encrypted)
        owner_store = SecretStore()
        decision = crypt.write_decision(encrypted, secret_store=owner_store)
        self.assertEqual(decision["method"], "AES-256")
        self.assertEqual(len(decision["owner_password"]), 32)
        self.assertIn("OWNER_RESET", decision["warnings"])
        self.assertEqual(owner_store.owner_password, decision["owner_password"])
        self.assertIn(decision["owner_password"], owner_store.values)
        restricted = crypt.Encryption(True, "", "o", 0, True)
        self.assertEqual(crypt.read_decision(restricted), "open-without-password")
        self.assertEqual(crypt.write_decision(restricted, can_modify=False)["error"], "ENCRYPTED")
        self.assertEqual(crypt.write_decision(restricted, can_assemble=False)["error"], "ENCRYPTED")
        self.assertTrue(crypt.write_decision(restricted, can_modify=False, can_assemble=False, owner_password="owner")["encrypt"])
        ignored_store = SecretStore()
        ignored = crypt.write_decision(restricted, can_modify=False, can_assemble=False,
                                       ignore_restrictions=True, secret_store=ignored_store)
        self.assertIn("RESTRICTIONS_IGNORED", ignored["warnings"])
        self.assertEqual(ignored_store.owner_password, ignored["owner_password"])
        self.assertIn("MIXED_ENCRYPTION", crypt.combine_decision([encrypted, crypt.Encryption()])["warnings"])
        self.assertIn("MIXED_ENCRYPTION", crypt.combine_decision([crypt.Encryption(), encrypted])["warnings"])
        self.assertIn("DECRYPTED_EXPORT", crypt.export_warnings(True))

    def test_page_range_parser(self):
        self.assertEqual(pages.parse_page_ranges("1-3,5,odd", 6), [0, 1, 2, 4, 0, 2, 4])
        self.assertEqual(pages.parse_page_ranges("even,5-3,-2", 6), [1, 3, 5, 4, 3, 2, 0, 1])
        with self.assertRaises(PdfError):
            pages.parse_page_ranges("1-9", 6)
        for invalid in ("9-", "-0", "even"):
            with self.subTest(invalid=invalid), self.assertRaises(PdfError) as caught:
                pages.parse_page_ranges(invalid, 6 if invalid != "even" else 1)
            self.assertEqual(caught.exception.code, "BAD_PAGE")

    def test_external_tool_runner_argv_success_failure_and_timeout(self):
        ok = run_tool([sys.executable, "-c", "print('ok')"], timeout=5, temp=self.root)
        self.assertEqual(ok.stdout.strip(), "ok")
        with self.assertRaises(clierr.Fail) as failed:
            run_tool([sys.executable, "-c", "import sys; print('fatal: failed', file=sys.stderr); sys.exit(2)"], timeout=5)
        self.assertEqual(failed.exception.code, "TOOL_FAILED")
        with self.assertRaises(clierr.Fail) as timed:
            run_tool([sys.executable, "-c", "import time; time.sleep(2)"], timeout=0.05)
        self.assertEqual(timed.exception.code, "TIMEOUT")
        with self.assertRaises(clierr.Fail) as missing:
            run_tool([str(self.root / "not-installed.exe")], timeout=5)
        self.assertEqual(missing.exception.code, "NOT_FOUND")
        with patch.dict(os.environ, {"PDF_TEST_PASSWORD": "hidden"}):
            result = run_tool([sys.executable, "-c", "import os; print(os.getenv('PDF_TEST_PASSWORD', 'removed'))"],
                              timeout=5, secret_env=["PDF_TEST_PASSWORD"], secret_values=["hidden"])
        self.assertEqual(result.stdout.strip(), "removed")
        with self.assertRaises(clierr.Fail) as secret_arg:
            run_tool([sys.executable, "-c", "print('hidden')"], secret_values=["hidden"])
        self.assertEqual(secret_arg.exception.code, "BAD_ARG")

    def test_coordinates_absolute_positions_rotated_crop_userunit(self):
        doc = pymupdf.open(self.fixtures["rotated"])
        page = doc[0]
        point = page.rect.tl * page.derotation_matrix
        visible_top_left = pymupdf.Rect(point.x, point.y, point.x, point.y)
        displayed_origin = coords.pymupdf_to_display(page, visible_top_left)
        self.assertAlmostEqual(displayed_origin.x0, 0, places=4)
        self.assertAlmostEqual(displayed_origin.y0, 0, places=4)
        spans = page.get_text("dict")["blocks"][0]["lines"][0]["spans"]
        source = pymupdf.Rect(spans[0]["bbox"])
        displayed = coords.pymupdf_to_display(page, source)
        self.assertAlmostEqual(displayed.x0, 214.081, places=2)
        self.assertAlmostEqual(displayed.y0, 14.111, places=2)
        roundtrip = coords.display_to_pymupdf(page, displayed)
        self.assertLess(abs(roundtrip.x0 - source.x0), 0.01)
        self.assertLess(abs(roundtrip.y1 - source.y1), 0.01)
        rotated = pymupdf.open()
        rpage = rotated.new_page(width=200, height=300)
        rpage.set_rotation(90)
        point = rpage.rect.tl * rpage.derotation_matrix
        origin = coords.pymupdf_to_display(rpage, pymupdf.Rect(point.x, point.y, point.x, point.y))
        self.assertAlmostEqual(origin.x0, 0, places=4)
        self.assertAlmostEqual(origin.y0, 0, places=4)
        rotated.close()
        self.assertLess(abs((coords.from_user_space(page, coords.to_user_space(page, source))).x0 - source.x0), 0.01)
        doc.close()

    def test_info_fixtures_and_json_stdout(self):
        from pdftools.commands.p1_info import info
        from pdftools.cli import Context
        for key in ("plain", "encrypted", "restricted", "signed", "scanned"):
            record = info(type("Args", (), {"file": self.fixtures[key]})(), Context("user-pass" if key == "encrypted" else None))
            self.assertEqual(record["type"], "report")
            self.assertIn("pages", record["data"])
        self.assertTrue(info(type("Args", (), {"file": self.fixtures["signed"]})(), Context()).get("data")["signed"])
        self.assertTrue(info(type("Args", (), {"file": self.fixtures["javascript"]})(), Context())["data"]["javascript"])
        self.assertEqual(info(type("Args", (), {"file": self.fixtures["font_stream"]})(), Context())["data"]["largest_space_user"]["kind"], "fonts")
        self.assertEqual(info(type("Args", (), {"file": self.fixtures["scanned"]})(), Context())["data"]["scanned_pages"], [1])
        restricted_report = info(type("Args", (), {"file": self.fixtures["restricted"]})(), Context())["data"]
        encrypted_report = info(type("Args", (), {"file": self.fixtures["encrypted"]})(), Context("user-pass"))["data"]
        self.assertTrue(encrypted_report["encrypted"])
        owner_report = info(type("Args", (), {"file": self.fixtures["encrypted"]})(), Context(owner_password="owner-pass"))["data"]
        self.assertEqual(owner_report["encryption"]["authenticated_as"], "owner")
        self.assertTrue(restricted_report["encryption"]["restrictions_only"])
        self.assertTrue(restricted_report["encryption"]["owner_password_set"])
        with self.assertRaises(PdfError) as wrong_password:
            info(type("Args", (), {"file": self.fixtures["encrypted"]})(), Context("wrong"))
        self.assertEqual(wrong_password.exception.code, "WRONG_PASSWORD")
        env = dict(os.environ, CLIERR_LOG_ROOT=str(self.root / "logs"))
        proc = subprocess.run([sys.executable, str(ROOT / "pdf.py"), "--json", "info", str(self.fixtures["damaged"])], capture_output=True, text=True, env=env)
        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual([json.loads(line)["type"] for line in proc.stdout.splitlines()], ["error"])
        self.assertNotIn("MuPDF", proc.stderr)
        secret = 'q"\\é漢\nline'
        secret_file = self.root / "password.txt"
        secret_file.write_bytes((secret + "\r\n").encode("utf-8"))
        failing = subprocess.run([sys.executable, str(ROOT / "pdf.py"), "--json", "info",
                                  str(self.fixtures["damaged"]), "--password-file", str(secret_file)],
                                 capture_output=True, text=True, env=env)
        record = json.loads(failing.stdout)
        self.assertEqual(record["type"], "error")
        log = Path(record["log"]).read_text(encoding="utf-8")
        for representation in (secret, json.dumps(secret), json.dumps(secret, ensure_ascii=False), repr(secret)):
            self.assertNotIn(representation, failing.stdout + failing.stderr + log)
        refused = subprocess.run([sys.executable, str(ROOT / "pdf.py"), "--json", "info",
                                   str(self.fixtures["plain"]), "--password", secret],
                                 capture_output=True, text=True, env=env)
        self.assertEqual(refused.returncode, 2)
        self.assertNotIn(secret, refused.stdout + refused.stderr)
        refused_record = json.loads(refused.stdout)
        refused_log = Path(refused_record["log"]).read_text(encoding="utf-8")
        for representation in (secret, json.dumps(secret), json.dumps(secret, ensure_ascii=False), repr(secret)):
            self.assertNotIn(representation, refused_log)
        refused_equal = subprocess.run([sys.executable, str(ROOT / "pdf.py"), "--json", "info",
                                        str(self.fixtures["plain"]), f"--password={secret}"],
                                       capture_output=True, text=True, env=env)
        self.assertEqual(refused_equal.returncode, 2)
        self.assertNotIn(secret, refused_equal.stdout + refused_equal.stderr)
        missing = subprocess.run([sys.executable, str(ROOT / "pdf.py"), "--json", "info",
                                  str(self.root / "missing.pdf")], capture_output=True, text=True, env=env)
        missing_record = json.loads(missing.stdout)
        self.assertEqual(missing_record["code"], "NOT_FOUND")
        self.assertIn("check the path", missing_record["fix"])
        self.assertEqual(len(missing.stderr.splitlines()), 1)
        bad_job_path = self.root / "bad.json"
        bad_job_path.write_text(json.dumps({"version": 1, "command": [], "input": [], "output": {},
            "password_file": [], "password_env": {}, "owner_password_file": 1,
            "owner_password_env": [], "steps": [{}, 3]}), encoding="utf-8")
        malformed = subprocess.run([sys.executable, str(ROOT / "pdf.py"), "--json", "--job", str(bad_job_path)],
                                   capture_output=True, text=True, env=env)
        self.assertEqual(malformed.returncode, 2)
        malformed_record = json.loads(malformed.stdout)
        self.assertEqual(malformed_record["code"], "BAD_ARG")
        self.assertEqual(len(malformed_record["problems"]), 8)
        self.assertEqual(len(malformed.stderr.splitlines()), 1)
        success = subprocess.run([sys.executable, str(ROOT / "pdf.py"), "--json", "info", str(self.fixtures["plain"])], capture_output=True, text=True, env=env)
        self.assertEqual(json.loads(success.stdout)["data"]["pages"], 2)
        unicode_pdf = self.root / "résumé.pdf"
        unicode_pdf.write_bytes(self.fixtures["plain"].read_bytes())
        utf8_env = dict(os.environ, CLIERR_LOG_ROOT=str(self.root / "logs"))
        utf8_env.pop("PYTHONUTF8", None)
        utf8_env.pop("PYTHONIOENCODING", None)
        utf8_result = subprocess.run([sys.executable, str(ROOT / "pdf.py"), "--json", "info", str(unicode_pdf)],
                                     capture_output=True, env=utf8_env)
        utf8_record = json.loads(utf8_result.stdout.decode("utf-8"))
        self.assertEqual(utf8_record["input"], str(unicode_pdf))

    def test_info_detects_javascript_at_each_action_location(self):
        from pdftools.commands.p1_info import info
        from pdftools.cli import Context
        import pikepdf
        for location in ("open_array", "next_array", "page_aa", "annotation_a", "annotation_aa",
                         "field_a", "field_aa", "outline_a"):
            path = self.root / f"javascript-{location}.pdf"
            with pikepdf.open(self.fixtures["plain"]) as pdf:
                js = pikepdf.Dictionary({"/S": pikepdf.Name("/JavaScript"), "/JS": pikepdf.String("x")})
                goto = pikepdf.Dictionary({"/S": pikepdf.Name("/GoTo")})
                if location == "open_array":
                    pdf.Root["/OpenAction"] = pikepdf.Array([js])
                elif location == "next_array":
                    goto["/Next"] = pikepdf.Array([js])
                    pdf.Root["/OpenAction"] = goto
                elif location == "page_aa":
                    pdf.pages[0].obj["/AA"] = pikepdf.Array([js])
                elif location in ("annotation_a", "annotation_aa"):
                    annot = pikepdf.Dictionary({"/Type": pikepdf.Name("/Annot"), "/Subtype": pikepdf.Name("/Text")})
                    annot["/A" if location == "annotation_a" else "/AA"] = js if location == "annotation_a" else pikepdf.Array([js])
                    pdf.pages[0].obj["/Annots"] = pikepdf.Array([annot])
                elif location in ("field_a", "field_aa"):
                    field = pikepdf.Dictionary({"/FT": pikepdf.Name("/Tx"), "/T": pikepdf.String("f")})
                    field["/A" if location == "field_a" else "/AA"] = js if location == "field_a" else pikepdf.Array([js])
                    pdf.Root["/AcroForm"] = pikepdf.Dictionary({"/Fields": pikepdf.Array([pdf.make_indirect(field)])})
                else:
                    outline = pdf.make_indirect(pikepdf.Dictionary({"/Title": pikepdf.String("x"), "/A": js}))
                    pdf.Root["/Outlines"] = pikepdf.Dictionary({"/First": outline, "/Last": outline, "/Count": 1})
                pdf.save(path)
            report = info(type("Args", (), {"file": path})(), Context())
            self.assertTrue(report["data"]["javascript"], location)

    def test_cli_handles_commands_without_file_and_unknown_log_name(self):
        import pdftools.cli as cli
        fake = type("Command", (), {"run": staticmethod(lambda args, context: {"type": "result", "command": "noop"}),
                                     "add_arguments": staticmethod(lambda parser: None)})()
        import contextlib
        import io as io_module
        with patch.object(cli, "discover", return_value={"noop": fake}), \
                patch.object(clierr, "LOG_ROOT", self.root / "logs"), \
                contextlib.redirect_stdout(io_module.StringIO()), \
                contextlib.redirect_stderr(io_module.StringIO()):
                self.assertEqual(cli.main(["noop"]), 0)
                result = cli.main(["--json", "unknown"])
        self.assertEqual(result, 2)
        logs = list((self.root / "logs").glob("pdftools/*.log"))
        self.assertTrue(all("?" not in path.name for path in logs))
        self.assertTrue(any(path.name.endswith("-unknown.log") for path in logs))

    def test_json_error_uses_interrupt_classification(self):
        import contextlib
        import io as io_module
        import pdftools.cli as cli
        fake = type("Command", (), {"run": staticmethod(lambda args, context: (_ for _ in ()).throw(KeyboardInterrupt())),
                                     "add_arguments": staticmethod(lambda parser: None)})()
        out, err = io_module.StringIO(), io_module.StringIO()
        with patch.object(cli, "discover", return_value={"stop": fake}), patch.object(clierr, "LOG_ROOT", self.root / "logs"), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(["--json", "stop"])
        record = json.loads(out.getvalue())
        self.assertEqual((code, record["code"]), (1, "INTERRUPTED"))
        self.assertEqual(len(err.getvalue().splitlines()), 1)


if __name__ == "__main__":
    unittest.main()
