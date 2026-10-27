"""Deterministic phase 4 fixtures, built without toolkit commands."""
from pathlib import Path

import numpy as np
import pikepdf
import pymupdf


def build_fixtures(folder):
    folder=Path(folder);folder.mkdir(parents=True,exist_ok=True)
    base=folder/"phase4.pdf"
    doc=pymupdf.open()
    for i in range(8):
        page=doc.new_page(width=300,height=400)
        page.insert_text((30,55),f"PHASE FOUR PAGE {i+1}",fontsize=16)
        page.insert_text((30,90),f"climate token{i+1} repeated line",fontsize=11)
        page.draw_rect(pymupdf.Rect(30,120,270,350),color=(.1,.2,.8),width=1)
    doc.set_toc([[1,"Chapter one",1],[1,"Chapter two",5]])
    doc[0].insert_link({"kind":pymupdf.LINK_URI,"from":pymupdf.Rect(30,40,160,60),"uri":"https://example.invalid"})
    doc[0].add_text_annot((200,100),"fixture note")
    doc.save(base);doc.close()
    with pikepdf.open(base,allow_overwriting_input=True) as pdf:
        pdf.docinfo["/CustomFixture"]="phase4"
        with pdf.open_metadata(set_pikepdf_as_editor=False) as meta:
            meta["dc:title"]="Fixture title"
        pdf.save(base)
    color=folder/"color.pdf"
    doc=pymupdf.open();page=doc.new_page(width=300,height=400)
    page.draw_rect(pymupdf.Rect(20,20,280,380),color=(.4,.5,.55),fill=(.4,.5,.55));page.insert_text((30,80),"COLOR PAGE",fontsize=18,color=(.35,.5,.55))
    doc.save(color);doc.close()
    saturated=folder/"saturated.pdf";doc=pymupdf.open();page=doc.new_page(width=300,height=400)
    page.draw_rect(pymupdf.Rect(20,20,280,380),color=(0,0,1),fill=(0,0,1));page.insert_text((30,80),"SATURATED BLUE",fontsize=18,color=(0,0,1))
    doc.save(saturated);doc.close()
    blank=folder/"blank-pages.pdf"
    doc=pymupdf.open()
    for text in ("KEEP ONE",None,"KEEP TWO"):
        page=doc.new_page(width=300,height=400)
        if text:page.insert_text((30,50),text)
    doc.save(blank);doc.close()
    all_blank=folder/"all-blank.pdf";doc=pymupdf.open();doc.new_page(width=300,height=400);doc.new_page(width=300,height=400);doc.save(all_blank);doc.close()
    damaged=folder/"damaged.pdf";damaged.write_bytes(b"%PDF-1.7\nnot a PDF")
    forms=folder/"forms.pdf";doc=pymupdf.open();page=doc.new_page(width=300,height=400)
    widget=pymupdf.Widget();widget.field_name="Fixture field";widget.field_type=pymupdf.PDF_WIDGET_TYPE_TEXT
    widget.field_value="fixture value";widget.rect=pymupdf.Rect(30,100,180,130);page.add_widget(widget);widget.update()
    doc.save(forms);doc.close()
    with pikepdf.open(forms,allow_overwriting_input=True) as pdf:
        for page in pdf.pages:
            for annot in page.get("/Annots",[]):
                if annot.get("/Subtype")==pikepdf.Name("/Widget") and "/AP" in annot: del annot["/AP"]
        if "/AcroForm" in pdf.Root: pdf.Root.AcroForm.NeedAppearances=True
        pdf.save(forms)
    return {"base":base,"color":color,"saturated":saturated,"blank":blank,"all_blank":all_blank,"damaged":damaged,"forms":forms}
