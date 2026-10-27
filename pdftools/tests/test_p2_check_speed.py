import tempfile
import unittest
from pathlib import Path
from unittest import mock

import numpy as np
import pymupdf

from pdftools import check
from pdftools.check import OperationResult, Placement, SSIM_MED, compare
from tests.fixtures.p2 import build_fixtures


class CompareSpeedTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.fixtures = build_fixtures(Path(cls.tmp.name) / "fixtures")

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    @staticmethod
    def _findings(findings):
        def rounded(value):
            if isinstance(value, float):
                return round(value, 4)
            if isinstance(value, dict):
                return {key: rounded(item) for key, item in value.items()}
            if isinstance(value, (list, tuple)):
                return [rounded(item) for item in value]
            return value

        return [{"tier": item.tier, "code": item.code, "page": item.page,
                 "details": rounded(item.details)} for item in findings]

    def test_calibration_findings_match_recorded_expectations(self):
        with pymupdf.open(self.fixtures["calibration_copy"]) as source, \
                pymupdf.open(self.fixtures["calibration_resaved"]) as output:
            self.assertEqual(self._findings(compare(source, output, "placements")), [])

        with pymupdf.open(self.fixtures["calibration_table"]) as source, \
                pymupdf.open(self.fixtures["calibration_table_changed"]) as output:
            self.assertEqual(self._findings(compare(source, output, "placements")), [
                {"tier": "invariant", "code": "TEXT_CHANGED", "page": 1,
                 "details": {"source_words": 5, "output_words": 5}},
                {"tier": "invariant", "code": "TILES_CHANGED", "page": 1,
                 "details": {"changed_tiles": 2, "boxes_mm": [
                     [29.718, 9.906, 39.624, 19.812],
                     [29.718, 19.812, 39.624, 29.718],
                 ]}},
            ])

        with pymupdf.open(self.fixtures["calibration_small_image"]) as source, \
                pymupdf.open(self.fixtures["calibration_small_gray_box"]) as output:
            expected = OperationResult(ssim_floor=SSIM_MED)
            self.assertEqual(self._findings(compare(source, output, "compress_lossy", expected)), [
                {"tier": "invariant", "code": "SSIM_LOW", "page": 1,
                 "details": {"ssim": 0.7391, "scores": [0.7391], "floor": 0.9}},
            ])

    def test_tier_stats_are_cached_per_document_page(self):
        with pymupdf.open(self.fixtures["calibration_copy"]) as source:
            output = pymupdf.open()
            output.insert_pdf(source)
            output.insert_pdf(source)
            operation = OperationResult(pages=[
                [Placement(None, 0)], [Placement(None, 0)],
            ])
            with mock.patch.object(check, "_tier_stats", wraps=check._tier_stats) as stats:
                compare(source, output, "placements", operation)
            self.assertEqual(stats.call_count, 3)
            output.close()

    def test_render_cache_is_released_between_output_pages(self):
        with pymupdf.open(self.fixtures["calibration_copy"]) as source:
            output = pymupdf.open()
            output.insert_pdf(source)
            output.insert_pdf(source)
            operation = OperationResult(pages=[
                [Placement(None, 0)], [Placement(None, 0)],
            ])
            original = check.render_rgb
            source_render_calls = []

            def record(page, dpi=check.RENDER_DPI, *args, **kwargs):
                if page.parent.name == source.name and dpi == check.RENDER_DPI:
                    source_render_calls.append(page.number)
                return original(page, dpi, *args, **kwargs)

            with mock.patch.object(check, "render_rgb", side_effect=record):
                compare(source, output, "placements", operation)
            self.assertEqual(source_render_calls, [0, 0])
            output.close()

    def test_identity_canvas_returns_equal_output_without_warp(self):
        with pymupdf.open(self.fixtures["calibration_copy"]) as source, \
                pymupdf.open(self.fixtures["calibration_copy"]) as output:
            pixels = check.render_rgb(source[0], unrotated=True)
            with mock.patch.object(check.cv2, "warpAffine", side_effect=AssertionError("unexpected warp")):
                result = check._placement_canvas(
                    [Placement(None, 0)], source, None, output[0], pixels,
                    renderer=lambda *args, **kwargs: pixels,
                )
            self.assertIs(result, pixels)

    def test_tile_reduction_keeps_extra_channel_differences(self):
        source = np.zeros((10, 10, 4), dtype=np.uint8)
        output = source.copy()
        output[0, 0, 3] = 9
        output[0, 1, 3] = 9
        self.assertEqual(check.tile_differences(source, output, tile_px=10), [
            {"box_mm": (0.0, 0.0, 7.2, 7.2), "fraction": 0.02, "max_delta": 9},
        ])


if __name__ == "__main__":
    unittest.main()
