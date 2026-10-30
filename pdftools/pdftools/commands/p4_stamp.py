"""Watermarks, page numbers, headers/footers, and annotation commands."""
import math
from pathlib import Path

import pymupdf

from .. import coords, textmatch
from ..check import OperationResult, Placement, ExpectedChanges
from ..errors import PdfError
from ..pages import parse_page_ranges
from ..pipeline import open_input, write_pdf
from ..registry import command


def _add_arguments(parser, name):
    parser.add_argument("file", type=Path)
    if name == "watermark":
        parser.add_argument("--text"); parser.add_argument("--image", type=Path)
        parser.add_argument("--position", default="center"); parser.add_argument("--angle", type=float, default=0)
        parser.add_argument("--opacity", type=float, default=.25); parser.add_argument("--fontsize", type=float, default=48)
        parser.add_argument("--under", action="store_true"); parser.add_argument("--pages", default="1-")
    elif name == "numbers":
        parser.add_argument("--format", default="{n}"); parser.add_argument("--start", type=int, default=1)
        parser.add_argument("--pages", default="1-"); parser.add_argument("--margin", type=float, default=12)
        parser.add_argument("--fontsize", type=float, default=10); parser.add_argument("--position", default="bottom-center")
    elif name == "headfoot":
        parser.add_argument("--header"); parser.add_argument("--footer"); parser.add_argument("--margin", type=float, default=12)
        parser.add_argument("--fontsize", type=float, default=9)
    elif name == "annotate":
        group = parser.add_mutually_exclusive_group()
        for option in ("highlight", "underline", "strike", "regex", "note", "textbox", "box", "arrow", "list", "remove"):
            group.add_argument("--" + option, nargs="?", const=True)
        parser.add_argument("--at"); parser.add_argument("--rect", nargs=5, type=float)
        parser.add_argument("--color", nargs=3, type=float, default=(1, 0, 0)); parser.add_argument("--pages", default="1-")
        parser.add_argument("--ids")
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--password-file", type=Path); parser.add_argument("--password-env")
    parser.add_argument("--owner-password-file", type=Path); parser.add_argument("--owner-password-env")
    parser.add_argument("--ignore-restrictions", action="store_true"); parser.add_argument("--break-signature", action="store_true")


def add_arguments_watermark(parser): _add_arguments(parser,"watermark")
def add_arguments_numbers(parser): _add_arguments(parser,"numbers")
def add_arguments_headfoot(parser): _add_arguments(parser,"headfoot")
def add_arguments_annotate(parser): _add_arguments(parser,"annotate")


def _spec(name):
    from ..registry import get
    return get(name)


def _annot_policy(source,output,operation):
    from ..check import compare
    policy="annot_remove" if operation and operation.annotation_delta is not None and operation.annotation_delta<0 else "annot_add"
    return compare(source,output,policy,operation)


def _anchor(page, position, margin):
    r = page.rect
    m = coords.mm_to_points(margin)
    positions = {"top-left": (r.x0+m, r.y0+m), "top-center": (r.width/2, r.y0+m),
                 "top-right": (r.x1-m, r.y0+m), "middle-left": (r.x0+m, r.height/2),
                 "center": (r.width/2, r.height/2), "middle-right": (r.x1-m, r.height/2),
                 "bottom-left": (r.x0+m, r.y1-m), "bottom-center": (r.width/2, r.y1-m),
                 "bottom-right": (r.x1-m, r.y1-m)}
    if position not in positions:
        raise PdfError("BAD_ARG", f"unknown position: {position}", "choose one of the nine page anchors")
    return pymupdf.Point(positions[position])


def _base_op(doc, pages):
    return OperationResult(pages=[[Placement(None, i)] for i in range(doc.page_count)],
                           expected=ExpectedChanges(regions=[{"page": i+1, "rect": r}
                           for i, values in pages.items() for r in (values if isinstance(values,list) else [values])]))


