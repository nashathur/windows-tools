import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
import sys
from unittest.mock import patch

import pikepdf
import pymupdf

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))

from pdftools import cli
from tests.fixtures.p3b import build_fixtures


class Phase3bCommandsTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.fixtures = build_fixtures(self.root / "fixtures")

    def tearDown(self):
        self.tmp.cleanup()

    def run_cli(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with patch("clierr.LOG_ROOT", self.root / "logs"), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(["--json", *map(str, argv)])
        value = json.loads(out.getvalue()) if out.getvalue().strip() else None
        if value and value.get("log") and Path(value["log"]).is_file():
            err.write(Path(value["log"]).read_text(encoding="utf-8", errors="replace"))
        return code, value, err.getvalue()

    def test_organize_recipe_duplicates_pages_and_renames_widgets(self):
        output = self.root / "dupe.pdf"
        code, record, err = self.run_cli(["organize", self.fixtures["main"], "1x2,2-4", "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pymupdf.open(output) as doc:
            self.assertEqual(doc.page_count, 5)
            self.assertEqual(doc[0].get_text().strip(), doc[1].get_text().strip())
            names = [widget.field_name for widget in (doc[0].widgets() or [])]
            names2 = [widget.field_name for widget in (doc[1].widgets() or [])]
            self.assertEqual(names, ["station"])
            self.assertEqual(names2, ["station#2"])
        with pikepdf.open(output) as pdf:
            self.assertEqual(len(pdf.pages), 5)
            self.assertTrue(pdf.Root.get("/Outlines"))
            labels = pdf.Root.get("/PageLabels")
            self.assertFalse(labels and labels.get("/Nums"))
            self.assertTrue(any(str(a.get("/Subtype")) == "/Link" for a in pdf.pages[0].obj.get("/Annots", [])))
            self.assertEqual([str(field.get("/T")) for field in pdf.Root["/AcroForm"]["/Fields"]],
                             ["station", "station#2"])
        self.assertIn("out 1", record["page_map"])

    def test_organize_blank_as_first_item_uses_input_page_one_size(self):
        output = self.root / "blank-first.pdf"
        code, record, err = self.run_cli(["organize", self.fixtures["main"], "blank,1", "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pikepdf.open(output) as pdf:
            self.assertEqual(len(pdf.pages), 2)
            self.assertAlmostEqual(float(pdf.pages[0].MediaBox[2]), 300, places=1)
            self.assertAlmostEqual(float(pdf.pages[0].MediaBox[3]), 400, places=1)

    def test_wrappers_rotation_blank_validation_and_remove_last_page(self):
        rotated = self.root / "rotated.pdf"
        code, record, _ = self.run_cli(["rotate", self.fixtures["main"], "--pages", "2-3", "--by", "90", "-o", rotated])
        self.assertEqual(code, 0, (record, _))
        with pikepdf.open(rotated) as pdf: self.assertEqual(len(pdf.pages), 4)
        with pikepdf.open(rotated) as pdf:
            self.assertEqual(int(pdf.pages[1].obj.get("/Rotate", 0)), 90)
            self.assertTrue(pdf.Root.get("/PageLabels", {}).get("/Nums"))
        blank = self.root / "blank.pdf"
        code, record, _ = self.run_cli(["blank", self.fixtures["main"], "--at", "1,3", "--size", "a4", "-o", blank])
        self.assertEqual(code, 0, (record, _))
        with pikepdf.open(blank) as pdf: self.assertEqual(len(pdf.pages), 6)
        code, error, _ = self.run_cli(["remove", self.fixtures["main"], "--pages", "1-4"])
        self.assertEqual(code, 1)
        self.assertEqual(error["code"], "BAD_PAGE")

    def test_remaining_wrappers_and_extract_default_name(self):
        cases = [
            (["extract", self.fixtures["main"], "--pages", "3-4"], ["A page 3", "A page 4"]),
            (["remove", self.fixtures["main"], "--pages", "2"], ["A page 1", "A page 3", "A page 4"]),
            (["reorder", self.fixtures["main"], "--order", "4,3,2,1"], ["A page 4", "A page 3", "A page 2", "A page 1"]),
            (["move", self.fixtures["main"], "--pages", "2-3", "--after", "4"], ["A page 1", "A page 4", "A page 2", "A page 3"]),
            (["swap", self.fixtures["main"], "--pages", "1,4"], ["A page 4", "A page 2", "A page 3", "A page 1"]),
            (["reverse", self.fixtures["main"], "--pages", "2-4"], ["A page 1", "A page 4", "A page 3", "A page 2"]),
            (["duplicate", self.fixtures["main"], "--pages", "3", "--times", "2"], ["A page 1", "A page 2", "A page 3", "A page 3", "A page 4"]),
        ]
        for index, (argv, expected_text) in enumerate(cases):
            output = self.root / f"wrapper-{index}.pdf"
            code, record, err = self.run_cli([*argv, "-o", output])
            self.assertEqual(code, 0, (argv, record, err))
            with pymupdf.open(output) as pdf:
                self.assertEqual([page.get_text().strip().splitlines()[0] for page in pdf], expected_text)
        extracted = self.fixtures["main"].with_name("main_p3-4.pdf")
        if extracted.exists(): extracted.unlink()
        code, record, err = self.run_cli(["extract", self.fixtures["main"], "--pages", "3-4"])
        self.assertEqual(code, 0, (record, err))
        self.assertEqual(Path(record["path"]), extracted)
        self.assertTrue(extracted.is_file())

    def test_single_input_keeps_javascript_and_attachments(self):
        output = self.root / "single-features.pdf"
        code, record, err = self.run_cli(["organize", self.fixtures["features"], "rev", "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pikepdf.open(output) as pdf:
            self.assertTrue(pdf.Root.get("/OpenAction"))
            self.assertTrue(pdf.Root.get("/Names", {}).get("/EmbeddedFiles"))
            self.assertTrue(pdf.Root.get("/StructTreeRoot"))

    def test_organize_opens_additional_recipe_files(self):
        appendix = self.root / "appendix.pdf"
        appendix.write_bytes(self.fixtures["other"].read_bytes())
        output = self.root / "recipe-extra.pdf"
        job = self.root / "extra.json"
        job.write_text(json.dumps({"version": 1, "command": "organize", "input": str(self.fixtures["main"]),
                                   "recipe": [{"pages": "1"}, {"file": "appendix.pdf", "pages": "2"}],
                                   "output": str(output)}), encoding="utf-8")
        code, record, err = self.run_cli(["--job", job])
        self.assertEqual(code, 0, (record, err))
        with pymupdf.open(output) as doc:
            self.assertEqual([p.get_text().strip().splitlines()[0] for p in doc], ["A page 1", "B page 2"])

    def test_recipe_file_password_is_registered_and_used(self):
        secured = self.root / "secure.pdf"
        password_file = self.root / "secure.pw"
        password_file.write_text("appendix-secret\n", encoding="utf-8")
        with pymupdf.open(self.fixtures["other"]) as source:
            source.save(secured, encryption=pymupdf.PDF_ENCRYPT_AES_256,
                        user_pw="appendix-secret", owner_pw="appendix-owner")
        output, job = self.root / "secure-out.pdf", self.root / "secure.json"
        job.write_text(json.dumps({"version": 1, "command": "organize", "input": str(self.fixtures["main"]),
                                   "recipe": [{"file": str(secured), "pages": "1", "password_file": str(password_file)}],
                                   "output": str(output)}), encoding="utf-8")
        code, record, err = self.run_cli(["--job", job])
        self.assertEqual(code, 0, (record, err))
        self.assertNotIn("appendix-secret", err)
        with pymupdf.open(output) as doc: self.assertEqual(doc[0].get_text().strip(), "B page 1")

    def test_command_password_reaches_catalog_helpers(self):
        secured = self.root / "secure-features.pdf"
        password_file = self.root / "primary.pw"
        password_file.write_text("catalog-secret\n", encoding="utf-8")
        with pymupdf.open(self.fixtures["features"]) as source:
            source.save(secured, encryption=pymupdf.PDF_ENCRYPT_AES_256,
                        user_pw="catalog-secret", owner_pw="catalog-owner")
        output = self.root / "secure-features-out.pdf"
        code, record, err = self.run_cli(["organize", secured, "rev", "--password-file", password_file, "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pikepdf.open(output, password="catalog-secret") as pdf:
            self.assertTrue(pdf.Root.get("/OpenAction"))
            self.assertTrue(pdf.Root.get("/Names", {}).get("/EmbeddedFiles"))

    def test_command_password_reaches_multi_input_feature_inspection(self):
        password_file = self.root / "merge-password.txt"
        password_file.write_text("shared-secret\n", encoding="utf-8")
        secured = []
        for name in ("first-secure.pdf", "second-secure.pdf"):
            path = self.root / name
            with pymupdf.open(self.fixtures["features"]) as source:
                source.save(path, encryption=pymupdf.PDF_ENCRYPT_AES_256,
                            user_pw="shared-secret", owner_pw="shared-owner")
            secured.append(path)
        output = self.root / "secure-merge.pdf"
        code, record, err = self.run_cli(["merge", *secured, "--password-file", password_file, "-o", output])
        self.assertEqual(code, 0, (record, err))
        self.assertIn("FEATURES_DROPPED", record["warnings"])

    def test_range_repeat_is_a_block(self):
        output = self.root / "range-repeat.pdf"
        code, record, err = self.run_cli(["organize", self.fixtures["main"], "1-3x2,4", "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pymupdf.open(output) as doc:
            self.assertEqual([p.get_text().strip().splitlines()[0] for p in doc],
                             ["A page 1", "A page 2", "A page 3", "A page 1", "A page 2", "A page 3", "A page 4"])

    def test_insert_requires_position_and_accepts_to_start(self):
        code, error, _ = self.run_cli(["insert", self.fixtures["main"], self.fixtures["other"], "--pages", "1"])
        self.assertEqual(code, 2)
        self.assertEqual(error["code"], "BAD_ARG")
        output = self.root / "insert-start.pdf"
        code, record, err = self.run_cli(["insert", self.fixtures["main"], self.fixtures["other"],
                                          "--pages", "1", "--to", "start", "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pymupdf.open(output) as doc: self.assertTrue(doc[0].get_text().startswith("B page 1"))

    def test_single_input_blank_preserves_catalog_features(self):
        output = self.root / "features-blank.pdf"
        code, record, err = self.run_cli(["organize", self.fixtures["features"], "1,blank,2", "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pikepdf.open(output) as pdf:
            self.assertTrue(pdf.Root.get("/OpenAction"))
            self.assertTrue(pdf.Root.get("/Names", {}).get("/EmbeddedFiles"))

    def test_structure_tree_pages_remap_and_dropped_elements_are_removed(self):
        output = self.root / "tagged-reordered.pdf"
        code, record, err = self.run_cli(["organize", self.fixtures["tagged"], "4,2", "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pikepdf.open(output) as pdf:
            pages = {tuple(page.obj.objgen) for page in pdf.pages}
            tree = pdf.Root["/StructTreeRoot"]
            kids = list(tree["/K"])
            self.assertEqual(len(kids), 2)
            self.assertEqual({tuple(kid["/Pg"].objgen) for kid in kids}, pages)
            nums = list(tree["/ParentTree"]["/Nums"])
            self.assertEqual(len(nums), 4)
            self.assertTrue(all(tuple(kid["/Pg"].objgen) in pages for kid in kids))

    def test_split_restores_catalog_actions_and_attachments(self):
        output = self.root / "feature-parts"
        code, record, err = self.run_cli(["split", self.fixtures["features"], "--every", "2", "-o", str(output) + "\\"])
        self.assertEqual(code, 0, (record, err))
        with pikepdf.open(next(Path(record["path"]).glob("*.pdf"))) as pdf:
            self.assertTrue(pdf.Root.get("/OpenAction"))
            self.assertTrue(pdf.Root.get("/Names", {}).get("/EmbeddedFiles"))

    def test_surviving_bookmark_levels_are_normalized(self):
        source = self.root / "nested.pdf"
        with pymupdf.open(self.fixtures["main"]) as doc:
            doc.set_toc([[1, "Parent", 1], [2, "Child", 2]])
            doc.save(source)
        output = self.root / "nested-out.pdf"
        code, record, err = self.run_cli(["organize", source, "2,blank", "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pymupdf.open(output) as doc: self.assertEqual(doc.get_toc()[0][0], 1)

    def test_split_puts_front_matter_in_first_bookmark_part(self):
        source = self.root / "front-matter.pdf"
        with pymupdf.open(self.fixtures["main"]) as doc:
            doc.set_toc([[1, "Chapter one", 3], [1, "Chapter two", 4]])
            doc.save(source)
        output = self.root / "front-parts"
        code, record, err = self.run_cli(["split", source, "--bookmarks", "-o", str(output) + "\\"])
        self.assertEqual(code, 0, (record, err))
        first = next(Path(record["path"]).glob("*Chapter one.pdf"))
        with pymupdf.open(first) as doc:
            self.assertEqual([p.get_text().strip().splitlines()[0] for p in doc], ["A page 1", "A page 2", "A page 3"])

    def test_implicit_blank_matches_rotated_predecessor(self):
        output = self.root / "rotated-blank.pdf"
        code, record, err = self.run_cli(["organize", self.fixtures["main"], "1@90,blank", "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pymupdf.open(output) as doc:
            self.assertEqual(tuple(round(v, 1) for v in doc[0].rect[2:]), (400.0, 300.0))
            self.assertEqual(tuple(round(v, 1) for v in doc[1].rect[2:]), (400.0, 300.0))

    def test_invalid_range_endpoints_are_rejected_without_crashing(self):
        for command, options in (("remove", ["--pages", "9-"]), ("reverse", ["--pages", "9-"])):
            code, error, _ = self.run_cli([command, self.fixtures["main"], *options])
            self.assertEqual(code, 2)
            self.assertEqual(error["code"], "BAD_ARG")

    def test_external_gotor_links_do_not_fail_internal_link_invariant(self):
        source = self.root / "external-link.pdf"
        with pymupdf.open(self.fixtures["main"]) as doc:
            doc[0].insert_link({"kind": pymupdf.LINK_GOTOR, "from": pymupdf.Rect(10, 200, 100, 220),
                                "file": "remote.pdf", "page": 9, "to": pymupdf.Point(0, 0)})
            doc.save(source)
        output = self.root / "external-out.pdf"
        code, record, err = self.run_cli(["organize", source, "rev", "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pymupdf.open(output) as doc:
            self.assertTrue(any(link["kind"] == pymupdf.LINK_GOTOR for p in doc for link in p.get_links()))

    def test_input_field_name_collisions_are_normalized(self):
        other_widget = self.root / "other-widget.pdf"
        with pymupdf.open(self.fixtures["other"]) as doc:
            widget = pymupdf.Widget()
            widget.field_type = pymupdf.PDF_WIDGET_TYPE_TEXT
            widget.field_name = "station"
            widget.field_value = "beta"
            widget.rect = pymupdf.Rect(30, 120, 180, 145)
            doc[0].add_widget(widget)
            doc.save(other_widget)
        output = self.root / "fields-merged.pdf"
        code, record, err = self.run_cli(["merge", self.fixtures["main"], other_widget, "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pymupdf.open(output) as doc:
            names = [w.field_name for page in doc for w in (page.widgets() or [])]
            self.assertEqual(names, ["station", "station#2"])

    def test_repeated_widget_names_avoid_existing_suffixes(self):
        output = self.root / "fields-suffix.pdf"
        code, record, err = self.run_cli(["organize", self.fixtures["collision"], "1x2,2-4", "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pymupdf.open(output) as doc:
            self.assertEqual([w.field_name for w in (doc[0].widgets() or [])], ["station"])
            self.assertEqual([w.field_name for w in (doc[1].widgets() or [])], ["station#3"])

    def test_split_declares_label_removal_even_for_whole_input(self):
        output = self.root / "whole-parts"
        code, record, err = self.run_cli(["split", self.fixtures["main"], "--every", "4", "-o", str(output) + "\\"])
        self.assertEqual(code, 0, (record, err))
        self.assertNotEqual(record.get("verdict"), "unchecked")

    def test_split_zero_interval_is_rejected(self):
        code, record, _ = self.run_cli(["split", self.fixtures["main"], "--every", "0"])
        self.assertEqual(code, 2)
        self.assertEqual(record["code"], "BAD_ARG")

    def test_split_bookmark_names_are_unique_after_sanitizing(self):
        source = self.root / "duplicate-bookmarks.pdf"
        with pymupdf.open(self.fixtures["main"]) as doc:
            doc.set_toc([[1, "Chapter", 1], [1, "Chapter", 3]])
            doc.save(source)
        output = self.root / "duplicate-bookmarks-parts"
        code, record, err = self.run_cli(["split", source, "--bookmarks", "-o", str(output) + "\\"])
        self.assertEqual(code, 0, (record, err))
        names = sorted(p.name for p in Path(record["path"]).glob("*.pdf"))
        self.assertEqual(names, ["duplicate-bookmarks_Chapter.pdf", "duplicate-bookmarks_Chapter_2.pdf"])

    def test_split_default_folder_omits_pdf_extension(self):
        source = self.fixtures["main"]
        code, record, err = self.run_cli(["split", source, "--every", "2"])
        self.assertEqual(code, 0, (record, err))
        self.assertEqual(Path(record["path"]).name, "main_split")

    def test_split_aggregates_part_warnings_findings_and_log_paths(self):
        output = self.root / "diagnostic-parts"
        code, record, err = self.run_cli(["split", self.fixtures["main"], "--every", "2", "-o", str(output) + "\\"])
        self.assertEqual(code, 0, (record, err))
        self.assertIn("LABELS_DROPPED", record["warnings"])
        self.assertIn("FEATURES_DROPPED", record["warnings"])
        self.assertTrue(record.get("findings"))
        self.assertIn("log_paths", record)

    def test_feature_loss_warning_changes_verdict_to_warn(self):
        no_labels = self.root / "feature-no-labels.pdf"
        with pymupdf.open(self.fixtures["main"]) as doc:
            doc.set_page_labels([])
            doc.save(no_labels)
        output = self.root / "feature-loss.pdf"
        code, record, err = self.run_cli(["merge", self.fixtures["features"], no_labels, "-o", output])
        self.assertEqual(code, 0, (record, err))
        self.assertIn("FEATURES_DROPPED", record["warnings"])
        self.assertEqual(record["verdict"], "warn")

    def test_recipe_container_and_file_types_are_bad_arguments(self):
        for recipe in (42, [{"file": 42, "pages": "1"}]):
            job = self.root / "bad-recipe.json"
            job.write_text(json.dumps({"version": 1, "command": "organize", "input": str(self.fixtures["main"]),
                                       "recipe": recipe}), encoding="utf-8")
            code, record, _ = self.run_cli(["--job", job])
            self.assertEqual(code, 2)
            self.assertEqual(record["code"], "BAD_ARG")

    def test_shorthand_errors_accumulate_with_semantic_problems(self):
        code, record, _ = self.run_cli(["organize", self.fixtures["main"], "bogus,also-bogus,99"])
        self.assertEqual(code, 2)
        self.assertEqual(record["code"], "BAD_ARG")
        self.assertGreaterEqual(len(record["problems"]), 3)

    def test_move_target_inside_block_is_bad_page(self):
        code, record, _ = self.run_cli(["move", self.fixtures["main"], "--pages", "2-3", "--after", "2"])
        self.assertEqual(code, 1)
        self.assertEqual(record["code"], "BAD_PAGE")

    def test_annotation_count_invariant_detects_dropped_annotation(self):
        from pdftools.commands import p3b_organize
        def omit_annotations(docs, rows, target, **kwargs):
            out = pymupdf.open()
            source = docs[str(self.fixtures["annotated"].resolve())][0]
            for row in rows:
                out.insert_pdf(source, from_page=row["page"], to_page=row["page"], annots=False, links=True)
            out.save(target)
            out.close()
        with patch.object(p3b_organize, "_write_rows", side_effect=omit_annotations):
            code, record, _ = self.run_cli(["organize", self.fixtures["annotated"], "rev", "-o", self.root / "bad-annot.pdf"])
        self.assertEqual(code, 3)
        self.assertTrue(any(item["code"] == "FEATURE_MISSING" for item in record.get("findings", [])))

    def test_large_organize_sample_does_not_serialize_whole_input(self):
        source = self.root / "many-pages.pdf"
        doc = pymupdf.open()
        for number in range(101):
            page = doc.new_page(width=100, height=100)
            page.insert_text((10, 20), f"page {number}")
        doc.save(source)
        doc.close()
        with patch.object(pymupdf.Document, "tobytes", side_effect=AssertionError("whole PDF serialized")):
            code, record, err = self.run_cli(["organize", source, "rev", "-o", self.root / "many-out.pdf"])
        self.assertEqual(code, 0, (record, err))

    def test_catalog_rewrite_is_skipped_without_restorable_features(self):
        from pdftools.commands import p3b_organize
        with patch.object(p3b_organize, "_restore_catalog_features", side_effect=AssertionError("unneeded rewrite")):
            code, record, err = self.run_cli(["organize", self.fixtures["main"], "rev", "-o", self.root / "plain-rev.pdf"])
        self.assertEqual(code, 0, (record, err))

    def test_merge_same_input_keeps_one_toc_entry_per_occurrence(self):
        source = self.root / "unbookmarked.pdf"
        with pymupdf.open(self.fixtures["other"]) as doc:
            doc.set_toc([])
            doc.save(source)
        output = self.root / "same-source-toc.pdf"
        code, record, err = self.run_cli(["merge", source, source, "--toc", "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pymupdf.open(output) as doc:
            self.assertEqual([entry[1] for entry in doc.get_toc() if entry[0] == 1], ["unbookmarked", "unbookmarked"])

    def test_merge_job_per_input_passwords_resolve_relative_to_job(self):
        secured = self.root / "private.pdf"
        with pymupdf.open(self.fixtures["other"]) as source:
            source.save(secured, encryption=pymupdf.PDF_ENCRYPT_AES_256,
                        user_pw="merge-secret", owner_pw="merge-owner")
        (self.root / "merge.pw").write_text("merge-secret\n", encoding="utf-8")
        job = self.root / "merge-job.json"
        output = self.root / "merge-job-out.pdf"
        job.write_text(json.dumps({"version": 1, "command": "merge", "input": str(self.fixtures["main"]),
                                   "files": [{"file": "private.pdf", "password_file": "merge.pw"}],
                                   "output": str(output)}), encoding="utf-8")
        code, record, err = self.run_cli(["--job", job])
        self.assertEqual(code, 0, (record, err))
        with pymupdf.open(output) as doc: self.assertEqual(doc.page_count, 7)

    def test_whole_recipe_validation_reports_every_problem(self):
        job = self.root / "bad.json"
        job.write_text(json.dumps({"version": 1, "input": str(self.fixtures["main"]),
                                   "steps": [{"cmd": "organize", "recipe": [
                                       {"pages": "90"}, {"pages": "1", "times": 0}, {"blank": "bad-size"}]}]}), encoding="utf-8")
        code, record, _ = self.run_cli(["--job", job])
        self.assertEqual(code, 2)
        self.assertEqual(record["code"], "BAD_ARG")
        self.assertGreaterEqual(len(record["problems"]), 3)

    def test_merge_interleave_and_multi_file_feature_warnings(self):
        output = self.root / "merged.pdf"
        code, record, err = self.run_cli(["merge", self.fixtures["features"], str(self.fixtures["other"]), "--toc", "-o", output])
        self.assertEqual(code, 0, (record, err))
        with pikepdf.open(output) as pdf:
            self.assertEqual(len(pdf.pages), 7)
            self.assertTrue(pdf.Root.get("/Outlines"))
            self.assertFalse(pdf.Root.get("/OpenAction"))
            self.assertFalse(pdf.Root.get("/AF"))
            self.assertFalse(pdf.Root.get("/Names", {}).get("/EmbeddedFiles"))
        self.assertIn("FEATURES_DROPPED", record["warnings"])
        self.assertIn("JavaScript, attachments", err)
        no_toc = self.root / "no-toc.pdf"
        with pymupdf.open(self.fixtures["other"]) as doc:
            doc.set_toc([])
            doc.save(no_toc)
        no_toc_merge = self.root / "no-toc-merge.pdf"
        code, record, err = self.run_cli(["merge", self.fixtures["main"], no_toc, "--toc", "-o", no_toc_merge])
        self.assertEqual(code, 0, (record, err))
        with pymupdf.open(no_toc_merge) as doc:
            self.assertEqual([entry[1] for entry in doc.get_toc() if entry[0] == 1], ["main", "no-toc"])
        interleaved = self.root / "interleave.pdf"
        code, record, _ = self.run_cli(["interleave", self.fixtures["main"], self.fixtures["other"], "--allow-uneven", "-o", interleaved])
        self.assertEqual(code, 0, record)
        with pymupdf.open(interleaved) as doc:
            self.assertEqual([p.get_text().split()[0] for p in doc], ["A", "B", "A", "B", "A", "B", "A"])

    def test_insert_job_with_a_path_containing_spaces(self):
        other = self.root / "other file.pdf"
        other.write_bytes(self.fixtures["other"].read_bytes())
        job = self.root / "insert.json"
        output = self.root / "inserted.pdf"
        job.write_text(json.dumps({"version": 1, "command": "insert", "input": str(self.fixtures["main"]),
                                   "other": str(other), "pages": "2-3", "after": 2, "output": str(output)}), encoding="utf-8")
        code, record, err = self.run_cli(["--job", job])
        self.assertEqual(code, 0, (record, err))
        with pikepdf.open(output) as pdf: self.assertEqual(len(pdf.pages), 6)

    def test_split_every_and_ranges_publish_folder(self):
        output = self.root / "parts"
        output.mkdir()
        code, record, err = self.run_cli(["split", self.fixtures["main"], "--every", "3", "-o", output])
        self.assertEqual(code, 0, (record, err))
        self.assertTrue(Path(record["path"]).is_dir())
        files = sorted(Path(record["path"]).glob("*.pdf"))
        self.assertEqual(len(files), 2)
        with pikepdf.open(files[0]) as pdf: self.assertEqual(len(pdf.pages), 3)
        with pikepdf.open(files[1]) as pdf: self.assertEqual(len(pdf.pages), 1)
        ranges = self.root / "range-output"
        ranges.mkdir()
        code, record, err = self.run_cli(["split", self.fixtures["main"], "--ranges", "1-2", "4", "-o", ranges])
        self.assertEqual(code, 0, (record, err))
        range_files = sorted(Path(record["path"]).glob("*.pdf"))
        self.assertEqual([p.name for p in range_files], ["main_p1-2.pdf", "main_p4.pdf"])
        with pikepdf.open(range_files[0]) as pdf: self.assertEqual(len(pdf.pages), 2)
        bookmarks = self.root / "bookmark-output"
        bookmarks.mkdir()
        code, record, err = self.run_cli(["split", self.fixtures["main"], "--bookmarks", "-o", bookmarks])
        self.assertEqual(code, 0, (record, err))
        bookmark_files = sorted(Path(record["path"]).glob("*.pdf"))
        self.assertEqual([p.name for p in bookmark_files], ["main_A first.pdf", "main_A last.pdf"])


if __name__ == "__main__":
    unittest.main()
