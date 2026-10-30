"""Coordinate space characterization across rotation, crop offsets and /UserUnit."""
import tempfile
import unittest
from pathlib import Path

import pymupdf

from pdftools import coords


ROTATIONS = (0, 90, 180, 270)
UNITS = (1, 1.5, 2)
RAW_CROP = pymupdf.Rect(20, 30, 420, 330)
MARK_USER = pymupdf.Rect(60, 260, 110, 280)


def build_page(path, rotation, unit):
    doc = pymupdf.open()
    page = doc.new_page(width=500, height=400)
    page.insert_text((70, 100), "CoordMark", fontsize=14)
    page.draw_rect((60, 120, 110, 140), color=(0, 0, 0), fill=(0, 0, 0))
    sample = pymupdf.Pixmap(pymupdf.csRGB, pymupdf.IRect(0, 0, 2, 2), False)
    page.insert_image((130, 80, 180, 120), stream=sample.tobytes("png"))
    doc.xref_set_key(page.xref, "CropBox", "[20 30 420 330]")
    if unit != 1:
        doc.xref_set_key(page.xref, "UserUnit", str(unit))
    page.set_rotation(rotation)
    doc.save(path)
    doc.close()
    return pymupdf.open(path)


class SharedCoordinatesTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_text_and_vector_apis_use_scaled_text_space(self):
        for rotation in ROTATIONS:
            for unit in UNITS:
                with self.subTest(rotation=rotation, unit=unit):
                    doc = build_page(self.root / f"{rotation}-{unit}.pdf", rotation, unit)
                    page = doc[0]
                    expected = pymupdf.Rect(40 * unit, 50 * unit, 90 * unit, 70 * unit)
                    word = pymupdf.Rect(page.get_text("words")[0][:4])
                    raw = page.get_text("rawdict")
                    first_char = pymupdf.Rect(raw["blocks"][0]["lines"][0]["spans"][0]["chars"][0]["bbox"])
                    found = page.search_for("CoordMark")[0]
                    drawing = pymupdf.Rect(page.get_drawings()[0]["rect"])
                    image = pymupdf.Rect(page.get_image_info()[0]["bbox"])
                    self.assertAlmostEqual(word.x0, 50 * unit, places=3)
                    self.assertAlmostEqual(first_char.x0, 50 * unit, places=3)
                    self.assertAlmostEqual(found.x0, 50 * unit, places=3)
                    for actual, expected_value in zip(found, word):
                        self.assertAlmostEqual(actual, expected_value, places=3)
                    self.assertAlmostEqual(coords.to_user_space(page,word).x0,70,places=3)
                    self.assertEqual(drawing, expected)
                    self.assertEqual(coords.to_user_space(page, drawing), MARK_USER)
                    # Image placement is also reported in text space (crop-relative and unit-scaled).
                    image_user = coords.to_user_space(page, image)
                    self.assertAlmostEqual(image_user.x0, 130, places=2)
                    self.assertAlmostEqual(image_user.y0, 280, places=2)
                    self.assertAlmostEqual(image_user.x1, 180, places=2)
                    self.assertAlmostEqual(image_user.y1, 320, places=2)
                    self.assertEqual(page.rotation, rotation)
                    expected_page = ((300, 400) if rotation in (90, 270) else (400, 300))
                    expected_page = tuple(value * unit for value in expected_page)
                    self.assertAlmostEqual(page.rect.width, expected_page[0], places=3)
                    self.assertAlmostEqual(page.rect.height, expected_page[1], places=3)
                    self.assertEqual(tuple(page.cropbox), (20, 70, 420, 370))
                    doc.close()

    def test_resolves_inherited_indirect_and_offset_page_boxes(self):
        for mode in ("inherited", "indirect", "offset"):
            with self.subTest(mode=mode):
                doc = pymupdf.open()
                page = doc.new_page(width=500, height=400)
                if mode == "inherited":
                    parent = int(doc.xref_get_key(page.xref, "Parent")[1].split()[0])
                    doc.xref_set_key(parent, "CropBox", "[20 30 420 330]")
                elif mode == "indirect":
                    box_xref = doc.get_new_xref()
                    doc.update_object(box_xref, "[20 30 420 330]")
                    doc.xref_set_key(page.xref, "CropBox", f"{box_xref} 0 R")
                else:
                    doc.xref_set_key(page.xref, "MediaBox", "[10 20 510 420]")
                    doc.xref_set_key(page.xref, "CropBox", "[30 50 430 350]")
                if mode == "offset":
                    got = coords.from_user_space(page, pymupdf.Rect(70, 300, 70, 300))
                    self.assertEqual(got, pymupdf.Rect(40, 50, 40, 50))
                else:
                    got = coords.from_user_space(page, pymupdf.Rect(60, 260, 110, 280))
                    self.assertEqual(got, pymupdf.Rect(40, 50, 90, 70))
                doc.close()

    def test_userunit_scaled_image_dpi_and_low_dpi_warning(self):
        import numpy as np
        from pdftools.check import _tier_stats, _tiers
        doc = pymupdf.open()
        page = doc.new_page(width=72, height=72)
        pix = pymupdf.Pixmap(pymupdf.csRGB, pymupdf.IRect(0, 0, 144, 144), False)
        pix.clear_with(128)
        page.insert_image((0, 0, 72, 72), stream=pix.tobytes("png"))
        doc.xref_set_key(page.xref, "UserUnit", "2")
        page = doc.reload_page(page)
        stats = _tier_stats(page)
        self.assertEqual(stats["image_dpi"], [72.0])
        findings = _tiers(page, page, 1)
        self.assertIn("LOW_DPI", {finding.code for finding in findings})
        doc.close()

    def test_rotated_sanitize_tiles_use_displayed_boxes(self):
        from pdftools.check import render_rgb, tile_differences
        doc = pymupdf.open()
        page = doc.new_page(width=200, height=300)
        page.set_rotation(90)
        source = render_rgb(page, 100, unrotated=True)
        output = source.copy()
        output[4:28, 4:28] = 0
        output[-28:-4, -28:-4] = 0
        changed = tile_differences(source, output, page=page, tile_px=40)
        boxes = [tuple(round(value, 1) for value in row["box_mm"]) for row in changed]
        self.assertIn((95.7, 0.0, 105.8, 10.2), boxes)
        icon_allowance = pymupdf.Rect(91.7, 0, 105.8, 12.4)
        unrelated = [pymupdf.Rect(row["box_mm"]) for row in changed
                     if not icon_allowance.intersects(pymupdf.Rect(row["box_mm"]))]
        self.assertTrue(unrelated)
        self.assertTrue(all(not box.intersects(icon_allowance) for box in unrelated))
        doc.close()

    def test_annotation_rect_is_pymupdf_page_space(self):
        for rotation in ROTATIONS:
            for unit in UNITS:
                with self.subTest(rotation=rotation, unit=unit):
                    doc = build_page(self.root / f"annot-{rotation}-{unit}.pdf", rotation, unit)
                    page = doc[0]
                    target = pymupdf.Rect(80, 90, 120, 110)
                    annot = page.add_rect_annot(target)
                    # annot.rect is already expressed in PyMuPDF page coordinates; its
                    # stroked bounds expand with /UserUnit just like the page geometry.
                    annotation_rect = pymupdf.Rect(annot.rect)
                    self.assertAlmostEqual(annotation_rect.width, target.width + 2 * unit, places=3)
                    self.assertAlmostEqual(annotation_rect.height, target.height + 2 * unit, places=3)
                    converted = coords.annotation_to_pymupdf(page, annotation_rect)
                    self.assertEqual(converted, annotation_rect)
                    doc.close()

    def test_display_roundtrip_and_render_clip_share_full_page_truth(self):
        for rotation in ROTATIONS:
            for unit in UNITS:
                with self.subTest(rotation=rotation, unit=unit):
                    doc = build_page(self.root / f"clip-{rotation}-{unit}.pdf", rotation, unit)
                    page = doc[0]
                    expected_text = pymupdf.Rect(40 * unit, 50 * unit, 90 * unit, 70 * unit)
                    if rotation == 90:
                        expected_points = pymupdf.Rect(230, 40, 250, 90) * unit
                    elif rotation == 180:
                        expected_points = pymupdf.Rect(310, 230, 360, 250) * unit
                    elif rotation == 270:
                        expected_points = pymupdf.Rect(50, 310, 70, 360) * unit
                    else:
                        expected_points = pymupdf.Rect(40, 50, 90, 70) * unit
                    expected_size = ((20 * unit, 50 * unit) if rotation in (90, 270)
                                     else (50 * unit, 20 * unit))
                    self.assertAlmostEqual(expected_points.width, expected_size[0], places=3)
                    self.assertAlmostEqual(expected_points.height, expected_size[1], places=3)
                    mm = expected_points * coords.MM_PER_POINT
                    recovered = coords.display_to_pymupdf(page, mm)
                    for actual, wanted in zip(recovered, expected_text):
                        self.assertAlmostEqual(actual, wanted, places=3)
                    full = page.get_pixmap(matrix=pymupdf.Matrix(2, 2), alpha=False)
                    clip = page.get_pixmap(matrix=pymupdf.Matrix(2, 2), clip=expected_points, alpha=False)
                    self.assertGreater(clip.width, 0)
                    self.assertGreater(clip.height, 0)
                    self.assertLessEqual(clip.width, full.width)
                    self.assertLessEqual(clip.height, full.height)
                    self.assertGreater(sum(clip.samples[i] < 8 for i in range(0, len(clip.samples), clip.n)), 0)
                    doc.close()


if __name__ == "__main__":
    unittest.main()