def _anchored_rect(page, position, width, height, margin=0):
    anchor=_anchor(page,position,margin);r=page.rect
    x0=anchor.x-width/2 if "left" not in position else anchor.x
    x1=x0+width
    if "right" in position: x0=anchor.x-width;x1=anchor.x
    y0=anchor.y-height/2 if not position.startswith("top-") and not position.startswith("bottom-") else anchor.y
    y1=y0+height
    if position.startswith("bottom-"): y0=anchor.y-height;y1=anchor.y
    return pymupdf.Rect(x0,y0,x1,y1) & r


def _validate_template(value,allowed):
    import string
    try:
        fields={field for _,field,_,_ in string.Formatter().parse(value) if field is not None}
    except ValueError as exc:
        raise PdfError("BAD_ARG",f"invalid stamp template: {exc}","use only supported named fields")
    unknown=fields-set(allowed)
    if unknown:raise PdfError("BAD_ARG",f"unknown stamp field: {sorted(unknown)[0]}",f"use {', '.join(sorted(allowed))}")


@command("watermark", 4, "pdf", "pdf", "_watermarked", True, "stamp")
def watermark(args, context):
    path = args.file.resolve()
    if bool(args.text) == bool(args.image):
        raise PdfError("BAD_ARG", "give exactly one of --text or --image", "choose text or image watermark")
    if not 0 <= args.opacity <= 1 or args.fontsize <= 0:
        raise PdfError("BAD_ARG", "opacity or font size is outside its valid range", "use opacity 0-1 and a positive font size")
    def writer(doc, target):
        selected = set(parse_page_ranges(args.pages, doc.page_count)); regions = {}
        image_stream = None
        if args.image:
            from PIL import Image
            import io
            im = Image.open(args.image).convert("RGBA")
            if args.angle:
                im = im.rotate(args.angle, expand=True, resample=Image.Resampling.BICUBIC)
            alpha = im.getchannel("A").point(lambda x: round(x * args.opacity))
            im.putalpha(alpha)
            buf = io.BytesIO(); im.save(buf, format="PNG"); image_stream = buf.getvalue()
        for i, page in enumerate(doc):
            if i not in selected: continue
            center = _anchor(page, args.position, 0)
            if image_stream:
                size = min(page.rect.width, page.rect.height) * .35
                rect = _anchored_rect(page,args.position,size,size)
                page.insert_image(rect, stream=image_stream, overlay=not args.under)
            else:
                text = args.text
                width = pymupdf.get_text_length(text, fontname="helv", fontsize=args.fontsize)
                box=_anchored_rect(page,args.position,width+2*args.fontsize,args.fontsize*1.7)
                point = pymupdf.Point(box.x0+args.fontsize, box.y1-args.fontsize/3)
                page.insert_text(point, text, fontsize=args.fontsize, fontname="helv",
                                       color=(0.5,0.5,0.5), fill_opacity=args.opacity,
                                       morph=(center, pymupdf.Matrix(args.angle)), overlay=not args.under)
                half_w=(width*abs(math.cos(math.radians(args.angle))+args.fontsize*abs(math.sin(math.radians(args.angle))/max(width,1))))/2
                half_h=(width*abs(math.sin(math.radians(args.angle)))+args.fontsize*abs(math.cos(math.radians(args.angle))))/2+args.fontsize
                box=pymupdf.Rect(center.x-half_w,center.y-half_h,center.x+half_w,center.y+half_h)
            regions[i] = tuple(coords.pymupdf_to_display(page, rect if image_stream else box))
        doc.save(target)
        return _base_op(doc, regions)
    return write_pdf(path, _spec("watermark"), args, context, writer)


