"""Small deterministic fixtures for phase 5; no toolkit command is used."""
from pathlib import Path

import numpy as np
import pymupdf
from PIL import Image, ImageDraw, ImageFont
from tests.fixtures import sans_font


def build_fixtures(folder):
    folder=Path(folder);folder.mkdir(parents=True,exist_ok=True)
    photo=folder/"photo.pdf";rng=np.random.default_rng(5)
    pixels=rng.integers(0,256,(1100,1100,3),dtype=np.uint8)
    im=Image.fromarray(pixels,"RGB");image=folder/"photo.jpg";im.save(image,quality=96)
    doc=pymupdf.open();page=doc.new_page(width=595,height=842)
    page.insert_image(pymupdf.Rect(25,25,570,817),filename=str(image));page.insert_text((35,45),"Known phase five words")
    doc.save(photo);doc.close()
    lossless=folder/"lossless.pdf";im=Image.new("RGB",(80,80),"white");d=ImageDraw.Draw(im);d.rectangle((4,4,75,75),fill="black");png=folder/"chart.png";im.save(png)
    doc=pymupdf.open();page=doc.new_page(width=300,height=300);page.insert_image(page.rect,filename=str(png));doc.save(lossless);doc.close()
    small=folder/"small.pdf";doc=pymupdf.open();doc.new_page();doc.ez_save(small,garbage=4,deflate=True,use_objstms=True);doc.close()
    scan=folder/"scan.pdf";im=Image.new("RGB",(900,300),"white");d=ImageDraw.Draw(im)
    font=ImageFont.truetype(sans_font(),48)
    d.text((25,45),"Hello world",font=font,fill="black");d.text((25,140),"Selamat pagi",font=font,fill="black");scanpng=folder/"scan.png";im.save(scanpng)
    doc=pymupdf.open();p=doc.new_page(width=216,height=72);p.insert_image(p.rect,filename=str(scanpng));doc.save(scan);doc.close()
    def text_scan(name,text,dpi,tilt=0):
        source=folder/f"{name}-source.pdf";doc=pymupdf.open();page=doc.new_page(width=612,height=792)
        page.insert_text((30,180),text,fontfile=str(sans_font()),fontsize=28)
        doc.save(source);doc.close()
        with pymupdf.open(source) as rendered:
            pix=rendered[0].get_pixmap(matrix=pymupdf.Matrix(dpi/72,dpi/72),alpha=False)
            image=Image.frombytes("RGB",(pix.width,pix.height),pix.samples)
        if tilt:image=image.rotate(tilt,expand=False,fillcolor="white")
        image_path=folder/f"{name}.png";image.save(image_path)
        target=folder/f"{name}.pdf";doc=pymupdf.open();page=doc.new_page(width=612,height=792)
        page.insert_image(page.rect,filename=str(image_path));doc.save(target);doc.close()
        return target
    numbers=text_scan("numbers","-2.3  1,250.00  -0.75  3.14159  -1,000.5",300)
    tilted=text_scan("tilted","Tilted words should remain readable",300,-4)
    lowdpi=text_scan("lowdpi","Low resolution quality warning",100)
    even=text_scan("even","Straight evenly lit words",300)
    return {"photo":photo,"lossless":lossless,"small":small,"scan":scan,
            "numbers":numbers,"tilted":tilted,"lowdpi":lowdpi,"even":even}
