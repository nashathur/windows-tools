import contextlib
import hashlib
import io
import json
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
import sys
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))

import pikepdf
import pymupdf
from PIL import Image

from pdftools.cli import main
from pdftools.commands import p3c_helpers
from tests.fixtures.p3c import build_fixtures


class ReadSpy:
    def __init__(self, stream, reads):
        self.stream = stream
        self.reads = reads

    def __enter__(self):
        self.stream.__enter__()
        return self

    def __exit__(self, *args):
        return self.stream.__exit__(*args)

    def read(self, size=-1):
        self.reads.append(size)
        return self.stream.read(size)

    def __getattr__(self, name):
        return getattr(self.stream, name)


class Phase3cTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.fixtures = build_fixtures(self.root)
        self.pdf = self.fixtures["pdf"]

    def tearDown(self):
        self.temp.cleanup()

    def run_cli(self, argv):
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            code = main(argv)
        return code, stdout.getvalue(), stderr.getvalue()

    def run_json(self, argv):
        code, stdout, stderr = self.run_cli(["--json", *argv])
        records = [json.loads(line) for line in stdout.splitlines()]
        return code, records, stderr

    def test_toimg_size_options_and_exclusivity(self):
        outputs = [("--dpi", "72", "dpi.png", (144, 72)),
                   ("--width", "300", "width.png", (300, 150)),
                   ("--height", "100", "height.png", (200, 100)),
                   ("--scale", "2", "scale.png", (288, 144))]
        for option, value, name, size in outputs:
            with self.subTest(option=option):
                target = self.root / name
                self.assertEqual(self.run_cli(["toimg", str(self.pdf), option, value,
                                               "--pages", "1", "-o", str(target)])[0], 0)
                with Image.open(target) as image:
                    self.assertEqual(image.size, size)
        sizes = (("--dpi", "72"), ("--width", "100"),
                 ("--height", "100"), ("--scale", "2"))
        for left in range(len(sizes)):
            for right in range(left + 1, len(sizes)):
                conflicting = (*sizes[left], *sizes[right])
                with self.subTest(conflicting=conflicting):
                    code, records, _ = self.run_json(["toimg", str(self.pdf), *conflicting])
                    self.assertEqual(code, 2)
                    self.assertEqual(records[0]["code"], "BAD_ARG")

    def test_toimg_page_selection_and_multifile_output(self):
        output = self.root / "selected"
        output.mkdir()
        self.assertEqual(self.run_cli(["toimg", str(self.pdf), "--pages", "2", "-o", str(output)])[0], 0)
        self.assertFalse((output / "phase3c_p001.png").exists())
        self.assertTrue((output / "phase3c_p002.png").exists())

        exact = self.root / "extensionless"
        self.assertEqual(self.run_cli(["toimg", str(self.pdf), "--pages", "1", "-o", str(exact)])[0], 0)
        self.assertTrue(exact.is_file())

    def test_toimg_multifile_render_streams_each_page_before_next(self):
        events = []
        original_render = p3c_helpers._render
        original_write = p3c_helpers.WRITERS["png"]["write"]

        def render(*args):
            events.append("render")
            return original_render(*args)

        def write(image, path, args):
            events.append("write")
            return original_write(image, path, args)

        with mock.patch.object(p3c_helpers, "_render", render), mock.patch.dict(
                p3c_helpers.WRITERS, {"png": {**p3c_helpers.WRITERS["png"], "write": write}}):
            self.assertEqual(self.run_cli(["toimg", str(self.pdf), "--pages", "1-2"])[0], 0)
        self.assertEqual(events, ["render", "write", "render", "write"])

    def test_toimg_clip_is_displayed_mm_on_rotated_page(self):
        target = self.root / "clip.png"
        full = self.root / "rotated-full.png"
        self.assertEqual(self.run_cli(["toimg", str(self.fixtures["rotated"]), "--dpi", "72",
                                       "--pages", "1", "-o", str(full)])[0], 0)
        self.assertEqual(self.run_cli(["toimg", str(self.fixtures["rotated"]), "--dpi", "72",
                                       "--clip", "0,0,25.4,12.7", "--pages", "1",
                                       "-o", str(target)])[0], 0)
        with Image.open(target) as image, Image.open(full) as full_image:
            self.assertEqual(image.size, (72, 36))
            self.assertEqual(list(image.getdata()), list(full_image.crop((0, 0, 72, 36)).getdata()))

    def test_toimg_gray_and_bw_threshold(self):
        gray, bw, high_threshold = (self.root / "gray.png", self.root / "bw.png",
                                    self.root / "bw-high-threshold.png")
        self.assertEqual(self.run_cli(["toimg", str(self.pdf), "--gray", "--pages", "1", "-o", str(gray)])[0], 0)
        threshold_pdf = self.fixtures["threshold"]
        self.assertEqual(self.run_cli(["toimg", str(threshold_pdf), "--bw", "--threshold", "100",
                                       "--pages", "1", "-o", str(bw)])[0], 0)
        self.assertEqual(self.run_cli(["toimg", str(threshold_pdf), "--bw", "--threshold", "220",
                                       "--pages", "1", "-o", str(high_threshold)])[0], 0)
        with Image.open(gray) as image:
            self.assertEqual(image.mode, "L")
        with Image.open(bw) as image:
            self.assertEqual(image.mode, "1")
        with Image.open(high_threshold) as image:
            self.assertEqual(image.mode, "1")
        with Image.open(bw) as low, Image.open(high_threshold) as high:
            self.assertEqual(low.getpixel((10, 10)), 0)
            self.assertEqual(low.getpixel((200, 10)), 255)
            self.assertEqual(high.getpixel((10, 10)), 0)
            self.assertEqual(high.getpixel((200, 10)), 0)
            self.assertNotEqual(low.getpixel((200, 10)), high.getpixel((200, 10)))

    def test_toimg_alpha_png(self):
        target = self.root / "alpha.png"
        self.assertEqual(self.run_cli(["toimg", str(self.pdf), "--alpha", "--pages", "1", "-o", str(target)])[0], 0)
        with Image.open(target) as image:
            self.assertEqual(image.mode, "RGBA")

    def test_toimg_gray_keeps_alpha_and_unsupported_alpha_combinations_are_bad_args(self):
        target = self.root / "alpha-gray.png"
        self.assertEqual(self.run_cli(["toimg", str(self.pdf), "--alpha", "--gray", "--pages", "1", "-o", str(target)])[0], 0)
        with Image.open(target) as image:
            self.assertEqual(image.mode, "LA")
            self.assertEqual(image.getpixel((0, 0))[1], 0)
        for extra in (("--bw",), ("--grid", "1x1"), ("--stitch", "v")):
            code, records, _ = self.run_json(["toimg", str(self.pdf), "--alpha", *extra])
            self.assertEqual(code, 2)
            self.assertEqual(records[0]["code"], "BAD_ARG")

    def test_toimg_rejects_non_finite_dpi_and_scale(self):
        for option, value in (("--dpi", "nan"), ("--scale", "inf")):
            code, records, _ = self.run_json(["toimg", str(self.pdf), option, value])
            self.assertEqual(code, 2)
            self.assertEqual(records[0]["code"], "BAD_ARG")
        code, records, _ = self.run_json(["toimg", str(self.pdf), "--dpi", "1e308"])
        self.assertEqual(code, 1)
        self.assertEqual(records[0]["code"], "TOO_BIG")

    def test_toimg_pixel_cap_includes_rounded_dimensions(self):
        rect = pymupdf.Rect(0, 0, 99999.1, 1000.0001)
        with self.assertRaises(p3c_helpers.PdfError) as caught:
            p3c_helpers._pixel_dimensions(rect, 1.0, 100_000_000)
        self.assertEqual(caught.exception.code, "TOO_BIG")

    def test_toimg_trailing_separator_selects_new_output_folder(self):
        output = self.root / "new-images"
        code, records, _ = self.run_json(["toimg", str(self.pdf), "--pages", "1", "-o", str(output) + "\\"])
        self.assertEqual(code, 0)
        self.assertEqual(records[0]["path"], str(output / "phase3c_p001.png"))
        self.assertTrue(Path(records[0]["path"]).is_file())

    def test_toimg_jpeg_quality(self):
        sizes = []
        for quality in (20, 95):
            target = self.root / f"quality-{quality}.jpg"
            self.assertEqual(self.run_cli(["toimg", str(self.pdf), "--format", "jpeg", "--quality", str(quality),
                                           "--pages", "1", "-o", str(target)])[0], 0)
            with Image.open(target) as image:
                self.assertEqual(image.format, "JPEG")
            sizes.append(target.stat().st_size)
        self.assertGreater(sizes[1], sizes[0])

    def test_toimg_stitch_vertical_and_horizontal(self):
        for direction, expected in (("v", (144, 144)), ("h", (288, 72))):
            target = self.root / f"stitch-{direction}.png"
            self.assertEqual(self.run_cli(["toimg", str(self.pdf), "--dpi", "72", "--stitch", direction,
                                           "-o", str(target)])[0], 0)
            with Image.open(target) as image:
                self.assertEqual(image.size, expected)

    def test_toimg_stitch_default_name_and_combined_size_preflight(self):
        for direction in ("v", "h"):
            self.assertEqual(self.run_cli(["toimg", str(self.pdf), "--dpi", "72", "--stitch", direction])[0], 0)
        self.assertTrue((self.root / "phase3c_stitch.png").exists())
        self.assertFalse((self.root / "phase3c_v.png").exists())
        with mock.patch.object(Image, "new", side_effect=AssertionError("allocated oversized canvas")):
            code, records, _ = self.run_json(["toimg", str(self.pdf), "--dpi", "72", "--grid", "1000x1000"])
        self.assertEqual(code, 1)
        self.assertEqual(records[0]["code"], "TOO_BIG")

    def test_toimg_disk_preflight_and_collision_warning(self):
        target = self.root / "existing.png"
        target.write_bytes(b"leave me")
        with mock.patch.object(p3c_helpers.io.shutil, "disk_usage", return_value=type("Usage", (), {"free": 0})()), \
             mock.patch.object(p3c_helpers, "_render", side_effect=AssertionError("rendered before disk preflight")):
            code, records, _ = self.run_json(["toimg", str(self.pdf), "--pages", "1", "-o", str(self.root / "not-there.png")])
        self.assertEqual(code, 1)
        self.assertEqual(records[0]["code"], "TOO_BIG")
        code, records, _ = self.run_json(["toimg", str(self.pdf), "--pages", "1", "-o", str(target)])
        self.assertEqual(code, 0)
        self.assertEqual(records[0]["verdict"], "warn")
        self.assertIn("NAME_TAKEN", records[0]["warnings"])

    def test_toimg_grid_has_page_labels(self):
        target = self.root / "grid.png"
        self.assertEqual(self.run_cli(["toimg", str(self.pdf), "--dpi", "72", "--grid", "2x1",
                                       "-o", str(target)])[0], 0)
        with Image.open(target) as image:
            self.assertEqual(image.size, (288, 96))
            self.assertTrue(any(image.getpixel((x, y)) != (255, 255, 255)
                                for x in range(0, 35) for y in range(0, 24)))

    def test_toimg_unknown_format_lists_supported_formats(self):
        code, records, stderr = self.run_json(["toimg", str(self.pdf), "--format", "bmp"])
        self.assertEqual(code, 2, (records, stderr))
        self.assertEqual(records[0]["code"], "BAD_ARG")
        self.assertIn("png, jpeg", records[0]["message"])

    def test_toimg_zip_multifile_and_zip_folder(self):
        self.assertEqual(self.run_cli(["toimg", str(self.pdf), "--pages", "1-2", "--zip"])[0], 0)
        archive_path = self.root / "phase3c_png.zip"
        with zipfile.ZipFile(archive_path) as archive:
            self.assertEqual(len([name for name in archive.namelist() if name.endswith(".png")]), 2)
        self.assertTrue((self.root / "phase3c_png").is_dir())
        zipped = self.root / "folder.zip"
        self.assertEqual(self.run_cli(["zip", str(self.root / "phase3c_png"), "-o", str(zipped)])[0], 0)
        with zipfile.ZipFile(zipped) as archive:
            self.assertEqual(len([name for name in archive.namelist() if name.endswith(".png")]), 2)
        self.assertFalse(any(".part" in name for name in archive.namelist()))

    def test_zip_dotted_folder_keeps_full_name(self):
        folder = self.root / "climate.2026"
        folder.mkdir()
        (folder / "report.txt").write_text("data")
        code, records, _ = self.run_json(["zip", str(folder)])
        self.assertEqual(code, 0)
        self.assertEqual(records[0]["path"], str(self.root / "climate.2026.zip"))
        self.assertTrue(Path(records[0]["path"]).is_file())

    def test_zip_duplicate_basenames_and_archive_staging_exclusion(self):
        left, right = self.root / "a", self.root / "b"
        left.mkdir(); right.mkdir()
        (left / "report.pdf").write_text("left")
        (right / "report.pdf").write_text("right")
        archive = self.root / "a" / "bundle.zip"
        self.assertEqual(self.run_cli(["zip", str(left / "report.pdf"), str(right / "report.pdf"), "-o", str(archive)])[0], 0)
        with zipfile.ZipFile(archive) as zipped:
            self.assertEqual(len(zipped.namelist()), 2)
            self.assertEqual({zipped.read(name) for name in zipped.namelist()}, {b"left", b"right"})
            self.assertFalse(any(".part" in name for name in zipped.namelist()))
        inside = self.root / "inside"
        inside.mkdir()
        (inside / "input.pdf").write_bytes(b"pdf")
        inside_archive = inside / "backup.zip"
        self.assertEqual(self.run_cli(["zip", str(inside), "-o", str(inside_archive)])[0], 0)
        with zipfile.ZipFile(inside_archive) as zipped:
            self.assertEqual(zipped.namelist(), ["inside/input.pdf"])
        out_dir = self.root / "new-output-dir"
        self.assertEqual(self.run_cli(["zip", str(left / "report.pdf"), "-o", str(out_dir) + "\\"])[0], 0)
        self.assertTrue((out_dir / "report.pdf.zip").is_file())
        folders = [self.root / "x", self.root / "x_2"]
        for folder, content in zip(folders, (b"first", b"third")):
            folder.mkdir()
            (folder / "same.pdf").write_bytes(content)
        repeated = self.root / "folders.zip"
        self.assertEqual(self.run_cli(["zip", str(folders[0]), str(folders[0]), str(folders[1]), "-o", str(repeated)])[0], 0)
        with zipfile.ZipFile(repeated) as zipped:
            names = zipped.namelist()
            self.assertEqual(len(names), len(set(names)))
            self.assertEqual({zipped.read(name) for name in names}, {b"first", b"third"})

    def test_toimg_zip_failure_reports_published_folder_and_dotted_names(self):
        dotted = self.root / "climate.2026.pdf"
        dotted.write_bytes(self.pdf.read_bytes())
        with mock.patch.object(p3c_helpers, "_zip_paths", side_effect=OSError("locked")):
            code, records, _ = self.run_json(["toimg", str(dotted), "--pages", "1-2", "--zip"])
        self.assertEqual(code, 0)
        self.assertEqual(records[0]["type"], "result")
        self.assertEqual(records[0]["verdict"], "warn")
        self.assertIn("ZIP_FAILED", records[0]["warnings"])
        self.assertTrue(Path(records[0]["path"]).is_dir())
        self.assertTrue((self.root / "climate.2026_png.zip").exists() is False)
        self.assertTrue((self.root / "climate.2026_png").exists())

    def test_toimg_zip_appends_suffix_to_complete_collision_resolved_folder(self):
        dotted = self.root / "climate.2026.pdf"
        dotted.write_bytes(self.pdf.read_bytes())
        (self.root / "climate.2026_png").mkdir()
        (self.root / "climate.2026_png.zip").write_bytes(b"existing")
        code, records, _ = self.run_json(["toimg", str(dotted), "--pages", "1-2", "--zip"])
        self.assertEqual(code, 0)
        self.assertEqual(records[0]["verdict"], "warn")
        self.assertEqual(Path(records[0]["path"]).name, "climate_2.2026_png")
        self.assertTrue(Path(records[0]["path"] + ".zip").exists())
        self.assertEqual((self.root / "climate.2026_png.zip").read_bytes(), b"existing")

    def test_toimg_zip_result_includes_collision_resolved_archive_path(self):
        requested = self.root / "phase3c_png.zip"
        requested.write_bytes(b"keep")
        code, records, _ = self.run_json(["toimg", str(self.pdf), "--pages", "1-2", "--zip"])
        self.assertEqual(code, 0)
        self.assertIn("NAME_TAKEN", records[0]["warnings"])
        self.assertEqual(records[0]["zip_path"], str(self.root / "phase3c_png_2.zip"))
        self.assertTrue(Path(records[0]["zip_path"]).is_file())
        self.assertEqual(requested.read_bytes(), b"keep")

    def test_toimg_dry_run_includes_page_map_and_zip_plan(self):
        code, records, _ = self.run_json(["--dry-run", "toimg", str(self.pdf), "--pages", "2,1", "--zip"])
        self.assertEqual(code, 0)
        result = records[0]
        self.assertTrue(result["provisional"])
        self.assertEqual([item["page"] for item in result["page_map"]], [2, 1])
        self.assertEqual(result["zip_path"], str(self.root / "phase3c_png.zip"))

    def test_dupes_sha256_and_streamed_large_file(self):
        first, second = self.root / "one.bin", self.root / "two.bin"
        payload = b"p3c-test-data" * (300_000)
        first.write_bytes(payload)
        second.write_bytes(payload)
        expected_hash = hashlib.sha256(payload).hexdigest()
        self.assertEqual(hashlib.sha256(first.read_bytes()).hexdigest(), expected_hash)
        reads = []
        original_open = Path.open

        def spy_open(path, *args, **kwargs):
            stream = original_open(path, *args, **kwargs)
            if path == first and args and args[0] == "rb":
                return ReadSpy(stream, reads)
            return stream

        with mock.patch.object(Path, "open", spy_open):
            code, records, _ = self.run_json(["dupes", str(self.root)])
        self.assertEqual(code, 0)
        self.assertTrue(any(str(first) in group and str(second) in group
                            for group in records[0]["data"]["identical"]))
        self.assertTrue(reads and all(0 < size <= 1024 * 1024 for size in reads))

    def test_dupes_similar_candidates(self):
        similar = self.root / "similar.pdf"
        with pymupdf.open(self.pdf) as doc:
            doc.set_metadata({"title": "same pages, different metadata"})
            doc.save(similar)
        code, records, _ = self.run_json(["dupes", str(self.root), "--similar"])
        self.assertEqual(code, 0)
        self.assertTrue(any(str(self.pdf) in group and str(similar) in group
                            for group in records[0]["data"]["similar_candidates"]))

    def test_dupes_similar_uses_password_and_reports_unreadable_pdfs(self):
        encrypted = self.root / "encrypted-similar.pdf"
        password_file = self.root / "password.txt"
        password_file.write_text("secret")
        with pikepdf.open(self.pdf) as doc:
            doc.docinfo["/Title"] = "different metadata"
            doc.save(encrypted, encryption=pikepdf.Encryption(owner="owner", user="secret"))
        code, records, _ = self.run_json(["dupes", str(self.root), "--similar", "--password-file", str(password_file)])
        self.assertEqual(code, 0)
        self.assertTrue(any(str(self.pdf) in group and str(encrypted) in group
                            for group in records[0]["data"]["similar_candidates"]))
        code, records, _ = self.run_json(["dupes", str(self.root), "--similar"])
        self.assertEqual(code, 0)
        self.assertEqual(records[0]["type"], "report")
        self.assertTrue(any(item["path"] == str(encrypted) for item in records[0]["data"]["uninspected"]))

    def test_compare_default_output_is_beside_b_and_explicit_output_is_honored(self):
        originals, revisions = self.root / "originals", self.root / "revisions"
        originals.mkdir(); revisions.mkdir()
        a, b = originals / "a.pdf", revisions / "b.pdf"
        a.write_bytes(self.fixtures["compare_a"].read_bytes())
        b.write_bytes(self.fixtures["compare_changed"].read_bytes())
        code, records, _ = self.run_json(["compare", str(a), str(b)])
        self.assertEqual(code, 0)
        artifact = next(record for record in records if record["type"] == "result")
        self.assertEqual(artifact["path"], str(revisions / "a_vs_b.pdf"))
        explicit = self.root / "custom.pdf"
        code, records, _ = self.run_json(["compare", str(a), str(b), "-o", str(explicit)])
        self.assertEqual(code, 0)
        artifact = next(record for record in records if record["type"] == "result")
        self.assertEqual(artifact["path"], str(explicit))
        output_dir = self.root / "compare-output"
        code, records, _ = self.run_json(["compare", str(a), str(b), "-o", str(output_dir) + "\\"])
        self.assertEqual(code, 0)
        artifact = next(record for record in records if record["type"] == "result")
        self.assertEqual(artifact["path"], str(output_dir / "a_vs_b.pdf"))

    def test_compare_changed_tiles_use_b_coordinates_when_rotations_differ(self):
        original, changed = self.fixtures["rotation_mismatch_a"], self.fixtures["rotation_mismatch_b"]
        code, records, _ = self.run_json(["compare", str(original), str(changed)])
        self.assertEqual(code, 0)
        artifact = next(record for record in records if record["type"] == "result")
        with pikepdf.open(artifact["path"]) as pdf:
            annots = pdf.pages[0].obj.get("/Annots")
            self.assertTrue(annots)
            rects = [tuple(float(v) for v in annot["/Rect"]) for annot in annots]
        with pymupdf.open(changed) as doc:
            page = doc[0]
            bbox = page.search_for("128")[0]
            expected = (bbox.x0, page.mediabox.height - bbox.y1,
                        bbox.x1, page.mediabox.height - bbox.y0)
        self.assertTrue(any(r[0] <= expected[2] and r[2] >= expected[0] and
                            r[1] <= expected[3] and r[3] >= expected[1] for r in rects), (rects, expected))

    def test_compare_signature_and_restriction_override_flags(self):
        signed, restricted = self.root / "signed.pdf", self.root / "restricted.pdf"
        with pikepdf.open(self.fixtures["compare_changed"]) as pdf:
            signature = pdf.make_indirect(pikepdf.Dictionary({"/FT": pikepdf.Name("/Sig"), "/V": pikepdf.Dictionary({"/Type": pikepdf.Name("/Sig"), "/Contents": pikepdf.String("x")})}))
            pdf.Root.AcroForm = pikepdf.Dictionary({"/Fields": pikepdf.Array([signature])})
            pdf.save(signed)
        code, records, _ = self.run_json(["compare", str(self.pdf), str(signed), "--break-signature"])
        self.assertEqual(code, 0)
        artifact = next(record for record in records if record["type"] == "result")
        self.assertIn("SIGNATURE_INVALIDATED", artifact["warnings"])
        with pymupdf.open(self.fixtures["compare_changed"]) as doc:
            doc.save(restricted, encryption=pymupdf.PDF_ENCRYPT_AES_256, owner_pw="owner", user_pw="",
                     permissions=int(pymupdf.PDF_PERM_ACCESSIBILITY))
        code, records, _ = self.run_json(["compare", str(self.pdf), str(restricted), "--ignore-restrictions"])
        self.assertEqual(code, 0)
        artifact = next(record for record in records if record["type"] == "result")
        self.assertIn("RESTRICTIONS_IGNORED", artifact["warnings"])

    def test_toimg_signed_export_warns_in_normal_and_dry_run_results(self):
        signed = self.root / "signed-export.pdf"
        with pikepdf.open(self.pdf) as pdf:
            signature = pdf.make_indirect(pikepdf.Dictionary({"/FT": pikepdf.Name("/Sig"), "/V": pikepdf.Dictionary({"/Type": pikepdf.Name("/Sig"), "/Contents": pikepdf.String("x")})}))
            pdf.Root.AcroForm = pikepdf.Dictionary({"/Fields": pikepdf.Array([signature])})
            pdf.save(signed)
        for prefix in ([], ["--dry-run"]):
            code, records, _ = self.run_json([*prefix, "toimg", str(signed), "--pages", "1"])
            self.assertEqual(code, 0)
            self.assertIn("SIGNATURE_NOT_RETAINED", records[0]["warnings"])

    def test_compare_unchanged_returns_report_and_unannotated_copy(self):
        same = self.root / "same.pdf"
        with pymupdf.open(self.fixtures["compare_a"]) as doc:
            doc.save(same)
        code, records, _ = self.run_json(["compare", str(self.fixtures["compare_a"]), str(same)])
        self.assertEqual(code, 0)
        report = next(record for record in records if record["type"] == "report")
        artifact = next(record for record in records if record["type"] == "result")
        self.assertEqual(report["verdict"], "report")
        self.assertEqual(artifact["path"], str(self.root / "compare_a_vs_same.pdf"))
        with pikepdf.open(artifact["path"]) as pdf:
            self.assertFalse(any(page.obj.get("/Annots") for page in pdf.pages))

    def test_compare_changed_digit_warn_and_annotation_position(self):
        original, changed = self.fixtures["compare_a"], self.fixtures["compare_changed"]
        input_hashes = {path: hashlib.sha256(path.read_bytes()).digest() for path in (original, changed)}
        code, records, _ = self.run_json(["compare", str(original), str(changed)])
        self.assertEqual(code, 0)
        report = next(record for record in records if record["type"] == "report")
        artifact = next(record for record in records if record["type"] == "result")
        self.assertEqual(report["verdict"], "warn")
        with pikepdf.open(artifact["path"]) as pdf:
            self.assertEqual(len(pdf.pages), 2)
            self.assertIsNone(pdf.pages[0].obj.get("/Annots"))
            annots = pdf.pages[1].obj.get("/Annots")
            self.assertTrue(annots)
            rects = [tuple(float(value) for value in annot["/Rect"]) for annot in annots]
        with pymupdf.open(changed) as doc:
            bbox = doc[1].search_for("128")[0]
            expected = (bbox.x0, doc[1].rect.height - bbox.y1,
                        bbox.x1, doc[1].rect.height - bbox.y0)
        self.assertTrue(any(rect[0] <= expected[2] and rect[2] >= expected[0] and
                            rect[1] <= expected[3] and rect[3] >= expected[1] for rect in rects),
                        (rects, expected))
        self.assertEqual({path: hashlib.sha256(path.read_bytes()).digest() for path in input_hashes}, input_hashes)

    def test_compare_rotated_page_annotation_maps_to_changed_page_space(self):
        original, changed = self.fixtures["rotated_compare_a"], self.fixtures["rotated_compare_changed"]
        code, records, _ = self.run_json(["compare", str(original), str(changed)])
        self.assertEqual(code, 0)
        artifact = next(record for record in records if record["type"] == "result")
        with pikepdf.open(artifact["path"]) as pdf:
            annotations = pdf.pages[0].obj.get("/Annots")
            self.assertTrue(annotations)
            rects = [tuple(float(value) for value in annot["/Rect"]) for annot in annotations]
        with pymupdf.open(changed) as doc:
            page = doc[0]
            bbox = page.search_for("128")[0]
            expected = (bbox.x0, page.mediabox.height - bbox.y1,
                        bbox.x1, page.mediabox.height - bbox.y0)
        self.assertTrue(any(rect[0] <= expected[2] and rect[2] >= expected[0] and
                            rect[1] <= expected[3] and rect[3] >= expected[1] for rect in rects),
                        (rects, expected))

    def test_compare_added_page_is_reported_unmatched(self):
        original, changed = self.fixtures["compare_a"], self.root / "added.pdf"
        with pymupdf.open(original) as doc:
            doc.new_page(width=400, height=300).insert_text((50, 60), "new page")
            doc.save(changed)
        code, records, _ = self.run_json(["compare", str(original), str(changed)])
        self.assertEqual(code, 0)
        report = next(record for record in records if record["type"] == "report")
        self.assertEqual(report["verdict"], "warn")
        self.assertTrue(any(finding["code"] == "UNMATCHED_OUTPUT" for finding in report["data"]["findings"]))

    def test_output_collision_input_hash_paths_and_dry_run(self):
        source = self.root / "climàte files" / "source.pdf"
        source.parent.mkdir()
        source.write_bytes(self.pdf.read_bytes())
        before = hashlib.sha256(source.read_bytes()).digest()
        target = self.root / "résultat image.png"
        target.write_bytes(b"keep")
        code, records, _ = self.run_json(["toimg", str(source), "--pages", "1", "-o", str(target)])
        self.assertEqual(code, 0)
        result = records[0]
        self.assertEqual(Path(result["path"]).stem, "résultat image_2")
        self.assertEqual(target.read_bytes(), b"keep")
        self.assertEqual(hashlib.sha256(source.read_bytes()).digest(), before)
        planned = self.root / "will-not-exist.pdf"
        code, records, _ = self.run_cli(["--dry-run", "toimg", str(source), "--pages", "1", "-o", str(planned)])
        self.assertEqual(code, 0)
        self.assertFalse(planned.exists())


if __name__ == "__main__":
    unittest.main()
