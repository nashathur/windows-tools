import hashlib
import random
import re
import shutil
from pathlib import Path

import numpy as np
import pikepdf
import pymupdf
from PIL import Image, ImageDraw, ImageFilter

import importlib.util

_p1_path = Path(__file__).with_name("p1.py")
_p1_spec = importlib.util.spec_from_file_location("pdftools_fixture_p1", _p1_path)
_p1_module = importlib.util.module_from_spec(_p1_spec)
_p1_spec.loader.exec_module(_p1_module)
build_p1 = _p1_module.build_fixtures


def build_fixtures(folder, bench=False):
    folder = Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    shared = build_p1(folder / "shared")
    plain = shared["plain"]
    result = dict(shared)
    result["equation"] = folder / "equation.pdf"
    doc = pymupdf.open()
    page = doc.new_page(width=595, height=842)
    page.insert_text((40, 60), "Equation: ∂T/∂t = κ ∇²T;  q = -k ∂T/∂z")
    page.insert_text((40, 100), "repeated word repeated word climate climate")
    doc.save(result["equation"])
    doc.close()
    result["repeated"] = result["equation"]

    result["annotations"] = folder / "annotations.pdf"
    doc = pymupdf.open(plain)
    page = doc[0]
    page.add_text_annot((40, 90), "note")
    page.add_rect_annot(pymupdf.Rect(60, 100, 100, 130))
    page.add_highlight_annot([pymupdf.Rect(25, 35, 130, 55)])
    page.add_underline_annot([pymupdf.Rect(25, 35, 130, 55)])
    page.add_strikeout_annot([pymupdf.Rect(25, 35, 130, 55)])
    page.add_squiggly_annot([pymupdf.Rect(25, 35, 130, 55)])
    page.add_line_annot((30, 150), (180, 170))
    page.add_circle_annot(pymupdf.Rect(160, 100, 200, 140))
    page.add_freetext_annot(pymupdf.Rect(30, 180, 170, 220), "free text")
    page.add_polygon_annot([pymupdf.Point(180, 180), pymupdf.Point(210, 185), pymupdf.Point(195, 210)])
    page.add_polyline_annot([pymupdf.Point(180, 220), pymupdf.Point(210, 225), pymupdf.Point(195, 245)])
    page.add_stamp_annot(pymupdf.Rect(190, 260, 260, 290))
    page.add_caret_annot((270, 280))
    page.add_ink_annot([[(210.0, 310.0), (220.0, 315.0), (230.0, 310.0)]])
    page.add_file_annot((250, 315), b"attachment", "fixture.txt")
    doc.save(result["annotations"])
    doc.close()

    result["widgets"] = folder / "widgets.pdf"
    doc = pymupdf.open()
    page = doc.new_page(width=300, height=400)
    widget = pymupdf.Widget()
    widget.field_name, widget.field_type = "name", pymupdf.PDF_WIDGET_TYPE_TEXT
    widget.rect = pymupdf.Rect(30, 30, 200, 60)
    widget.field_value = "Ada"
    page.add_widget(widget)
    doc.save(result["widgets"])
    doc.close()
    result["widgets_no_appearance"] = folder / "widgets-no-appearance.pdf"
    with pikepdf.open(result["widgets"]) as pdf:
        pdf.Root["/AcroForm"]["/NeedAppearances"] = True
        for page_obj in pdf.pages:
            for annot in page_obj.obj.get("/Annots", []):
                if str(annot.get("/Subtype")) == "/Widget":
                    if "/AP" in annot:
                        del annot["/AP"]
        pdf.save(result["widgets_no_appearance"])

    result["links_bookmarks"] = folder / "links-bookmarks.pdf"
    doc = pymupdf.open(plain)
    doc.set_toc([[1, "First", 1], [1, "Second", 2]])
    doc[0].insert_link({"kind": pymupdf.LINK_URI, "from": pymupdf.Rect(20, 20, 130, 35), "uri": "https://example.invalid"})
    doc.save(result["links_bookmarks"])
    doc.close()

    result["chart"] = folder / "chart-shared-indexed.pdf"
    pixels = np.zeros((160, 240, 3), dtype=np.uint8)
    pixels[:] = (255, 255, 255)
    pixels[::8, :, :] = (210, 30, 40)
    pixels[:, ::9, :] = (20, 90, 220)
    im = Image.fromarray(pixels, "RGB")
    palette = im.quantize(colors=16)
    png = folder / "chart.png"
    palette.save(png)
    doc = pymupdf.open()
    for _ in range(2):
        page = doc.new_page(width=300, height=400)
        page.insert_image(pymupdf.Rect(20, 20, 280, 190), filename=str(png))
        page.insert_text((25, 220), "thin coloured chart lines")
    doc.save(result["chart"])
    doc.close()

    result["optional_content"] = folder / "optional-content.pdf"
    doc = pymupdf.open()
    page = doc.new_page(width=300, height=400)
    layer = doc.add_ocg("Climate layer")
    page.insert_text((30, 60), "Layered label", oc=layer)
    doc.save(result["optional_content"])
    doc.close()
    result["scanned_image"] = folder / "scanned-image.pdf"
    doc = pymupdf.open()
    page = doc.new_page(width=360, height=240)
    page.insert_image(page.rect, filename=str(png))
    doc.save(result["scanned_image"])
    doc.close()

    result["pending_redaction"] = folder / "pending-redaction.pdf"
    doc = pymupdf.open(plain)
    doc[0].add_redact_annot(pymupdf.Rect(20, 30, 180, 60))
    doc.save(result["pending_redaction"])
    doc.close()

    result["javascript_attachment"] = folder / "javascript-attachment.pdf"
    with pikepdf.open(plain) as pdf:
        js = pikepdf.Dictionary({"/S": pikepdf.Name("/JavaScript"), "/JS": pikepdf.String("app.alert('x')")})
        pdf.Root["/OpenAction"] = js
        pdf.attachments["fixture.txt"] = b"fixture attachment"
        embedded = pdf.make_stream(b"associated fixture")
        filespec = pdf.make_indirect(pikepdf.Dictionary({"/Type": pikepdf.Name("/Filespec"),
            "/F": pikepdf.String("associated.txt"), "/EF": pikepdf.Dictionary({"/F": embedded})}))
        pdf.Root["/AF"] = pikepdf.Array([filespec])
        pdf.save(result["javascript_attachment"])

    result["recoverable_xref"] = folder / "recoverable-xref.pdf"
    raw = plain.read_bytes()
    match = list(re.finditer(rb"startxref\s+\d+", raw))[-1]
    result["recoverable_xref"].write_bytes(raw[:match.start()] + re.sub(rb"\d+$", b"0", match.group(0)) + raw[match.end():])
    result["truncated"] = folder / "truncated.pdf"
    result["truncated"].write_bytes(raw[:max(64, len(raw) // 3)])

    rng = np.random.default_rng(1701)
    noise = rng.normal(128, 48, (600, 900, 3)).clip(0, 255).astype(np.uint8)
    photo = Image.fromarray(noise, "RGB").filter(ImageFilter.GaussianBlur(2))
    draw = ImageDraw.Draw(photo)
    draw.rectangle((35, 35, 300, 200), fill=(45, 122, 68), outline=(245, 210, 80), width=8)
    draw.line((0, 500, 899, 80), fill=(35, 60, 170), width=13)
    draw.text((55, 70), "FIELD STATION 2026", fill=(255, 255, 255), stroke_width=2, stroke_fill=(0, 0, 0))
    photo_path = folder / "calibration-photo.jpg"
    photo.save(photo_path, quality=95)
    result["calibration_photo"] = folder / "calibration-photo.pdf"
    doc = pymupdf.open()
    page = doc.new_page(width=595, height=842)
    page.insert_image(pymupdf.Rect(35, 90, 435, 357), filename=str(photo_path))
    page.insert_text((455, 125), "Temperature\nWind\nHumidity", fontsize=12)
    doc.save(result["calibration_photo"])
    doc.close()
    result["calibration_gray"] = folder / "calibration-gray.pdf"
    gray_path = folder / "calibration-gray.png"
    photo.convert("L").save(gray_path)
    doc = pymupdf.open()
    page = doc.new_page(width=595, height=842)
    page.insert_image(pymupdf.Rect(35, 90, 435, 357), filename=str(gray_path))
    page.insert_text((455, 125), "Temperature\nWind\nHumidity", fontsize=12)
    doc.save(result["calibration_gray"])
    doc.close()
    result["calibration_photo_q80"] = folder / "calibration-photo-q80.pdf"
    doc = pymupdf.open(result["calibration_photo"])
    page = doc[0]
    info = page.get_image_info(xrefs=True)[0]
    image = Image.frombytes("RGB", (photo.width, photo.height), photo.tobytes())
    q80 = folder / "q80.jpg"
    image.save(q80, quality=80)
    page.replace_image(info["xref"], filename=str(q80))
    doc.save(result["calibration_photo_q80"])
    doc.close()
    for quality in (65, 50):
        result[f"calibration_photo_q{quality}"] = folder / f"calibration-photo-q{quality}.pdf"
        doc = pymupdf.open(result["calibration_photo"])
        page = doc[0]
        jpg = folder / f"q{quality}.jpg"
        image.save(jpg, quality=quality)
        page.replace_image(page.get_image_info(xrefs=True)[0]["xref"], filename=str(jpg))
        doc.save(result[f"calibration_photo_q{quality}"])
        doc.close()
    result["calibration_small_image"] = folder / "calibration-small-image.pdf"
    doc = pymupdf.open()
    page = doc.new_page(width=595, height=842)
    page.insert_text((40, 80), "A large page with a small image")
    page.insert_image(pymupdf.Rect(240, 300, 300, 340), filename=str(photo_path))
    doc.save(result["calibration_small_image"])
    doc.close()
    result["calibration_small_gray_box"] = folder / "calibration-small-gray-box.pdf"
    doc = pymupdf.open(result["calibration_small_image"])
    page = doc[0]
    page.delete_image(page.get_image_info(xrefs=True)[0]["xref"])
    page.draw_rect(pymupdf.Rect(240, 300, 300, 340), color=(0.5, 0.5, 0.5), fill=(0.5, 0.5, 0.5))
    doc.save(result["calibration_small_gray_box"])
    doc.close()

    result["calibration_table"] = folder / "calibration-table.pdf"
    doc = pymupdf.open()
    page = doc.new_page(width=400, height=300)
    page.insert_text((30, 60), "Station 123.4  Temperature 25.6 C")
    page.draw_line((20, 80), (380, 80), color=(0, 0, 0), width=0.5)
    doc.save(result["calibration_table"])
    doc.close()
    result["calibration_table_changed"] = folder / "calibration-table-changed.pdf"
    doc = pymupdf.open()
    page = doc.new_page(width=400, height=300)
    page.insert_text((30, 60), "Station 123.9  Temperature 25.6 C")
    page.draw_line((20, 80), (380, 80), color=(0, 0, 0), width=0.5)
    doc.save(result["calibration_table_changed"])
    doc.close()

    result["calibration_copy"] = folder / "calibration-copy.pdf"
    shutil.copyfile(plain, result["calibration_copy"])
    result["calibration_removed"] = folder / "calibration-removed.pdf"
    white_path = folder / "white-photo.png"
    Image.new("RGB", photo.size, "white").save(white_path)
    doc = pymupdf.open(result["calibration_photo"])
    page = doc[0]
    page.replace_image(page.get_image_info(xrefs=True)[0]["xref"], filename=str(white_path))
    doc.save(result["calibration_removed"])
    doc.close()
    result["calibration_resaved"] = folder / "calibration-resaved.pdf"
    doc = pymupdf.open(plain)
    doc.save(result["calibration_resaved"])
    doc.close()
    result["determinism_hash"] = hashlib.sha256(result["equation"].read_bytes()).hexdigest()

    if bench:
        result["benchmark"] = folder / "benchmark-500.pdf"
        doc = pymupdf.open()
        rng = random.Random(481516)
        for i in range(500):
            page = doc.new_page(width=595, height=842)
            words = " ".join(f"{rng.randrange(10**5):05d}" for _ in range(12))
            page.insert_text((40, 60), f"Page {i + 1:03d} deterministic climate data {words}")
        doc.save(result["benchmark"])
        doc.close()
    return result
