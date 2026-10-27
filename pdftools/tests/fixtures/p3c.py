from pathlib import Path

import pymupdf


def build_fixtures(folder):
    folder = Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    pdf = folder / "phase3c.pdf"
    doc = pymupdf.open()
    for text in ("First climate page", "Second climate page"):
        page = doc.new_page(width=144, height=72)
        page.insert_text((12, 24), text)
    doc.save(pdf)
    doc.close()

    threshold = folder / "threshold.pdf"
    doc = pymupdf.open()
    page = doc.new_page(width=120, height=40)
    for x, gray in ((0, 0.35), (60, 0.75)):
        page.draw_rect(pymupdf.Rect(x, 0, x + 60, 40), color=None, fill=(gray, gray, gray))
    doc.save(threshold)
    doc.close()

    rotated = folder / "rotated.pdf"
    doc = pymupdf.open()
    page = doc.new_page(width=240, height=160)
    page.insert_text((30, 45), "Rotated clipping fixture")
    page.set_cropbox(pymupdf.Rect(10, 15, 230, 155))
    page.set_rotation(90)
    doc.save(rotated)
    doc.close()

    compare_a = folder / "compare_a.pdf"
    compare_changed = folder / "compare_changed.pdf"
    for path, second_text in ((compare_a, "Station value 123 followed by unchanged observation details recorded across the daily climate report 2026."),
                              (compare_changed, "Station value 128 followed by unchanged observation details recorded across the daily climate report 2026.")):
        doc = pymupdf.open()
        page = doc.new_page(width=400, height=300)
        page.insert_text((50, 60), "First page unchanged")
        page = doc.new_page(width=400, height=300)
        page.insert_textbox(pymupdf.Rect(50, 60, 350, 200), second_text)
        doc.save(path)
        doc.close()

    rotated_compare_a = folder / "rotated_compare_a.pdf"
    rotated_compare_changed = folder / "rotated_compare_changed.pdf"
    for path, text in ((rotated_compare_a, "Rotated value 123 followed by unchanged observation details recorded across the daily climate report 2026."),
                       (rotated_compare_changed, "Rotated value 128 followed by unchanged observation details recorded across the daily climate report 2026.")):
        doc = pymupdf.open()
        page = doc.new_page(width=400, height=300)
        page.insert_textbox(pymupdf.Rect(45, 80, 350, 240), text)
        page.set_rotation(90)
        doc.save(path)
        doc.close()

    rotation_mismatch_a = folder / "rotation_mismatch_a.pdf"
    rotation_mismatch_b = folder / "rotation_mismatch_b.pdf"
    for path, text, rotation in ((rotation_mismatch_a, "Rotation mismatch value 123 followed by unchanged observations across the climate report." , 90),
                                (rotation_mismatch_b, "Rotation mismatch value 128 followed by unchanged observations across the climate report.", 0)):
        doc = pymupdf.open()
        page = doc.new_page(width=400, height=300)
        page.insert_textbox(pymupdf.Rect(45, 80, 350, 240), text)
        page.set_rotation(rotation)
        doc.save(path)
        doc.close()

    return {"pdf": pdf, "threshold": threshold, "rotated": rotated, "compare_a": compare_a,
            "compare_changed": compare_changed,
            "rotated_compare_a": rotated_compare_a,
            "rotated_compare_changed": rotated_compare_changed,
            "rotation_mismatch_a": rotation_mismatch_a,
            "rotation_mismatch_b": rotation_mismatch_b}
