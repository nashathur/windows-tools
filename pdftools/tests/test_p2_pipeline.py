import contextlib
import hashlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))
import pymupdf

from pdftools import cli
from pdftools.check import Finding, OperationResult, Placement
from pdftools.pipeline import write_pdf
from pdftools.registry import command, get
from tests.fixtures.p1 import build_fixtures


def add_arguments(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--break-signature", action="store_true")
    parser.add_argument("--ignore-restrictions", action="store_true")
    parser.add_argument("--keep-failed", action="store_true")


def copy_writer(doc, path):
    doc.save(path, garbage=3, deflate=True)


def fail_policy(source, output, operation):
    return [Finding("invariant", "TEST_FAIL", "deliberate test failure")]


@command("p2-test-write", 2, "pdf", "pdf", "_test", True, "placements")
def test_write(args, context):
    return write_pdf(args.file, get("p2-test-write"), args, context, copy_writer)


@command("p2-test-map", 2, "pdf", "pdf", "_map", True, "placements")
def test_map(args, context):
    operation = OperationResult(pages=[[Placement(str(args.file), 0)]])
    return write_pdf(args.file, get("p2-test-map"), args, context, copy_writer, operation=operation)


@command("p2-test-encrypted", 2, "pdf", "pdf", "_encrypted", True, "placements")
def test_encrypted(args, context):
    return write_pdf(args.file, get("p2-test-encrypted"), args, context, copy_writer)


@command("p2-test-invariant", 2, "pdf", "pdf", "_bad", True, fail_policy)
def test_invariant(args, context):
    return write_pdf(args.file, get("p2-test-invariant"), args, context, copy_writer)


@command("p2-test-security", 2, "pdf", "pdf", "_unsafe", True, fail_policy)
def test_security(args, context):
    return write_pdf(args.file, get("p2-test-security"), args, context, copy_writer, security=True)


@command("p2-test-keep", 2, "pdf", "pdf", "_keep", True, "placements")
def test_keep(args, context):
    return write_pdf(args.file, get("p2-test-keep"), args, context,
                     lambda doc, path: (copy_writer(doc, path), OperationResult(keep_input="NO_GAIN"))[1])


class PipelineTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.fixtures = build_fixtures(self.root / "fixtures")
        self.fixtures["encrypted_allowed"] = self.root / "fixtures" / "encrypted-allowed.pdf"
        doc = pymupdf.open(self.fixtures["plain"])
        permissions = (pymupdf.PDF_PERM_ACCESSIBILITY | pymupdf.PDF_PERM_PRINT | pymupdf.PDF_PERM_COPY |
                       pymupdf.PDF_PERM_MODIFY | pymupdf.PDF_PERM_ANNOTATE | pymupdf.PDF_PERM_ASSEMBLE | pymupdf.PDF_PERM_FORM)
        doc.save(self.fixtures["encrypted_allowed"], encryption=pymupdf.PDF_ENCRYPT_AES_256,
                 owner_pw="owner-pass", user_pw="user-pass", permissions=permissions)
        doc.close()
        with pymupdf.open(self.fixtures["encrypted_allowed"]) as encrypted_doc:
            self.assertEqual(encrypted_doc.authenticate("user-pass"), 2)
            self.assertIn("Climate", encrypted_doc[0].get_text())
        from pdftools.pipeline import open_input
        with open_input(self.fixtures["encrypted_allowed"], "user-pass") as opened:
            self.assertIn("Climate", opened[0].get_text())

    def tearDown(self):
        self.tmp.cleanup()

    def run_cli(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with patch.object(cli, "discover", wraps=cli.discover), \
                patch("clierr.LOG_ROOT", self.root / "logs"), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(["--json", *map(str, argv)])
        return code, json.loads(out.getvalue()) if out.getvalue().strip() else None, err.getvalue()

    def test_ok_publish_collision_and_input_hash(self):
        source = self.fixtures["plain"]
        before = hashlib.sha256(source.read_bytes()).hexdigest()
        output = self.root / "result.pdf"
        code, record, _ = self.run_cli(["p2-test-write", source, "-o", output])
        self.assertEqual((code, record["verdict"], Path(record["path"])), (0, "ok", output))
        code, second, err = self.run_cli(["p2-test-write", source, "-o", output])
        self.assertEqual((code, Path(second["path"]).name), (0, "result_2.pdf"))
        self.assertIn("NAME_TAKEN", second["warnings"])
        self.assertIn("WARN pdf p2-test-write NAME_TAKEN:", err)
        self.assertEqual(hashlib.sha256(source.read_bytes()).hexdigest(), before)

    def test_keep_input_publishes_nothing_and_points_at_input(self):
        source = self.fixtures["plain"]
        code, record, err = self.run_cli(["p2-test-keep", source])
        self.assertEqual((code, Path(record["path"]), record["verdict"], record["warnings"]),
                         (0, source.resolve(), "warn", ["NO_GAIN"]))
        self.assertEqual(sorted(p.name for p in source.parent.iterdir() if "_keep" in p.name or ".part" in p.name), [])
        self.assertIn("WARN pdf p2-test-keep NO_GAIN:", err)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            cli.emit(dict(record, note="1000 → 1000 bytes (0 %)"), False)
        self.assertTrue(out.getvalue().rstrip().endswith("verdict warn; 1000 → 1000 bytes (0 %)"))

    def test_invariant_failure_publishes_unchecked_and_exit_three(self):
        source = self.fixtures["plain"]
        code, record, err = self.run_cli(["p2-test-invariant", source])
        self.assertEqual(code, 3)
        self.assertEqual(record["verdict"], "unchecked")
        self.assertTrue(Path(record["path"]).name.endswith("_UNCHECKED.pdf"))
        self.assertTrue(record["findings"])
        self.assertTrue(Path(record["log"]).is_file())
        self.assertIn("WARN pdf p2-test-invariant TEST_FAIL:", err)

    def test_security_failure_publishes_nothing_or_named_diagnostic(self):
        source = self.fixtures["plain"]
        code, record, _ = self.run_cli(["p2-test-security", source])
        self.assertEqual(code, 3, record)
        self.assertEqual((record["verdict"], record["path"]), ("failed", None))
        code, record, _ = self.run_cli(["p2-test-security", source, "--keep-failed"])
        self.assertEqual(code, 3)
        self.assertEqual(record["verdict"], "failed")
        self.assertTrue(Path(record["path"]).name.endswith("_FAILED_UNSAFE.pdf"))

    def test_dry_run_creates_no_output_or_stage(self):
        source = self.fixtures["plain"]
        target = self.root / "dry.pdf"
        code, record, _ = self.run_cli(["--dry-run", "p2-test-write", source, "-o", target])
        self.assertEqual((code, record["verdict"], record["provisional"], record["path"]), (0, "report", True, str(target)))
        self.assertFalse(target.exists())
        self.assertEqual(list(self.root.glob(".*.part.pdf")), [])
        self.assertEqual(record["page_map"], [])

    def test_dry_run_json_lines_has_report_verdict_and_serialized_page_map(self):
        out, err = io.StringIO(), io.StringIO()
        with patch("clierr.LOG_ROOT", self.root / "logs"), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(["--json", "--dry-run", "p2-test-write", str(self.fixtures["plain"])])
        rows = [json.loads(line) for line in out.getvalue().splitlines()]
        self.assertEqual(code, 0)
        self.assertEqual(len(rows), 1)
        self.assertEqual((rows[0]["verdict"], rows[0]["provisional"]), ("report", True))
        json.dumps(rows[0]["page_map"])

    def test_dry_run_serializes_placement_objects(self):
        code, record, _ = self.run_cli(["--dry-run", "p2-test-map", self.fixtures["plain"]])
        self.assertEqual(code, 0)
        self.assertEqual(record["verdict"], "report")
        self.assertEqual(record["page_map"][0][0]["source_page"], 0)
        self.assertIsInstance(record["page_map"][0][0]["source_doc"], str)

    def test_encrypted_roundtrip_and_signature_gate(self):
        password_file = self.root / "password.txt"
        password_file.write_text("user-pass\n", encoding="utf-8")
        code, record, _ = self.run_cli(["p2-test-encrypted", self.fixtures["encrypted_allowed"], "--password-file", password_file])
        self.assertEqual(code, 0, (record, _))
        with pymupdf.open(record["path"]) as doc:
            self.assertTrue(doc.needs_pass)
            self.assertTrue(doc.authenticate("user-pass"))
            self.assertIn("Climate", doc[0].get_text())
        code, record, _ = self.run_cli(["p2-test-write", self.fixtures["signed"]])
        self.assertEqual((code, record["code"]), (1, "SIGNED"))
        code, record, err = self.run_cli(["p2-test-write", self.fixtures["signed"], "--break-signature"])
        self.assertEqual(code, 0)
        self.assertIn("SIGNATURE_INVALIDATED", record["warnings"])

    def test_restrictions_gate_and_override(self):
        code, record, _ = self.run_cli(["p2-test-write", self.fixtures["restricted"]])
        self.assertEqual((code, record.get("code")), (1, "ENCRYPTED"), record)
        code, record, _ = self.run_cli(["p2-test-write", self.fixtures["restricted"], "--ignore-restrictions"])
        self.assertEqual(code, 0)
        self.assertIn("RESTRICTIONS_IGNORED", record["warnings"])

    def test_user_password_with_restrictions_is_allowed(self):
        password = self.root / "user.txt"
        password.write_text("user-pass\n", encoding="utf-8")
        code, record, _ = self.run_cli(["p2-test-write", self.fixtures["encrypted"], "--password-file", password])
        self.assertEqual(code, 0, record)
        self.assertTrue(Path(record["path"]).is_file())

    def test_bad_owner_password_reports_owner_password_fix(self):
        password = self.root / "owner.txt"
        password.write_text("wrong-owner\n", encoding="utf-8")
        code, record, _ = self.run_cli(["p2-test-write", self.fixtures["restricted"], "--owner-password-file", password])
        self.assertEqual((code, record["code"]), (1, "WRONG_PASSWORD"), record)
        self.assertIn("--owner-password-file", record["fix"])

    def test_default_name_collision_silently_gets_suffix_two(self):
        source = self.fixtures["plain"]
        first_code, first, _ = self.run_cli(["p2-test-write", source])
        second_code, second, err = self.run_cli(["p2-test-write", source])
        self.assertEqual((first_code, second_code), (0, 0))
        self.assertEqual(Path(second["path"]).name, "plain_test_2.pdf")
        self.assertNotIn("NAME_TAKEN", second["warnings"])
        self.assertNotIn("NAME_TAKEN", err)


class RunEachTest(unittest.TestCase):
    def test_failing_file_is_recorded_and_sets_exit_4(self):
        from pdftools.errors import PdfError
        from pdftools.pipeline import run_each

        def one(path):
            if path.name == "bad.pdf":
                raise PdfError("BAD_INPUT", "cannot read", "fix it")
            return {"type": "result", "command": "meta", "path": str(path)}

        context = cli.Context(None, False, [], None)
        records = run_each(["a.pdf", "bad.pdf", "c.pdf"], one, "meta", context)
        self.assertEqual([r["type"] for r in records], ["result", "error", "result", "summary"])
        self.assertEqual(records[1]["code"], "BAD_INPUT")
        self.assertEqual((records[-1]["files"], records[-1]["ok"], records[-1]["failed"]), (3, 2, 1))
        self.assertEqual(context.exit_code, 4)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            cli.emit(records, True)
        self.assertEqual(len([json.loads(line) for line in out.getvalue().splitlines()]), 4)
        unchecked = lambda path: {"type": "result", "command": "meta", "path": str(path), "verdict": "unchecked"}
        context = cli.Context(None, False, [], None)
        records = run_each(["a.pdf"], unchecked, "meta", context)
        self.assertEqual((records[-1]["ok"], records[-1]["failed"], context.exit_code), (0, 1, 4))


if __name__ == "__main__":
    unittest.main()
