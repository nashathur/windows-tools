from pathlib import Path

import pymupdf
import pikepdf


def build_fixtures(folder):
    folder = Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    result = {}
    for key, prefix, count in (("main", "A", 4), ("other", "B", 3)):
        path = folder / f"{key}.pdf"
        doc = pymupdf.open()
        for i in range(count):
            page = doc.new_page(width=300, height=400)
            page.insert_text((30, 50), f"{prefix} page {i + 1}", fontsize=16)
        doc.set_toc([[1, f"{prefix} first", 1], [1, f"{prefix} last", count]])
        if key == "main":
            doc[0].insert_link({"kind": pymupdf.LINK_GOTO, "from": pymupdf.Rect(20, 80, 120, 100), "page": 2,
                                "to": pymupdf.Point(30, 30)})
            widget = pymupdf.Widget()
            widget.field_type = pymupdf.PDF_WIDGET_TYPE_TEXT
            widget.field_name = "station"
            widget.field_value = "alpha"
            widget.rect = pymupdf.Rect(30, 120, 180, 145)
            doc[0].add_widget(widget)
            doc.set_page_labels([{"startpage": 0, "prefix": "R", "style": "D", "firstpagenum": 1}])
        doc.save(path)
        doc.close()
        result[key] = path
    decorated = folder / "features.pdf"
    with pikepdf.open(result["main"]) as pdf:
        pdf.Root["/OpenAction"] = pikepdf.Dictionary({"/S": pikepdf.Name("/JavaScript"), "/JS": pikepdf.String("app.alert('x')")})
        tree = pdf.make_indirect(pikepdf.Dictionary({"/Type": pikepdf.Name("/StructTreeRoot"), "/K": pikepdf.Array()}))
        pdf.Root["/StructTreeRoot"] = tree
        pdf.Root["/MarkInfo"] = pikepdf.Dictionary({"/Marked": True})
        pdf.attachments["station.txt"] = b"station attachment"
        pdf.save(decorated)
    result["features"] = decorated
    tagged = folder / "tagged.pdf"
    with pikepdf.open(result["main"]) as pdf:
        root = pdf.make_indirect(pikepdf.Dictionary({"/Type": pikepdf.Name("/StructTreeRoot"), "/ParentTreeNextKey": 4}))
        elements = pikepdf.Array()
        for index, page in enumerate(pdf.pages):
            page.obj["/StructParents"] = index
            element = pdf.make_indirect(pikepdf.Dictionary({
                "/Type": pikepdf.Name("/StructElem"), "/S": pikepdf.Name("/P"),
                "/P": root, "/Pg": page.obj, "/K": index,
            }))
            elements.append(element)
        root["/K"] = elements
        root["/ParentTree"] = pdf.make_indirect(pikepdf.Dictionary({
            "/Nums": pikepdf.Array([value for index in range(len(pdf.pages))
                                    for value in (index, pikepdf.Array([elements[index]]))])
        }))
        pdf.Root["/StructTreeRoot"] = root
        pdf.Root["/MarkInfo"] = pikepdf.Dictionary({"/Marked": True})
        pdf.save(tagged)
    result["tagged"] = tagged

    annotated = folder / "annotated.pdf"
    with pymupdf.open(result["main"]) as doc:
        doc[0].add_text_annot((60, 180), "note")
        doc.save(annotated)
    result["annotated"] = annotated

    collision = folder / "collision.pdf"
    with pymupdf.open(result["main"]) as doc:
        widget = pymupdf.Widget()
        widget.field_type = pymupdf.PDF_WIDGET_TYPE_TEXT
        widget.field_name = "station#2"
        widget.field_value = "beta"
        widget.rect = pymupdf.Rect(30, 120, 180, 145)
        doc[1].add_widget(widget)
        doc.save(collision)
    result["collision"] = collision
    return result
