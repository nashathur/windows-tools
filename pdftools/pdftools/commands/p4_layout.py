"""Page layout, color, and flattening operations."""
import math
from pathlib import Path

import pymupdf

from .. import coords
from ..check import OperationResult, Placement, ExpectedChanges, Features, Finding, normalize_words
from ..errors import PdfError
from ..pages import parse_page_ranges
from ..pipeline import open_input, write_pdf
from ..registry import command


def _add_arguments(parser,name):
    parser.add_argument("file",type=Path)
    if name=="flatten":
        parser.add_argument("--only",choices=("forms","annots")); parser.add_argument("--raster",type=int)
    elif name=="crop":
        parser.add_argument("--margins",nargs=4,type=float); parser.add_argument("--auto",action="store_true"); parser.add_argument("--pad",type=float,default=3); parser.add_argument("--pages",default="1-")
    elif name=="cut":
        group=parser.add_mutually_exclusive_group(required=True); group.add_argument("--v",type=float); group.add_argument("--h",type=float); group.add_argument("--grid")
        parser.add_argument("--rtl",action="store_true")
    elif name=="resize":
        parser.add_argument("--size",required=True); parser.add_argument("--scale",choices=("fit",)); parser.add_argument("--pad",action="store_true")
    elif name=="nup":
        parser.add_argument("--grid",type=int,choices=(2,4,8,16,32),required=True); parser.add_argument("--sheet"); parser.add_argument("--order",choices=("row","col"),default="row"); parser.add_argument("--gap",type=float,default=0); parser.add_argument("--border",action="store_true")
    elif name=="booklet":
        parser.add_argument("--sheet"); parser.add_argument("--flip",choices=("short","long"),default="short"); parser.add_argument("--binding",choices=("left","right"),default="left")
    elif name=="gray":
        parser.add_argument("--no-gs",action="store_true")
    parser.add_argument("--flatten-annots",action="store_true")
    parser.add_argument("-o","--output",type=Path)
    parser.add_argument("--password-file",type=Path); parser.add_argument("--password-env")
    parser.add_argument("--owner-password-file",type=Path); parser.add_argument("--owner-password-env")
    parser.add_argument("--ignore-restrictions",action="store_true"); parser.add_argument("--break-signature",action="store_true")


def add_arguments_flatten(parser): _add_arguments(parser,"flatten")
def add_arguments_crop(parser): _add_arguments(parser,"crop")
def add_arguments_cut(parser): _add_arguments(parser,"cut")
def add_arguments_resize(parser): _add_arguments(parser,"resize")
def add_arguments_nup(parser): _add_arguments(parser,"nup")
def add_arguments_booklet(parser): _add_arguments(parser,"booklet")
def add_arguments_gray(parser): _add_arguments(parser,"gray")


def _spec(name):
    from ..registry import get
    return get(name)


def _flatten_policy(source,output,operation):
    from ..check import compare
    policy="flatten_raster" if operation and operation.expected.text_removed==["__raster__"] else "flatten"
    return compare(source,output,policy,operation)


def _operation(doc, maps, dropped=None):
    return OperationResult(pages=maps,features=Features(dropped=dropped or {}))


def _dropped_feature_counts(doc):
    return {"links": sum(len(page.get_links()) for page in doc),
            "annotations": sum(sum(1 for _ in (page.annots() or [])) for page in doc),
            "widgets": sum(sum(1 for _ in (page.widgets() or [])) for page in doc)}


def _layout_source(doc,args):
    dropped=_dropped_feature_counts(doc)
    if args.flatten_annots:
        doc.bake(annots=True,widgets=True)
    return dropped


