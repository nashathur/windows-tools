from pathlib import Path

import pikepdf
import pymupdf
from PIL import Image, ImageDraw, ImageFont
import importlib.util


def build_fixtures(folder):
    folder=Path(folder); folder.mkdir(parents=True,exist_ok=True)
    p2_path=Path(__file__).with_name("p2.py")
    spec=importlib.util.spec_from_file_location("p7_p2_fixtures",p2_path)
    p2=importlib.util.module_from_spec(spec);spec.loader.exec_module(p2)
    result=dict(p2.build_fixtures(folder/"shared"))
    base=folder/"base.pdf"
    doc=pymupdf.open(); page=doc.new_page(width=300,height=400)
    page.insert_text((35,60),"Ada Lovelace met Ada Lovelace")
    page.insert_text((35,100),"Second line")
    doc.set_toc([[1,"Ada Lovelace",1]])
    doc.save(base);doc.close()
    active=folder/"active.pdf"
    with pikepdf.open(base) as pdf:
        js=pikepdf.Dictionary({"/S":pikepdf.Name("/JavaScript"),"/JS":pikepdf.String("app.alert('x')")})
        pdf.Root["/OpenAction"]=pikepdf.Dictionary({"/S":pikepdf.Name("/GoTo"),"/Next":pikepdf.Array([js])})
        pdf.docinfo["/Title"]="fixture metadata"
        pdf.save(active)
    scan_image=folder/"scan.png"
    image=Image.new("RGB",(1800,500),"white");draw=ImageDraw.Draw(image)
    font=ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",96)
    draw.text((50,150),"Sutamto Pranudyo Surabaya",font=font,fill="black");image.save(scan_image)
    scanned=folder/"scanned.pdf";doc=pymupdf.open();page=doc.new_page(width=432,height=120)
    page.insert_image(page.rect,filename=str(scan_image));doc.save(scanned);doc.close()
    mixed=folder/"selectable-header-scan.pdf";doc=pymupdf.open();page=doc.new_page(width=432,height=150)
    page.insert_text((20,20),"Selectable header")
    page.insert_image(pymupdf.Rect(0,30,432,150),filename=str(scan_image));doc.save(mixed);doc.close()
    ocr_layer=folder/"ocr-layer.pdf";doc=pymupdf.open();page=doc.new_page(width=432,height=150)
    page.insert_image(pymupdf.Rect(0,0,432,120),filename=str(scan_image));page.insert_text((15,140),"Sutamto",fontname="helv",fontsize=16)
    doc.save(ocr_layer);doc.close()
    with pikepdf.open(ocr_layer) as pdf:
        for font in pdf.pages[0].obj["/Resources"]["/Font"].values():
            if "/BaseFont" in font: font["/BaseFont"]=pikepdf.Name("/GlyphLessFont")
        pdf.save(ocr_layer.with_suffix(".glyph.pdf"))
    ocr_layer.with_suffix(".glyph.pdf").replace(ocr_layer)
    repeated_scan=folder/"repeated-scan.pdf";image=Image.new("RGB",(1800,800),"white");draw=ImageDraw.Draw(image)
    font=ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",96)
    draw.text((50,100),"Ada Lovelace",font=font,fill="black");draw.text((50,450),"Ada Lovelace",font=font,fill="black")
    repeated_png=folder/"repeated-scan.png";image.save(repeated_png)
    doc=pymupdf.open();page=doc.new_page(width=432,height=192);page.insert_image(page.rect,filename=str(repeated_png))
    doc.save(repeated_scan);doc.close()
    rotated_target=folder/"rotated-target.pdf";doc=pymupdf.open(result["rotated"])
    doc[0].insert_text((100,100),"InsideCrop")
    doc[0].insert_image(pymupdf.Rect(40,20,150,70),filename=str(scan_image))
    name_pixels=draw.textbbox((50,150),"Sutamto",font=font)
    image_info=doc[0].get_image_info(xrefs=True)[-1]
    image_matrix=pymupdf.Matrix(image_info["transform"])
    name_unit=pymupdf.Rect(name_pixels[0]/1800,name_pixels[1]/500,
                           name_pixels[2]/1800,name_pixels[3]/500)
    rotated_name_box=tuple(name_unit*image_matrix)
    doc.save(rotated_target);doc.close()
    hidden_off=folder/"hidden-off.pdf"
    doc=pymupdf.open(result["optional_content"]);doc[0].insert_text((30,100),"hidden phrase",render_mode=3);doc.save(hidden_off);doc.close()
    with pikepdf.open(hidden_off) as pdf:
        config=pdf.Root["/OCProperties"]["/D"];group=pdf.Root["/OCProperties"]["/OCGs"][0]
        config["/OFF"]=pikepdf.Array([group]);config["/ON"]=pikepdf.Array()
        pdf.save(hidden_off.with_suffix(".tmp.pdf"))
    hidden_off.with_suffix(".tmp.pdf").replace(hidden_off)
    result.update({"base":base,"active":active,"scanned":scanned,"hidden_off":hidden_off,
                   "mixed_scan":mixed,"ocr_layer":ocr_layer,"repeated_scan":repeated_scan})
    result["rotated_target"]=rotated_target
    result["rotated_target_name_box"]=rotated_name_box
    return result
