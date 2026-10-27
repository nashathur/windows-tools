from pathlib import Path

import pymupdf


def build_fixtures(folder, bench=False):
    folder = Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    source = folder / "source.pdf"
    doc = pymupdf.open()
    page = doc.new_page(width=595, height=842)
    page.insert_text((40, 60), "Phase three A climate report")
    page.insert_text((40, 100), "Temperature rainfall wind")
    page = doc.new_page(width=595, height=842)
    page.insert_text((40, 60), "Second page with readable text")
    doc.set_metadata({"title": "Phase 3a fixture", "author": "Tests"})
    doc.save(source)
    doc.close()

    labeled = folder / "labeled.pdf"
    doc = pymupdf.open()
    for index in range(3):
        page = doc.new_page(width=300, height=400)
        page.insert_text((30, 40), f"Labeled page {index + 1}")
    doc.set_page_labels([{"startpage": 0, "prefix": "", "style": "r", "firstpagenum": 1}])
    doc.save(labeled)
    doc.close()

    encrypted = folder / "encrypted.pdf"
    doc = pymupdf.open(source)
    doc.save(encrypted, encryption=pymupdf.PDF_ENCRYPT_AES_256, owner_pw="owner-pass",
             user_pw="user-pass", permissions=0)
    doc.close()

    restricted = folder / "restricted.pdf"
    doc = pymupdf.open(source)
    doc.save(restricted, encryption=pymupdf.PDF_ENCRYPT_AES_256, owner_pw="owner-pass",
             user_pw="", permissions=0)
    doc.close()

    reordered = folder / "reordered.pdf"
    doc = pymupdf.open()
    page = doc.new_page(width=595, height=842)
    page.insert_text((40, 60), "Second page with readable text")
    page = doc.new_page(width=595, height=842)
    page.insert_text((40, 60), "Phase three A climate report")
    page.insert_text((40, 100), "Temperature rainfall wind")
    doc.save(reordered)
    doc.close()

    empty = folder / "empty.pdf"
    doc = pymupdf.open()
    doc.new_page(width=300, height=400)
    doc.save(empty)
    doc.close()

    image_source = folder / "image-source.pdf"
    doc = pymupdf.open()
    page = doc.new_page(width=300, height=400)
    page.draw_rect(pymupdf.Rect(30, 40, 150, 190), color=(0.1, 0.3, 0.8), fill=(0.8, 0.2, 0.1))
    page.draw_line((15, 260), (280, 310), color=(0, 0, 0), width=4)
    doc.save(image_source)
    doc.close()
    image_copy = folder / "image-copy.pdf"
    image_copy.write_bytes(image_source.read_bytes())
    image_duplicate_source = folder / "image-duplicate-source.pdf"
    doc = pymupdf.open()
    for _ in range(2):
        page = doc.new_page(width=300, height=400)
        page.draw_rect(pymupdf.Rect(30, 40, 150, 190), color=(0.1, 0.3, 0.8), fill=(0.8, 0.2, 0.1))
        page.draw_line((15, 260), (280, 310), color=(0, 0, 0), width=4)
    doc.save(image_duplicate_source)
    doc.close()

    folder_input = folder / "folder"
    folder_input.mkdir(exist_ok=True)
    (folder_input / source.name).write_bytes(source.read_bytes())
    (folder_input / empty.name).write_bytes(empty.read_bytes())
    (folder_input / restricted.name).write_bytes(restricted.read_bytes())
    result = {"source": source, "reordered": reordered, "empty": empty, "folder": folder_input,
              "image_source": image_source, "image_copy": image_copy,
              "image_duplicate_source": image_duplicate_source, "labeled": labeled, "encrypted": encrypted,
              "restricted": restricted}
    if bench:
        benchmark = folder / "benchmark-500.pdf"
        doc = pymupdf.open()
        for index in range(500):
            page = doc.new_page(width=595, height=842)
            page.insert_text((40, 60), f"Page {index + 1:03d} deterministic climate report")
            page.insert_text((40, 100), f"Humidity {index % 100} rainfall {index * 3} wind {index * 7}")
        doc.save(benchmark)
        doc.close()
        result["benchmark"] = benchmark
    return result