def _write_layout(path, name, args, context, writer):
    if getattr(args,"dry_run",False):
        import io
        with open_input(path,context.password or context.owner_password) as source:
            planned=writer(source,io.BytesIO())
        if not isinstance(planned,OperationResult): planned=OperationResult()
        result=write_pdf(path,_spec(name),args,context,lambda doc,target: planned,operation=planned)
        if result.get("type")=="result": result["pages"]=len(planned.pages)
        return result
    def source_writer(doc, target):
        if not args.flatten_annots:
            return writer(doc, target)
        baseline = Path(context.temp) / f"{name}-flattened-baseline.pdf"
        baked_path = Path(context.temp) / f"{name}-flattened-baked.pdf"
        doc.save(baseline, garbage=3, deflate=True)
        with pymupdf.open(baseline) as baked:
            baked.bake(annots=True, widgets=True)
            baked.save(baked_path, garbage=3, deflate=True)
        baked_path.replace(baseline)
        with pymupdf.open(baseline) as baked:
            result = writer(baked, target)
        if not isinstance(result, OperationResult):
            result = OperationResult()
        result.baseline = str(baseline)
        return result
    return write_pdf(path, _spec(name), args, context, source_writer)


def _layout_policy(source,output,operation):
    """Compare text and rendered tiles per source placement, including blank slots."""
    from collections import Counter
    import cv2
    import numpy as np
    from ..check import render_rgb, ssim_gray, _feature_counts
    findings=[]
    if len(output)!=len(operation.pages):
        return [Finding("invariant","PAGE_COUNT","layout output page count differs from the plan",
                        details={"expected":len(operation.pages),"actual":len(output)})]
    for out_no,raw in enumerate(operation.pages):
        placements=raw if isinstance(raw,(list,tuple)) else [raw]
        page=output[out_no]
        output_words=page.get_text("words")
        for placement in placements:
            if placement.source_page=="blank":
                if not placement.dest_rect: continue
                rect=pymupdf.Rect(placement.dest_rect)+(2,2,-2,-2)
                if page.get_textbox(rect).strip():
                    findings.append(Finding("invariant","BLANK_PLACEMENT_NOT_BLANK","blank layout slot contains text",out_no+1))
                    continue
                pixels=render_rgb(page,clip=rect,unrotated=True)
                if pixels.size and np.any(np.min(pixels,axis=2)<245):
                    findings.append(Finding("invariant","BLANK_PLACEMENT_NOT_BLANK","blank layout slot contains visible content",out_no+1))
                continue
            if placement.source_page=="generated" or not placement.dest_rect: continue
            source_page=source[int(placement.source_page)]
            dest=pymupdf.Rect(placement.dest_rect)
            a,b,c,d,e,f=placement.transform;det=a*d-b*c
            if abs(det)<1e-9: continue
            pts=[pymupdf.Point((d*(x-e)-c*(y-f))/det,(-b*(x-e)+a*(y-f))/det)
                 for x,y in ((dest.x0,dest.y0),(dest.x1,dest.y0),(dest.x0,dest.y1),(dest.x1,dest.y1))]
            source_region=pymupdf.Rect(min(p.x for p in pts),min(p.y for p in pts),max(p.x for p in pts),max(p.y for p in pts)) & source_page.rect
            expected=Counter(normalize_words(row[4] for row in source_page.get_text("words")
                                             if source_region.contains(pymupdf.Rect(row[:4]))))
            actual=Counter(normalize_words(row[4] for row in output_words if dest.contains(pymupdf.Rect(row[:4]))))
            missing={word:count-actual[word] for word,count in expected.items() if actual[word]<count}
            if missing:
                findings.append(Finding("invariant","PLACEMENT_TEXT_MISMATCH","source words are missing from their destination rectangle",
                                        out_no+1,{"missing":missing,"source_page":placement.source_page}))
            source_pixels=render_rgb(source_page,clip=source_region,unrotated=True)
            output_pixels=render_rgb(page,clip=dest,unrotated=True)
            if source_pixels.size and output_pixels.size:
                sx,sy=100/72,100/72
                affine=np.array([[a,c,(a*source_region.x0+c*source_region.y0+e-dest.x0)*sx],
                                 [b,d,(b*source_region.x0+d*source_region.y0+f-dest.y0)*sy]],dtype=np.float64)
                expected_pixels=cv2.warpAffine(source_pixels,affine,(output_pixels.shape[1],output_pixels.shape[0]),
                                               flags=cv2.INTER_LINEAR,borderMode=cv2.BORDER_CONSTANT,borderValue=(255,255,255))
                score=ssim_gray(expected_pixels,output_pixels)
                if score<.90:
                    findings.append(Finding("invariant","PLACEMENT_TILES_MISMATCH","visible content differs within a planned placement",
                                            out_no+1,{"ssim":round(score,4),"source_page":placement.source_page}))
    if operation.features.dropped:
        actual=_feature_counts(output);before=_feature_counts(source)
        for feature,count in operation.features.dropped.items():
            expected=before.get(feature,0)-count
            if actual.get(feature)==expected:
                findings.append(Finding("new_warning","FEATURES_DROPPED",f"{feature} count changed as requested",
                                        details={"source":before.get(feature,0),"dropped":count,"expected":expected,"output":actual.get(feature)}))
            else:
                findings.append(Finding("invariant","FEATURE_DROP_MISMATCH",f"{feature} count differs from planned drop",
                                        details={"expected":expected,"actual":actual.get(feature)}))
    return findings


