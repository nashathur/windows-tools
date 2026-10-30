from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import pymupdf
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))

from pdftools.commands import p8_eqread, p8_tables


def _part(folder, equation=True, table=True):
    folder = Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    doc = pymupdf.open()
    page = doc.new_page()
    page.insert_text((90, 170), "Name Value alpha 2 beta 3")
    page.draw_rect(pymupdf.Rect(72, 140, 360, 250), width=1)
    page.draw_line((220, 140), (220, 250), width=1)
    page.draw_line((72, 175), (360, 175), width=1)
    page.draw_line((72, 212), (360, 212), width=1)
    source = folder / "source.pdf"
    doc.save(source)
    doc.close()
    wrong_table = "| wrong | value |\n|---|---|\n| alpha | 2 |"
    (folder / "part.src.md").write_text(("<!--eq:e1-->\n\n" if equation else "") +
                                        (wrong_table if table else ""), encoding="utf-8")
    (folder / "part.md").write_text(wrong_table + "\n", encoding="utf-8")
    crop = folder / "eq.png"
    Image.new("RGB", (180, 55), "white").save(crop)
    eqs = ([{"id": "e1", "page": 1, "box": [80, 120, 300, 180], "crop": "eq.png", "pdf_text": "x = 1",
             "bold": "", "labels": [], "crop_sha": "x", "readings": [], "latex": None}] if equation else [])
    (folder / "eqs.json").write_text(json.dumps(eqs), encoding="utf-8")
    table_html = "| wrong | value |\n|---|---|\n| alpha | 2 |" if table else ""
    table_block = {"id": "/page/0/Table/1", "block_type": "Table", "bbox": [72, 140, 360, 250],
                   "html": table_html}
    page_data = {"id": "/page/0/Page/0", "children": ([{"id": table_block["id"], "text": table_html}] if table else [])}
    (folder / "part.blocks.json").write_text(json.dumps({"pages": [{"id": "/page/0/Page/0",
        "children": [table_block] if table else []}]}), encoding="utf-8")
    (folder / "part.pages.json").write_text(json.dumps({"source_pdf": str(source), "ocr_pages": [],
        "pages": [page_data]}), encoding="utf-8")
    if not table:
        (folder / "part.md").write_text("", encoding="utf-8")
    candidate = None
    if table:
        img = folder / "table.png"
        Image.new("RGB", (200, 100), "white").save(img)
        candidate = {"id": 1, "label": 1, "page": 1, "box": (72, 702, 360, 592), "shape": (3, 2),
                     "current": "| wrong | value |\n|---|---|\n| alpha | 2 |", "scanned": False,
                     "image": str(img), "source_id": table_block["id"], "block_id": table_block["id"],
                     "blocks_path": folder / "part.blocks.json", "pages_path": folder / "part.pages.json"}
    return folder, candidate


class P8SheetTests(unittest.TestCase):
    def test_pass1_combines_labels_and_single_kind_parts_work(self):
        with tempfile.TemporaryDirectory() as temp:
            cases = []
            for name, eq, table in (("both", True, True), ("tables", False, True), ("equations", True, False)):
                cases.append(_part(Path(temp) / name, eq, table))
            candidates = {str(folder): ([candidate] if candidate else []) for folder, candidate in cases}
            calls = []

            def fake_ask(prompt_path, folder, role, effort, images):
                prompt = Path(prompt_path).read_text(encoding="utf-8")
                calls.append((Path(folder).name, prompt, [Path(image).name for image in images]))
                answers = []
                if "E1" in prompt:
                    answers.append("E1\tx = 1")
                if "T1" in prompt:
                    answers.append("T1\t| Name | Value | || |---|---| || | alpha | 2 | || | beta | 3 |")
                return "\n".join(answers)

            with patch.object(p8_tables, "pending", side_effect=lambda folder: candidates[str(folder)]), \
                 patch.object(p8_tables, "_coverage", return_value=1.0), \
                 patch.object(p8_eqread, "judge", return_value={"latex": "x = 1", "problems": [], "minor": [],
                                                                        "checked": True, "unchecked_reason": None}), \
                 patch("pdftools.commands.p8_codex.ask", side_effect=fake_ask):
                p8_eqread.run([str(folder) for folder, _ in cases])

            self.assertEqual(len(calls), 3)
            combined = next(call for call in calls if call[0] == "both")
            self.assertIn("E1", combined[1]); self.assertIn("T1", combined[1])
            self.assertIn("pass1_1.png", combined[1]); self.assertEqual(combined[2], ["pass1_1.png"])
            both_folder = cases[0][0]
            eqs = json.loads((both_folder / "eqs.json").read_text(encoding="utf-8"))
            self.assertEqual(eqs[0]["latex"], "x = 1")
            part_md = (both_folder / "part.md").read_text(encoding="utf-8")
            self.assertIn("Name | Value |", part_md)
            self.assertIn("| beta | 3 |", part_md)
            self.assertTrue(any("E1" in call[1] and "T1" not in call[1] for call in calls))
            self.assertTrue(any("T1" in call[1] and "E1" not in call[1] for call in calls))

    def _scanned_table_crop(self, path, blur=0):
        image = Image.new("RGB", (1400, 700), "white")
        draw = ImageDraw.Draw(image)
        xs, ys = (100, 700, 1300), (70, 250, 430, 610)
        for y in ys:
            draw.line((xs[0], y, xs[-1], y), fill=(100, 100, 100), width=3)
        for x in xs:
            for ya, yb in zip(ys, ys[1:]):
                draw.line((x, ya + 10, x, yb - 10), fill=(100, 100, 100), width=3)
        font_candidates = (
            Path("C:/Windows/Fonts/arialbd.ttf"),
            Path("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"),
            Path("/System/Library/Fonts/Supplemental/Arial Bold.ttf"),
        )
        font_path = next((candidate for candidate in font_candidates if candidate.is_file()), None)
        font = ImageFont.truetype(str(font_path), 60) if font_path else ImageFont.load_default(size=60)
        for word, x, y in (("Name", 160, 210), ("Value", 760, 210), ("alpha", 160, 390),
                           ("2", 760, 390), ("beta", 160, 570), ("3", 760, 570)):
            draw.text((x, y - font.size), word, fill="black", font=font)
        if blur:
            image = image.filter(ImageFilter.GaussianBlur(blur))
        image.save(path)

    def test_real_tesseract_confidence_controls_scanned_table_reads(self):
        from tests.tooling import require_tool
        tesseract, _ = require_tool(self, "tesseract", "table confidence uses Tesseract TSV")
        which_patch = patch.object(p8_tables.shutil, "which", return_value=tesseract)
        which_patch.start()
        self.addCleanup(which_patch.stop)
        with tempfile.TemporaryDirectory() as temp:
            clear = Path(temp) / "clear.png"
            blurred = Path(temp) / "blurred.png"
            self._scanned_table_crop(clear)
            self._scanned_table_crop(blurred, blur=5)
            self.assertGreaterEqual(p8_tables._confidence(clear, temp), 85)
            self.assertLess(p8_tables._confidence(blurred, temp), 85)


if __name__ == "__main__":
    unittest.main()
