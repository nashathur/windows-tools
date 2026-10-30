"""Phase 6 image commands: img2pdf route table, images, imgtext/imginfo/imgdiff, toimg formats.

Every assertion uses an engine other than the code under test: pikepdf/pypdf for PDF structure,
Pillow for images, pymupdf rendering for what a viewer shows.
"""

import contextlib
import io
import json
import math
import os
import shutil
import struct
import sys
import tempfile
import time
import unittest
import warnings
import zlib
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))

import numpy as np
import pikepdf
import pymupdf
from PIL import Image, ImageChops, ImageDraw, ImageFont, ImageOps

import clierr
from pdftools import cli, tools
from pdftools.commands import p3c_helpers, p6_images
from pdftools.errors import PdfError
from tests.tooling import require_tool as shared_require_tool

QUADRANTS = ((255, 0, 0), (0, 200, 0), (0, 0, 255), (255, 255, 0))  # TL, TR, BL, BR


def tool_path(name):
    """PATH first, then the dev env's own tool folders (PATH lacks them outside `conda run`)."""
    folders = ((Path(sys.prefix) / "Library" / "bin", Path(sys.prefix) / "Scripts") if os.name == "nt"
               else (Path(sys.prefix) / "bin",))
    with mock.patch.dict(os.environ, {"PATH": os.pathsep.join([os.environ.get("PATH", ""), *map(str, folders)])}):
        return tools.find_tool(name) if name in tools.TOOL_CANDIDATES else shutil.which(name)


def quadrant_image(size=(120, 80)):
    image = Image.new("RGB", size)
    w, h = size
    draw = ImageDraw.Draw(image)
    for index, box in enumerate(((0, 0, w // 2, h // 2), (w // 2, 0, w, h // 2), (0, h // 2, w // 2, h), (w // 2, h // 2, w, h))):
        draw.rectangle(box, fill=QUADRANTS[index])
    return image


def exif_with(orientation=None, taken=None):
    exif = Image.Exif()
    if orientation:
        exif[274] = orientation
    if taken:
        exif[0x8769] = {36867: taken}
    return exif


def png_bytes(width, height, colour_type, depth, raw_pixels):
    """Minimal non-interlaced PNG writer (Pillow cannot write 16-bit RGBA)."""
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))
    row = len(raw_pixels) // height
    data = b"".join(b"\x00" + raw_pixels[y * row:(y + 1) * row] for y in range(height))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, depth, colour_type, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(data)) + chunk(b"IEND", b""))