@command("flatten",4,"pdf","pdf","_flattened",True,_flatten_policy)
def flatten(args,context):
    path=args.file.resolve()
    if args.raster is not None and args.raster<1:
        raise PdfError("BAD_ARG","raster DPI must be positive","use a positive DPI such as 200")
    def writer(doc,target):
        if args.raster:
            out=pymupdf.open();baseline_doc=pymupdf.open();maps=[]
            baseline=Path(context.temp)/"flatten-raster-baseline.pdf"
            for i,page in enumerate(doc):
                pix=page.get_pixmap(matrix=pymupdf.Matrix(args.raster/72,args.raster/72),alpha=False,annots=True)
                image=pymupdf.Pixmap(pix)
                expected_page=baseline_doc.new_page(width=page.rect.width,height=page.rect.height)
                expected_page.insert_image(expected_page.rect,stream=image.tobytes("png"))
                dest=out.new_page(width=page.rect.width,height=page.rect.height)
                dest.insert_image(dest.rect,stream=image.tobytes("png"))
                maps.append([Placement(None,i)])
            # Independently record the source rendered with annotations before writing the candidate.
            baseline_doc.save(baseline);baseline_doc.close();out.save(target);out.close()
            return OperationResult(pages=maps,baseline=str(baseline),expected=ExpectedChanges(text_removed=["__raster__"]))
        forms=args.only in (None,"forms"); annots=args.only in (None,"annots")
        dropped={}; regions=[]; field_values=[]
        if annots:
            for i,page in enumerate(doc):
                for annot in page.annots() or []:
                    dropped["annotations"]=dropped.get("annotations",0)+1
                    rect=coords.annotation_to_pymupdf(page,annot)
                    regions.append({"page":i+1,"rect":tuple(coords.pymupdf_to_display(page,rect))})
        if forms:
            for page in doc:
                for widget in page.widgets() or []:
                    dropped["widgets"]=dropped.get("widgets",0)+1
                    rect=coords.annotation_to_pymupdf(page,widget)
                    regions.append({"page":page.number+1,"rect":tuple(coords.pymupdf_to_display(page,rect))})
                    if widget.field_value:field_values.append(str(widget.field_value))
        acro=doc.xref_get_key(doc.pdf_catalog(),"AcroForm")
        need_type,need_value=doc.xref_get_key(doc.pdf_catalog(),"AcroForm/NeedAppearances")
        needs=need_type=="bool" and need_value.lower()=="true"
        for page in doc:
            for widget in page.widgets() or []:
                ap_type,ap_value=doc.xref_get_key(widget.xref,"AP")
                if forms and (needs or ap_type=="null" or not ap_value):
                    try: widget.update()
                    except Exception: pass
        doc.bake(annots=annots,widgets=forms)
        doc.save(target)
        if forms and field_values:
            with pymupdf.open(target) as check_doc:
                for value in field_values:
                    if not any(value in page.get_text() for page in check_doc):
                        raise PdfError("CHECK_FAILED",f"flattened field value is missing: {value}","check the field appearance before flattening")
        return OperationResult(pages=[[Placement(None,i)] for i in range(doc.page_count)],
                               features=Features(dropped=dropped),expected=ExpectedChanges(regions=regions))
    return write_pdf(path,_spec("flatten"),args,context,writer)


