from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import pymupdf

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))

from tests.fixtures.p8 import build, build_typeset_variants
from pdftools.commands import p8_eqread, p8_extract, p8_fix, p8_md, p8_qa
from pdftools.commands import p8_codex
from pdftools.commands import p8_tables
from pdftools.commands.p8_io import atomic_write

PDF = ROOT / "pdf.py"


class P8EquationTests(unittest.TestCase):
    def _typeset_pdf(self):
        return ROOT / "tests" / "fixtures" / "data" / "p8_typeset.pdf"

    def test_parse_answer(self):
        self.assertEqual(p8_eqread.parse_answer("E1\t\\frac{d}{dt}\n`E2: $x = 1 \\quad (2.3)$`\nnoise\nE10 | y\n"),
                         {1: "\\frac{d}{dt}", 2: "x = 1 \\quad (2.3)", 10: "y"})

    def test_for_check_and_doubtful(self):
        self.assertEqual(p8_eqread.for_check("x = 1 \\tag{2.3}"), "x = 1  \\quad (2.3)")
        same = lambda a, b: p8_eqread.doubtful({"problems": [], "checked": False,
            "readings": [{"by": "pass2", "latex": a}, {"by": "pass3", "latex": b}]})
        self.assertTrue(same("\\frac{x}{y}", "\\frac{y}{x}"))
        self.assertFalse(same("\\|x\\| \\quad (1.2)", "\\lVert x\\rVert\\tag{1.2}"))

    def test_wanted_passes(self):
        ok = {"problems": [], "minor": [], "checked": True}
        self.assertTrue(p8_eqread.wanted({**ok, "readings": []}, "pass1"))
        self.assertFalse(p8_eqread.wanted({**ok, "readings": [{"by":"pass1","latex":"x"}]}, "pass2"))
        self.assertTrue(p8_eqread.wanted({**ok, "minor":["neighbour"], "readings":[{"by":"pass1","latex":"x"}]}, "pass2"))
        self.assertTrue(p8_eqread.wanted({**ok, "problems":["symbols"], "readings":[{"by":"pass1","latex":"x"},{"by":"pass2","latex":"x"}]}, "pass3"))

    def test_merge_formulas(self):
        f = lambda i, y0, y1, cls="formula": {"index": i, "class": cls, "bbox": (100, y0, 500, y1)}
        got = p8_extract.merge_formulas([f(0, 300, 322), f(1, 325, 333), f(2, 331, 346), f(3, 360, 380, "text"), f(4, 390, 400)])
        self.assertEqual([(b["index"], b["bbox"][1], b["bbox"][3]) for b in got],
                         [(0, 300, 322), (1, 325, 346), (3, 360, 380), (4, 390, 400)])

    def test_typeset_equations_are_extracted_and_crops_are_calibrated(self):
        from PIL import Image
        import numpy as np
        with tempfile.TemporaryDirectory() as td:
            folder = Path(p8_extract.convert(str(self._typeset_pdf()), td, "0-0", "typeset", read=False))
            eqs = json.loads((folder / "eqs.json").read_text(encoding="utf-8"))
            self.assertGreaterEqual(len(eqs), 3)
            self.assertTrue((folder / "typeset.src.md").read_text(encoding="utf-8").count("<!--eq:") >= 3)
            self.assertTrue(all((folder / e["crop"]).is_file() for e in eqs))
            with pymupdf.open(self._typeset_pdf()) as source:
                render_size = (round(source[0].rect.width * 200 / 72), round(source[0].rect.height * 200 / 72))
            heights = []
            for eq in eqs:
                with Image.open(folder / eq["crop"]) as im:
                    self.assertLessEqual(im.width, max(render_size))
                    self.assertLessEqual(im.height, max(render_size))
                    ink = np.asarray(im.convert("L")) < 160
                    rows = np.flatnonzero(ink.any(axis=1))
                    bands = np.split(rows, np.flatnonzero(np.diff(rows) > 1) + 1)
                    heights.extend(len(band) for band in bands if len(band))
            self.assertGreaterEqual(float(np.median(heights)), 20)
            self.assertLessEqual(float(np.median(heights)), 30)

    def test_rotated_typeset_equation_crops_are_upright_and_match(self):
        from PIL import Image
        with tempfile.TemporaryDirectory() as td:
            td = Path(td)
            normal_dir = Path(p8_extract.convert(str(self._typeset_pdf()), str(td), "0-0", "normal", read=False))
            normal = json.loads((normal_dir / "eqs.json").read_text(encoding="utf-8"))
            import numpy as np
            for rotation in (90, 180, 270):
                with self.subTest(rotation=rotation):
                    rotated_pdf = td / f"rotated-{rotation}.pdf"
                    source = pymupdf.open(self._typeset_pdf())
                    rotated_doc = pymupdf.open()
                    rotated_doc.insert_pdf(source)
                    rotated_doc[0].set_rotation(rotation)
                    rotated_doc.save(rotated_pdf)
                    rotated_doc.close(); source.close()
                    rotated_dir = Path(p8_extract.convert(str(rotated_pdf), str(td), "0-0",
                                                           f"rotated-{rotation}", read=False))
                    rotated = json.loads((rotated_dir / "eqs.json").read_text(encoding="utf-8"))
                    self.assertGreaterEqual(len(rotated), 3)
                    for left, right in zip(normal, rotated):
                        with Image.open(normal_dir / left["crop"]) as a, Image.open(rotated_dir / right["crop"]) as b:
                            self.assertGreater(a.width, a.height)
                            self.assertGreater(b.width, b.height)
                            b = b.resize(a.size)
                            difference = float(np.abs(np.asarray(a.convert("L"), dtype=float) -
                                                      np.asarray(b.convert("L"), dtype=float)).mean())
                            self.assertLess(difference, 35, f"asymmetric glyphs turned for /Rotate {rotation}")

    def test_userunit_and_offset_rotated_crop_equation_coordinates(self):
        from PIL import Image
        import numpy as np
        with tempfile.TemporaryDirectory() as td:
            td = Path(td)
            variants = build_typeset_variants(td)
            plain_dir = Path(p8_extract.convert(str(self._typeset_pdf()), str(td), "0-0", "plain_coords", read=False))
            outputs = {name: Path(p8_extract.convert(str(path), str(td), "0-0", name, read=False))
                       for name, path in variants.items()}
            plain_eqs = json.loads((plain_dir / "eqs.json").read_text(encoding="utf-8"))
            plain_pages = json.loads((plain_dir / "plain_coords.pages.json").read_text(encoding="utf-8"))
            plain_boxes = {block["eq"]: block["bbox"] for block in plain_pages["pages"][0]["children"]
                           if block.get("eq")}
            with pymupdf.open(self._typeset_pdf()) as source:
                page_height = source[0].rect.height
            for name, folder in outputs.items():
                actual_eqs = json.loads((folder / "eqs.json").read_text(encoding="utf-8"))
                actual_pages = json.loads((folder / f"{name}.pages.json").read_text(encoding="utf-8"))
                actual_boxes = {block["eq"]: block["bbox"] for block in actual_pages["pages"][0]["children"]
                                if block.get("eq")}
                self.assertEqual([item["id"] for item in actual_eqs], [item["id"] for item in plain_eqs])
                for base, item in zip(plain_eqs, actual_eqs):
                    with Image.open(plain_dir / base["crop"]) as a, Image.open(folder / item["crop"]) as b:
                        b = b.resize(a.size)
                        self.assertLess(float(np.abs(np.asarray(a.convert("L"), dtype=float) -
                                                     np.asarray(b.convert("L"), dtype=float)).mean()), 35)
                    x0, y0, x1, y1 = plain_boxes[base["id"]]
                    if name == "userunit":
                        expected_display = (2 * x0, 2 * y0, 2 * x1, 2 * y1)
                        # Scale the displayed box by /UserUnit, then hand-convert it
                        # back to the unscaled PDF user origin.
                        expected_user = (x0, page_height - y1, x1, page_height - y0)
                    else:
                        # CropBox [20 100 575 800] has PyMuPDF top offset 42 and width 555.
                        expected_display = (y0 - 42, 555 - (x1 - 20), y1 - 42, 555 - (x0 - 20))
                        dx0, dy0, dx1, dy1 = expected_display
                        local_x0, local_y0 = 555 - dy1, dx0
                        local_x1, local_y1 = 555 - dy0, dx1
                        expected_user = (20 + local_x0, 800 - local_y0,
                                         20 + local_x1, 800 - local_y1)
                    for actual, expected in zip(actual_boxes[item["id"]], expected_display):
                        self.assertAlmostEqual(actual, expected, delta=3)
                    actual_user = item["box"]
                    self.assertLessEqual(actual_user[0], expected_user[0] + 3)
                    self.assertGreaterEqual(actual_user[1], expected_user[1] - 3)
                    self.assertGreaterEqual(actual_user[2], expected_user[2] - 3)
                    self.assertLessEqual(actual_user[3], expected_user[3] + 3)

    def test_linux_codex_argv(self):
        got = p8_codex.linux_argv("codex", "gpt-6.1-sol", "high", "/work", "/work/prompt.answer.md", ["a.png", "b.png"])
        self.assertEqual(got, ["codex", "exec", "-m", "gpt-6.1-sol", "-c", "model_reasoning_effort=high",
                               "-s", "read-only", "-C", "/work", "--skip-git-repo-check", "-o",
                               "/work/prompt.answer.md", "-i", "a.png", "-i", "b.png", "-"])

    def test_linux_codex_prompt_is_stdin(self):
        with tempfile.TemporaryDirectory() as td:
            folder = Path(td); prompt = folder / "read.md"; prompt.write_text("read this equation", encoding="utf-8")
            Path(str(prompt) + ".answer.md").write_text("E1\tx", encoding="utf-8")
            with patch.object(p8_codex, "WINDOWS", False), \
                 patch.object(p8_codex.shutil, "which", return_value="/usr/bin/codex"), \
                 patch.object(p8_codex.Path, "is_file", return_value=True), \
                 patch.object(p8_codex, "_role_models", return_value={"quick": "gpt-6.1-sol"}), \
                 patch.object(p8_codex, "run_tool") as run:
                self.assertEqual(p8_codex.ask(prompt, folder, "quick", "medium", ["sheet.png"]), "E1\tx")
            argv = run.call_args.args[0]
            self.assertEqual(argv[-1], "-")
            self.assertEqual(run.call_args.kwargs["input"], "read this equation")
            self.assertIn("-i", argv)

    def test_codex_child_environment_strips_password_variable(self):
        from types import SimpleNamespace
        from pdftools.cli import Context
        from pdftools.tools import run_tool as actual_run_tool
        with tempfile.TemporaryDirectory() as td, patch.dict(os.environ,
                {"PDFTOOLS_TEST_PASSWORD": "secret-value", "PDFTOOLS_TEST_VISIBLE": "kept"}):
            folder = Path(td); prompt = folder / "read.md"; prompt.write_text("read", encoding="utf-8")
            captured = []

            def child_probe(argv, **kwargs):
                probe = [sys.executable, "-c", "import json,os;print(json.dumps({k:os.getenv(k) for k in ('PDFTOOLS_TEST_PASSWORD','PDFTOOLS_TEST_VISIBLE')}))"]
                result = actual_run_tool(probe, cwd=folder, env=kwargs["env"],
                                         secret_env=kwargs["secret_env"], secret_values=kwargs["secret_values"])
                captured.append(json.loads(result.stdout))
                Path(argv[argv.index("-o") + 1]).write_text("E1\tx", encoding="utf-8")

            context = Context(password="secret-value", secret_env=("PDFTOOLS_TEST_PASSWORD",))
            context.secret_store = SimpleNamespace(values=["secret-value"])
            with patch.object(p8_codex, "WINDOWS", False), \
                 patch.object(p8_codex.shutil, "which", return_value="codex"), \
                 patch.object(p8_codex.Path, "is_file", return_value=True), \
                 patch.object(p8_codex, "_role_models", return_value={"quick": "model"}), \
                 patch.object(p8_codex, "run_tool", side_effect=child_probe):
                self.assertEqual(p8_codex.ask(prompt, folder, "quick", "medium", [], context=context), "E1\tx")
            self.assertEqual(captured, [{"PDFTOOLS_TEST_PASSWORD": None, "PDFTOOLS_TEST_VISIBLE": "kept"}])

    def test_windows_codex_argv(self):
        with tempfile.TemporaryDirectory() as td:
            folder = Path(td); prompt = folder / "read.md"; prompt.write_text("prompt", encoding="utf-8")
            Path(str(prompt) + ".answer.md").write_text("answer", encoding="utf-8")
            with patch.object(p8_codex, "WINDOWS", True), patch.object(p8_codex.shutil, "which", return_value="powershell.exe"), \
                 patch.object(p8_codex.Path, "is_file", return_value=True), patch.object(p8_codex, "run_tool") as run:
                self.assertEqual(p8_codex.ask(prompt, folder, "review", "high", []), "answer")
            self.assertEqual(run.call_args.args[0][1:5], ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            self.assertIn("-PromptFile", run.call_args.args[0])

    def test_atomic_write_preserves_previous_file_on_failure(self):
        with tempfile.TemporaryDirectory() as td:
            target = Path(td) / "report.md"
            target.write_text("old", encoding="utf-8")
            def fail(temp):
                temp.write_text("partial", encoding="utf-8")
                raise RuntimeError("interrupted")
            with self.assertRaisesRegex(RuntimeError, "interrupted"):
                atomic_write(target, writer=fail)
            self.assertEqual(target.read_text(encoding="utf-8"), "old")
            self.assertEqual(list(Path(td).iterdir()), [target])


class P8CommandTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.pdf = build(self.root / "sample.pdf")

    def tearDown(self):
        self.temp.cleanup()

    def _cli(self, *args):
        env = os.environ.copy()
        env["NODE_PATH"] = "/home/agent/src/windows-tools/pdftools/node_modules"
        return subprocess.run([sys.executable, str(PDF), "--json", *map(str, args)], cwd=ROOT,
                              env=env, text=True, capture_output=True)

    def _md_args(self, pdf, output, *flags):
        import argparse
        parser = argparse.ArgumentParser()
        p8_md.add_arguments_md(parser)
        args = parser.parse_args([str(pdf), "-o", str(output), *flags])
        args.dry_run = False
        return args

    def test_legacy_pdf2md_folder_requires_matching_source_metadata(self):
        other = build(self.root / "other.pdf")
        output = self.root / "legacy"
        part = output / "01_chapter"; part.mkdir(parents=True)
        (part / "chapter.blocks.json").write_text(json.dumps({"pages": []}), encoding="utf-8")
        (part / "chapter.pages.json").write_text(json.dumps({"source_pdf": str(other)}), encoding="utf-8")
        result = self._cli("md", self.pdf, "-o", output, "--no-read", "--no-qa")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("BAD_ARG", result.stderr)
        self.assertFalse((output / ".pdftools-md.json").exists())

        missing_meta = self.root / "legacy-no-source" / "01_chapter"
        missing_meta.mkdir(parents=True)
        (missing_meta / "chapter.blocks.json").write_text(json.dumps({"pages": []}), encoding="utf-8")
        result = self._cli("md", self.pdf, "-o", missing_meta.parent, "--no-read", "--no-qa")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("BAD_ARG", result.stderr)
        self.assertFalse((missing_meta.parent / ".pdftools-md.json").exists())

        wrong_hash = self.root / "legacy-wrong-hash" / "01_chapter"; wrong_hash.mkdir(parents=True)
        (wrong_hash / "chapter.blocks.json").write_text(json.dumps({"pages": []}), encoding="utf-8")
        (wrong_hash / "chapter.pages.json").write_text(json.dumps({"source_pdf": str(self.pdf),
                                                                    "source_sha256": "0" * 64}), encoding="utf-8")
        result = self._cli("md", self.pdf, "-o", wrong_hash.parent, "--no-read", "--no-qa")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("BAD_ARG", result.stderr)
        self.assertFalse((wrong_hash.parent / ".pdftools-md.json").exists())

    def test_table_tokens_preserve_signs_and_unicode(self):
        self.assertNotEqual(p8_tables._words("-2"), p8_tables._words("2"))
        self.assertEqual(p8_tables._coverage(p8_tables._words("café"), p8_tables._words("cafe\u0301")), 1.0)

    def test_table_tesseract_child_environment_strips_password_variable(self):
        from types import SimpleNamespace
        from PIL import Image
        from pdftools.cli import Context
        from pdftools.tools import run_tool as actual_run_tool
        with tempfile.TemporaryDirectory() as td, patch.dict(os.environ,
                {"PDFTOOLS_TEST_PASSWORD": "secret-value", "PDFTOOLS_TEST_VISIBLE": "kept"}):
            image = Path(td) / "table.png"
            Image.new("RGB", (100, 60), "white").save(image)
            context = Context(password="secret-value", secret_env=("PDFTOOLS_TEST_PASSWORD",))
            context.secret_store = SimpleNamespace(values=["secret-value"])
            captured = []

            def child_probe(argv, **kwargs):
                probe = [sys.executable, "-c", "import json,os;print(json.dumps({k:os.getenv(k) for k in ('PDFTOOLS_TEST_PASSWORD','PDFTOOLS_TEST_VISIBLE')}))"]
                result = actual_run_tool(probe, cwd=td, env=kwargs["env"], secret_env=kwargs["secret_env"],
                                         secret_values=kwargs["secret_values"])
                captured.append(json.loads(result.stdout))
                return SimpleNamespace(stdout="")

            with patch.object(p8_tables.shutil, "which", return_value="tesseract"), \
                 patch("pdftools.tools.run_tool", side_effect=child_probe):
                p8_tables._confidence(image, td, context=context)
            self.assertTrue(captured)
            self.assertTrue(all(item == {"PDFTOOLS_TEST_PASSWORD": None, "PDFTOOLS_TEST_VISIBLE": "kept"}
                                for item in captured))

    def test_garbled_table_issue_uses_its_pdf_page_and_only_flags_that_table(self):
        folder = self.root / "table_qa"; folder.mkdir()
        source = folder / "source.pdf"
        doc = pymupdf.open()
        for _ in range(10): doc.new_page(width=300, height=240)
        doc.save(source); doc.close()
        pages = [{"id": f"/page/{pi}/Page/0", "block_type": "Page", "bbox": [0, 0, 300, 240], "html": "",
                  "children": []} for pi in range(10)]
        ordinary = {"id": "/page/0/Table/0", "block_type": "Table", "bbox": [40, 40, 200, 120],
                    "html": "<table><tr><td>A</td></tr></table>"}
        flagged = {"id": "/page/9/Table/0", "block_type": "Table", "bbox": [40, 40, 200, 120],
                   "html": "<table><tr><td>B</td></tr></table>", "table_garbled": True, "table_garbled_page": 10}
        pages[0]["children"].append(ordinary); pages[9]["children"].append(flagged)
        (folder / "part.blocks.json").write_text(json.dumps({"source_pdf": str(source), "pages": pages}), encoding="utf-8")
        (folder / "part.pages.json").write_text(json.dumps({"source_pdf": str(source), "pages": pages}), encoding="utf-8")
        (folder / "part.md").write_text("table", encoding="utf-8")
        (folder / "eqs.json").write_text("[]", encoding="utf-8")
        p8_qa.run(folder, dpi=40)
        report = json.loads((folder / "qa_report.json").read_text(encoding="utf-8"))
        issues = [issue for issue in report["issues"] if issue["reason"] == "table may be garbled; see PDF"]
        self.assertEqual(len(issues), 1)
        self.assertEqual(issues[0]["page"], 10)
        self.assertTrue(issues[0]["crop"].endswith("p010_Table_0.png"), issues[0]["crop"])

    def test_invalid_chunk_sizes_fail_before_output_creation(self):
        for chunk in (0, -1):
            with self.subTest(chunk=chunk):
                output = self.root / f"bad-chunk-{chunk}"
                result = self._cli("md", self.pdf, "-o", output, "--chunk", chunk, "--no-read", "--no-qa")
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertIn("BAD_ARG", result.stderr)
                self.assertFalse(output.exists())

    def test_md_equations_passes_reuse_force_and_no_read(self):
        from pdftools.cli import Context
        pdf = ROOT / "tests" / "fixtures" / "data" / "p8_typeset.pdf"
        output = self.root / "typeset_md"
        calls = []

        def fake_ask(prompt_path, folder, role, effort, images, context=None):
            prompt = Path(prompt_path).read_text(encoding="utf-8")
            pass_number = {"quick": 1, "review": 2}[role]
            if effort == "xhigh": pass_number = 3
            answers = []
            for line in prompt.splitlines():
                match = re.match(r"E(\d+) \(p\.\d+\): PDF text: (.*)", line)
                if not match:
                    continue
                label, hint = int(match.group(1)), match.group(2)
                if "(2.1)" in hint:
                    latex = r"\frac{d\mathbf{u}}{dt} + 2\boldsymbol{\Omega} \times \mathbf{u} = -\frac{1}{\rho}\nabla p + \mathbf{g} \tag{2.1}"
                elif "(2.2)" in hint:
                    first = (r"\frac{\partial v}{\partial t} - fv = -\frac{1}{\rho_0}\frac{\partial p}{\partial x}"
                             if pass_number == 1 else r"\frac{\partial u}{\partial t} - fv = -\frac{1}{\rho_0}\frac{\partial p}{\partial x}")
                    second = r"\frac{\partial w}{\partial t} + fu = -\frac{1}{\rho_0}\frac{\partial p}{\partial y}"
                    latex = rf"\begin{{aligned}}{first} \quad (2.2) \\ {second} \quad (2.3)\end{{aligned}}"
                else:
                    latex = r"f = 2\Omega\sin\varphi, \quad \tilde{u} = \bar{u} + u' \tag{2.4}"
                answers.append(f"E{label}\t{latex}")
            calls.append((role, effort, len(answers)))
            return "\n".join(answers)

        with patch.object(p8_codex, "ask", side_effect=fake_ask):
            extracted = p8_md.md(self._md_args(pdf, output, "--no-read", "--no-qa"), Context())
            self.assertEqual(calls, [])
            result = p8_md.md(self._md_args(pdf, output), Context())
            self.assertEqual(result["verdict"], "warn")
            part = next(output.glob("*/eqs.json")).parent
            eqs = json.loads((part / "eqs.json").read_text(encoding="utf-8"))
            self.assertEqual([e["status"] for e in eqs], ["ok", "doubtful", "ok"],
                             [(e["pdf_text"], e["latex"], e.get("problems")) for e in eqs])
            self.assertIn("EQ_DOUBTFUL", result["warnings"])
            self.assertIn("possible OCR error", (part / f"{part.name}.md").read_text(encoding="utf-8"))
            self.assertEqual([e["by"] for e in eqs[1]["readings"]], ["pass1", "pass2", "pass3"])
            initial_calls = len(calls)
            p8_md.md(self._md_args(pdf, output), Context())
            self.assertEqual(len(calls), initial_calls)
            p8_md.md(self._md_args(pdf, output, "--force"), Context())
            preserved = json.loads((part / "eqs.json").read_text(encoding="utf-8"))
            self.assertEqual([e["readings"] for e in eqs], [e["readings"] for e in preserved])
            preserved[0]["latex"] = r"\frac{d\mathbf{v}}{dt} + 2\boldsymbol{\Omega} \times \mathbf{u} = -\frac{1}{\rho}\nabla p + \mathbf{g} \tag{2.1}"
            preserved[2]["latex"] = preserved[2]["latex"].replace(r"\tilde", r"\bar", 1)
            (part / "eqs.json").write_text(json.dumps(preserved), encoding="utf-8")
            p8_extract.render(str(part))
            p8_qa.run(str(part))
            report = json.loads((part / "qa_report.json").read_text(encoding="utf-8"))
            reasons = " ".join(issue["reason"] for issue in report["issues"])
            self.assertIn("symbols", reasons)
            self.assertIn("accents differ", reasons)
            self.assertFalse((part / "qa_autofix.txt").exists())
            self.assertIsNone(p8_qa.accent_swap(r"\bar{u} + \bar{v}", ["~"], ["¯"]))
            self.assertEqual(p8_qa.accent_swap(r"\bar{u}", ["~"], ["¯"]), r"\tilde{u}")
            preserved[0]["latex"] = r"\frac{a}{"
            (part / "eqs.json").write_text(json.dumps(preserved), encoding="utf-8")
            p8_extract.render(str(part))
            p8_qa.run(str(part))
            report = json.loads((part / "qa_report.json").read_text(encoding="utf-8"))
            self.assertTrue(any("KaTeX" in issue["reason"] for issue in report["issues"]))
            md_path = part / f"{part.name}.md"
            md_path.write_text(md_path.read_text(encoding="utf-8").replace("2.2", ""), encoding="utf-8")
            self.assertNotIn("(2.2)", p8_qa.eq_labels(md_path.read_text(encoding="utf-8")))
            import argparse
            p8_md.mdqa(argparse.Namespace(folder=output), Context())
            report = json.loads((part / "qa_report.json").read_text(encoding="utf-8"))
            self.assertTrue(any("not in Marker" in issue["reason"] or "not in Markdown" in issue["reason"]
                                or "missing" in issue["reason"]
                                for issue in report["issues"]), report["issues"])
            no_read_output = self.root / "unread_md"
            unread = p8_md.md(self._md_args(pdf, no_read_output, "--no-read"), Context())
            self.assertIn("EQ_UNREAD", unread["warnings"])

    def test_md_list_reports_plan_and_repair(self):
        result = self._cli("md", self.pdf, "--list", "--level", "auto")
        self.assertEqual(result.returncode, 0, result.stderr)
        record = json.loads(result.stdout)
        self.assertEqual(record["type"], "report")
        self.assertIn("Chapter One", record["data"])

    def test_md_local_run_and_export(self):
        output = self.root / "book_md"
        result = self._cli("md", self.pdf, "-o", output, "--no-read", "--no-qa", "--level", "auto")
        self.assertIn(result.returncode, (0, 4), result.stdout + result.stderr)
        record = json.loads(result.stdout)
        self.assertEqual(record["kind"], "folder")
        self.assertTrue(record["parts"])
        self.assertTrue(all(isinstance(part.get("converted"), bool) for part in record["parts"]))
        self.assertTrue((output / "00_index.md").exists())
        exp = self._cli("mdexport", output)
        self.assertEqual(exp.returncode, 0, exp.stderr)
        self.assertEqual(json.loads(exp.stdout)["command"], "mdexport")

    def test_input_hash_mismatch_fails(self):
        output = self.root / "book_md"
        result = self._cli("md", self.pdf, "-o", output, "--no-read", "--no-qa")
        self.assertIn(result.returncode, (0, 4), result.stderr)
        altered = self.root / "other.pdf"
        doc = pymupdf.open(self.pdf); doc[0].insert_text((30, 300), "changed"); doc.save(altered); doc.close()
        result = self._cli("md", altered, "-o", output, "--no-read", "--no-qa")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("BAD_ARG", result.stderr)

    def test_fix_unmatched_block(self):
        folder = self.root / "part"; folder.mkdir()
        (folder / "part.blocks.json").write_text(json.dumps({"pages": []}), encoding="utf-8")
        (folder / "part.md").write_text("Some text with x = 1.\n", encoding="utf-8")
        fixes = self.root / "fixes.txt"; fixes.write_text("p3 wrong text\n- y = 2\n+ y = 3\n", encoding="utf-8")
        result = self._cli("mdfix", folder, fixes)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(len(result.stderr.splitlines()), 1, result.stderr)
        self.assertIn("FIX_FAILED", result.stderr)

    def test_missing_input_commands_emit_one_error_line(self):
        cases = [("md", self.root / "missing.pdf"), ("mdread", self.root / "missing"),
                 ("mdqa", self.root / "missing"), ("mdexport", self.root / "missing")]
        for command, path in cases:
            with self.subTest(command=command):
                result = self._cli(command, path)
                self.assertNotEqual(result.returncode, 0)
                lines = result.stderr.splitlines()
                self.assertEqual(len(lines), 1, result.stderr)
                self.assertRegex(lines[0], r"^ERROR pdf " + command + r" [A-Z_]+:")

    def test_fix_applies_and_logs(self):
        folder = self.root / "part"; folder.mkdir()
        (folder / "part.blocks.json").write_text(json.dumps({"pages": []}), encoding="utf-8")
        md = folder / "part.md"; md.write_text("The value is x = 1.\n", encoding="utf-8")
        fixes = self.root / "fixes.txt"
        fixes.write_text("p3 verified\n- x = 1\n+ x = 2\n\nok p4 checked\n", encoding="utf-8")
        result = self._cli("mdfix", folder, fixes)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("x = 2", md.read_text(encoding="utf-8"))
        self.assertIn("| 3 |", (folder / "review_notes.md").read_text(encoding="utf-8"))

    def test_fix_selects_front_matter_markdown_from_blocks_basename(self):
        folder = self.root / "00_front_matter"; folder.mkdir()
        (folder / "00_front_matter.blocks.json").write_text(json.dumps({"pages": []}), encoding="utf-8")
        markdown = folder / "00_front_matter.md"
        markdown.write_text("The value is x = 1.\n", encoding="utf-8")
        fixes = self.root / "front-fixes.txt"
        fixes.write_text("p1 verified\n- x = 1\n+ x = 2\n", encoding="utf-8")
        result = self._cli("mdfix", folder, fixes)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("x = 2", markdown.read_text(encoding="utf-8"))

    def test_export_rejects_input_overlap_and_uses_collision_safe_names(self):
        from pdftools.errors import PdfError
        from pdftools.commands.p8_export import unique_path
        folder = self.root / "part"; folder.mkdir()
        source_pdf = self.root / "source.pdf"
        doc = pymupdf.open(); doc.new_page(); doc.save(source_pdf); doc.close()
        (folder / "part.md").write_text("body", encoding="utf-8")
        (folder / "part.blocks.json").write_text(json.dumps({"source_pdf": str(source_pdf), "pages": [
            {"id": "/page/0/Page/0", "children": []}]}), encoding="utf-8")
        args = __import__("argparse").Namespace(folder=folder, output=folder, skip="index", title=None)
        with self.assertRaisesRegex(PdfError, "overlaps"):
            p8_md.mdexport(args, None)
        destination = self.root / "part.md"
        destination.write_text("old", encoding="utf-8")
        self.assertEqual(unique_path(destination).name, "part_2.md")

    def test_layout_extract_does_not_enable_ocr(self):
        with pymupdf.open(self.pdf) as doc, patch("pymupdf4llm.to_markdown", wraps=__import__("pymupdf4llm").to_markdown) as call:
            p8_extract.extract(str(self.pdf), [1])
            self.assertIs(call.call_args.kwargs["use_ocr"], False)

    def test_table_structure_counts_born_digital_scanned_and_not_checkable(self):
        digital_path = self.root / "ruled-table.pdf"
        doc = pymupdf.open()
        page = doc.new_page(width=300, height=240)
        page.draw_rect(pymupdf.Rect(50, 60, 250, 140), width=1)
        page.draw_line((50, 100), (250, 100), width=1)
        page.draw_line((150, 60), (150, 140), width=1)
        page.insert_text((65, 85), "A", fontsize=11); page.insert_text((165, 85), "B", fontsize=11)
        page.insert_text((65, 125), "C", fontsize=11); page.insert_text((165, 125), "D", fontsize=11)
        doc.save(digital_path); doc.close()
        digital = p8_qa.Pdf(str(digital_path))
        try:
            self.assertEqual(p8_qa._source_table_shape(digital, 0, (45, 180, 255, 100), False), (2, 2))
        finally:
            digital.doc.close()
        scanned = p8_qa.Pdf(str(self.pdf))
        try:
            self.assertEqual(p8_qa._source_table_shape(scanned, 7, (70, 702, 365, 592), True), (3, 2))
            self.assertIsNone(p8_qa._source_table_shape(scanned, 7, (400, 500, 550, 400), True))
        finally:
            scanned.doc.close()

    def test_table_read_acceptance_and_rejection_for_digital_and_ocr(self):
        from unittest.mock import call

        def make_table_folder(folder, scanned=False):
            folder.mkdir()
            pdf_path = folder / "table.pdf"
            doc = pymupdf.open(); page = doc.new_page(width=300, height=240)
            page.draw_rect(pymupdf.Rect(50, 60, 250, 140), width=1)
            page.draw_line((50, 100), (250, 100), width=1)
            page.draw_line((150, 60), (150, 140), width=1)
            for text, point in (("Name", (65, 85)), ("Value", (165, 85)),
                                ("alpha", (65, 125)), ("2", (165, 125))):
                page.insert_text(point, text, fontsize=11)
            doc.save(pdf_path); doc.close()
            page = {"id": "/page/0/Page/0", "block_type": "Page", "bbox": [0, 0, 300, 240],
                    "children": [{"id": "/page/0/Table/0", "block_type": "Table", "bbox": [45, 60, 255, 140],
                                  "text": "| Name Value |\n|---|\n| alpha 2 |", "html": "| Name Value |"}]}
            page_data = {"source_pdf": str(pdf_path), "ocr_pages": [1] if scanned else [], "pages": [page]}
            (folder / "table.pages.json").write_text(json.dumps(page_data), encoding="utf-8")
            (folder / "table.blocks.json").write_text(json.dumps({"pages": [page]}), encoding="utf-8")
            (folder / "table.md").write_text(page["children"][0]["text"], encoding="utf-8")
            (folder / "table.src.md").write_text(page["children"][0]["text"], encoding="utf-8")

        accepted = "| Name | Value |\n|---|---|\n| alpha | 2 |"
        with tempfile.TemporaryDirectory() as td:
            td = Path(td)
            digital = td / "digital"; make_table_folder(digital)
            with patch.object(p8_codex, "ask", return_value=f"T1\t{accepted.replace(chr(10), ' || ')}") as ask:
                p8_eqread.run([str(digital)])
                ask.assert_called_once()
            self.assertIn("| alpha | 2 |", (digital / "table.md").read_text(encoding="utf-8"))

            rejected = td / "rejected"; make_table_folder(rejected)
            with patch.object(p8_codex, "ask", return_value="T1\t| wrong | || |---| || |too few |"):
                p8_eqread.run([str(rejected)])
            self.assertIn("table may be garbled; see PDF p. 1", (rejected / "table.md").read_text(encoding="utf-8"))

            scanned = td / "scanned"; make_table_folder(scanned, scanned=True)
            with patch.object(p8_tables, "_confidence", return_value=40), \
                 patch.object(p8_qa.Pdf, "text_in_box", return_value=""), \
                 patch.object(p8_codex, "ask", side_effect=[f"T1\t{accepted.replace(chr(10), ' || ')}",
                                                               f"T1\t{accepted.replace(chr(10), ' || ')}"]) as ask:
                p8_eqread.run([str(scanned)])
                self.assertEqual(ask.call_count, 2)
            self.assertIn("| alpha | 2 |", (scanned / "table.md").read_text(encoding="utf-8"))

            confident = td / "confident"; make_table_folder(confident, scanned=True)
            page_data = json.loads((confident / "table.pages.json").read_text(encoding="utf-8"))
            page_data["pages"][0]["children"][0]["text"] = accepted
            (confident / "table.pages.json").write_text(json.dumps(page_data), encoding="utf-8")
            with patch.object(p8_tables, "_confidence", return_value=90), patch.object(p8_codex, "ask") as ask:
                p8_eqread.run([str(confident)])
                ask.assert_not_called()

            unknown_shape = td / "unknown_shape"; make_table_folder(unknown_shape, scanned=True)
            with patch.object(p8_tables, "_confidence", return_value=90), \
                 patch.object(p8_qa, "_source_table_shape", return_value=None), \
                 patch.object(p8_codex, "ask", side_effect=[f"T1\t{accepted.replace(chr(10), ' || ')}",
                                                               f"T1\t{accepted.replace(chr(10), ' || ')}"]) as ask:
                p8_eqread.run([str(unknown_shape)])
                self.assertEqual(ask.call_count, 2)

    def test_pass1_equation_reading_is_saved_if_table_review_fails(self):
        from PIL import Image
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as td:
            folder = Path(td) / "part"
            folder.mkdir()
            pdf_path = folder / "table.pdf"
            doc = pymupdf.open()
            page = doc.new_page(width=300, height=240)
            page.insert_text((65, 85), "Equation x = 1", fontsize=11)
            page.draw_rect(pymupdf.Rect(50, 100, 250, 180), width=1)
            page.draw_line((50, 140), (250, 140), width=1)
            page.draw_line((150, 100), (150, 180), width=1)
            for text, point in (("Name", (65, 125)), ("Value", (165, 125)),
                                ("alpha", (65, 165)), ("2", (165, 165))):
                page.insert_text(point, text, fontsize=11)
            doc.save(pdf_path)
            doc.close()
            crop = folder / "table.png"
            Image.new("RGB", (200, 80), "white").save(crop)
            table = {"id": 1, "label": 1, "page": 1, "box": (45, 180, 255, 100), "shape": (2, 2),
                     "current": "| Name | Value |\n|---|---|\n| alpha | 2 |", "scanned": True,
                     "image": str(crop), "source_id": "/page/0/Table/0", "block_id": "/page/0/Table/0",
                     "blocks_path": folder / "part.blocks.json", "pages_path": folder / "part.pages.json",
                     "pdf_path": str(pdf_path)}
            table_data = {"id": table["source_id"], "block_type": "Table", "bbox": [45, 100, 255, 180],
                          "html": table["current"], "text": table["current"]}
            (folder / "part.blocks.json").write_text(json.dumps({"pages": [{"children": [table_data]}]}),
                                                      encoding="utf-8")
            (folder / "part.pages.json").write_text(json.dumps({"source_pdf": str(pdf_path), "ocr_pages": [1],
                "pages": [{"id": "/page/0/Page/0", "children": [table_data]}]}), encoding="utf-8")
            (folder / "part.md").write_text(table["current"], encoding="utf-8")
            (folder / "part.src.md").write_text(table["current"], encoding="utf-8")
            eq_crop = folder / "eq.png"
            Image.new("RGB", (180, 50), "white").save(eq_crop)
            eqs = [{"id": "e1", "page": 1, "box": [60, 70, 150, 95], "crop": "eq.png", "pdf_text": "x = 1",
                    "bold": "", "labels": [], "readings": [], "latex": None}]
            (folder / "eqs.json").write_text(json.dumps(eqs), encoding="utf-8")
            answer = "E1\tx = 1\nT1\t| Name | Value | || |---|---| || | alpha | 2 |"
            with patch.object(p8_tables, "pending", return_value=[table]), \
                 patch.object(p8_codex, "ask", side_effect=[answer, RuntimeError("table review failed")]):
                with self.assertRaisesRegex(RuntimeError, "table review failed"):
                    p8_eqread.run([str(folder)])
            saved = json.loads((folder / "eqs.json").read_text(encoding="utf-8"))
            self.assertEqual(saved[0]["readings"], [{"by": "pass1", "latex": "x = 1"}])
            table_checkpoint = json.loads((folder / "table_readings.json").read_text(encoding="utf-8"))
            self.assertEqual(table_checkpoint[table["block_id"]]["pass1"],
                             "Name | Value |\n|---|---|\n| alpha | 2 |")


    def test_shared_missing_text_alignment_keeps_columns_and_finds_lost_prose(self):
        pdf = p8_qa.Pdf(str(self.pdf))
        try:
            page_text = pdf.textpage(5).get_text_range()
            _, runs = p8_qa.lost_runs(pdf, 5, page_text, [], 5)
            self.assertEqual(runs, [])
            tokens, runs = p8_qa.lost_runs(pdf, 2, "", [], 5)
            self.assertTrue(tokens)
            self.assertTrue(runs)
        finally:
            pdf.doc.close()

    def test_reference_gate_counts_marker_missing_text_and_compares_crop_pixels(self):
        from PIL import Image
        from tests.p8_reference import compare_part
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            baseline, actual = root / "baseline", root / "actual"
            baseline.mkdir(); actual.mkdir()
            # As in reality: pdf2md's crop is the large 200 dpi one; the new crop is that crop downscaled.
            from PIL import ImageDraw
            detailed = Image.new("RGB", (300, 90), "white")
            draw = ImageDraw.Draw(detailed)
            for x in range(10, 290, 14):
                draw.rectangle((x, 20, x + 6, 70), fill="black")
            detailed.save(baseline / "eq.png")
            detailed.resize((100, 30), Image.LANCZOS).save(actual / "eq.png")
            eq = {"id": "e1", "box": [10, 20, 30, 40], "crop": "eq.png", "crop_sha": "obsolete"}
            (baseline / "eqs.json").write_text(json.dumps([eq]), encoding="utf-8")
            (actual / "eqs.json").write_text(json.dumps([{**eq, "crop_sha": "different"}]), encoding="utf-8")
            (baseline / "qa_report.json").write_text(json.dumps({"counts": {}, "issues": []}), encoding="utf-8")
            (actual / "qa_report.json").write_text(json.dumps({"counts": {}, "issues": [
                {"page": 1, "box": [10, 10, 100, 20],
                 "reason": "PDF text missing from Marker (8 words): recovered paragraph"}]}), encoding="utf-8")
            (actual / "part.pages.json").write_text(json.dumps({"pages": []}), encoding="utf-8")
            differences = compare_part("part", baseline, actual)
            self.assertTrue(any("lost-text issues increased (0 -> 1)" in item for item in differences), differences)
            self.assertFalse(any("crop pixels differ" in item for item in differences), differences)
            # A different equation at the new size must still be caught.
            other = Image.new("RGB", (100, 30), "white")
            ImageDraw.Draw(other).rectangle((5, 5, 95, 25), fill="black")
            other.save(actual / "eq.png")
            differences = compare_part("part", baseline, actual)
            self.assertTrue(any("crop pixels differ" in item for item in differences), differences)

    def test_crop_calibration_downscales_but_never_upscales(self):
        from PIL import Image, ImageDraw
        with pymupdf.open(self.pdf) as doc:
            page = doc[2]
            large = Image.new("RGB", (160, 160), "white")
            draw = ImageDraw.Draw(large)
            for y in (10, 60, 110): draw.rectangle((20, y, 140, y + 39), fill="black")
            path = self.root / "large.png"; large.save(path)
            p8_extract._calibrate_crop(path, page, (0, 0, page.rect.width, page.rect.height))
            with Image.open(path) as calibrated:
                self.assertLess(calibrated.height, large.height)
            small = Image.new("RGB", (50, 50), "white")
            draw = ImageDraw.Draw(small)
            for y in (5, 20, 35): draw.rectangle((5, y, 45, y + 9), fill="black")
            path2 = self.root / "small.png"; small.save(path2)
            p8_extract._calibrate_crop(path2, page, (0, 0, page.rect.width, page.rect.height))
            with Image.open(path2) as calibrated:
                self.assertEqual(calibrated.size, small.size)

    def test_table_crop_uses_equation_orientation_and_line_height_calibration(self):
        from PIL import Image, ImageDraw
        import numpy as np
        folder = self.root / "large_table"; folder.mkdir()
        source = folder / "source.pdf"
        doc = pymupdf.open(); page = doc.new_page(width=500, height=400)
        page.draw_rect(pymupdf.Rect(40, 40, 460, 360), width=2)
        for y in (120, 200, 280): page.draw_line((40, y), (460, y), width=2)
        page.insert_text((70, 100), "Station 123", fontsize=48)
        page.insert_text((70, 180), "Rainfall 456", fontsize=48)
        page.insert_text((70, 260), "Temperature 789", fontsize=48)
        doc.save(source); doc.close()
        table = {"id": "/page/0/Table/0", "block_type": "Table", "bbox": [40, 40, 460, 360],
                 "text": "| Station | Value |", "html": "<table></table>"}
        page_data = {"source_pdf": str(source), "pages": [{"id": "/page/0/Page/0", "children": [table]}]}
        (folder / "part.pages.json").write_text(json.dumps(page_data), encoding="utf-8")
        (folder / "part.blocks.json").write_text(json.dumps({"source_pdf": str(source), "pages": [page_data["pages"][0]]}), encoding="utf-8")
        from pdftools.commands import p8_extract
        original_calibrate = p8_extract._calibrate_crop

        def calibrate_table(path, page, box):
            synthetic = Image.new("RGB", (160, 160), "white")
            draw = ImageDraw.Draw(synthetic)
            for y in (10, 60, 110): draw.rectangle((20, y, 140, y + 39), fill="black")
            synthetic.save(path)
            return original_calibrate(path, page, box)

        with patch.object(p8_extract, "_calibrate_crop", side_effect=calibrate_table) as calibration:
            pending = p8_tables.prepare(folder)
        self.assertTrue(pending)
        calibration.assert_called_once()
        with Image.open(pending[0]["image"]) as crop:
            ink = np.asarray(crop.convert("L")) < 160
            rows = np.flatnonzero(ink.any(axis=1))
            bands = np.split(rows, np.flatnonzero(np.diff(rows) > 1) + 1)
            heights = [len(band) for band in bands if len(band)]
            self.assertGreaterEqual(float(np.median(heights)), 20)
            self.assertLessEqual(float(np.median(heights)), 30)


if __name__ == "__main__":
    unittest.main()