@command("numbers", 4, "pdf", "pdf", "_numbered", True, "stamp")
def numbers(args, context):
    path = args.file.resolve()
    _validate_template(args.format,{"n","total","label"})
    if args.fontsize<=0:raise PdfError("BAD_ARG","font size must be positive","use a positive --fontsize")
    def writer(doc, target):
        selected = set(parse_page_ranges(args.pages, doc.page_count)); regions = {}
        number = 0
        for i, page in enumerate(doc):
            if i not in selected: continue
            value = args.format.format(n=args.start+number, total=doc.page_count, label=page.get_label() or str(i+1))
            number += 1
            point = _anchor(page, args.position, args.margin)
            rect = pymupdf.Rect(point.x-100, point.y-args.fontsize, point.x+100, point.y+args.fontsize)
            remaining=page.insert_textbox(rect, value, fontsize=args.fontsize, fontname="helv", align=1)
            if remaining<0: raise PdfError("BAD_ARG","number text does not fit its box","reduce --fontsize or shorten --format")
            regions[i] = tuple(coords.pymupdf_to_display(page, rect))
        doc.save(target); return _base_op(doc, regions)
    return write_pdf(path, _spec("numbers"), args, context, writer)


@command("headfoot", 4, "pdf", "pdf", "_headfoot", True, "stamp")
def headfoot(args, context):
    path = args.file.resolve()
    if not args.header and not args.footer:
        raise PdfError("BAD_ARG", "give --header or --footer", "provide text for one or both positions")
    for template in (args.header,args.footer):
        if template:_validate_template(template,{"n","total","date","file"})
    if args.fontsize<=0:raise PdfError("BAD_ARG","font size must be positive","use a positive --fontsize")
    import datetime
    def writer(doc, target):
        regions = {}; date = datetime.date.today().isoformat()
        for i, page in enumerate(doc):
            parts = []
            for value, y in ((args.header, coords.mm_to_points(args.margin)),
                             (args.footer, page.rect.height-coords.mm_to_points(args.margin))):
                if not value: continue
                text = value.format(n=i+1,total=doc.page_count,date=date,file=path.name)
                rect = pymupdf.Rect(24,y-args.fontsize,page.rect.width-24,y+args.fontsize)
                remaining=page.insert_textbox(rect,text,fontsize=args.fontsize,fontname="helv",align=1)
                if remaining<0: raise PdfError("BAD_ARG","header or footer text does not fit its box","reduce --fontsize or shorten the text")
                parts.append(tuple(coords.pymupdf_to_display(page,rect)))
            if parts:
                regions[i] = parts
        doc.save(target); return _base_op(doc, regions)
    return write_pdf(path, _spec("headfoot"), args, context, writer)


