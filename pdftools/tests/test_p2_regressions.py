import tempfile
import unittest
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))

import pymupdf

from pdftools.check import (ExpectedChanges, Features, OperationResult, Placement,
                            _inside_expected, _remove_words, compare)
from pdftools.registry import BUILTIN_POLICIES


class Phase2RegressionTest(unittest.TestCase):
    def setUp(self):
        self.doc = pymupdf.open()
        page = self.doc.new_page(width=300, height=400)
        page.insert_text((30, 60), "2026 Climate report")

    def tearDown(self):
        self.doc.close()

    def output_copy(self):
        out = pymupdf.open()
        out.insert_pdf(self.doc)
        return out

    def test_p2_rotation_is_compared_unrotated_and_rotation_is_checked(self):
        self.doc[0].set_rotation(0)
        out = self.output_copy()
        out[0].set_rotation(90)
        operation = OperationResult(pages=[[Placement(None, 0, rotation=90)]])
        codes = {finding.code for finding in compare(self.doc, out, "rotate", operation)}
        self.assertNotIn("TILES_CHANGED", codes)
        self.assertNotIn("ROTATION_CHANGED", codes)
        out[0].set_rotation(180)
        self.assertIn("ROTATION_CHANGED", {f.code for f in compare(self.doc, out, "rotate", operation)})
        out.close()

    def test_p2_page_count_is_checked_for_every_writing_policy(self):
        source = pymupdf.open()
        source.new_page()
        source.new_page()
        out = pymupdf.open()
        out.insert_pdf(source, from_page=0, to_page=0)
        for policy in BUILTIN_POLICIES - {"report"}:
            with self.subTest(policy=policy):
                self.assertIn("PAGE_COUNT", {f.code for f in compare(source, out, policy)})
        source.close()
        out.close()

    def test_p2_blank_placement_must_be_ink_free(self):
        out = pymupdf.open()
        page = out.new_page()
        page.insert_text((20, 30), "not blank")
        operation = OperationResult(pages=[[Placement(None, "blank")]])
        self.assertIn("BLANK_PLACEMENT_NOT_BLANK",
                      {f.code for f in compare(self.doc, out, "placements", operation)})
        ink = pymupdf.open()
        ink_page = ink.new_page()
        ink_page.draw_rect(pymupdf.Rect(20, 20, 40, 40), color=(0, 0, 0), fill=(0, 0, 0))
        self.assertIn("BLANK_PLACEMENT_NOT_BLANK",
                      {f.code for f in compare(self.doc, ink, "placements", operation)})
        ink.close()
        out.close()

    def test_p2_region_uses_tile_overlap_and_documented_space(self):
        op = OperationResult(expected=ExpectedChanges(regions=[{"page": 1, "rect": (10, 10, 20, 20)}]))
        self.assertTrue(_inside_expected({"box_mm": (19, 19, 29, 29)}, op, 1))
        self.assertFalse(_inside_expected({"box_mm": (19, 19, 29, 29)}, op, 2))

    def test_p2_expected_text_removes_only_declared_multiplicity(self):
        self.assertEqual(_remove_words(("2026", "2026"), ["2026"]), ("2026",))

    def test_p2_stamp_existing_word_and_overlapping_tile_edges_pass(self):
        out = self.output_copy()
        out[0].insert_text((100, 130), "2026")
        region = (30, 35, 60, 55)
        operation = OperationResult(pages=[[Placement(None, 0)]],
                                    expected=ExpectedChanges(regions=[region], text_added=["2026"]))
        findings = compare(self.doc, out, "stamp", operation)
        self.assertFalse([f for f in findings if f.tier == "invariant"])
        out.close()

    def test_p2_annotation_changes_do_not_change_content_tiles(self):
        out = self.output_copy()
        out[0].add_highlight_annot([pymupdf.Rect(25, 45, 120, 65)])
        op = OperationResult(pages=[[Placement(None, 0)]], annotation_delta=1)
        self.assertFalse([f for f in compare(self.doc, out, "annot_add", op) if f.tier == "invariant"])
        out.close()

    def test_p2_lossy_ssim_is_per_image_and_flat_box_fails(self):
        from tests.fixtures.p2 import build_fixtures
        with tempfile.TemporaryDirectory() as folder:
            fixtures = build_fixtures(Path(folder))
            with pymupdf.open(fixtures["calibration_small_image"]) as source, \
                    pymupdf.open(fixtures["calibration_small_gray_box"]) as damaged:
                findings = compare(source, damaged, "compress_lossy", OperationResult(ssim_floor=0.90))
                self.assertIn("SSIM_LOW", {f.code for f in findings})

    def test_p2_features_dropped_is_source_minus_count_warning(self):
        source = pymupdf.open()
        source.new_page()
        source[0].add_text_annot((20, 20), "note")
        out = pymupdf.open()
        out.insert_pdf(source)
        out[0].delete_annot(next(out[0].annots()))
        operation = OperationResult(features=Features(dropped={"annotations": 1}))
        findings = compare(source, out, "placements", operation)
        self.assertTrue(any(f.code == "FEATURES_DROPPED" and f.tier == "new_warning" for f in findings))
        source.close()
        out.close()

    def test_p2_crop_scopes_words_and_checks_cropbox(self):
        out = self.output_copy()
        crop = pymupdf.Rect(20, 30, 150, 100)
        out[0].set_cropbox(crop)
        operation = OperationResult(pages=[[Placement(None, 0)]], cropboxes=[tuple(crop)])
        self.assertNotIn("TEXT_CHANGED", {f.code for f in compare(self.doc, out, "crop", operation)})
        out[0].set_cropbox(pymupdf.Rect(20, 30, 160, 100))
        self.assertIn("CROPBOX_CHANGED", {f.code for f in compare(self.doc, out, "crop", operation)})
        out.close()

    def test_p2_dest_clip_scopes_words_and_tiles_to_placed_source(self):
        out = self.output_copy()
        out[0].set_cropbox(pymupdf.Rect(0, 0, 60, 100))
        operation = OperationResult(pages=[[Placement(None, 0, dest_rect=(0, 0, 60, 100))]])
        findings = compare(self.doc, out, "placements", operation)
        self.assertFalse([f for f in findings if f.tier == "invariant"])
        out.close()

    def test_p2_registry_rejects_phase_ten(self):
        from pdftools.registry import command
        with self.assertRaises(ValueError):
            command("bad-phase-ten", 10, "pdf", "pdf", "", False, "report")

    def test_p2_estimate_warns_only_above_ten_seconds(self):
        from pdftools.pipeline import announce_estimate
        from contextlib import redirect_stderr
        from io import StringIO
        stream = StringIO()
        with redirect_stderr(stream):
            self.assertEqual(announce_estimate(10, 1), 10)
        self.assertEqual(stream.getvalue(), "")
        with redirect_stderr(stream):
            self.assertEqual(announce_estimate(20, 0.6), 12)
        self.assertEqual(stream.getvalue(), "estimate: ~12 s for 20 pages\n")

    def test_p2_compare_caches_page_render_per_dpi(self):
        from unittest.mock import patch
        import pdftools.check as check
        out = self.output_copy()
        original = check.render_rgb
        calls = []
        def counted(page, dpi=100, clip=None, **kwargs):
            calls.append((id(page.parent), page.number, dpi))
            return original(page, dpi, clip, **kwargs)
        with patch.object(check, "render_rgb", side_effect=counted):
            compare(self.doc, out, "gray")
        self.assertEqual(len(calls), len(set(calls)))
        out.close()


if __name__ == "__main__":
    unittest.main()