class Base(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.addCleanup(self.temp.cleanup)

    def run_cli(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch("clierr.LOG_ROOT", self.root / "logs"), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(["--json", "--stdout", *map(str, argv)])
        rows = [json.loads(line) for line in out.getvalue().splitlines() if line.strip()]
        return code, rows, err.getvalue()

    def ok(self, *argv):
        code, rows, err = self.run_cli(*argv)
        self.assertEqual(code, 0, (rows, err))
        return rows[0] if len(rows) == 1 else rows, err

    def fails(self, argv, expected_code):
        code, rows, err = self.run_cli(*argv)
        self.assertNotEqual(code, 0, rows)
        self.assertEqual(rows[0]["type"], "error", rows)
        self.assertEqual(rows[0]["code"], expected_code, rows)
        self.assertEqual(len([line for line in err.splitlines() if line.startswith("ERROR")]), 1, err)
        return rows[0]

    def require_tool(self, name):
        return shared_require_tool(self, name, f"{name} is required for this integration test")[0]

    def save(self, image, name, fmt=None, **kwargs):
        path = self.root / name
        image.save(path, fmt, **kwargs)
        return path

    # ----- PDF inspection with independent engines -----
    def render(self, pdf_path, index=0):
        with pymupdf.open(pdf_path) as doc:
            pix = doc[index].get_pixmap(dpi=72, alpha=False)
            return Image.frombytes("RGB", (pix.width, pix.height), pix.samples)

    def shown_corners(self, pdf_path):
        """Colours just inside the four corners of the drawn picture, as a viewer renders it."""
        rendered = self.render(pdf_path)
        box = ImageChops.difference(rendered, Image.new("RGB", rendered.size, "white")).getbbox()
        self.assertIsNotNone(box, "page is blank")
        x0, y0, x1, y1 = box
        return [rendered.getpixel(p) for p in ((x0 + 4, y0 + 4), (x1 - 5, y0 + 4), (x0 + 4, y1 - 5), (x1 - 5, y1 - 5))]

    def assertColoursClose(self, got, expected):
        for a, b in zip(got, expected):
            self.assertLessEqual(max(abs(x - y) for x, y in zip(a, b)), 40, (got, expected))


class Img2PdfRouteTable(Base):
    def test_jpeg_bytes_embedded_unchanged_one_page_verified(self):
        path = self.save(quadrant_image(), "photo.jpg", "JPEG", quality=85)
        record, _ = self.ok("img2pdf", path, "-o", self.root / "out.pdf")
        self.assertEqual(record["verdict"], "ok")
        with pikepdf.open(self.root / "out.pdf") as pdf:
            self.assertEqual(len(pdf.pages), 1)
            image = next(iter(pdf.pages[0].Resources.XObject.values()))
            self.assertEqual(str(image.Filter), "/DCTDecode")
            self.assertEqual(image.read_raw_bytes(), path.read_bytes())

    def test_cmyk_jpeg_stays_cmyk_and_bytes_unchanged(self):
        path = self.save(Image.new("CMYK", (60, 40), (0, 255, 255, 0)), "cmyk.jpg", "JPEG")
        record, _ = self.ok("img2pdf", path, "-o", self.root / "out.pdf")
        with pikepdf.open(self.root / "out.pdf") as pdf:
            image = next(iter(pdf.pages[0].Resources.XObject.values()))
            self.assertEqual(str(image.ColorSpace), "/DeviceCMYK")
            self.assertEqual(image.read_raw_bytes(), path.read_bytes())
        self.assertEqual(record["warnings"], [])

    def test_exif_orientation_6_uses_page_rotation_and_keeps_jpeg_bytes(self):
        path = self.save(quadrant_image(), "rot6.jpg", "JPEG", quality=90, exif=exif_with(6))
        self.ok("img2pdf", path, "-o", self.root / "out.pdf")
        with pikepdf.open(self.root / "out.pdf") as pdf:
            self.assertEqual(int(pdf.pages[0].get("/Rotate", 0)), 90)
            image = next(iter(pdf.pages[0].Resources.XObject.values()))
            self.assertEqual(image.read_raw_bytes(), path.read_bytes())
        with pymupdf.open(self.root / "out.pdf") as doc:
            self.assertAlmostEqual(doc[0].rect.width, 595.276, delta=1)  # displayed A4 portrait
        expected = ImageOps.exif_transpose(Image.open(path))
        w, h = expected.size
        want = [expected.getpixel(p) for p in ((2, 2), (w - 3, 2), (2, h - 3), (w - 3, h - 3))]
        self.assertColoursClose(self.shown_corners(self.root / "out.pdf"), want)

    def test_exif_orientation_3_and_8_display_upright(self):
        for orientation in (3, 8):
            with self.subTest(orientation=orientation):
                path = self.save(quadrant_image(), f"rot{orientation}.jpg", "JPEG", quality=90, exif=exif_with(orientation))
                out = self.root / f"out{orientation}.pdf"
                self.ok("img2pdf", path, "-o", out)
                expected = ImageOps.exif_transpose(Image.open(path))
                w, h = expected.size
                want = [expected.getpixel(p) for p in ((2, 2), (w - 3, 2), (2, h - 3), (w - 3, h - 3))]
                self.assertColoursClose(self.shown_corners(out), want)

    def test_mirrored_exif_orientation_goes_through_pillow_and_displays_upright(self):
        for orientation in (2, 5):
            with self.subTest(orientation=orientation):
                path = self.save(quadrant_image(), f"mir{orientation}.jpg", "JPEG", quality=95, exif=exif_with(orientation))
                out = self.root / f"mir{orientation}.pdf"
                record, _ = self.ok("img2pdf", path, "-o", out)
                expected = ImageOps.exif_transpose(Image.open(path))
                w, h = expected.size
                want = [expected.getpixel(p) for p in ((2, 2), (w - 3, 2), (2, h - 3), (w - 3, h - 3))]
                self.assertColoursClose(self.shown_corners(out), want)
                with pikepdf.open(out) as pdf:
                    self.assertEqual(int(pdf.pages[0].get("/Rotate", 0)), 0)

    def test_png_pixels_unchanged(self):
        source = quadrant_image((50, 30))
        path = self.save(source, "plain.png")
        self.ok("img2pdf", path, "-o", self.root / "out.pdf")
        with pikepdf.open(self.root / "out.pdf") as pdf:
            image = next(iter(pdf.pages[0].Resources.XObject.values()))
            pixels = pikepdf.PdfImage(image).as_pil_image()
            self.assertEqual(ImageChops.difference(pixels.convert("RGB"), source).getbbox(), None)

    def test_16bit_gray_png_is_not_clipped_to_white(self):
        path = self.save(Image.new("I;16", (40, 30), 30000), "gray16.png")
        record, _ = self.ok("img2pdf", path, "-o", self.root / "out.pdf")
        centre = self.render(self.root / "out.pdf").getpixel((297, 421))
        self.assertTrue(100 <= centre[0] <= 135, centre)  # 30000 / 65535 * 255 = 117

    def test_to_8bit_keeps_the_top_byte_for_16bit_gray(self):
        for mode, value in (("I;16", 30000), ("I", 30000)):
            with self.subTest(mode=mode):
                out = p6_images._to_8bit(Image.new(mode, (3, 3), value))
                self.assertEqual(out.getpixel((1, 1)), 117)
        floats = p6_images._to_8bit(Image.new("F", (2, 2), 0.5))
        self.assertEqual(floats.getpixel((0, 0)), 128)

    def test_auto_native_page_checks_both_a3_dimensions(self):
        args = mock.Mock(page="auto", orient="auto", margin="10mm", fit="contain")
        width, height, _ = p6_images._place(args, 4000, 4000, (300, 300))
        self.assertEqual((width, height), p6_images._A4)

    def test_lossy_gray_pixels_never_keep_an_incompatible_icc_profile(self):
        from PIL import ImageCms
        args = mock.Mock(lossy=90, flatten_alpha=False)
        gray = Image.new("L", (10, 10), 128)
        gray.info["icc_profile"] = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()
        image, attached = p6_images._prepare(gray, args, [])
        self.assertEqual(image.mode, "RGB")
        self.assertIsNone(attached)
        image.close(); gray.close()

    def test_16bit_rgba_png_is_refused_by_img2pdf_and_falls_back_with_warning(self):
        pixel = struct.pack(">HHHH", 65535, 0, 0, 40000)
        path = self.root / "rgba16.png"
        path.write_bytes(png_bytes(8, 8, 6, 16, pixel * 8 * 8))
        record, _ = self.ok("img2pdf", path, "-o", self.root / "out.pdf")
        self.assertIn("BIT_DEPTH_REDUCED", record["warnings"])
        self.assertEqual(record["verdict"], "warn")

    def test_rgba_alpha_kept_as_soft_mask_and_flattened_on_request(self):
        rgba = Image.new("RGBA", (40, 30), (255, 0, 0, 100))
        path = self.save(rgba, "alpha.png")
        self.ok("img2pdf", path, "-o", self.root / "a.pdf")
        with pikepdf.open(self.root / "a.pdf") as pdf:
            image = next(iter(pdf.pages[0].Resources.XObject.values()))
            self.assertIn("/SMask", image)
            mask = pikepdf.PdfImage(image.SMask).as_pil_image()
            self.assertEqual(mask.getpixel((5, 5)), 100)
        self.ok("img2pdf", path, "--flatten-alpha", "-o", self.root / "f.pdf")
        with pikepdf.open(self.root / "f.pdf") as pdf:
            image = next(iter(pdf.pages[0].Resources.XObject.values()))
            self.assertNotIn("/SMask", image)

    def test_rgba_with_lossy_is_jpeg_plus_soft_mask(self):
        path = self.save(Image.new("RGBA", (40, 30), (0, 128, 255, 90)), "alpha.png")
        self.ok("img2pdf", path, "--lossy", "-o", self.root / "out.pdf")
        with pikepdf.open(self.root / "out.pdf") as pdf:
            image = next(iter(pdf.pages[0].Resources.XObject.values()))
            self.assertEqual(str(image.Filter), "/DCTDecode")
            self.assertIn("/SMask", image)
            self.assertEqual(pikepdf.PdfImage(image.SMask).as_pil_image().getpixel((3, 3)), 90)

    def test_lossy_quality_applies_to_non_jpeg_and_leaves_jpeg_alone(self):
        png = self.save(quadrant_image(), "q.png")
        jpg = self.save(quadrant_image(), "q.jpg", "JPEG", quality=70)
        self.ok("img2pdf", png, "--lossy", "50", "-o", self.root / "p.pdf")
        with pikepdf.open(self.root / "p.pdf") as pdf:
            self.assertEqual(str(next(iter(pdf.pages[0].Resources.XObject.values())).Filter), "/DCTDecode")
        self.ok("img2pdf", jpg, "--lossy", "50", "-o", self.root / "j.pdf")
        with pikepdf.open(self.root / "j.pdf") as pdf:
            self.assertEqual(next(iter(pdf.pages[0].Resources.XObject.values())).read_raw_bytes(), jpg.read_bytes())
        self.fails(["img2pdf", png, "--lossy", "0", "-o", self.root / "x.pdf"], "BAD_ARG")

    def test_max_dpi_keeps_page_size_and_reduces_pixels(self):
        path = self.save(Image.new("RGB", (600, 400), "white"), "scan.png", dpi=(300, 300))
        self.ok("img2pdf", path, "--max-dpi", "150", "-o", self.root / "out.pdf")
        with pymupdf.open(self.root / "out.pdf") as doc:
            self.assertAlmostEqual(doc[0].rect.width, 144, delta=0.5)  # 600 px at 300 dpi
            self.assertAlmostEqual(doc[0].rect.height, 96, delta=0.5)
            self.assertEqual(doc[0].get_images(full=True)[0][2:4], (300, 200))

    def test_max_dpi_uses_placed_size_when_there_is_no_dpi_metadata(self):
        path = self.save(Image.new("RGB", (3000, 2000), "white"), "big.png")
        self.ok("img2pdf", path, "--max-dpi", "150", "-o", self.root / "out.pdf")
        with pymupdf.open(self.root / "out.pdf") as doc:
            width_px = doc[0].get_images(full=True)[0][2]
            placed_in = doc[0].get_image_info()[0]["bbox"][2] - doc[0].get_image_info()[0]["bbox"][0]
            self.assertLessEqual(width_px, 150 * placed_in / 72 + 2)
            self.assertLess(width_px, 3000)
        self.fails(["img2pdf", path, "--max-dpi", "0", "-o", self.root / "x.pdf"], "BAD_ARG")

    def test_sort_exif_reads_the_exif_ifd_and_warns_on_fallback(self):
        folder = self.root / "shots"
        folder.mkdir()
        Image.new("RGB", (20, 10)).save(folder / "a.jpg", "JPEG", exif=exif_with(taken="2024:05:01 10:00:00"))
        Image.new("RGB", (30, 10)).save(folder / "b.jpg", "JPEG", exif=exif_with(taken="2019:01:01 10:00:00"))
        record, _ = self.ok("img2pdf", folder, "--sort", "exif", "-o", self.root / "out.pdf")
        self.assertNotIn("NO_EXIF_DATE", record["warnings"])
        with pymupdf.open(self.root / "out.pdf") as doc:
            self.assertEqual([doc[i].get_images(full=True)[0][2] for i in range(2)], [30, 20])
        nodate = folder / "c.jpg"
        Image.new("RGB", (40, 10)).save(nodate, "JPEG")
        os.utime(nodate, (1_000_000_000, 1_000_000_000))  # 2001: before both dated files
        record, err = self.ok("img2pdf", folder, "--sort", "exif", "-o", self.root / "out3.pdf")
        self.assertIn("NO_EXIF_DATE", record["warnings"])
        self.assertIn("WARN pdf img2pdf NO_EXIF_DATE", err)
        with pymupdf.open(self.root / "out3.pdf") as doc:
            self.assertEqual([doc[i].get_images(full=True)[0][2] for i in range(3)], [40, 30, 20])

    def test_filename_with_brackets_is_a_file_not_a_glob(self):
        path = self.save(Image.new("RGB", (20, 10), "red"), "scan [1].png")
        self.ok("img2pdf", path, "-o", self.root / "out.pdf")
        self.assertTrue((self.root / "out.pdf").is_file())

    def test_folder_is_natural_sorted_and_page_count_matches(self):
        folder = self.root / "scans"
        folder.mkdir()
        for number, width in ((10, 30), (2, 20), (1, 10)):
            Image.new("RGB", (width, 8), "white").save(folder / f"img{number}.png")
        record, _ = self.ok("img2pdf", folder, "-o", self.root / "out.pdf")
        self.assertEqual(record["pages"], 3)
        with pymupdf.open(self.root / "out.pdf") as doc:
            self.assertEqual([doc[i].get_images(full=True)[0][2] for i in range(3)], [10, 20, 30])

    def test_each_continues_after_a_bad_file_and_emits_summary_exit_four(self):
        good1 = self.save(Image.new("RGB", (20, 10), "white"), "a.png")
        bad = self.root / "b.png"; bad.write_bytes(b"not an image")
        good2 = self.save(Image.new("RGB", (30, 10), "black"), "c.png")
        code, rows, err = self.run_cli("img2pdf", good1, bad, good2, "--each")
        self.assertEqual(code, 4, (rows, err))
        self.assertEqual([row["type"] for row in rows], ["result", "error", "result", "summary"])
        self.assertEqual((rows[-1]["files"], rows[-1]["ok"], rows[-1]["failed"]), (3, 2, 1))

    def test_each_creates_a_new_output_directory_and_uses_it(self):
        image = self.save(Image.new("RGB", (20, 10), "white"), "source.png")
        folder = self.root / "new-output"
        code, rows, err = self.run_cli("img2pdf", image, "--each", "-o", str(folder) + "\\")
        self.assertEqual(code, 0, (rows, err))
        self.assertTrue((folder / "source.pdf").is_file())

    def test_each_rejects_an_exact_file_destination_for_multiple_sources(self):
        a = self.save(Image.new("RGB", (20, 10), "white"), "a.png")
        b = self.save(Image.new("RGB", (20, 10), "black"), "b.png")
        self.fails(["img2pdf", a, b, "--each", "-o", self.root / "single.pdf"], "BAD_ARG")

    def test_image_pdf_failure_is_checked_not_ok(self):
        path = self.save(Image.new("RGB", (20, 10), "red"), "a.png")
        with mock.patch.object(p6_images, "_check_pdf", side_effect=PdfError("CHECK_FAILED", "mismatch", "rerun")):
            code, rows, err = self.run_cli("img2pdf", path, "-o", self.root / "out.pdf")
        self.assertEqual((code, rows[0]["verdict"]), (3, "unchecked"), (rows, err))
        self.assertTrue(rows[0]["path"].endswith("_UNCHECKED.pdf"))

    def test_eps_rasterization_uses_bounded_captured_tool_runner(self):
        eps = self.root / "input.eps"
        eps.write_text("%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 10 10\nshowpage\n", encoding="ascii")
        def fake(argv, **kwargs):
            output = next(value.split("=", 1)[1] for value in argv if str(value).startswith("-sOutputFile="))
            Image.new("RGB", (10, 10), "red").save(output)
            return mock.Mock(stdout="", stderr="", returncode=0)
        with mock.patch("shutil.which", side_effect=lambda name: "gs.exe" if name == "gswin64c" else None), \
                mock.patch.object(p6_images.tools, "run_tool", side_effect=fake) as run:
            _, image = p6_images._image(eps)
        self.assertEqual(run.call_args.kwargs["timeout"], 30)
        self.assertTrue(run.call_args.kwargs["cwd"])
        image.close()

    def test_ocr_timeout_follows_the_page_count_and_runs_in_the_run_temp_folder(self):
        paths = [self.save(Image.new("RGB", (20, 10), "white"), f"p{i}.png") for i in range(3)]
        calls = []
        def fake(argv, **kwargs):
            calls.append((list(argv), kwargs))
            shutil.copyfile(argv[-2], argv[-1])
            return mock.Mock(returncode=0)
        with mock.patch("shutil.which", side_effect=lambda name: "ocrmypdf.exe" if name == "ocrmypdf" else None), \
                mock.patch.object(tools, "run_tool", side_effect=fake), \
                mock.patch.object(p6_images.io, "disk_budget", wraps=p6_images.io.disk_budget) as budget:
            self.ok("img2pdf", *paths, "--ocr", "-o", self.root / "out.pdf")
        argv, kwargs = calls[0]
        self.assertEqual(kwargs["timeout"], 180)  # max(120, 60 s x 3 pages)
        self.assertIsNotNone(kwargs["temp"])
        self.assertEqual(kwargs["cwd"], kwargs["temp"])
        self.assertIn("--jobs", argv)
        self.assertEqual(budget.call_args.kwargs["multiplier"], 20)

    def test_ocr_integration_makes_text_searchable(self):
        self.require_tool("ocrmypdf")
        self.require_tool("tesseract")
        image = Image.new("RGB", (900, 200), "white")
        ImageDraw.Draw(image).text((30, 60), "HELLO WORLD", fill="black", font=ImageFont.load_default(size=90))
        path = self.save(image, "text.png")
        self.ok("img2pdf", path, "--ocr", "-o", self.root / "ocr.pdf")  # real env: tools.dev_tool_env joins tessdata
        with pymupdf.open(self.root / "ocr.pdf") as doc:
            self.assertIn("HELLO", doc[0].get_text().upper())

    def test_eps_through_ghostscript(self):
        self.require_tool("ghostscript")
        eps = self.root / "box.eps"
        eps.write_text("%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 100 50\n1 0 0 setrgbcolor 0 0 100 50 rectfill\nshowpage\n", encoding="ascii")
        with mock.patch.dict(os.environ, {"PATH": str(Path(tool_path("ghostscript")).parent) + os.pathsep + os.environ["PATH"]}):
            record, _ = self.ok("img2pdf", eps, "-o", self.root / "out.pdf")
        self.assertEqual((record["pages"], record["verdict"]), (1, "ok"))
        with pymupdf.open(self.root / "out.pdf") as doc:  # cropped to the bounding box at 300 dpi, not a Letter page
            self.assertEqual([(i[2], i[3]) for i in doc[0].get_images(full=True)], [(417, 208)])

    def test_svg_stays_vector_and_raw_formats_are_refused(self):
        svg = self.root / "v.svg"
        svg.write_text('<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="red"/></svg>', encoding="utf-8")
        record, _ = self.ok("img2pdf", svg, "-o", self.root / "out.pdf")
        self.assertEqual((record["verdict"], record["warnings"]), ("ok", []))
        self.assertColoursClose(self.shown_corners(self.root / "out.pdf"), [(255, 0, 0)] * 4)  # the red rectangle is drawn
        with pymupdf.open(self.root / "out.pdf") as doc:
            self.assertEqual(doc[0].get_images(), [])
        raw = self.root / "x.dng"
        raw.write_bytes(b"II*\x00")
        self.fails(["img2pdf", raw, "-o", self.root / "x.pdf"], "UNSUPPORTED")

    def test_page_layout_options(self):
        path = self.save(Image.new("RGB", (400, 200), "white"), "wide.png")
        self.ok("img2pdf", path, "--page", "letter", "-o", self.root / "l.pdf")
        with pymupdf.open(self.root / "l.pdf") as doc:
            self.assertEqual((round(doc[0].rect.width), round(doc[0].rect.height)), (792, 612))  # landscape per image
        self.ok("img2pdf", path, "--page", "a4", "--orient", "portrait", "-o", self.root / "p.pdf")
        with pymupdf.open(self.root / "p.pdf") as doc:
            self.assertLess(doc[0].rect.width, doc[0].rect.height)
        self.fails(["img2pdf", path, "--margin", "huge", "-o", self.root / "m.pdf"], "BAD_ARG")


class ImagesCommand(Base):
    def make_pdf(self, name="src.pdf", inline=True, smask_size=None, cmyk=False, inline_same_box=False):
        doc = pymupdf.open()
        page = doc.new_page()
        buffer = io.BytesIO()
        quadrant_image((30, 20)).save(buffer, "PNG")
        page.insert_image(pymupdf.Rect(50, 50, 150, 120), stream=buffer.getvalue())
        path = self.root / name
        doc.save(path)
        doc.close()
        with pikepdf.open(path, allow_overwriting_input=True) as pdf:
            page = pdf.pages[0]
            contents = page.obj.Contents
            content = b"".join(c.read_bytes() for c in contents) if isinstance(contents, pikepdf.Array) else contents.read_bytes()
            if inline:
                pixels = bytes([0, 255, 0] * 4)
                x, y = (50, 50) if inline_same_box else (200, 200)
                content += f"\nq 100 0 0 100 {x} {y} cm\n".encode() + b"BI /W 2 /H 2 /CS /RGB /BPC 8 ID " + pixels + b"\nEI Q\n"
                page.Contents = pdf.make_stream(content)
            if smask_size or cmyk:
                width, height = (20, 10)
                mask = pdf.make_stream(bytes([128]) * (smask_size[0] * smask_size[1]) if smask_size else bytes([128]) * 200)
                mask.Type, mask.Subtype = pikepdf.Name.XObject, pikepdf.Name.Image
                mask.Width, mask.Height = smask_size or (20, 10)
                mask.ColorSpace, mask.BitsPerComponent = pikepdf.Name.DeviceGray, 8
                comps, space = (4, pikepdf.Name.DeviceCMYK) if cmyk else (3, pikepdf.Name.DeviceRGB)
                base = pdf.make_stream(bytes([10]) * (width * height * comps))
                base.Type, base.Subtype = pikepdf.Name.XObject, pikepdf.Name.Image
                base.Width, base.Height, base.ColorSpace, base.BitsPerComponent = width, height, space, 8
                base.SMask = mask
                page.Resources.XObject["/Masked"] = base
                page.Contents = pdf.make_stream(content + b"\nq 50 0 0 25 300 300 cm /Masked Do Q\n")
            pdf.save()
        return path

    def test_xobject_and_inline_images_are_each_written_once(self):
        source = self.make_pdf()
        record, _ = self.ok("images", source, "-o", str(self.root / "out") + "\\")
        files = sorted(p.name for p in Path(record["path"]).iterdir() if p.name != "images.json")
        self.assertEqual(len(files), 2, files)
        self.assertEqual(len(json.loads((Path(record["path"]) / "images.json").read_text(encoding="utf-8"))), 2)
        self.assertEqual(record["verdict"], "ok")
        sizes = sorted(Image.open(Path(record["path"]) / name).size for name in files)
        self.assertEqual(sizes, [(2, 2), (30, 20)])

    def test_inline_image_with_xobject_bbox_is_not_exported_twice(self):
        source = self.make_pdf(inline_same_box=True)
        record, _ = self.ok("images", source, "-o", str(self.root / "out") + "\\")
        rows = json.loads((Path(record["path"]) / "images.json").read_text(encoding="utf-8"))
        self.assertEqual(len(rows), 2)

    def test_oversized_xobject_is_preflighted_before_extract_image(self):
        source = self.make_pdf(inline=False)
        with pikepdf.open(source, allow_overwriting_input=True) as pdf:
            image = next(iter(pdf.pages[0].Resources.XObject.values()))
            image.Width, image.Height = 20001, 10001
            pdf.save()
        with mock.patch.object(pymupdf.Document, "extract_image", side_effect=AssertionError("decoded before cap")):
            record, _ = self.ok("images", source, "-o", str(self.root / "big") + "\\")
        self.assertIn("SKIPPED_TOO_BIG", record["warnings"])

    def test_oversized_image_is_not_decoded_by_the_page_wide_inline_pass(self):
        source = self.make_pdf(inline=False)
        with pikepdf.open(source, allow_overwriting_input=True) as pdf:
            image = next(iter(pdf.pages[0].Resources.XObject.values()))
            image.Width, image.Height = 20001, 10001
            pdf.save()
        real = pymupdf.Page.get_text
        def guarded(page, option="text", *a, **kw):
            if option == "dict":
                raise AssertionError("get_text('dict') would decode the skipped oversized image")
            return real(page, option, *a, **kw)

        def guarded_info(page, *a, **kw):
            raise AssertionError("get_image_info would decode the skipped oversized image")

        with mock.patch.object(pymupdf.Page, "get_text", guarded), \
                mock.patch.object(pymupdf.Page, "get_image_info", guarded_info):
            record, _ = self.ok("images", source, "-o", str(self.root / "big2") + "\\")
        self.assertIn("SKIPPED_TOO_BIG", record["warnings"])

    def test_min_px_also_applies_to_inline_images(self):
        source = self.make_pdf()
        record, _ = self.ok("images", source, "--min-px", "100", "-o", str(self.root / "out") + "\\")
        files = [p.name for p in Path(record["path"]).iterdir() if p.name != "images.json"]
        self.assertEqual(len(files), 1, files)

    def test_soft_mask_of_a_different_size_is_resampled_and_merged(self):
        source = self.make_pdf(inline=False, smask_size=(10, 5))
        record, _ = self.ok("images", source, "-o", str(self.root / "out") + "\\")
        merged = [Image.open(p) for p in Path(record["path"]).glob("*.png") if Image.open(p).size == (20, 10)]
        self.assertEqual(len(merged), 1)
        self.assertEqual(merged[0].mode, "RGBA")
        self.assertEqual(merged[0].getpixel((3, 3))[3], 128)

    def test_cmyk_base_with_soft_mask_is_converted_not_fatal(self):
        source = self.make_pdf(inline=False, cmyk=True)
        record, _ = self.ok("images", source, "-o", str(self.root / "out") + "\\")
        self.assertTrue(any(Image.open(p).size == (20, 10) and Image.open(p).mode == "RGBA" for p in Path(record["path"]).glob("*.png")))

    def test_one_bad_image_is_a_warning_and_the_rest_is_published(self):
        source = self.make_pdf(inline=False, smask_size=(10, 5))
        with mock.patch.object(p6_images, "_save_decoded", side_effect=RuntimeError("boom")):
            record, err = self.ok("images", source, "-o", str(self.root / "out") + "\\")
        self.assertIn("IMAGE_FAILED", record["warnings"])
        self.assertEqual(record["verdict"], "warn")
        self.assertTrue((Path(record["path"]) / "images.json").is_file())

    def test_oversized_decode_is_skipped_with_a_warning(self):
        source = self.make_pdf(inline=False, smask_size=(10, 5))
        with mock.patch.object(p6_images, "MAX_PIXELS", 100):
            record, _ = self.ok("images", source, "-o", str(self.root / "out") + "\\")
        self.assertIn("SKIPPED_TOO_BIG", record["warnings"])
        self.assertEqual([p for p in Path(record["path"]).glob("*.png") if Image.open(p).size == (20, 10)], [])

    def test_ocr_reports_pages_with_no_recognized_text(self):
        image = self.save(Image.new("RGB", (40, 30), "white"), "blank.png")
        def fake(argv, **kwargs):
            shutil.copyfile(argv[-2], argv[-1])
            return mock.Mock(returncode=0, stdout="", stderr="")
        with mock.patch("shutil.which", side_effect=lambda name: "ocr.exe" if name == "ocrmypdf" else None), \
                mock.patch.object(tools, "run_tool", side_effect=fake):
            record, err = self.ok("img2pdf", image, "--ocr", "-o", self.root / "ocr.pdf")
        self.assertIn("OCR_EMPTY", record["warnings"])

    def test_encrypted_pdf_export_warns(self):
        source = self.make_pdf(inline=False)
        locked = self.root / "locked.pdf"
        with pikepdf.open(source) as pdf:
            pdf.save(locked, encryption=pikepdf.Encryption(user="s3cret", owner="s3cret"))
        password = self.root / "pw.txt"
        password.write_text("s3cret", encoding="utf-8")
        record, err = self.ok("images", locked, "--password-file", password, "-o", str(self.root / "out") + "\\")
        self.assertIn("DECRYPTED_EXPORT", record["warnings"])
        self.assertEqual(record["verdict"], "warn")
        self.assertNotIn("s3cret", err)


class ImageHelpers(Base):
    def test_pillow_pixel_cap_matches_the_toolkit_limit(self):
        self.assertEqual(Image.MAX_IMAGE_PIXELS, 100_000_000)
        under = self.root / "under.png"
        Image.new("1", (14000, 13600)).save(under)  # 190 MP: above Pillow's warning threshold, below its error cap
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", Image.DecompressionBombWarning)
            path, image = p6_images._image(under)
        self.assertEqual(image.size, (14000, 13600))
        image.close()
        over = self.root / "over.png"
        Image.new("1", (21000, 10000)).save(over)  # 210 MP
        with self.assertRaises(PdfError) as caught:
            p6_images._image(over)
        self.assertEqual(caught.exception.code, "TOO_BIG")

    def test_imginfo_reads_the_exif_ifd_date(self):
        path = self.save(Image.new("RGB", (40, 20), "white"), "d.jpg", "JPEG", exif=exif_with(6, "2021:02:03 04:05:06"))
        record, _ = self.ok("imginfo", path)
        self.assertEqual(record["data"]["exif_date"], "2021:02:03 04:05:06")
        self.assertEqual(record["data"]["orientation"], 6)
        self.assertEqual(record["data"]["size"], "40x20")
        self.assertEqual(record["verdict"], "report")

    def test_imginfo_reports_blankness_and_16bit_gray_metadata(self):
        white = self.save(Image.new("L", (50, 50), 255), "white.png")
        text = Image.new("L", (50, 50), 255)
        ImageDraw.Draw(text).text((5, 5), "TEXT", fill=0)
        text_path = self.save(text, "text.png")
        self.assertTrue(self.ok("imginfo", white)[0]["data"]["blank"])
        self.assertFalse(self.ok("imginfo", text_path)[0]["data"]["blank"])
        bw = Image.new("1", (50, 50), 0)
        ImageDraw.Draw(bw).rectangle((0, 0, 24, 49), fill=1)
        checker = self.save(bw, "checker.png")
        self.assertFalse(self.ok("imginfo", checker)[0]["data"]["blank"])
        deep = self.save(Image.new("I;16", (20, 10), 30000), "depth16.png")
        data = self.ok("imginfo", deep)[0]["data"]
        self.assertEqual((data["bit_depth"], data["colour"]), (16, "gray"))

    def test_imgdiff_finds_the_changed_tile_row_by_row(self):
        a = Image.new("RGB", (300, 200), "white")
        b = a.copy()
        ImageDraw.Draw(b).rectangle((200, 150, 260, 190), fill="black")
        pa, pb = self.save(a, "a.png"), self.save(b, "b.png")
        record, _ = self.ok("imgdiff", pa, pb)
        boxes = [f["box_px"] for f in record["data"]["findings"] if f["code"] == "TILE_CHANGED"]
        self.assertTrue(boxes)
        self.assertTrue(all(box[0] >= 189 and box[1] >= 113 for box in boxes), boxes)
        same, _ = self.ok("imgdiff", pa, pa)
        self.assertEqual(same["data"]["findings"], [])
        self.assertEqual(same["verdict"], "report")
        small = self.save(Image.new("RGB", (150, 100), "white"), "s.png")
        record, _ = self.ok("imgdiff", pa, small)
        self.assertEqual(record["data"]["findings"][0]["code"], "SIZE_CHANGED")

    def test_imgdiff_includes_alpha_channel(self):
        transparent = self.save(Image.new("RGBA", (100, 100), (20, 40, 60, 0)), "transparent.png")
        opaque = self.save(Image.new("RGBA", (100, 100), (20, 40, 60, 255)), "opaque.png")
        record, _ = self.ok("imgdiff", transparent, opaque)
        self.assertTrue(any(row["code"] == "TILE_CHANGED" for row in record["data"]["findings"]))

    def test_imgtext_runs_tesseract_in_the_run_temp_folder(self):
        self.require_tool("tesseract")
        image = Image.new("RGB", (900, 200), "white")
        ImageDraw.Draw(image).text((30, 60), "HELLO WORLD", fill="black", font=ImageFont.load_default(size=90))
        path = self.save(image, "t.png")
        with mock.patch.dict(os.environ, {"PATH": str(Path(tool_path("tesseract")).parent) + os.pathsep + os.environ["PATH"]}), \
                mock.patch.object(tools, "run_tool", wraps=tools.run_tool) as run:
            record, _ = self.ok("imgtext", path, "--lang", "eng")
        self.assertIn("HELLO", record["data"]["text"].upper())
        self.assertGreater(record["data"]["mean_confidence"], 0)
        self.assertIsNotNone(run.call_args.kwargs["temp"])
        self.assertEqual(run.call_args.kwargs["cwd"], run.call_args.kwargs["temp"])
        self.assertEqual(record["verdict"], "report")


class ToimgFormats(Base):
    def make_pdf(self, pages=3):
        doc = pymupdf.open()
        for number in range(pages):
            page = doc.new_page(width=200, height=100)
            page.insert_text((20, 50), f"Page {number + 1}", fontsize=18)
        path = self.root / "doc.pdf"
        doc.save(path)
        doc.close()
        return path

    def test_svg_export_is_vector_with_text_as_paths_by_default(self):
        pdf = self.make_pdf(1)
        record, _ = self.ok("toimg", pdf, "--format", "svg", "-o", self.root / "p.svg")
        text = Path(record["path"]).read_text(encoding="utf-8")
        self.assertTrue(text.lstrip().startswith("<?xml") or "<svg" in text[:200])
        self.assertIn("<path", text)
        self.assertNotIn("<text", text)
        self.assertEqual(record["verdict"], "report")  # vector output is not content-checked
        record, err = self.ok("toimg", pdf, "--format", "svg", "--svg-text", "-o", self.root / "t.svg")
        self.assertIn("<text", Path(record["path"]).read_text(encoding="utf-8"))
        self.assertIn("SVG_TEXT_NEEDS_FONTS", record["warnings"])
        self.assertIn("WARN pdf toimg SVG_TEXT_NEEDS_FONTS", err)

    def test_svg_respects_requested_page_geometry_without_rasterizing(self):
        pdf = self.make_pdf(1)
        svg = self.root / "vector.svg"
        svg.write_text('<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="red"/></svg>', encoding="utf-8")
        self.ok("img2pdf", svg, "-o", pdf)
        record, _ = self.ok("img2pdf", svg, "--page", "a4", "--orient", "portrait", "-o", self.root / "a4.pdf")
        with pymupdf.open(record["path"]) as doc:
            self.assertAlmostEqual(doc[0].rect.width, 595.276, delta=1)
            self.assertAlmostEqual(doc[0].rect.height, 841.89, delta=1)
            self.assertEqual(doc[0].get_images(), [])

    def test_svg_refuses_raster_options(self):
        pdf = self.make_pdf(1)
        for option in (["--clip", "0,0,10,10"], ["--gray"], ["--bw"], ["--dpi", "72"], ["--alpha"], ["--stitch", "v"]):
            with self.subTest(option=option):
                self.fails(["toimg", pdf, "--format", "svg", *option], "BAD_ARG")
        self.fails(["toimg", pdf, "--format", "png", "--svg-text"], "BAD_ARG")

    def test_svg_multipage_goes_to_a_folder_of_svg_files(self):
        record, _ = self.ok("toimg", self.make_pdf(3), "--format", "svg", "-o", str(self.root / "svgs") + "\\")
        self.assertEqual(sorted(p.suffix for p in Path(record["path"]).iterdir()), [".svg"] * 3)

    def test_multipage_tiff_single_has_one_frame_per_page(self):
        record, _ = self.ok("toimg", self.make_pdf(3), "--format", "tiff", "--single", "--dpi", "72", "-o", self.root / "all.tiff")
        self.assertEqual(record["kind"], "image")
        with Image.open(record["path"]) as tiff:
            self.assertEqual(tiff.n_frames, 3)
            self.assertEqual(tiff.size, (200, 100))
            tiff.seek(2)
            self.assertEqual(tiff.size, (200, 100))
        self.assertEqual(record["verdict"], "ok")
        self.assertTrue(record["path"].endswith(".tiff"))
        self.fails(["toimg", self.make_pdf(3), "--format", "png", "--single"], "BAD_ARG")
        self.fails(["toimg", self.make_pdf(3), "--format", "tiff", "--single", "--stitch", "v"], "BAD_ARG")

    def test_corrupt_raster_export_is_published_unchecked(self):
        pdf = self.make_pdf(1)
        def corrupt(image, path, args):
            stream = io.BytesIO(); image.save(stream, format="PNG")
            data = stream.getvalue()
            Path(path).write_bytes(data[:len(data) // 2])  # IDAT cut short: the header still opens
        with mock.patch.dict(p3c_helpers.WRITERS, {"png": {**p3c_helpers.WRITERS["png"], "write": corrupt}}):
            code, rows, err = self.run_cli("toimg", pdf, "--dpi", "72", "-o", self.root / "bad.png")
        self.assertEqual((code, rows[0]["verdict"]), (3, "unchecked"), (rows, err))
        self.assertTrue(rows[0]["path"].endswith("_UNCHECKED.png"))

    def test_tiff_group4_single_with_bw(self):
        record, _ = self.ok("toimg", self.make_pdf(2), "--format", "tiff", "--single", "--bw", "--tiff-compression", "group4",
                            "--dpi", "72", "-o", self.root / "g4.tiff")
        with Image.open(record["path"]) as tiff:
            self.assertEqual(tiff.n_frames, 2)
            self.assertEqual(tiff.mode, "1")

    def test_extension_follows_the_format_for_every_layout(self):
        pdf = self.make_pdf(3)
        cases = {"jpeg": "jpg", "webp": "webp", "tiff": "tiff", "bmp": "bmp", "gif": "gif", "png": "png"}
        for fmt, ext in cases.items():
            with self.subTest(fmt=fmt):
                folder, _ = self.ok("toimg", pdf, "--format", fmt, "--dpi", "72", "-o", str(self.root / f"f_{fmt}") + "\\")
                names = sorted(p.name for p in Path(folder["path"]).iterdir())
                self.assertEqual(names, [f"doc_p00{n}.{ext}" for n in (1, 2, 3)])
                for name in names:
                    with Image.open(Path(folder["path"]) / name) as image:
                        self.assertEqual(image.size, (200, 100))
                single, _ = self.ok("toimg", pdf, "--format", fmt, "--pages", "1", "--dpi", "72", "-o", self.root / f"s_{fmt}")
                self.assertEqual(Path(single["path"]).name, f"s_{fmt}")  # explicit -o name is kept
                default, _ = self.ok("toimg", pdf, "--format", fmt, "--pages", "2", "--dpi", "72")
                self.assertEqual(Path(default["path"]).name, f"doc_p002.{ext}")
                stitched, _ = self.ok("toimg", pdf, "--format", fmt, "--stitch", "v", "--dpi", "72")
                self.assertEqual(Path(stitched["path"]).name, f"doc_stitch.{ext}")
                with Image.open(stitched["path"]) as image:
                    self.assertEqual(image.size, (200, 300))

    def test_format_aliases_and_unknown_format(self):
        pdf = self.make_pdf(1)
        record, _ = self.ok("toimg", pdf, "--format", "jpg", "--dpi", "72")
        self.assertTrue(record["path"].endswith("_p001.jpg"))
        self.fails(["toimg", pdf, "--format", "pcx"], "BAD_ARG")

    def test_folder_discovery_includes_jpeg2000_extensions(self):
        folder = self.root / "jp2"
        folder.mkdir()
        (folder / "a.jp2").write_bytes(b"placeholder")
        (folder / "b.j2k").write_bytes(b"placeholder")
        files = p6_images._expand_inputs(folder)
        self.assertEqual({path.suffix for path in files}, {".j2k", ".jp2"})

    def test_avif_and_heic_round_trip(self):
        pdf = self.make_pdf(1)
        for fmt in ("avif", "heic"):
            with self.subTest(fmt=fmt):
                record, _ = self.ok("toimg", pdf, "--format", fmt, "--dpi", "72")
                import pillow_avif  # noqa: F401
                import pillow_heif
                pillow_heif.register_heif_opener()
                with Image.open(record["path"]) as image:
                    self.assertEqual(image.size, (200, 100))
                self.assertTrue(record["path"].endswith(f"_p001.{fmt}"))
        self.fails(["toimg", pdf, "--format", "heic", "--lossless"], "BAD_ARG")


TSV_HEAD = "level\tpage\tblock\tpar\tline\tword\tleft\ttop\twidth\theight\tconf\ttext\n"
TSV_WORD = "5\t1\t1\t1\t1\t1\t0\t0\t10\t10\t96\tHello\n"
SVG_RED = '<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="red"/></svg>'


def stale_part(folder, kind="file"):
    """A leftover staging file (or folder) of a dead process, older than the one-hour sweep limit."""
    path = Path(folder) / ".old.2147483000-deadbeef.part.pdf"
    if kind == "dir":
        path.mkdir()
    else:
        path.write_bytes(b"x")
    old = time.time() - 7200
    os.utime(path, (old, old))
    return path


class BatchBFixes(Base):
    """Regression tests for the phase 6 review findings on the image commands."""

    def pdf_with_sizes(self, sizes, name="mixed.pdf"):
        doc = pymupdf.open()
        for width, height in sizes:
            doc.new_page(width=width, height=height).insert_text((10, 30), "page", fontsize=12)
        path = self.root / name
        doc.save(path)
        doc.close()
        return path

    def pdf_with_image(self, name="src.pdf"):
        buffer = io.BytesIO()
        quadrant_image((30, 20)).save(buffer, "PNG")
        doc = pymupdf.open()
        doc.new_page().insert_image(pymupdf.Rect(50, 50, 150, 120), stream=buffer.getvalue())
        path = self.root / name
        doc.save(path)
        doc.close()
        return path

    def corrupt_png_writer(self):
        def corrupt(image, path, args):
            stream = io.BytesIO(); image.save(stream, format="PNG")
            data = stream.getvalue()
            Path(path).write_bytes(data[:len(data) // 2])
        return mock.patch.dict(p3c_helpers.WRITERS, {"png": {**p3c_helpers.WRITERS["png"], "write": corrupt}})

    # ----- 1: toimg --single checks each frame against its own page -----
    def test_tiff_single_with_pages_of_different_sizes_is_ok(self):
        pdf = self.pdf_with_sizes([(200, 100), (100, 200)])
        code, rows, err = self.run_cli("toimg", pdf, "--format", "tiff", "--single", "--dpi", "72", "-o", self.root / "m.tiff")
        self.assertEqual((code, rows[0]["verdict"]), (0, "ok"), (rows, err))
        with Image.open(rows[0]["path"]) as tiff:
            tiff.seek(1)
            self.assertEqual(tiff.size, (100, 200))

    # ----- 3 and 27: imgtext output file and unreadable-by-Tesseract formats -----
    def test_imgtext_output_option_writes_the_text_file(self):
        png = self.save(Image.new("RGB", (30, 20), "white"), "t.png")
        out = self.root / "text.txt"
        fake = mock.Mock(return_value=mock.Mock(stdout=TSV_HEAD + TSV_WORD, stderr="", returncode=0))
        with mock.patch("shutil.which", side_effect=lambda n: n + ".exe"), mock.patch.object(tools, "run_tool", fake):
            record, _ = self.ok("imgtext", png, "-o", out)
        self.assertEqual(out.read_text(encoding="utf-8"), "Hello")
        self.assertEqual(record["data"]["output"], str(out))

    def test_imgtext_gives_tesseract_a_png_for_formats_it_cannot_read(self):
        tga = self.save(Image.new("RGB", (30, 20), "white"), "t.tga")
        seen = []
        def fake(argv, **kwargs):
            with Image.open(argv[1]) as opened:  # the file Tesseract would get
                seen.append((Path(argv[1]).suffix, opened.format))
            return mock.Mock(stdout=TSV_HEAD + TSV_WORD, stderr="", returncode=0)
        with mock.patch("shutil.which", side_effect=lambda n: n + ".exe"), mock.patch.object(tools, "run_tool", side_effect=fake):
            self.ok("imgtext", tga)
        self.assertEqual(seen, [(".png", "PNG")])

    # ----- 4 and 28: EPS runs in the run temp folder, keeps tool error codes, bounds the raster -----
    def eps_file(self, box="0 0 10 10"):
        eps = self.root / "in.eps"
        eps.write_text(f"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: {box}\nshowpage\n", encoding="ascii")
        return eps

    def test_eps_tool_failure_keeps_its_code_and_runs_in_the_run_temp_folder(self):
        eps = self.eps_file()
        cwds = []
        def slow(argv, **kwargs):
            cwds.append(kwargs["cwd"])
            raise clierr.Fail("TIMEOUT", "gs ran over 30 s", "raise the timeout")
        with mock.patch("shutil.which", side_effect=lambda n: n + ".exe"), mock.patch.object(tools, "run_tool", side_effect=slow):
            self.fails(["img2pdf", eps, "-o", self.root / "e.pdf"], "TIMEOUT")
        self.assertEqual(Path(cwds[0]).parent, Path(tempfile.gettempdir()) / "pdftools")

    def test_eps_resolution_is_lowered_so_the_raster_stays_within_the_pixel_limit(self):
        eps = self.eps_file("0 0 50000 50000")
        resolutions = []
        def fake(argv, **kwargs):
            resolutions.append(next(int(a[2:]) for a in argv if str(a).startswith("-r")))
            Image.new("RGB", (10, 10), "red").save(next(a.split("=", 1)[1] for a in argv if str(a).startswith("-sOutputFile=")))
            return mock.Mock(stdout="", stderr="", returncode=0)
        with mock.patch("shutil.which", side_effect=lambda n: n + ".exe"), mock.patch.object(tools, "run_tool", side_effect=fake):
            _, image = p6_images._image(eps)
            image.close()
        self.assertLess(resolutions[0], 300)
        self.assertLessEqual(50000 * resolutions[0] / 72 * 50000 * resolutions[0] / 72, p6_images.MAX_PIXELS)

    def test_eps_atend_bounding_box_is_read_from_the_trailer(self):
        eps = self.root / "atend.eps"
        eps.write_bytes(b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: (atend)\n" + b"%" * 70000 +
                        b"\n%%Trailer\n%%BoundingBox: 0 0 100 50\nshowpage\n")
        self.assertEqual(p6_images._eps_bbox(eps), (0.0, 0.0, 100.0, 50.0))
        self.assertEqual(p6_images._eps_dpi(eps), 300)

    def test_bounded_svg_render_scale_respects_decoded_pixel_cap(self):
        rect = pymupdf.Rect(0, 0, 100000, 50000)
        scale = p6_images._bounded_scale(rect, 200 / 72)
        self.assertLessEqual(math.ceil(rect.width * scale) * math.ceil(rect.height * scale),
                             p6_images.MAX_PIXELS)

    def test_eps_magic_with_png_extension_still_uses_run_tool(self):
        eps = self.root / "renamed.png"
        eps.write_text("%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 10 10\nshowpage\n", encoding="ascii")
        calls = []
        def fake_run(argv, **kwargs):
            calls.append((argv, kwargs))
            destination = next(arg.split("=", 1)[1] for arg in argv if arg.startswith("-sOutputFile="))
            Image.new("RGBA", (8, 8), "white").save(destination)
        with mock.patch("pdftools.commands.p6_images.shutil.which", return_value="gs.exe"), \
             mock.patch("pdftools.commands.p6_images.tools.run_tool", side_effect=fake_run):
            _, image = p6_images._image(eps, secret_env=("TOKEN",))
        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0][1]["secret_env"], ("TOKEN",))
        image.close()

    def test_truncated_jpeg_is_rejected_before_embedding(self):
        source = self.save(Image.new("RGB", (120, 80), "white"), "truncated.jpg", "JPEG")
        source.write_bytes(source.read_bytes()[:-100])
        with self.assertRaises(PdfError) as caught:
            p6_images._image(source)
        self.assertEqual(caught.exception.code, "BAD_INPUT")

    # ----- 5: several named images -----
    def test_two_named_images_are_named_after_the_first_stem(self):
        a = self.save(Image.new("RGB", (20, 10), "white"), "alpha.png")
        b = self.save(Image.new("RGB", (20, 10), "black"), "beta.png")
        record, _ = self.ok("img2pdf", a, b)
        self.assertEqual(Path(record["path"]).name, "alpha_images.pdf")
        folder = self.root / "scans"
        folder.mkdir()
        for name in ("x.png", "y.png"):
            Image.new("RGB", (20, 10), "white").save(folder / name)
        record, _ = self.ok("img2pdf", folder)
        self.assertEqual(Path(record["path"]).name, "scans_images.pdf")

    # ----- 7 and 21: SVG pages -----
    def test_svg_page_honours_fit_stretch_and_is_checked(self):
        svg = self.root / "v.svg"
        svg.write_text(SVG_RED, encoding="utf-8")
        record, _ = self.ok("img2pdf", svg, "--page", "a4", "--orient", "portrait", "--fit", "stretch", "--margin", "0", "-o", self.root / "s.pdf")
        self.assertEqual(record["verdict"], "ok")
        box = ImageChops.difference(self.render(self.root / "s.pdf"), Image.new("RGB", (595, 842), "white")).getbbox()
        self.assertGreaterEqual(box[2] - box[0], 590)
        self.assertGreaterEqual(box[3] - box[1], 838)

    def test_blank_svg_page_is_a_warning_not_ok(self):
        svg = self.root / "blank.svg"
        svg.write_text('<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="50">'
                       '<image xlink:href="missing.png" width="100" height="50"/></svg>', encoding="utf-8")
        record, _ = self.ok("img2pdf", svg, "-o", self.root / "b.pdf")
        self.assertEqual((record["verdict"], record["warnings"]), ("warn", ["SVG_BLANK"]))

    # ----- 9: oversized image is never decoded by the page-wide metadata pass -----
    def test_oversized_xobject_is_not_decoded_by_get_image_info(self):
        source = self.pdf_with_image()
        with pikepdf.open(source, allow_overwriting_input=True) as pdf:
            image = next(iter(pdf.pages[0].Resources.XObject.values()))
            image.Width, image.Height = 20001, 10001
            pdf.save()
        with mock.patch.object(pymupdf, "Pixmap", side_effect=AssertionError("decoded an oversized image")):
            record, _ = self.ok("images", source, "-o", str(self.root / "big") + "\\")
        self.assertIn("SKIPPED_TOO_BIG", record["warnings"])

    # ----- 12: secret environment variables never reach Tesseract, Ghostscript or ocrmypdf -----
    def test_image_tool_launches_drop_the_secret_environment(self):
        png = self.save(Image.new("RGB", (20, 10), "white"), "t.png")
        eps = self.eps_file()
        calls = []
        def fake(argv, **kwargs):
            calls.append((Path(str(argv[0])).stem, kwargs.get("secret_env")))
            output = next((a.split("=", 1)[1] for a in argv if str(a).startswith("-sOutputFile=")), None)
            if output:
                Image.new("RGB", (10, 10), "red").save(output)
            elif Path(str(argv[0])).stem == "ocrmypdf":
                shutil.copyfile(argv[-2], argv[-1])
            return mock.Mock(stdout=TSV_HEAD, stderr="", returncode=0)
        context = cli.Context(secret_env=("PDFTOOLS_TEST_SECRET",))
        parser = cli.build_parser()
        with mock.patch("shutil.which", side_effect=lambda n: n + ".exe"), mock.patch.object(tools, "run_tool", side_effect=fake), \
                contextlib.redirect_stderr(io.StringIO()):
            p6_images.imgtext(parser.parse_args(["imgtext", str(png)]), context)
            p6_images.img2pdf(parser.parse_args(["img2pdf", str(eps), "-o", str(self.root / "e.pdf")]), context)
            p6_images.img2pdf(parser.parse_args(["img2pdf", str(png), "--ocr", "-o", str(self.root / "o.pdf")]), context)
        self.assertEqual(sorted(name for name, _ in calls), ["gswin64c", "ocrmypdf", "tesseract"])
        for name, secret in calls:
            self.assertEqual(secret, ("PDFTOOLS_TEST_SECRET",), name)

    # ----- 14 and 15: dry runs -----
    def test_each_dry_run_creates_no_output_folder(self):
        image = self.save(Image.new("RGB", (20, 10), "white"), "source.png")
        folder = self.root / "never"
        code, rows, err = self.run_cli("--dry-run", "img2pdf", image, "--each", "-o", str(folder) + "\\")
        self.assertEqual(code, 0, (rows, err))
        self.assertFalse(folder.exists())

    def test_dry_run_verdict_is_report_marked_provisional(self):
        image = self.save(Image.new("RGB", (20, 10), "white"), "source.png")
        for argv in (("img2pdf", image), ("images", self.pdf_with_image())):
            with self.subTest(command=argv[0]):
                code, rows, err = self.run_cli("--dry-run", *argv)
                self.assertEqual((code, rows[0]["verdict"], rows[0].get("provisional")), (0, "report", True), (rows, err))

    # ----- 16: HEIC bit depth -----
    def test_heic_10bit_depth_is_reported_and_warned(self):
        import pillow_heif
        heic = self.root / "deep.heic"
        pillow_heif.from_bytes("RGB;16", (16, 16), (np.arange(16 * 16 * 3, dtype="<u2") * 64).tobytes()).save(heic, bit_depth=10, quality=90)
        self.assertEqual(self.ok("imginfo", heic)[0]["data"]["bit_depth"], 10)
        record, _ = self.ok("img2pdf", heic, "-o", self.root / "deep.pdf")
        self.assertEqual((record["verdict"], record["warnings"]), ("warn", ["BIT_DEPTH_REDUCED"]))

    # ----- 17: SVG export is not held to the raster limits -----
    def test_svg_export_of_a_huge_page_is_not_refused_by_the_pixel_limit(self):
        pdf = self.pdf_with_sizes([(14400, 14400)], "huge.pdf")
        code, rows, err = self.run_cli("toimg", pdf, "--format", "svg")
        self.assertEqual(code, 0, (rows, err))
        self.assertTrue(Path(rows[0]["path"]).is_file())

    # ----- 20: OCR output is what gets checked -----
    def test_ocr_output_is_checked_before_it_is_published(self):
        path = self.save(Image.new("RGB", (20, 10), "white"), "a.png")
        def wrong_ocr(argv, **kwargs):
            buffer = io.BytesIO(); Image.new("RGB", (7, 7), "black").save(buffer, "PNG")
            with pymupdf.open() as doc:
                doc.new_page().insert_image(pymupdf.Rect(10, 10, 60, 60), stream=buffer.getvalue())
                doc.save(argv[-1])
            return mock.Mock(returncode=0)
        with mock.patch("shutil.which", side_effect=lambda n: "ocrmypdf.exe" if n == "ocrmypdf" else None), \
                mock.patch.object(tools, "run_tool", side_effect=wrong_ocr):
            code, rows, err = self.run_cli("img2pdf", path, "--ocr", "-o", self.root / "out.pdf")
        self.assertEqual((code, rows[0]["verdict"]), (3, "unchecked"), (rows, err))
        self.assertTrue(rows[0]["path"].endswith("_UNCHECKED.pdf"))

    # ----- 22 and 23 and 24 -----
    def test_imginfo_of_a_tiff_with_dpi_is_json_clean(self):
        tiff = self.save(Image.new("RGB", (20, 10), "white"), "d.tiff", dpi=(300, 300))
        record, _ = self.ok("imginfo", tiff)
        self.assertEqual(record["data"]["dpi"], [300.0, 300.0])

    def test_imgdiff_sees_a_difference_between_16bit_values_above_255(self):
        a = self.save(Image.fromarray(np.full((20, 20), 30000, dtype="<u2")), "a16.png")
        b = self.save(Image.fromarray(np.full((20, 20), 60000, dtype="<u2")), "b16.png")
        record, _ = self.ok("imgdiff", a, b)
        self.assertTrue(record["data"]["findings"])
        self.assertEqual(record["verdict"], "warn")

    def test_images_default_folder_is_named_after_the_pdf_without_pdf_extension(self):
        source = self.pdf_with_image("report.pdf")
        record, _ = self.ok("images", source)
        self.assertEqual(Path(record["path"]), self.root / "report_images")
        self.assertTrue(Path(record["path"]).is_dir())

    # ----- 25: an unchecked export is not also a name clash -----
    def test_unchecked_toimg_export_does_not_warn_name_taken(self):
        pdf = self.pdf_with_sizes([(200, 100)], "one.pdf")
        with self.corrupt_png_writer():
            code, rows, err = self.run_cli("toimg", pdf, "--dpi", "72", "-o", self.root / "bad.png")
        self.assertEqual((code, rows[0]["verdict"]), (3, "unchecked"), (rows, err))
        self.assertNotIn("NAME_TAKEN", rows[0]["warnings"])
        many = self.pdf_with_sizes([(200, 100)] * 2, "many.pdf")
        with self.corrupt_png_writer():
            code, rows, err = self.run_cli("toimg", many, "--dpi", "72", "-o", str(self.root / "bad") + "\\")
        self.assertEqual((code, rows[0]["verdict"]), (3, "unchecked"), (rows, err))
        self.assertTrue(rows[0]["path"].endswith("_UNCHECKED"))
        self.assertNotIn("NAME_TAKEN", rows[0]["warnings"])

    # ----- 26: --max-dpi leaves embedded JPEG data alone -----
    def test_max_dpi_does_not_turn_a_jpeg_into_a_larger_png(self):
        noise = np.random.default_rng(1).integers(0, 256, (400, 600, 3), dtype=np.uint8)
        jpeg = self.save(Image.fromarray(noise), "noise.jpg", "JPEG", quality=90)
        self.ok("img2pdf", jpeg, "--max-dpi", "50", "-o", self.root / "out.pdf")
        with pikepdf.open(self.root / "out.pdf") as pdf:
            image = next(iter(pdf.pages[0].Resources.XObject.values()))
            self.assertEqual(str(image.Filter), "/DCTDecode")
            self.assertEqual(image.read_raw_bytes(), jpeg.read_bytes())

    # ----- 29: skipped RAW files are reported -----
    def test_raw_files_in_a_folder_are_reported_not_silently_dropped(self):
        folder = self.root / "mixed"
        folder.mkdir()
        Image.new("RGB", (20, 10), "white").save(folder / "a.png")
        (folder / "b.dng").write_bytes(b"II*\x00")
        record, err = self.ok("img2pdf", folder, "-o", self.root / "m.pdf")
        self.assertEqual((record["pages"], record["verdict"], record["warnings"]), (1, "warn", ["SKIPPED_UNSUPPORTED"]))
        only = self.root / "rawonly"
        only.mkdir()
        (only / "b.dng").write_bytes(b"II*\x00")
        self.fails(["img2pdf", only, "-o", self.root / "r.pdf"], "UNSUPPORTED")

    def test_each_carries_discovery_warnings_on_its_summary_once(self):
        folder = self.root / "mixed-each"
        folder.mkdir()
        Image.new("RGB", (20, 10), "white").save(folder / "a.png")
        (folder / "b.dng").write_bytes(b"II*\x00")
        code, rows, stderr = self.run_cli("img2pdf", folder, "--each", "-o", str(self.root / "out") + "\\")
        self.assertEqual(code, 0, (rows, stderr))
        self.assertIn("SKIPPED_UNSUPPORTED", rows[-1]["warnings"])
        self.assertEqual(stderr.count("WARN pdf img2pdf SKIPPED_UNSUPPORTED"), 1)

    # ----- 30: selftest -----
    def test_selftest_reports_an_image_format_that_cannot_be_written_back(self):
        def broken(image, path, args):
            Path(path).write_bytes(b"II*\x00not a tiff")
        with mock.patch.dict(p3c_helpers.WRITERS, {"tiff": {**p3c_helpers.WRITERS["tiff"], "write": broken}}):
            code, rows, err = self.run_cli("selftest")
        failures = rows[0]["data"]["failures"]
        self.assertTrue(any(f.startswith("image format tiff") for f in failures), failures)
        self.assertFalse(any(f.startswith("image format") and "tiff" not in f for f in failures), failures)
        self.assertEqual(code, 1)

    def test_images_selftest_extracts_a_real_image(self):
        output = self.root / "test-write.folder"
        p6_images.selftest_write(self.pdf_with_sizes([(100, 100)]), output)
        self.assertEqual([p.name for p in sorted(output.iterdir())], ["image_001.png", "images.json"])

    # ----- 35: stale staging left by crashed runs is swept -----
    def test_img2pdf_and_images_sweep_stale_staging(self):
        image = self.save(Image.new("RGB", (20, 10), "white"), "a.png")
        stale = stale_part(self.root)
        self.ok("img2pdf", image, "-o", self.root / "a.pdf")
        self.assertFalse(stale.exists())
        source = self.pdf_with_image()
        (self.root / "out").mkdir()
        stale = stale_part(self.root / "out", "dir")  # the folder the output goes into
        self.ok("images", source, "-o", str(self.root / "out") + "\\")
        self.assertFalse(stale.exists())

    # ----- 36 and 38 -----
    def test_multipage_folder_is_named_after_the_file_extension(self):
        record, _ = self.ok("toimg", self.pdf_with_sizes([(200, 100)] * 2, "two.pdf"), "--format", "jpeg", "--dpi", "72")
        self.assertEqual(Path(record["path"]).name, "two_jpg")

    def test_stitch_canvas_is_white_where_no_page_covers_it(self):
        pdf = self.pdf_with_sizes([(200, 100), (100, 200)], "st.pdf")
        record, _ = self.ok("toimg", pdf, "--stitch", "v", "--dpi", "72")
        with Image.open(record["path"]) as stitched:
            self.assertEqual(stitched.size, (200, 300))
            self.assertEqual(stitched.convert("RGB").getpixel((150, 250)), (255, 255, 255))

    # ----- 39 -----
    def test_dead_helpers_are_gone(self):
        self.assertFalse(hasattr(p6_images, "add_noop"))
        self.assertNotIn("_layout_args", Path(p6_images.__file__).read_text(encoding="utf-8"))

    def test_imgtext_dry_run_does_not_run_ocr_or_publish(self):
        image = self.save(Image.new("RGB", (20, 10), "white"), "dry.png")
        target = self.root / "dry.txt"
        code, rows, err = self.run_cli("--dry-run", "imgtext", image, "-o", target)
        self.assertEqual(code, 0, err)
        self.assertTrue(rows[0]["data"]["provisional"])
        self.assertFalse(target.exists())

    def test_images_without_images_is_warned(self):
        pdf = self.root / "noimages.pdf"
        with pymupdf.open() as doc:
            page = doc.new_page()
            page.insert_text((50, 50), "No images here")
            doc.save(pdf)
        record, _ = self.ok("images", pdf)
        self.assertEqual(record["verdict"], "warn")
        self.assertIn("NO_IMAGES", record["warnings"])

    def test_folder_img2pdf_output_names_pdf_inside_folder(self):
        folder = self.root / "batch"
        folder.mkdir()
        self.save(Image.new("RGB", (20, 10), "white"), "first.png").replace(folder / "first.png")
        output = self.root / "published"
        code, rows, err = self.run_cli("--dry-run", "img2pdf", folder, "-o", str(output) + "\\")
        self.assertEqual(code, 0, err)
        self.assertEqual(Path(rows[0]["path"]), (output / "batch_images.pdf").resolve())
        self.assertTrue(Path(rows[0]["path"]).is_absolute())


if __name__ == "__main__":
    unittest.main()
