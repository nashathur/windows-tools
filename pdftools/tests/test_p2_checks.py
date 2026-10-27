import tempfile
import unittest
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))

import pymupdf
import numpy as np
import cv2
from pdftools.check import (GRAY_CHROMA_MAX, GRAY_LUMINANCE_MAX_DELTA, SSIM_MED, SSIM_HIGH, SSIM_MAX, TILE_CHANGED_FRACTION,
                            OperationResult, Placement,
                            compare, page_stats, render_rgb, ssim_gray, tile_differences)
from pdftools.registry import BUILTIN_POLICIES, command, commands_by_phase, output_formats
from pdftools.textmatch import find_matches
from tests.fixtures.p2 import build_fixtures


class CheckAndTextTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.fixtures = build_fixtures(Path(cls.tmp.name) / "fixtures")

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def test_calibration_margins(self):
        with pymupdf.open(self.fixtures["calibration_photo"]) as a, \
                pymupdf.open(self.fixtures["calibration_photo_q80"]) as b:
            photo_a, photo_b = render_rgb(a[0]), render_rgb(b[0])
            self.assertEqual(page_stats(a[0])["max_chroma"], 167)
            self.assertEqual(page_stats(b[0])["max_chroma"], 189)
            x0,y0,x1,y1=(int(v*100/72) for v in (35,90,435,357))
            region_a, region_b = photo_a[y0:y1,x0:x1], photo_b[y0:y1,x0:x1]
            photo_ssim = ssim_gray(region_a, region_b)
            photo_delta = np.abs(region_a.astype(np.int16) - region_b.astype(np.int16))
        self.assertGreater(photo_ssim, SSIM_MED)
        with pymupdf.open(self.fixtures["calibration_table"]) as a, \
                pymupdf.open(self.fixtures["calibration_table_changed"]) as b:
            table_tiles = tile_differences(render_rgb(a[0]), render_rgb(b[0]))
            self.assertTrue(compare(a, b, "placements"))
        with pymupdf.open(self.fixtures["calibration_photo"]) as a, \
                pymupdf.open(self.fixtures["calibration_removed"]) as b:
            removed_tiles = tile_differences(render_rgb(a[0]), render_rgb(b[0]))
            removed_ssim = ssim_gray(render_rgb(a[0]), render_rgb(b[0]))
            self.assertTrue(compare(a, b, "placements"))
        with pymupdf.open(self.fixtures["calibration_copy"]) as a, \
                pymupdf.open(self.fixtures["calibration_copy"]) as b:
            self.assertFalse(tile_differences(render_rgb(a[0]), render_rgb(b[0])))
        with pymupdf.open(self.fixtures["calibration_resaved"]) as a, \
                pymupdf.open(self.fixtures["calibration_copy"]) as b:
            self.assertFalse(tile_differences(render_rgb(a[0]), render_rgb(b[0])))
        changed_fraction = max(item["fraction"] for item in table_tiles)
        removed_fraction = max(item["fraction"] for item in removed_tiles)
        self.assertGreater(changed_fraction, TILE_CHANGED_FRACTION)
        self.assertAlmostEqual(changed_fraction, 0.02564, places=4)
        self.assertEqual(max(item["max_delta"] for item in table_tiles), 255)
        self.assertGreater(changed_fraction - TILE_CHANGED_FRACTION, 0.015)
        self.assertGreater(removed_fraction, TILE_CHANGED_FRACTION)
        scores = {80: photo_ssim}
        channel_deltas = {80: int(photo_delta.max())}
        changed_fractions = {80: max(item["fraction"] for item in tile_differences(photo_a, photo_b))}
        for quality in (65,50):
            with pymupdf.open(self.fixtures["calibration_photo"]) as a, \
                    pymupdf.open(self.fixtures[f"calibration_photo_q{quality}"]) as b:
                aa,bb=render_rgb(a[0]),render_rgb(b[0])
                scores[quality]=ssim_gray(aa[y0:y1,x0:x1],bb[y0:y1,x0:x1])
                channel_deltas[quality]=int(np.abs(aa[y0:y1,x0:x1].astype(np.int16)-
                                                   bb[y0:y1,x0:x1].astype(np.int16)).max())
                changed_fractions[quality]=max(item["fraction"] for item in tile_differences(aa,bb))
        self.assertGreaterEqual(scores[80], SSIM_MED)
        self.assertGreaterEqual(scores[65], SSIM_HIGH)
        self.assertGreaterEqual(scores[50], SSIM_MAX)
        self.assertAlmostEqual(scores[80], 0.9928, places=3)
        self.assertAlmostEqual(scores[65], 0.9881, places=3)
        self.assertAlmostEqual(scores[50], 0.9823, places=3)
        self.assertEqual(channel_deltas, {80: 35, 65: 53, 50: 56})
        self.assertEqual([round(changed_fractions[q] * 100, 2) for q in (80, 65, 50)], [22.16, 31.56, 37.34])
        self.assertGreater(scores[80] - SSIM_MED, 0.09)
        self.assertGreater(scores[65] - SSIM_HIGH, 0.13)
        self.assertGreater(scores[50] - SSIM_MAX, 0.20)
        with pymupdf.open(self.fixtures["calibration_photo"]) as source, \
                pymupdf.open(self.fixtures["calibration_photo_q80"]) as q80:
            lossy = compare(source, q80, "compress_lossy", OperationResult(ssim_floor=SSIM_MED))
            self.assertFalse([f for f in lossy if f.tier == "invariant"])
            q80_tiles = tile_differences(render_rgb(source[0]), render_rgb(q80[0]), page=q80[0])
            self.assertGreater(max((t["fraction"] for t in q80_tiles), default=0), TILE_CHANGED_FRACTION)
            self.assertLessEqual(int(photo_delta.max()), 40)
            image_box = pymupdf.Rect(*(v * 25.4 / 72 for v in (35, 90, 435, 357)))
            self.assertFalse([t for t in q80_tiles if not pymupdf.Rect(t["box_mm"]).intersects(image_box)])
            self.assertLess(int(photo_delta.max()) - 8, 30)
        self.assertGreater(removed_ssim, SSIM_MED)
        self.assertGreater(removed_fraction, TILE_CHANGED_FRACTION)
        self.assertGreater(changed_fraction, TILE_CHANGED_FRACTION)
        with pymupdf.open(self.fixtures["calibration_gray"]) as gray:
            gray_chroma = page_stats(gray[0])["max_chroma"]
            self.assertEqual(gray_chroma, 0)
            self.assertLessEqual(gray_chroma, GRAY_CHROMA_MAX)
            self.assertEqual(GRAY_CHROMA_MAX - gray_chroma, 2)
        with pymupdf.open(self.fixtures["calibration_photo"]) as photo, \
                pymupdf.open(self.fixtures["calibration_gray"]) as gray, \
                pymupdf.open(self.fixtures["calibration_removed"]) as removed:
            photo_gray = cv2.cvtColor(render_rgb(photo[0]), cv2.COLOR_RGB2GRAY).astype(np.int16)
            converted_gray = cv2.cvtColor(render_rgb(gray[0]), cv2.COLOR_RGB2GRAY).astype(np.int16)
            removed_gray = cv2.cvtColor(render_rgb(removed[0]), cv2.COLOR_RGB2GRAY).astype(np.int16)
            passing_luminance_delta = int(np.abs(photo_gray - converted_gray).max())
            failing_luminance_delta = int(np.abs(photo_gray - removed_gray).max())
            self.assertLessEqual(passing_luminance_delta, GRAY_LUMINANCE_MAX_DELTA)
            self.assertGreater(failing_luminance_delta, GRAY_LUMINANCE_MAX_DELTA)
            self.assertEqual(passing_luminance_delta, 5)
            self.assertEqual(failing_luminance_delta, 254)
            self.assertEqual(GRAY_LUMINANCE_MAX_DELTA - passing_luminance_delta, 19)
            self.assertEqual(failing_luminance_delta - GRAY_LUMINANCE_MAX_DELTA, 230)
        with pymupdf.open(self.fixtures["calibration_small_image"]) as small, \
                pymupdf.open(self.fixtures["calibration_small_gray_box"]) as damaged:
            bad = compare(small, damaged, "compress_lossy", OperationResult(ssim_floor=SSIM_MED))
            failed = next(f for f in bad if f.code == "SSIM_LOW")
            self.assertAlmostEqual(failed.details["ssim"], 0.7391, places=3)
            self.assertGreater(SSIM_MED - failed.details["ssim"], 0.16)
        with pymupdf.open(self.fixtures["calibration_photo"]) as photo, \
                pymupdf.open(self.fixtures["calibration_removed"]) as removed:
            removed_result = compare(photo, removed, "compress_lossy", OperationResult(ssim_floor=SSIM_MED))
            removed_image_ssim = next(f.details["ssim"] for f in removed_result if f.code == "SSIM_LOW")
            self.assertAlmostEqual(removed_image_ssim, 0.6369, places=3)
            self.assertGreater(SSIM_MED - removed_image_ssim, 0.26)

    def test_all_builtin_policies_accept_an_identical_fixture_pair(self):
        with pymupdf.open(self.fixtures["plain"]) as source, pymupdf.open(self.fixtures["calibration_copy"]) as identical:
            for policy in sorted(BUILTIN_POLICIES):
                with self.subTest(policy=policy):
                    self.assertFalse([f for f in compare(source, identical, policy) if f.tier == "invariant"])
        with pymupdf.open(self.fixtures["calibration_table"]) as source, \
                pymupdf.open(self.fixtures["calibration_table_changed"]) as changed:
            for policy in sorted(BUILTIN_POLICIES - {"report"}):
                with self.subTest(broken_policy=policy):
                    self.assertTrue([f for f in compare(source, changed, policy) if f.tier == "invariant"])
        with pymupdf.open(self.fixtures["calibration_photo"]) as color:
            self.assertTrue([f for f in compare(color, color, "gray") if f.tier == "invariant"])

    def test_placement_map_compares_the_named_source_page(self):
        source_path = self.fixtures["equation"]
        source = pymupdf.open(source_path)
        source.new_page(width=200, height=200).insert_text((20, 40), "second page")
        source_path = Path(self.tmp.name) / "two-pages.pdf"
        source.save(source_path)
        source.close()
        with pymupdf.open(source_path) as source_doc:
            output = pymupdf.open(source_path)
            output.select([1])
            operation = OperationResult(pages=[[Placement(str(source_path), 1)]])
            self.assertFalse([f for f in compare(source_doc, output, "placements", operation,
                                                 source_docs={str(source_path): source_doc}) if f.tier == "invariant"])
            output.close()

    def test_text_matches_are_per_line_repeated_and_rotated(self):
        doc = pymupdf.open()
        page = doc.new_page(width=300, height=400)
        page.insert_text((30, 50), "Climate")
        page.insert_text((30, 75), "report")
        page.insert_text((30, 110), "repeat repeat")
        cross = find_matches(doc, r"Climate\s+report", regex=True)
        self.assertEqual(len(cross["matches"][0]["quads"]), 2)
        literal = find_matches(doc, "Climate report")
        self.assertEqual(literal["total"], 1)
        self.assertEqual(len(literal["matches"][0]["quads"]), 2)
        repeated = find_matches(doc, "repeat")
        page2 = doc.new_page(width=300, height=400)
        page2.insert_text((30, 50), "repeat")
        repeated = find_matches(doc, "repeat")
        self.assertEqual(repeated["counts"], {1: 2, 2: 1})
        second = find_matches(doc, "repeat", occurrence=[2])
        self.assertEqual([item["occurrence"] for item in second["matches"]], [2])
        page = doc[0]
        page.set_rotation(90)
        rotated = find_matches(doc, "Climate")
        self.assertEqual(len(rotated["matches"][0]["quads"]), 1)
        doc.close()

    def test_registry_policies_and_phase_lookups(self):
        self.assertEqual(commands_by_phase("2")["thumb"].phase, "2")
        self.assertIn("pdf", output_formats())
        self.assertGreaterEqual(len(BUILTIN_POLICIES), 15)
        with self.assertRaises(ValueError):
            command("bad-kind", 2, "unknown", "pdf", "", False, "report")
        with self.assertRaises(ValueError):
            command("bad-policy", 2, "pdf", "pdf", "", False, "missing")
        with self.assertRaises(ValueError):
            command("bad-suffix", 2, "pdf", "pdf", None, False, "report")


if __name__ == "__main__":
    unittest.main()
