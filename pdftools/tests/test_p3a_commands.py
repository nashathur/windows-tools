import contextlib
import io
import json
import os
import sys
import tempfile
import unittest
from io import BytesIO
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))

import pymupdf

from pdftools import cli
from tests.fixtures import sans_font
from tests.fixtures.p3a import build_fixtures


class Phase3aCommandsTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.fixtures = build_fixtures(self.root / "fixtures")

    def tearDown(self):
        self.tmp.cleanup()

    def run_cli(self, *argv, json_output=True):
        out, err = io.StringIO(), io.StringIO()
        flags = ["--json"] if json_output else []
        with patch("clierr.LOG_ROOT", self.root / "logs"), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main([*flags, *map(str, argv)])
        record = json.loads(out.getvalue()) if json_output and out.getvalue().strip() else out.getvalue()
        return code, record, err.getvalue()

    def test_info_folder_summarizes_pdfs_and_info_file_shape_stays(self):
        code, record, _ = self.run_cli("info", self.fixtures["folder"])
        self.assertEqual(code, 0)
        lines = record["data"].splitlines()
        self.assertEqual(len(lines), 3)
        self.assertTrue(any("source.pdf" in line and "2 pages" in line for line in lines))
        self.assertTrue(any("Phase 3a fixture" in line and "text layer=yes" in line for line in lines))
        self.assertTrue(any("restricted.pdf" in line and "encrypted=yes" in line for line in lines))
        code, single, _ = self.run_cli("info", self.fixtures["source"])
        self.assertEqual(code, 0)
        self.assertIsInstance(single["data"], dict)
        self.assertEqual(single["data"]["pages"], 2)

    def test_pages_reports_labels_size_rotation_blank_and_first_line(self):
        code, record, _ = self.run_cli("pages", self.fixtures["source"])
        self.assertEqual(code, 0)
        self.assertIn("A4", record["data"])
        self.assertIn("Phase three A climate report", record["data"])
        self.assertIn("rotation 0°", record["data"])

    def test_pages_labels_all_pages_and_render_based_blank_detection(self):
        code, labels, _ = self.run_cli("pages", self.fixtures["labeled"])
        self.assertEqual(code, 0)
        self.assertIn("label i", labels["data"])
        self.assertIn("label ii", labels["data"])
        code, blank, _ = self.run_cli("pages", self.fixtures["empty"])
        self.assertEqual(code, 0)
        self.assertIn("blank yes", blank["data"])
        code, drawn, _ = self.run_cli("pages", self.fixtures["image_source"])
        self.assertEqual(code, 0)
        self.assertIn("blank no", drawn["data"])

    def test_text_layout_and_file_output(self):
        code, record, _ = self.run_cli("text", self.fixtures["source"])
        self.assertEqual(code, 0)
        self.assertIn("Temperature rainfall wind", record["data"])
        output = self.root / "export.txt"
        code, result, _ = self.run_cli("text", self.fixtures["source"], "--layout", "-o", output)
        self.assertEqual(code, 0, result)
        self.assertTrue(output.is_file())
        self.assertIn("Second page", output.read_text(encoding="utf-8"))

    def test_count_per_page_total_and_empty_page(self):
        code, record, _ = self.run_cli("count", self.fixtures["source"])
        self.assertEqual(code, 0)
        self.assertIn("page 1: words", record["data"])
        self.assertIn("total: words", record["data"])
        code, record, _ = self.run_cli("count", self.fixtures["empty"], "--total")
        self.assertEqual(code, 0)
        self.assertEqual(record["data"]["pages_without_text_layer"], 1)
        self.assertEqual(record["data"]["words"], 0)

    def test_json_contains_pages_blocks_lines_spans_and_metadata(self):
        code, record, _ = self.run_cli("json", self.fixtures["source"])
        self.assertEqual(code, 0)
        data = record["data"] if "data" in record else json.loads(Path(record["path"]).read_text(encoding="utf-8"))
        self.assertEqual(len(data["pages"]), 2)
        self.assertTrue(data["pages"][0]["blocks"])
        span = data["pages"][0]["spans"][0]
        self.assertIn("font", span)
        self.assertIn("bbox_pymupdf", span)
        self.assertIn("bbox_mm", span)
        self.assertAlmostEqual(span["bbox_mm"][0], span["bbox_pymupdf"][0] * 25.4 / 72, delta=0.02)
        self.assertEqual(data["metadata"]["title"], "Phase 3a fixture")
        output = self.root / "export.json"
        code, result, _ = self.run_cli("json", self.fixtures["source"], "-o", output)
        self.assertEqual(code, 0, result)
        self.assertEqual(json.loads(output.read_text(encoding="utf-8"))["metadata"]["title"], "Phase 3a fixture")

    def test_json_bbox_millimetres_are_correct_after_crop_and_rotation(self):
        from pdftools import coords
        rotated = self.root / "rotated-cropped.pdf"
        doc = pymupdf.open()
        page = doc.new_page(width=600, height=800)
        page.insert_text((80, 100), "Rotated cropped coordinates")
        page.set_cropbox(pymupdf.Rect(30, 40, 560, 760))
        page.set_rotation(90)
        doc.save(rotated)
        doc.close()
        code, report, _ = self.run_cli("json", rotated)
        self.assertEqual(code, 0)
        actual = json.loads(report["data"])["pages"][0]["spans"][0]
        with pymupdf.open(rotated) as doc:
            page = doc[0]
            bbox = pymupdf.Rect(actual["bbox_pymupdf"])
            expected = coords.pymupdf_to_display(page, bbox)
        for got, want in zip(actual["bbox_mm"], expected):
            self.assertAlmostEqual(got, want, delta=0.001)

    def test_json_and_pages_coordinate_grid_with_userunit(self):
        from pdftools import coords
        for rotation in (0,90,180,270):
            for unit in (1,1.5,2):
                with self.subTest(rotation=rotation,unit=unit):
                    source=self.root/f"json-grid-{rotation}-{unit}.pdf"
                    doc=pymupdf.open();page=doc.new_page(width=500,height=400)
                    page.insert_text((70,150),"Grid coordinate label")
                    doc.xref_set_key(page.xref,"CropBox","[20 30 420 330]")
                    if unit!=1:doc.xref_set_key(page.xref,"UserUnit",str(unit))
                    page.set_rotation(rotation);doc.save(source);doc.close()
                    code,report,_=self.run_cli("json",source)
                    self.assertEqual(code,0,report)
                    span=json.loads(report["data"])["pages"][0]["spans"][0]
                    x0,y0,x1,y1=span["bbox_pymupdf"]
                    width,height=400*unit,300*unit
                    if rotation==90: expected=(height-y1,x0,height-y0,x1)
                    elif rotation==180: expected=(width-x1,height-y1,width-x0,height-y0)
                    elif rotation==270: expected=(y0,width-x1,y1,width-x0)
                    else: expected=(x0,y0,x1,y1)
                    expected=tuple(value*25.4/72 for value in expected)
                    for got,want in zip(span["bbox_mm"],expected):
                        self.assertAlmostEqual(got,want,delta=.001)
                    code,pages,_=self.run_cli("pages",source)
                    self.assertEqual(code,0,pages)
                    shown_width,shown_height=((height,width) if rotation in (90,270) else (width,height))
                    self.assertIn(f"{shown_width*25.4/72:.2f}",pages["data"])
                    self.assertIn(f"{shown_height*25.4/72:.2f} mm",pages["data"])

    def test_html_outputs_per_page_xhtml(self):
        code, record, _ = self.run_cli("html", self.fixtures["source"])
        self.assertEqual(code, 0)
        self.assertIn('<div id="page0">', record["data"])
        output = self.root / "export.html"
        code, result, _ = self.run_cli("html", self.fixtures["source"], "-o", output)
        self.assertEqual(code, 0, result)
        self.assertTrue(output.is_file())

    def test_text_json_html_directory_outputs_keep_their_extensions(self):
        destination = self.root / "exports"
        destination.mkdir()
        for command, extension in (("text", ".txt"), ("json", ".json"), ("html", ".html")):
            code, record, _ = self.run_cli(command, self.fixtures["source"], "-o", destination)
            self.assertEqual(code, 0, record)
            self.assertTrue((destination / ("source" + extension)).is_file())

    def test_size_lists_objects_page_usage_and_totals(self):
        code, record, _ = self.run_cli("size", self.fixtures["source"])
        self.assertEqual(code, 0)
        self.assertTrue(record["data"]["objects"])
        self.assertEqual(len(record["data"]["page_usage"]), 2)
        self.assertEqual(record["data"]["total_bytes"], self.fixtures["source"].stat().st_size)

    def test_size_counts_raw_embedded_attachment_form_and_soft_mask_streams_once(self):
        attached = self.root / "attached.pdf"
        doc = pymupdf.open(self.fixtures["source"])
        doc.embfile_add("notes.txt", b"stored attachment payload")
        doc.save(attached)
        doc.close()
        code, report, _ = self.run_cli("size", attached)
        self.assertEqual(code, 0, report)
        attachment = report["data"]["attachments"][0]
        import pikepdf
        with pikepdf.open(attached) as pdf:
            stream = pdf.attachments["notes.txt"].obj["/EF"]["/F"]
            self.assertEqual(attachment["bytes"], len(stream.read_raw_bytes()))
            self.assertIn(attachment["xref"], {obj["xref"] for obj in report["data"]["objects"]})

        from PIL import Image
        rgba = Image.new("RGBA", (8, 8), (230, 40, 60, 120))
        png = BytesIO()
        rgba.save(png, format="PNG")
        source = self.root / "form-source.pdf"
        doc = pymupdf.open()
        page = doc.new_page()
        page.insert_image(pymupdf.Rect(40, 40, 180, 180), stream=png.getvalue())
        doc.save(source)
        doc.close()
        imported = self.root / "form-imported.pdf"
        doc = pymupdf.open()
        page = doc.new_page()
        with pymupdf.open(source) as source_doc:
            page.show_pdf_page(page.rect, source_doc, 0)
        doc.save(imported)
        doc.close()
        code, report, _ = self.run_cli("size", imported)
        self.assertEqual(code, 0, report)
        xrefs = {obj["xref"] for obj in report["data"]["objects"]}
        self.assertEqual(len(xrefs), len(report["data"]["objects"]))
        with pikepdf.open(imported) as pdf:
            page_xobj = pdf.pages[0].obj["/Resources"]["/XObject"]
            form_stream = next(obj for obj in page_xobj.values() if str(obj.get("/Subtype", "")) == "/Form")
            self.assertIn(form_stream.objgen[0], xrefs)
            images, masks = set(), set()
            def walk(resource_dict, seen):
                for xobj in resource_dict.get("/XObject", {}).values():
                    if not isinstance(xobj, pikepdf.Stream) or xobj.objgen[0] in seen:
                        continue
                    seen.add(xobj.objgen[0])
                    subtype = str(xobj.get("/Subtype", ""))
                    if subtype == "/Image":
                        images.add(xobj.objgen[0])
                        smask = xobj.get("/SMask")
                        if isinstance(smask, pikepdf.Stream):
                            masks.add(smask.objgen[0])
                    if subtype == "/Form":
                        walk(xobj.get("/Resources", {}), seen)
            walk(pdf.pages[0].obj["/Resources"], set())
            self.assertTrue(images)
            self.assertTrue(masks)
            self.assertTrue(images | masks <= xrefs)
            actual = {obj["xref"]: obj["bytes"] for obj in report["data"]["objects"]}
            for xref, count in actual.items():
                pdf_object = pdf.get_object((xref, 0))
                if isinstance(pdf_object, pikepdf.Stream):
                    self.assertEqual(count, len(pdf_object.read_raw_bytes()), f"xref {xref}")

    def test_size_metadata_is_the_stored_info_dictionary(self):
        import pikepdf
        empty = self.root / "no-info.pdf"
        doc = pymupdf.open()
        doc.new_page()
        doc.save(empty)
        doc.close()
        code, report, _ = self.run_cli("size", empty)
        self.assertEqual(code, 0, report)
        self.assertEqual(report["data"]["metadata_bytes"], 0)
        custom = self.root / "custom-info.pdf"
        with pikepdf.open(empty) as pdf:
            pdf.docinfo["/CustomKey"] = "stored-value"
            pdf.save(custom)
        code, report, _ = self.run_cli("size", custom)
        self.assertEqual(code, 0, report)
        with pikepdf.open(custom) as pdf:
            expected = len(bytes(pdf.trailer["/Info"].unparse()))
        self.assertEqual(report["data"]["metadata_bytes"], expected)

    def test_size_font_file_stream_uses_raw_stored_bytes(self):
        font_pdf = self.root / "embedded-font.pdf"
        doc = pymupdf.open()
        page = doc.new_page()
        page.insert_font(fontname="TestFont", fontfile=sans_font())
        page.insert_text((40, 60), "Embedded font stream", fontname="TestFont")
        doc.save(font_pdf)
        doc.close()
        code, report, _ = self.run_cli("size", font_pdf)
        self.assertEqual(code, 0, report)
        import pikepdf
        with pikepdf.open(font_pdf) as pdf:
            font = next(iter(pdf.pages[0].obj["/Resources"]["/Font"].values()))
            candidate = list(font.get("/DescendantFonts", []))[0] if font.get("/DescendantFonts") else font
            descriptor = candidate["/FontDescriptor"]
            font_stream = next(descriptor[key] for key in ("/FontFile", "/FontFile2", "/FontFile3") if key in descriptor)
            rows = {obj["xref"]: obj["bytes"] for obj in report["data"]["objects"]}
            self.assertIn(font_stream.objgen[0], rows)
            self.assertEqual(rows[font_stream.objgen[0]], len(font_stream.read_raw_bytes()))

    def test_check_inspect_and_auto_or_manual_against_matching(self):
        code, record, _ = self.run_cli("check", self.fixtures["source"])
        self.assertEqual(code, 0)
        self.assertEqual(len(record["data"]["pages"]), 2)
        code, record, _ = self.run_cli("check", self.fixtures["reordered"], "--against", self.fixtures["source"])
        self.assertEqual(code, 0)
        self.assertEqual(record["data"]["pairs"], [
            {"output_page": 1, "source_page": 2}, {"output_page": 2, "source_page": 1}])
        code, record, _ = self.run_cli("check", self.fixtures["reordered"], "--against", self.fixtures["source"],
                                       "--map", "1=2,2=1")
        self.assertEqual(code, 0)
        self.assertEqual([pair["source_page"] for pair in record["data"]["pairs"]], [2, 1])
        code, record, _ = self.run_cli("check", self.fixtures["image_copy"], "--against", self.fixtures["image_source"])
        self.assertEqual(code, 0)
        self.assertEqual(record["data"]["pairs"][0]["source_page"], 1)
        code, record, _ = self.run_cli("check", self.fixtures["image_copy"], "--against",
                                       self.fixtures["image_duplicate_source"])
        self.assertEqual(code, 0)
        self.assertEqual(record["data"]["ambiguous"], {"1": [1, 2]})

    def test_check_reports_differences_rotation_and_preexisting_tiers(self):
        original, changed = self.root / "same.pdf", self.root / "changed.pdf"
        doc = pymupdf.open()
        phrase = "climate data " * 100 + "123"
        doc.new_page().insert_textbox(pymupdf.Rect(40, 60, 550, 780), phrase)
        doc.save(original)
        doc.close()
        doc = pymupdf.open()
        doc.new_page().insert_textbox(pymupdf.Rect(40, 60, 550, 780), "climate data " * 100 + "124")
        doc.save(changed)
        doc.close()
        code, result, _ = self.run_cli("check", changed, "--against", original)
        self.assertEqual(code, 0)
        self.assertEqual(result["verdict"], "warn", result)
        self.assertTrue(result["data"]["findings"])
        code, same, _ = self.run_cli("check", original, "--against", original)
        self.assertEqual(code, 0)
        self.assertEqual(same["verdict"], "report")
        self.assertFalse(same["data"]["findings"])
        rotated = self.root / "rotated.pdf"
        doc = pymupdf.open(original)
        doc[0].set_rotation(180)
        doc.save(rotated)
        doc.close()
        code, rotation, _ = self.run_cli("check", rotated, "--against", original)
        self.assertEqual(code, 0)
        self.assertIn("ROTATION_CHANGED", {row["code"] for row in rotation["data"]["findings"]})
        code, preexisting, _ = self.run_cli("check", self.fixtures["empty"], "--against", self.fixtures["empty"])
        self.assertEqual(code, 0)
        self.assertEqual(preexisting["verdict"], "report")
        self.assertGreater(preexisting["data"]["pre_existing_count"], 0)
        self.assertFalse(any(row["tier"] == "pre_existing" for row in preexisting["data"]["findings"]))
        code, verbose, _ = self.run_cli("-v", "check", self.fixtures["empty"], "--against", self.fixtures["empty"])
        self.assertEqual(code, 0)
        self.assertTrue(any(row["tier"] == "pre_existing" for row in verbose["data"]["findings"]))

    def test_check_job_against_resolves_from_job_directory(self):
        job_dir, other_cwd = self.root / "job", self.root / "elsewhere"
        job_dir.mkdir()
        other_cwd.mkdir()
        (job_dir / "input.pdf").write_bytes(self.fixtures["source"].read_bytes())
        (job_dir / "origin.pdf").write_bytes(self.fixtures["source"].read_bytes())
        job = job_dir / "check.json"
        job.write_text(json.dumps({"version": 1, "command": "check", "input": "input.pdf", "against": "origin.pdf"}), encoding="utf-8")
        cwd = Path.cwd()
        try:
            os.chdir(other_cwd)
            code, result, _ = self.run_cli("--job", job)
        finally:
            os.chdir(cwd)
        self.assertEqual(code, 0, result)
        self.assertEqual(result["data"]["against"], str((job_dir / "origin.pdf").resolve()))

    def test_authenticated_inspection_and_stdout_export_warning(self):
        password = self.root / "password.txt"
        password.write_text("user-pass\n", encoding="utf-8")
        code, result, _ = self.run_cli("check", self.fixtures["encrypted"], "--password-file", password)
        self.assertEqual(code, 0, result)
        for supplied in (None, "wrong"):
            pwfile = password
            if supplied:
                pwfile = self.root / "wrong.txt"
                pwfile.write_text(supplied + "\n", encoding="utf-8")
            args = ("--password-file", pwfile) if supplied else ()
            code, failed, _ = self.run_cli("check", self.fixtures["encrypted"], *args)
            self.assertEqual(code, 1, (failed, _))
            self.assertEqual(failed["code"], "WRONG_PASSWORD")
        for command in ("text", "json", "html"):
            owner = self.root / "owner.txt"
            owner.write_text("owner-pass\n", encoding="utf-8")
            code, report, err = self.run_cli(command, self.fixtures["encrypted"], "--password-file", owner)
            self.assertEqual(code, 0, (report, err, Path(report.get("log", "")).read_text(encoding="utf-8") if report.get("log") else ""))
            self.assertEqual(report["verdict"], "warn")
            self.assertIn("DECRYPTED_EXPORT", err)
            output = self.root / f"encrypted-{command}.{ {'text': 'txt', 'json': 'json', 'html': 'html'}[command] }"
            code, saved, _ = self.run_cli(command, self.fixtures["encrypted"], "--password-file", owner, "-o", output)
            self.assertEqual(code, 0, saved)
            self.assertIn("DECRYPTED_EXPORT", saved["warnings"])
            self.assertTrue(output.is_file())

    def test_destination_permission_failure_names_output_path(self):
        output = self.root / "denied.txt"
        with patch("pdftools.commands.p3a_core.write_non_pdf", side_effect=PermissionError("no access")):
            code, result, _ = self.run_cli("text", self.fixtures["source"], "-o", output)
        self.assertEqual(code, 1, result)
        self.assertEqual(result["code"], "LOCKED")
        self.assertIn(str(output), result["message"])

    def test_open_calls_startfile_and_does_not_launch_viewer(self):
        with patch("pdftools.commands.p3a_core.os.startfile", create=True) as startfile:
            code, record, _ = self.run_cli("open", self.fixtures["source"])
        self.assertEqual(code, 0)
        self.assertEqual(record["command"], "open")
        startfile.assert_called_once_with(str(self.fixtures["source"].resolve()))

    def test_bounded_text_output_and_stdout_override(self):
        long_pdf = self.root / "long.pdf"
        doc = pymupdf.open()
        for index in range(101):
            page = doc.new_page()
            page.insert_text((30, 40), f"Page {index + 1} text")
        doc.save(long_pdf)
        doc.close()
        code, bounded, _ = self.run_cli("text", long_pdf)
        self.assertEqual(code, 0)
        self.assertEqual(bounded["lines"], 201)
        self.assertIn("path", bounded)
        code, full, _ = self.run_cli("--stdout", "text", long_pdf)
        self.assertEqual(code, 0)
        self.assertIn("data", full)
        self.assertIn("Page 101", full["data"])


if __name__ == "__main__":
    unittest.main()