@command("crop",4,"pdf","pdf","_cropped",True,"crop")
def crop(args,context):
    path=args.file.resolve()
    if not args.auto and not args.margins:
        raise PdfError("BAD_ARG","give --margins or --auto","choose crop margins in mm or automatic content bounds")
    def writer(doc,target):
        chosen=set(parse_page_ranges(args.pages,doc.page_count)); maps=[]; cropboxes=[]
        for i,page in enumerate(doc):
            if i in chosen:
                if args.margins:
                    top,right,bottom,left=args.margins
                    displayed=coords.pymupdf_to_display(page,coords.cropbox_to_pymupdf(page,page.cropbox))
                    shown=pymupdf.Rect(displayed.x0+left,displayed.y0+top,displayed.x1-right,displayed.y1-bottom)
                    box=coords.display_to_pymupdf(page,shown)
                    box=coords.pymupdf_to_cropbox(page,box)
                else:
                    boxes=[pymupdf.Rect(item[1]) for item in page.get_bboxlog() if item[0] not in {"ignore-text","ignore-path","ignore-image"}]
                    rect=boxes[0] if boxes else page.rect
                    for content_box in boxes[1:]: rect |= content_box
                    shown=coords.pymupdf_to_display(page,rect)
                    display_page=coords.pymupdf_to_display(page,coords.cropbox_to_pymupdf(page,page.cropbox))
                    shown=(shown+(-args.pad,-args.pad,args.pad,args.pad)) & display_page
                    box=coords.display_to_pymupdf(page,shown)
                    box=coords.pymupdf_to_cropbox(page,box)
                if box.is_empty or box.width<=0 or box.height<=0: raise PdfError("BAD_ARG","crop box has no area","reduce the margins or padding")
                page.set_cropbox(box)
            cropboxes.append(tuple(page.cropbox)); maps.append([Placement(None,i)])
        doc.save(target); return OperationResult(pages=maps,cropboxes=cropboxes)
    return write_pdf(path,_spec("crop"),args,context,writer)


def _sheet_size(value, fallback):
    if not value: return fallback
    named={"a3":(841.89,1190.55),"a4":(595.28,841.89),"letter":(612,792)}
    if value.lower() in named: return named[value.lower()]
    try:
        w,h=(coords.mm_to_points(x) for x in value.lower().split("x")); return w,h
    except Exception: raise PdfError("BAD_ARG",f"invalid sheet size: {value}","use a3, a4, letter, or WxH in mm")


