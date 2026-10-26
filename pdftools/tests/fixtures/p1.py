from pathlib import Path
import hashlib

import pikepdf
import pymupdf


def build_fixtures(folder):
    folder = Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    plain = folder / "plain.pdf"
    doc = pymupdf.open()
    page = doc.new_page(width=300, height=400)
    page.insert_text((30, 50), "Climate report text")
    page = doc.new_page(width=300, height=400)
    page.insert_text((30, 50), "Second page")
    doc.save(plain)
    doc.close()
    encrypted = folder / "encrypted.pdf"
    doc = pymupdf.open(plain)
    doc.save(encrypted, encryption=pymupdf.PDF_ENCRYPT_AES_256, owner_pw="owner-pass", user_pw="user-pass",
             permissions=int(pymupdf.PDF_PERM_ACCESSIBILITY))
    doc.close()
    restricted = folder / "restricted.pdf"
    doc = pymupdf.open(plain)
    doc.save(restricted, encryption=pymupdf.PDF_ENCRYPT_AES_256, owner_pw="owner-pass", user_pw="",
             permissions=int(pymupdf.PDF_PERM_ACCESSIBILITY))
    doc.close()
    scanned = folder / "scanned.pdf"
    doc = pymupdf.open()
    doc.new_page(width=300, height=400)
    doc.save(scanned)
    doc.close()
    signed = folder / "signed.pdf"
    with pikepdf.open(plain) as pdf:
        field = pikepdf.Dictionary({"/FT": pikepdf.Name("/Sig"), "/T": pikepdf.String("Signature1"),
                                    "/V": pikepdf.Dictionary({"/Type": pikepdf.Name("/Sig"), "/Contents": pikepdf.String("x")})})
        acro = pdf.Root.get("/AcroForm", pikepdf.Dictionary({"/Fields": pikepdf.Array()}))
        acro["/Fields"].append(pdf.make_indirect(field))
        pdf.Root["/AcroForm"] = acro
        pdf.save(signed)
    javascript = folder / "javascript.pdf"
    with pikepdf.open(plain) as pdf:
        js = lambda: pikepdf.Dictionary({"/S": pikepdf.Name("/JavaScript"), "/JS": pikepdf.String("app.alert('x')")})
        pdf.Root["/OpenAction"] = pikepdf.Dictionary({"/S": pikepdf.Name("/GoTo"), "/Next": pikepdf.Array([js()])})
        pdf.pages[0].obj["/AA"] = pikepdf.Array([pikepdf.Dictionary({"/S": pikepdf.Name("/GoTo"), "/Next": pikepdf.Array([js()])})])
        pdf.pages[0].obj["/Annots"] = pikepdf.Array([pikepdf.Dictionary({"/Type": pikepdf.Name("/Annot"),
            "/Subtype": pikepdf.Name("/Text"), "/A": js(), "/AA": pikepdf.Array([js()])})])
        field = pikepdf.Dictionary({"/FT": pikepdf.Name("/Tx"), "/T": pikepdf.String("Field"), "/A": js(), "/AA": pikepdf.Array([js()])})
        pdf.Root["/AcroForm"] = pikepdf.Dictionary({"/Fields": pikepdf.Array([pdf.make_indirect(field)])})
        outline = pikepdf.Dictionary({"/Title": pikepdf.String("Outline"), "/A": js()})
        pdf.Root["/Outlines"] = pikepdf.Dictionary({"/First": pdf.make_indirect(outline), "/Last": outline, "/Count": 1})
        pdf.save(javascript)
    rotated = folder / "rotated.pdf"
    with pikepdf.open(plain) as pdf:
        page = pdf.pages[0]
        page.obj["/Rotate"] = 90
        page.obj["/CropBox"] = pikepdf.Array([20, 30, 270, 360])
        page.obj["/UserUnit"] = 2
        pdf.save(rotated)
    damaged = folder / "damaged.pdf"
    damaged.write_bytes(b"%PDF-1.7\nthis is not a valid PDF\n%%EOF")
    font_stream = folder / "font-stream.pdf"
    with pikepdf.open(plain) as pdf:
        page_obj = pdf.pages[0].obj
        resources = page_obj.get("/Resources", pikepdf.Dictionary())
        resources["/Font"] = pikepdf.Dictionary()
        font_data = pdf.make_stream(b"".join(hashlib.sha256(str(i).encode()).digest() for i in range(200)))
        descriptor = pdf.make_indirect(pikepdf.Dictionary({"/Type": pikepdf.Name("/FontDescriptor"),
            "/FontName": pikepdf.Name("/FakeFont"), "/Flags": 4, "/FontBBox": pikepdf.Array([0, 0, 1000, 1000]),
            "/ItalicAngle": 0, "/Ascent": 800, "/Descent": -200, "/CapHeight": 700, "/StemV": 80,
            "/FontFile2": font_data}))
        font = pdf.make_indirect(pikepdf.Dictionary({"/Type": pikepdf.Name("/Font"), "/Subtype": pikepdf.Name("/TrueType"),
            "/BaseFont": pikepdf.Name("/FakeFont"), "/FirstChar": 32, "/LastChar": 32,
            "/Widths": pikepdf.Array([500]), "/Encoding": pikepdf.Name("/WinAnsiEncoding"), "/FontDescriptor": descriptor}))
        resources["/Font"]["/Ffake"] = font
        page_obj["/Resources"] = resources
        pdf.save(font_stream)
    return {"plain": plain, "encrypted": encrypted, "restricted": restricted,
            "scanned": scanned, "signed": signed, "javascript": javascript, "rotated": rotated,
            "font_stream": font_stream, "damaged": damaged}


if __name__ == "__main__":
    import sys
    build_fixtures(sys.argv[1])
