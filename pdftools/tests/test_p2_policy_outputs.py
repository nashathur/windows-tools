import tempfile
import unittest
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))

import pymupdf

from pdftools.check import ExpectedChanges, Features, OperationResult, Placement, compare
from tests.fixtures.p2 import build_fixtures


class PolicyOutputTest(unittest.TestCase):
    def test_p2_transformed_policy_outputs_pass_and_broken_outputs_fail(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        policy_dir = Path(temp.name)
        source = pymupdf.open()
        page = source.new_page(width=300, height=400)
        page.insert_text((30, 60), "2026 Climate report")
        source_path = policy_dir / "source.pdf"
        source.save(source_path)
        source.close()

        with pymupdf.open(source_path) as source:
            rotated = pymupdf.open(source_path)
            rotated[0].set_rotation(90)
            rot_op = OperationResult(pages=[[Placement(None, 0, rotation=90)]])
            self.assertFalse([f for f in compare(source, rotated, "rotate", rot_op) if f.tier == "invariant"])
            self.assertIn("ROTATION_CHANGED", {f.code for f in compare(source, rotated, "rotate",
                          OperationResult(pages=[[Placement(None, 0, rotation=180)]]))})
            rotated.close()

            cropped = pymupdf.open(source_path)
            box = pymupdf.Rect(20, 30, 150, 100)
            cropped[0].set_cropbox(box)
            crop_op = OperationResult(pages=[[Placement(None, 0)]], cropboxes=[tuple(box)])
            self.assertFalse([f for f in compare(source, cropped, "crop", crop_op) if f.tier == "invariant"])
            self.assertIn("CROPBOX_CHANGED", {f.code for f in compare(source, cropped, "crop",
                          OperationResult(pages=[[Placement(None, 0)]], cropboxes=[(20, 30, 151, 100)]))})
            cropped.close()

            stamped = pymupdf.open(source_path)
            stamped[0].insert_text((100, 130), "2026")
            stamp_op = OperationResult(pages=[[Placement(None, 0)]],
                expected=ExpectedChanges(regions=[(30, 35, 60, 55)], text_added=["2026"]))
            self.assertFalse([f for f in compare(source, stamped, "stamp", stamp_op) if f.tier == "invariant"])
            broken_stamp = OperationResult(pages=[[Placement(None, 0)]],
                expected=ExpectedChanges(regions=[(200, 200, 220, 220)], text_added=["2026"]))
            self.assertIn("TILES_CHANGED", {f.code for f in compare(source, stamped, "stamp", broken_stamp)})
            stamped.close()

            annotated = pymupdf.open(source_path)
            annotated[0].add_highlight_annot([pymupdf.Rect(25, 45, 130, 65)])
            self.assertFalse([f for f in compare(source, annotated, "annot_add",
                OperationResult(pages=[[Placement(None, 0)]], annotation_delta=1)) if f.tier == "invariant"])
            self.assertIn("ANNOTATION_COUNT", {f.code for f in compare(source, annotated, "annot_add",
                OperationResult(pages=[[Placement(None, 0)]], annotation_delta=2))})
            annotated.close()

            with pymupdf.open(source_path) as annotated_source:
                annotated_source[0].add_text_annot((30, 100), "note")
                ann_path = policy_dir / "annotations.pdf"
                annotated_source.save(ann_path)
            annotated_source = pymupdf.open(ann_path)
            removed = pymupdf.open(ann_path)
            removed[0].delete_annot(next(removed[0].annots()))
            self.assertFalse([f for f in compare(annotated_source, removed, "annot_remove",
                OperationResult(pages=[[Placement(None, 0)]], annotation_delta=-1)) if f.tier == "invariant"])
            self.assertIn("ANNOTATION_COUNT", {f.code for f in compare(annotated_source, removed, "annot_remove",
                OperationResult(pages=[[Placement(None, 0)]], annotation_delta=-2))})
            annotated_source.close(); removed.close()

            resaved_path = policy_dir / "resaved.pdf"
            with pymupdf.open(source_path) as before:
                before.save(resaved_path, garbage=3, deflate=True)
            lossless = pymupdf.open(resaved_path)
            self.assertFalse([f for f in compare(source, lossless, "compress_lossless") if f.tier == "invariant"])
            self.assertFalse([f for f in compare(source, lossless, "placements") if f.tier == "invariant"])
            lossless.close()
            changed = pymupdf.open()
            bad_page = changed.new_page(width=300, height=400)
            bad_page.insert_text((30, 60), "2027 Climate report")
            self.assertIn("TEXT_CHANGED", {f.code for f in compare(source, changed, "compress_lossless")})
            self.assertFalse(compare(source, changed, "report"))
            changed.close()

            raster_source = pymupdf.open(source_path)
            pix = raster_source[0].get_pixmap(matrix=pymupdf.Matrix(100 / 72, 100 / 72), alpha=False)
            raster = pymupdf.open()
            raster_page = raster.new_page(width=300, height=400)
            raster_page.insert_image(raster_page.rect, stream=pix.tobytes("png"))
            raster_op = OperationResult(pages=[[Placement(None, 0)]])
            raster_findings = compare(raster_source, raster, "flatten_raster", raster_op)
            self.assertFalse([f for f in raster_findings if f.tier == "invariant"])
            self.assertTrue(any(f.code == "TEXT_RASTERIZED" for f in raster_findings))
            broken_raster = pymupdf.open()
            broken_raster.new_page(width=300, height=400)
            self.assertIn("TILES_CHANGED", {f.code for f in compare(raster_source, broken_raster,
                                                                     "flatten_raster", raster_op)})
            raster_source.close(); raster.close(); broken_raster.close()

        with tempfile.TemporaryDirectory() as folder:
            fixtures = build_fixtures(Path(folder))
            with pymupdf.open(fixtures["calibration_photo"]) as photo, \
                    pymupdf.open(fixtures["calibration_photo_q80"]) as q80:
                self.assertFalse([f for f in compare(photo, q80, "compress_lossy",
                    OperationResult(ssim_floor=0.90)) if f.tier == "invariant"])
            with pymupdf.open(fixtures["calibration_small_image"]) as photo, \
                    pymupdf.open(fixtures["calibration_small_gray_box"]) as bad:
                self.assertIn("SSIM_LOW", {f.code for f in compare(photo, bad, "compress_lossy",
                    OperationResult(ssim_floor=0.90))})

    def test_p2_flatten_widget_value_and_broken_missing_value(self):
        source = pymupdf.open()
        page = source.new_page(width=300, height=400)
        widget = pymupdf.Widget()
        widget.field_name = "name"
        widget.field_type = pymupdf.PDF_WIDGET_TYPE_TEXT
        widget.rect = pymupdf.Rect(30, 30, 130, 60)
        widget.field_value = "Ada"
        page.add_widget(widget)
        with tempfile.TemporaryDirectory() as folder:
            source_path = Path(folder) / "form.pdf"
            source.save(source_path)
            source.close()
            with pymupdf.open(source_path) as before:
                flat = pymupdf.open()
                flat_page = flat.new_page(width=300, height=400)
                flat_page.insert_text((30, 50), "Ada")
                op = OperationResult(pages=[[Placement(None, 0)]],
                    expected=ExpectedChanges(regions=[(0, 0, 55, 30)]),
                    features=Features(dropped={"widgets": 1}))
                findings = compare(before, flat, "flatten", op)
                self.assertFalse([f for f in findings if f.tier == "invariant"])
                self.assertTrue(any(f.code == "FEATURES_DROPPED" for f in findings))
                broken = pymupdf.open()
                broken.new_page(width=300, height=400)
                self.assertIn("TEXT_CHANGED", {f.code for f in compare(before, broken, "flatten", op)})
                flat.close(); broken.close()

    def test_p2_gray_recolor_and_broken_color_output(self):
        source = pymupdf.open()
        page = source.new_page(width=300, height=400)
        page.insert_text((30, 60), "Climate")
        page.draw_rect(pymupdf.Rect(80, 100, 160, 150), color=(1, 0, 0), fill=(1, 0, 0))
        recolored = pymupdf.open()
        out_page = recolored.new_page(width=300, height=400)
        out_page.insert_text((30, 60), "Climate")
        gray = 76 / 255
        out_page.draw_rect(pymupdf.Rect(80, 100, 160, 150), color=(gray, gray, gray), fill=(gray, gray, gray))
        self.assertFalse([f for f in compare(source, recolored, "gray") if f.tier == "invariant"])
        self.assertIn("COLOR_REMAINS", {f.code for f in compare(source, source, "gray")})
        source.close(); recolored.close()

    def test_p2_ocr_invisible_text_and_broken_missing_text(self):
        with tempfile.TemporaryDirectory() as folder:
            fixtures = build_fixtures(Path(folder))
            with pymupdf.open(fixtures["scanned_image"]) as source:
                recognized = pymupdf.open(fixtures["scanned_image"])
                recognized[0].insert_text((25, 35), "station", render_mode=3)
                op = OperationResult(pages=[[Placement(None, 0)]], expected=ExpectedChanges(text_added=["station"]))
                self.assertFalse([f for f in compare(source, recognized, "ocr", op) if f.tier == "invariant"])
                self.assertIn("TEXT_ADDITION_MISSING", {f.code for f in compare(source, source, "ocr", op)})
                recognized.close()

    def test_p2_rmblank_and_structure_outputs_pass_and_broken_outputs_fail(self):
        source = pymupdf.open()
        source.new_page(width=300, height=400).insert_text((30, 60), "Keep")
        source.new_page(width=300, height=400)
        trimmed = pymupdf.open()
        trimmed.insert_pdf(source, from_page=0, to_page=0)
        op = OperationResult(pages=[[Placement(None, 0)]])
        self.assertFalse([f for f in compare(source, trimmed, "rmblank", op) if f.tier == "invariant"])
        bad = pymupdf.open()
        bad.new_page(width=300, height=400)
        self.assertIn("TEXT_CHANGED", {f.code for f in compare(source, bad, "rmblank", op)})
        structured = pymupdf.open()
        structured.insert_pdf(source, from_page=0, to_page=0)
        metadata = dict(structured.metadata)
        metadata["title"] = "New title"
        structured.set_metadata(metadata)
        self.assertFalse([f for f in compare(source, structured, "structure", op) if f.tier == "invariant"])
        self.assertIn("TEXT_CHANGED", {f.code for f in compare(source, bad, "structure", op)})
        source.close(); trimmed.close(); bad.close(); structured.close()


if __name__ == "__main__":
    unittest.main()