@command("cut",4,"pdf","pdf","_cut",True,_layout_policy)
def cut(args,context):
    def writer(doc,target):
        dropped=_layout_source(doc,args)
        out=pymupdf.open(); maps=[]
        for i,source in enumerate(doc):
            if args.grid:
                try: cols,rows=map(int,args.grid.lower().split("x"))
                except Exception: raise PdfError("BAD_ARG","invalid cut grid","use --grid CxR")
                if cols<1 or rows<1: raise PdfError("BAD_ARG","grid values must be positive","use --grid CxR")
                boxes=[]
                for row in range(rows):
                    for col in range(cols): boxes.append(pymupdf.Rect(source.rect.x0+source.rect.width*col/cols,source.rect.y0+source.rect.height*row/rows,source.rect.x0+source.rect.width*(col+1)/cols,source.rect.y0+source.rect.height*(row+1)/rows))
            elif args.v is not None:
                x=coords.mm_to_points(args.v)
                if not source.rect.x0<x<source.rect.x1: raise PdfError("BAD_ARG","vertical cut must be inside the page","choose a displayed mm position within the page")
                boxes=[pymupdf.Rect(source.rect.x0,source.rect.y0,x,source.rect.y1),pymupdf.Rect(x,source.rect.y0,source.rect.x1,source.rect.y1)]
                if args.rtl: boxes.reverse()
            else:
                y=coords.mm_to_points(args.h)
                if not source.rect.y0<y<source.rect.y1: raise PdfError("BAD_ARG","horizontal cut must be inside the page","choose a displayed mm position within the page")
                boxes=[pymupdf.Rect(source.rect.x0,source.rect.y0,source.rect.x1,y),pymupdf.Rect(source.rect.x0,y,source.rect.x1,source.rect.y1)]
            for box in boxes:
                source_clip,panel_box,dest_rect,matrix=coords.cut_panel_geometry(source,box)
                out.insert_pdf(doc,from_page=i,to_page=i)
                page=out[-1]
                pdf_box=coords.cropbox_to_pdf_space(source,panel_box)
                box_text="["+" ".join(f"{value:g}" for value in pdf_box)+"]"
                out.xref_set_key(page.xref,"MediaBox",box_text)
                out.xref_set_key(page.xref,"CropBox",box_text)
                for annot in list(page.annots() or []):page.delete_annot(annot)
                for link in page.get_links():page.delete_link(link)
                for widget in list(page.widgets() or []):page.delete_widget(widget)
                maps.append([Placement(None,i,matrix,tuple(dest_rect))])
        out.save(target); out.close(); return OperationResult(pages=maps,features=Features(dropped=dropped))
    return _write_layout(args.file.resolve(),"cut",args,context,writer)


@command("resize",4,"pdf","pdf","_resized",True,_layout_policy)
def resize(args,context):
    def writer(doc,target):
        dropped=_layout_source(doc,args)
        if not args.scale and not args.pad: raise PdfError("BAD_ARG","choose --scale fit or --pad","select how pages should fit the requested size")
        fallback=(doc[0].rect.width,doc[0].rect.height); w,h=_sheet_size(args.size,fallback)
        out=pymupdf.open(); maps=[]
        for i,source in enumerate(doc):
            page=out.new_page(width=w,height=h); scale=min(w/source.rect.width,h/source.rect.height) if args.scale else 1
            if args.pad and (source.rect.width>w or source.rect.height>h):
                scale=min(w/source.rect.width,h/source.rect.height); import clierr; clierr.warn("pdf resize","PAD_TOO_SMALL","content exceeds target; fitted to page","choose a larger --size")
            if args.pad and scale==1: x=(w-source.rect.width)/2;y=(h-source.rect.height)/2
            else: x=(w-source.rect.width*scale)/2;y=(h-source.rect.height*scale)/2
            r=pymupdf.Rect(x,y,x+source.rect.width*scale,y+source.rect.height*scale)
            page.show_pdf_page(r,doc,i)
            maps.append([Placement(None,i,(scale,0,0,scale,x,y),tuple(r))])
        out.save(target);out.close();return OperationResult(pages=maps,features=Features(dropped=dropped))
    return _write_layout(args.file.resolve(),"resize",args,context,writer)