@command("annotate", 4, "pdf", "pdf", "_annotated", True, _annot_policy)
def annotate(args, context):
    path = args.file.resolve()
    if args.list:
        with open_input(path, context.password or context.owner_password) as doc:
            return {"type":"report","command":"annotate","input":str(path),"verdict":"report",
                    "data":[{"page":i+1,"annotations":[{"xref":a.xref,"type":a.type[1],"content":a.info.get("content","")}
                    for a in (page.annots() or [])]} for i,page in enumerate(doc)]}
    if args.remove is not None:
        try:
            with open_input(path,context.password or context.owner_password) as source_doc:
                pages=set(parse_page_ranges(args.pages, source_doc.page_count))
        except Exception as exc:
            raise PdfError("BAD_ARG",f"invalid page selection: {exc}","use --pages with valid 1-based page ranges") from exc
        try:
            ids = None if args.remove in ("all",True) else {int(x) for x in (args.ids or str(args.remove)).split(",")}
            if ids is not None and (not ids or any(x<1 for x in ids)): raise ValueError("annotation IDs must be positive integers")
        except Exception as exc:
            raise PdfError("BAD_ARG",f"invalid annotation IDs: {exc}","use --remove all or comma-separated positive IDs") from exc
        def remove_writer(doc,target):
            count=0
            for i,page in enumerate(doc):
                if i not in pages: continue
                for annot in list(page.annots() or []):
                    if ids is None or annot.xref in ids:
                        page.delete_annot(annot); count+=1
            doc.save(target)
            return OperationResult(annotation_delta=-count)
        return write_pdf(path,_spec("annotate"),args,context,remove_writer,operation=OperationResult(),security=False)
    kinds=[("highlight",args.highlight),("underline",args.underline),("strike",args.strike),("regex",args.regex)]
    selected=[(name,value) for name,value in kinds if value not in (None,False)]
    if len(selected)>1 or not (selected or args.note is not None or args.textbox is not None or args.box is not None or args.arrow is not None):
        raise PdfError("BAD_ARG","choose one annotation action","choose one text mark, --note, --textbox, --box, or --arrow")
    if selected and not str(selected[0][1]).strip():
        raise PdfError("BAD_ARG","annotation text or pattern is empty","provide non-empty text or a regular expression")
    def writer(doc,target):
        pages=set(parse_page_ranges(args.pages,doc.page_count)); added=0
        action,value=selected[0] if selected else (None,None)
        if action:
            if action=="regex":
                try: __import__("re").compile(value)
                except Exception as exc: raise PdfError("BAD_ARG",f"invalid regular expression: {exc}","provide a valid regex pattern") from exc
                found=textmatch.find_matches(doc,value,regex=True,pages=pages)
            else:
                matches=[]
                for page_no in sorted(pages):
                    for quad in doc[page_no].search_for(value,quads=True):
                        matches.append({"page":page_no+1,"quads":[quad]})
                found={"matches":matches}
            if not found["matches"]:
                raise PdfError("BAD_ARG","no text matches the annotation pattern","choose text present on the selected pages")
            for item in found["matches"]:
                page=doc[item["page"]-1]
                for quad in item["quads"]:
                    annot=(page.add_highlight_annot(quad) if action in {"highlight","regex"} else
                           page.add_underline_annot(quad) if action=="underline" else page.add_strikeout_annot(quad))
                    annot.set_colors(stroke=tuple(args.color)); annot.update(); added+=1
        elif args.note is not None:
            try:
                p,x,y=args.at.split(","); page_value=float(p); page_no=int(page_value)
                if page_value!=page_no or not 1<=page_no<=doc.page_count: raise ValueError("page number out of range")
                page=doc[page_no-1]
                point=coords.display_to_pymupdf(page,(float(x),float(y),float(x),float(y)))[:2]
            except Exception as exc: raise PdfError("BAD_ARG","invalid --at coordinate","use an integral page,x_mm,y_mm within the document") from exc
            doc[page_no-1].add_text_annot(point,args.note); added=1
        elif args.textbox is not None:
            try:
                p,x0,y0,x1,y1=args.rect; page_no=int(p)
                if p!=page_no or not 1<=page_no<=doc.page_count: raise ValueError("page number out of range")
                page=doc[page_no-1]; rect=coords.display_to_pymupdf(page,(x0,y0,x1,y1))
            except Exception as exc: raise PdfError("BAD_ARG","invalid --rect","use an integral page,x0,y0,x1,y1 in displayed mm") from exc
            page.add_freetext_annot(rect,args.textbox); added=1
        elif args.box is not None or args.arrow is not None:
            try:
                vals=[float(x) for x in str(args.box or args.arrow).split(",")]; p=int(vals[0])
                if vals[0]!=p or not 1<=p<=doc.page_count: raise ValueError("page number out of range")
                page=doc[p-1]
                rect=coords.display_to_pymupdf(page,vals[1:5])
            except Exception: raise PdfError("BAD_ARG","invalid geometry","use page,x0,y0,x1,y1 in displayed mm")
            if args.box is not None: page.add_rect_annot(rect)
            else:
                annot=page.add_line_annot(rect.tl,rect.br)
                annot.set_line_ends(pymupdf.PDF_ANNOT_LE_NONE,pymupdf.PDF_ANNOT_LE_OPEN_ARROW)
                annot.update()
            added=1
        doc.save(target); return OperationResult(annotation_delta=added)
    return write_pdf(path,_spec("annotate"),args,context,writer,operation=OperationResult())