@command("nup",4,"pdf","pdf","_nup",True,_layout_policy)
def nup(args,context):
    grids={2:(2,1),4:(2,2),8:(4,2),16:(4,4),32:(8,4)}
    def writer(doc,target):
        dropped=_layout_source(doc,args)
        cols,rows=grids[args.grid]; default=(doc[0].rect.height,doc[0].rect.width) if args.grid in (2,8,32) else (doc[0].rect.width,doc[0].rect.height)
        w,h=_sheet_size(args.sheet,default)
        if (w>h)!=(cols>rows): w,h=h,w
        gap=coords.mm_to_points(args.gap)
        cellw=(w-gap*(cols+1))/cols;cellh=(h-gap*(rows+1))/rows
        if min(cellw,cellh)<=0: raise PdfError("BAD_ARG","gap leaves no room for pages","reduce --gap")
        out=pymupdf.open(); maps=[];regions=[]
        for start in range(0,len(doc),cols*rows):
            page=out.new_page(width=w,height=h); placements=[]
            for slot in range(cols*rows):
                index=start+slot
                row,col=(slot//cols,slot%cols) if args.order=="row" else (slot%rows,slot//rows)
                rect=pymupdf.Rect(gap+col*(cellw+gap),gap+row*(cellh+gap),gap+col*(cellw+gap)+cellw,gap+row*(cellh+gap)+cellh)
                if index>=len(doc):
                    placements.append(Placement(None,"blank",dest_rect=tuple(rect)))
                    if args.border:
                        page.draw_rect(rect,color=(0,0,0),width=.5)
                        regions.append({"page":len(maps)+1,"rect":tuple(coords.pymupdf_to_display(page,rect))})
                    continue
                scale=min(rect.width/doc[index].rect.width,rect.height/doc[index].rect.height)
                placed=pymupdf.Rect(rect.x0+(rect.width-doc[index].rect.width*scale)/2,rect.y0+(rect.height-doc[index].rect.height*scale)/2,rect.x0+(rect.width+doc[index].rect.width*scale)/2,rect.y0+(rect.height+doc[index].rect.height*scale)/2)
                page.show_pdf_page(placed,doc,index)
                placements.append(Placement(None,index,(scale,0,0,scale,placed.x0,placed.y0),tuple(placed)))
                if args.border:
                    page.draw_rect(rect,color=(0,0,0),width=.5)
                    regions.append({"page":len(maps)+1,"rect":tuple(coords.pymupdf_to_display(page,rect))})
            maps.append(placements)
        out.save(target);out.close();return OperationResult(pages=maps,features=Features(dropped=dropped),expected=ExpectedChanges(regions=regions))
    return _write_layout(args.file.resolve(),"nup",args,context,writer)


@command("booklet",4,"pdf","pdf","_booklet",True,_layout_policy)
def booklet(args,context):
    def writer(doc,target):
        dropped=_layout_source(doc,args)
        original=len(doc); total=((original+3)//4)*4; sheets=total//4
        w,h=_sheet_size(args.sheet,(doc[0].rect.height,doc[0].rect.width))
        if w<h: w,h=h,w
        out=pymupdf.open(); maps=[]
        for k in range(sheets):
            indices=[total-1-2*k,2*k,2*k+1,total-2-2*k]
            if args.binding=="right": indices=[indices[1],indices[0],indices[3],indices[2]]
            for side in range(2):
                page=out.new_page(width=w,height=h); placements=[]
                for half in range(2):
                    slot=side*2+half; idx=indices[slot]
                    x=half*w/2; rect=pymupdf.Rect(x,0,x+w/2,h)
                    if idx>=original:
                        placements.append(Placement(None,"blank",dest_rect=tuple(rect)));continue
                    scale=min(rect.width/doc[idx].rect.width,rect.height/doc[idx].rect.height)
                    placed=pymupdf.Rect(x+(rect.width-doc[idx].rect.width*scale)/2,(h-doc[idx].rect.height*scale)/2,x+(rect.width+doc[idx].rect.width*scale)/2,(h+doc[idx].rect.height*scale)/2)
                    rotate_back=bool(side and args.flip=="long")
                    page.show_pdf_page(placed,doc,idx,rotate=(180 if rotate_back else 0))
                    transform=((-scale,0,0,-scale,placed.x0+doc[idx].rect.width*scale,placed.y0+doc[idx].rect.height*scale)
                               if rotate_back else (scale,0,0,scale,placed.x0,placed.y0))
                    placements.append(Placement(None,idx,transform,tuple(placed)))
                maps.append(placements)
        out.save(target);out.close();return OperationResult(pages=maps,features=Features(dropped=dropped))
    return _write_layout(args.file.resolve(),"booklet",args,context,writer)


@command("gray",4,"pdf","pdf","_gray",True,"gray")
def gray(args,context):
    path=args.file.resolve()
    def writer(doc,target):
        import numpy as np, shutil, clierr
        original=Path(context.temp)/"gray-original.pdf"
        tmp=Path(context.temp)/"gray-gs.pdf"
        try:
            doc.save(original)
            doc.recolor(components=1);doc.save(target)
            with pymupdf.open(target) as candidate,pymupdf.open(original) as original_doc:
                chroma=0;lum_delta=0
                for page_no,page in enumerate(candidate):
                    pix=page.get_pixmap(matrix=pymupdf.Matrix(.5,.5),alpha=False)
                    rgb=np.frombuffer(pix.samples,dtype=np.uint8).reshape(pix.height,pix.width,pix.n)[...,:3]
                    chroma=max(chroma,int(np.max(rgb.max(axis=2).astype(np.int16)-rgb.min(axis=2).astype(np.int16))))
                    srcpix=original_doc[page_no].get_pixmap(matrix=pymupdf.Matrix(.5,.5),alpha=False)
                    srgb=np.frombuffer(srcpix.samples,dtype=np.uint8).reshape(srcpix.height,srcpix.width,srcpix.n)[...,:3]
                    import cv2
                    lum_delta=max(lum_delta,int(np.abs(cv2.cvtColor(srgb,cv2.COLOR_RGB2GRAY).astype(np.int16)-cv2.cvtColor(rgb,cv2.COLOR_RGB2GRAY).astype(np.int16)).max()))
            if (chroma>2 or lum_delta>24) and not args.no_gs:
                has_links=any(page.get_links() for page in doc)
                has_annots=any(list(page.annots() or []) for page in doc)
                has_widgets=any(list(page.widgets() or []) for page in doc)
                try: has_signatures=doc.get_sigflags()>0
                except Exception: has_signatures=False
                if has_links or has_annots or has_widgets or has_signatures:
                    reason=", ".join(name for name,present in (("forms",has_widgets),("links",has_links),("annotations",has_annots),("signatures",has_signatures)) if present)
                    context.gray_skip_reason=reason
                    clierr.warn("pdf gray","SKIPPED_GS",f"Ghostscript fallback skipped because the PDF contains {reason}","review grayscale output")
                else:
                    from ..tools import find_tool, run_tool
                    gs=find_tool("ghostscript")
                    if not gs: raise PdfError("NOT_FOUND","Ghostscript is not available for grayscale fallback","install Ghostscript or use --no-gs")
                    # Windows' sRGB profile when present; elsewhere Ghostscript's built-in default_rgb.icc (also sRGB).
                    srgb=Path(r"C:\Windows\System32\spool\drivers\color\sRGB Color Space Profile.icm")
                    argv=[gs,"-dSAFER","-dBATCH","-dNOPAUSE","-dQUIET","-sDEVICE=pdfwrite","-dCompatibilityLevel=1.7",
                          "-dAutoRotatePages=/None","-dDetectDuplicateImages=true","-dCompressFonts=true","-dSubsetFonts=true",
                          "-dUseCIEColor","-dUseFastColor=true",*([f"-sDefaultRGBProfile={srgb}"] if srgb.is_file() else []),
                          "-sColorConversionStrategy=Gray","-dProcessColorModel=/DeviceGray","-dDownsampleColorImages=true",
                          "-dDownsampleGrayImages=true","-dDownsampleMonoImages=false","-dColorImageResolution=300",
                          "-dGrayImageResolution=300","-dColorImageDownsampleThreshold=1.5","-dGrayImageDownsampleThreshold=1.5",
                          "-dAutoFilterColorImages=true","-dAutoFilterGrayImages=true","-o",str(tmp),str(original)]
                    run_tool(argv,timeout=max(120,30*doc.page_count),cwd=context.temp,temp=context.temp,
                             secret_env=context.secret_env,secret_values=context.secret_store.values if context.secret_store else ())
                    Path(target).unlink(missing_ok=True);shutil.copy2(tmp,target)
        finally:
            original.unlink(missing_ok=True);tmp.unlink(missing_ok=True)
        return OperationResult(pages=[[Placement(None,i)] for i in range(doc.page_count)])
    result=write_pdf(path,_spec("gray"),args,context,writer)
    if getattr(context,"gray_skip_reason",None) and result.get("type")=="result":
        result["warnings"].append("SKIPPED_GS")
    return result
