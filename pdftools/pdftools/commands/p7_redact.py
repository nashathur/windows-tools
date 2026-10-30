"""Apply irreversible text, image, and vector redactions."""
import re
import shutil
import xml.etree.ElementTree as ET
from pathlib import Path

import pymupdf
import pikepdf
from rapidfuzz.distance import Levenshtein

from .. import coords, textmatch, tools
from ..check import Finding, OperationResult, normalize_words
from ..errors import PdfError
from ..pages import parse_page_ranges
from ..pipeline import write_pdf
from ..registry import command, get


def add_arguments_redact(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("--text", action="append", default=[])
    parser.add_argument("--regex", action="append", default=[])
    parser.add_argument("--rect", action="append", default=[])
    parser.add_argument("--pages", default="1-")
    parser.add_argument("--occurrence", action="append", type=int, default=[])
    parser.add_argument("--graphics", choices=("covered", "touched"), default="covered")
    parser.add_argument("--ocr", action="store_true"); parser.add_argument("--lang", default="eng+ind")
    parser.add_argument("--max-distance", type=int)
    parser.add_argument("--apply-existing", action="store_true")
    parser.add_argument("--keep-failed", action="store_true")
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--each", action="store_true")
    parser.add_argument("--password-file", type=Path); parser.add_argument("--password-env")
    parser.add_argument("--owner-password-file", type=Path); parser.add_argument("--owner-password-env")
    parser.add_argument("--ignore-restrictions", action="store_true"); parser.add_argument("--break-signature", action="store_true")


def _norm(s): return " ".join(str(s).casefold().split())


def _rect_arg(value):
    try:
        bits = [float(x.strip()) for x in value.split(",")]
        if len(bits) != 5 or int(bits[0]) != bits[0] or bits[0] < 1: raise ValueError()
        if bits[3] <= bits[1] or bits[4] <= bits[2]: raise ValueError()
        return int(bits[0])-1, tuple(bits[1:])
    except Exception as exc: raise PdfError("BAD_ARG", "invalid --rect", "use page,x0,y0,x1,y1 in displayed millimetres") from exc


def _ocr_matches(page, lang, context, temp):
    env = tools.dev_tool_env(temp, lang)
    exe = shutil.which("tesseract", path=env["PATH"])
    if not exe: raise PdfError("NOT_FOUND", "tesseract not found", "install tesseract in the dev environment")
    png, out = Path(temp)/"page.png", Path(temp)/"ocr"
    pix = page.get_pixmap(matrix=pymupdf.Matrix(300/72, 300/72), alpha=False)
    pix.save(png); del pix
    tools.run_tool([exe, str(png), str(out), "-l", lang, "tsv"], cwd=temp, env=env, temp=temp, timeout=120,
                   secret_env=context.secret_env,
                   secret_values=context.secret_store.values if context.secret_store else ())
    rows=[]
    raster=page.get_pixmap(matrix=pymupdf.Matrix(300/72,300/72),alpha=False)
    scale_x=page.rect.width/raster.width; scale_y=page.rect.height/raster.height
    for row in Path(str(out)+".tsv").read_text(encoding="utf-8").splitlines()[1:]:
        cols=row.split("\t", 11)
        if len(cols)<12 or not cols[11].strip(): continue
        try:
            x,y,w,h=map(float,cols[6:10])
            # TSV bounds trace glyphs tightly; include a small raster-pixel margin
            # so antialiased edge pixels cannot survive the removal.
            margin=5
            displayed=pymupdf.Rect(max(0,x-margin)*scale_x,max(0,y-margin)*scale_y,
                                   min(raster.width,x+w+margin)*scale_x,min(raster.height,y+h+margin)*scale_y)
            mm=coords.displayed_points_to_mm(displayed)
            rect=coords.display_to_pymupdf(page,mm)
            key=tuple(int(value) for value in cols[2:6])
            rows.append((cols[11].strip(),rect,key))
        except ValueError: continue
    return rows


def _append_black_fill(page, rect):
    """Append an opaque black rectangle in raw PDF user coordinates."""
    box=coords.to_user_space(page,rect)
    content=(f"q 0 g {box.x0:.6f} {box.y0:.6f} {box.width:.6f} {box.height:.6f} re f Q\n").encode("ascii")
    doc=page.parent
    xref=doc.get_new_xref()
    doc.update_object(xref,"<<>>")
    doc.update_stream(xref,content)
    kind,value=doc.xref_get_key(page.xref,"Contents")
    refs=re.findall(r"\d+\s+\d+\s+R",value) if kind in {"array","xref"} else []
    refs.append(f"{xref} 0 R")
    doc.xref_set_key(page.xref,"Contents","["+" ".join(refs)+"]")


def _fuzzy_words(words, target, bound):
    wanted=_norm(target).split()
    if not wanted:return []
    candidates=[]
    for size in {max(1,len(wanted)-1),len(wanted),len(wanted)+1}:
        for start in range(max(0,len(words)-size+1)):
            window=words[start:start+size]
            text=" ".join(row[0] for row in window)
            distance=Levenshtein.distance(_norm(target),_norm(text))
            if distance<=bound:
                lines=[]
                for _, part in window:
                    rect=pymupdf.Rect(part)
                    line=next((box for box in lines if abs(box.y0-rect.y0) < max(3, rect.height*.35)), None)
                    if line is None: lines.append(rect)
                    else: lines[lines.index(line)]=line|rect
                candidates.append((distance,start,start+size,text,lines))
    candidates.sort(key=lambda row:(row[0],row[1],row[2]))
    kept=[]
    for item in candidates:
        if any(item[1]<old[2] and old[1]<item[2] for old in kept):continue
        kept.append(item)
    return kept


def _xmp_text(data):
    root=ET.fromstring(data)
    values=[]
    for elem in root.iter():
        if elem.text: values.append(elem.text)
        if elem.tail: values.append(elem.tail)
        values.extend(str(value) for value in elem.attrib.values())
    return "\n".join(values)


def _xmp_contains(data, targets):
    try: return _contains_target(_xmp_text(data),targets)
    except Exception: return True


def _walk_pdf(obj, seen=None):
    seen=set() if seen is None else seen
    gen=getattr(obj,"objgen",(0,0))
    if gen!=(0,0):
        if gen in seen:return
        seen.add(gen)
    yield obj
    if isinstance(obj,(pikepdf.Dictionary,pikepdf.Stream)):
        for value in list(obj.values()): yield from _walk_pdf(value,seen)
    elif isinstance(obj,pikepdf.Array):
        for value in list(obj): yield from _walk_pdf(value,seen)


def _ocr_regex_lines(words, pattern):
    groups={}
    for word,rect,key in words: groups.setdefault(key[:3],[]).append((key[3],word,rect))
    result=[]
    for rows in groups.values():
        rows.sort(key=lambda row:row[0])
        joined=" ".join(row[1] for row in rows)
        try: found=list(re.finditer(pattern,joined,re.I))
        except re.error: found=[]
        if not found: continue
        offsets=[];pos=0
        for _,word,rect in rows:
            offsets.append((pos,pos+len(word),rect));pos+=len(word)+1
        for match in found:
            hit=[r for a,b,r in offsets if a<match.end() and match.start()<b]
            if hit:
                rect=pymupdf.Rect(hit[0])
                for part in hit[1:]:rect|=part
                result.append((match.group(0),rect))
    return result


def _remove_widget_appearances(field):
    seen=set()
    def visit(obj):
        gen=getattr(obj,"objgen",(0,0))
        if gen!=(0,0):
            if gen in seen:return
            seen.add(gen)
        if isinstance(obj,pikepdf.Dictionary):
            if str(obj.get("/Subtype",""))=="/Widget" and "/AP" in obj: del obj["/AP"]
            for child in obj.get("/Kids",[]): visit(child)
    visit(field)


def _policy(source, output, operation):
    findings=[]; targets=getattr(operation,"_targets",[])
    boxes=operation.measurements[0]["boxes"]
    all_occ=operation.measurements[0]["all_occurrences"]
    for pno, page in enumerate(output, 1):
        for row in boxes:
            if row["page"] != pno: continue
            box=pymupdf.Rect(row["box"])
            raw=page.get_text("rawdict",clip=box)
            clipped_chars=isinstance(raw,dict) and any(span.get("chars") for block in raw.get("blocks",[]) for line in block.get("lines",[]) for span in line.get("spans",[]))
            if clipped_chars:
                findings.append(Finding("invariant","REDACT_TEXT_REMAINS","characters remain inside a chosen redaction area",pno))
            try:
                from pypdf import PdfReader
                reader=PdfReader(output.name,password=getattr(operation,"_run_password","") or None)
                pdf_page=reader.pages[pno-1]
                def visitor(text,cm,tm,font_dict,font_size):
                    if not text.strip():return
                    x=float(tm[4])*float(cm[0])+float(tm[5])*float(cm[2])+float(cm[4])
                    y=float(tm[4])*float(cm[1])+float(tm[5])*float(cm[3])+float(cm[5])
                    point=coords.from_user_space(page,pymupdf.Rect(x,y,x,y))
                    if box.contains(pymupdf.Point(point.x0,point.y0)) and (clipped_chars or _contains_target(text,targets)):
                        findings.append(Finding("invariant","PYPDF_TEXT_REMAINS","independent extraction found text inside a chosen area",pno))
                pdf_page.extract_text(visitor_text=visitor)
            except Exception as exc:
                findings.append(Finding("invariant","PYPDF_READBACK","independent text extraction failed",pno,details={"error":str(exc)[:200]}))
            try:
                scale=100/72
                pixels=page.get_pixmap(matrix=pymupdf.Matrix(scale,scale),alpha=False)
                # 1 pt inset: anti-aliased edge pixels are not the fill colour.
                inner=pymupdf.Rect(box.x0+1,box.y0+1,box.x1-1,box.y1-1) if box.width>3 and box.height>3 else box
                render_box=coords.to_displayed_points(page,inner)
                x0=max(0,int(render_box.x0*pixels.width/page.rect.width));x1=min(pixels.width,int(render_box.x1*pixels.width/page.rect.width))
                y0=max(0,int(render_box.y0*pixels.height/page.rect.height));y1=min(pixels.height,int(render_box.y1*pixels.height/page.rect.height))
                rgb=pixels.samples; n=pixels.n
                total=max(0,x1-x0)*max(0,y1-y0)
                if total == 0:
                    findings.append(Finding("invariant","IMAGE_REMAINS","redacted area rendered as an empty clip",pno))
                    continue
                colors={}
                for y in range(y0,y1):
                    for x in range(x0,x1):
                        at=(y*pixels.width+x)*n;color=tuple(rgb[at:at+3])
                        colors[color]=colors.get(color,0)+1
                # The redaction fill is black: a box that is merely uniform (white paper around a small
                # unredacted name) must not pass.
                fill_fraction=sum(count for color,count in colors.items() if max(color)<=8)/total if colors else 0
                if fill_fraction<.98:
                    findings.append(Finding("invariant","IMAGE_REMAINS","pixels inside a chosen redaction area are not filled black",pno,
                                            details={"fill_fraction":fill_fraction}))
            except Exception as exc:
                findings.append(Finding("invariant","PIXEL_READBACK","redacted area could not be rendered",pno,details={"error":str(exc)[:200]}))
            try:
                for image in page.get_image_info(xrefs=True):
                    bbox=pymupdf.Rect(image["bbox"])
                    if not bbox.intersects(box) or not image.get("xref"): continue
                    pix=pymupdf.Pixmap(output, image["xref"])
                    if pix.n<3 or pix.colorspace is None or pix.colorspace.n!=3:
                        pix=pymupdf.Pixmap(pymupdf.csRGB,pix)
                    transform=pymupdf.Matrix(image.get("transform",(1,0,0,1,0,0)))
                    inv=~transform
                    mapped=box*inv
                    x0=max(0,int(mapped.x0*pix.width));x1=min(pix.width,int(mapped.x1*pix.width))
                    y0=max(0,int(mapped.y0*pix.height));y1=min(pix.height,int(mapped.y1*pix.height))
                    if x1<=x0 or y1<=y0:
                        continue
                    samples=pix.samples; n=pix.n
                    x0+=min(2,max(0,(x1-x0)//4)); x1-=min(2,max(0,(x1-x0)//4))
                    y0+=min(2,max(0,(y1-y0)//4)); y1-=min(2,max(0,(y1-y0)//4))
                    color_counts={}
                    for y in range(y0,y1):
                        for x in range(x0,x1):
                            at=(y*pix.width+x)*n; color=tuple(samples[at:at+3])
                            color_counts[color]=color_counts.get(color,0)+1
                    # MuPDF rewrites redacted image pixels as one exact colour (lossless, also for JPEG
                    # sources), so anything less than all of them means original pixels survived.
                    filled=bool(color_counts) and len(color_counts)==1
                    if not filled:
                        findings.append(Finding("invariant","IMAGE_SOURCE_REMAINS","source image pixels remain under an overlay",pno))
                        break
            except Exception as exc:
                findings.append(Finding("invariant","IMAGE_READBACK","image pixels could not be checked",pno,details={"error":str(exc)[:200]}))
    allowed_boxes=operation.measurements[0].get("allowed_text_boxes",[])
    for pno, (a,b) in enumerate(zip(source,output),1):
        def outside(words):
            regions=boxes+allowed_boxes
            return [w[4] for w in words if not any(r["page"]==pno and pymupdf.Rect(r["box"]).intersects(pymupdf.Rect(w[:4])) for r in regions)]
        kept=outside(a.get_text("words"));out=outside(b.get_text("words"))
        if normalize_words(kept)!=normalize_words(out): findings.append(Finding("invariant","TEXT_OUTSIDE_CHANGED","words outside chosen boxes changed",pno))
    if all_occ:
        text="\n".join(p.get_text() for p in output)
        try:
            from pypdf import PdfReader
            independent="\n".join(page.extract_text() or "" for page in PdfReader(
                output.name,password=getattr(operation,"_run_password","") or None).pages)
        except Exception as exc:
            findings.append(Finding("invariant","PYPDF_READBACK","independent text extraction failed",details={"error":str(exc)[:200]}))
            independent=""
        for target,regex in targets:
            try: found=bool(re.search(target,text,re.I|re.S)) if regex else _norm(target) in _norm(text)
            except re.error: found=True
            if found: findings.append(Finding("invariant","TARGET_REMAINS","a selected target remains in extracted text"))
            try: found=bool(re.search(target,independent,re.I|re.S)) if regex else _norm(target) in _norm(independent)
            except re.error: found=True
            if found: findings.append(Finding("invariant","PYPDF_TARGET_REMAINS","independent extraction found a selected target"))
        try:
            with pikepdf.open(output.name,password=getattr(operation,"_run_password","") or "") as pdf:
                seen=set();remaining=[]
                def inspect(obj):
                    gen=obj.objgen if hasattr(obj,"objgen") else (0,0)
                    if gen!=(0,0):
                        if gen in seen:return
                        seen.add(gen)
                    if isinstance(obj,pikepdf.String):
                        value=str(obj)
                        for pattern,is_regex in targets:
                            try: hit=bool(re.search(pattern,value,re.I)) if is_regex else _norm(pattern) in _norm(value)
                            except re.error: hit=True
                            if hit:remaining.append(True);break
                    elif isinstance(obj,pikepdf.Dictionary):
                        for value in obj.values():inspect(value)
                    elif isinstance(obj,pikepdf.Array):
                        for value in obj:inspect(value)
                inspect(pdf.Root)
                for value in pdf.docinfo.values():inspect(value)
            if remaining:findings.append(Finding("invariant","STRUCTURE_TARGET_REMAINS","a selected target remains in document strings"))
        except Exception as exc:
            findings.append(Finding("invariant","STRUCTURE_READBACK","redacted document strings could not be checked",details={"error":str(exc)[:200]}))
        try:
            from pypdf import PdfReader
            from pypdf.generic import (ArrayObject, ByteStringObject, DictionaryObject,
                                       IndirectObject, NameObject, StreamObject, TextStringObject)
            reader=PdfReader(output.name,password=getattr(operation,"_run_password","") or None)
            seen=set();remaining=[]
            def inspect_pypdf(obj):
                if isinstance(obj,IndirectObject):
                    key=(obj.idnum,obj.generation)
                    if key in seen:return
                    seen.add(key);obj=obj.get_object()
                if isinstance(obj,StreamObject):
                    inspect_pypdf(dict(obj))
                    if str(obj.get("/Type",""))=="/Metadata":
                        if _xmp_contains(obj.get_data(),targets):remaining.append(True)
                elif isinstance(obj,(TextStringObject,ByteStringObject,NameObject)):
                    if _contains_target(str(obj),targets):remaining.append(True)
                elif isinstance(obj,DictionaryObject):
                    for key,value in obj.items():
                        inspect_pypdf(key);inspect_pypdf(value)
                elif isinstance(obj,(dict,ArrayObject,list,tuple)):
                    values=obj.items() if isinstance(obj,dict) else obj
                    for value in values:inspect_pypdf(value)
            inspect_pypdf(reader.trailer)
            if remaining:findings.append(Finding("invariant","PYPDF_STRUCTURE_TARGET_REMAINS","independent document structure contains a selected target"))
        except Exception as exc:
            findings.append(Finding("invariant","PYPDF_STRUCTURE_READBACK","independent document structure could not be checked",details={"error":str(exc)[:200]}))
    return findings


def _contains_target(value,targets):
    value=str(value)
    for target,is_regex in targets:
        try:
            if re.search(target,value,re.I|re.S) if is_regex else _norm(target) in _norm(value):return True
        except re.error:return True
    return False


@command("redact", "7", "pdf", "pdf", "_redacted", True, _policy)
def redact(args, context):
    path=args.file.resolve()
    targets=[(str(t),False) for t in args.text]+[(str(t),True) for t in args.regex]
    if not targets and not args.rect: raise PdfError("BAD_ARG","redact needs at least one target","give --text, --regex, or --rect")
    for target, is_regex in targets:
        if not target.strip(): raise PdfError("BAD_ARG","redaction target is empty","provide non-empty text or pattern")
        if is_regex:
            try: re.compile(target)
            except re.error as exc: raise PdfError("BAD_ARG",f"invalid regular expression: {exc}","provide a valid regex pattern") from exc
    if args.max_distance is not None and args.max_distance < 0: raise PdfError("BAD_ARG","max distance must not be negative","use --max-distance 0 or a positive integer")
    for target,_ in targets:
        if context.secret_store is not None: context.secret_store.add(target)
    rects=[_rect_arg(r) for r in args.rect]
    last_operation = {}
    def writer(doc,target_path):
        selected=set(parse_page_ranges(args.pages,doc.page_count))
        for i in selected:
            pending=[a for a in doc[i].annots() or [] if a.type[0]==pymupdf.PDF_ANNOT_REDACT]
            if pending and not args.apply_existing:
                raise PdfError("REDACTIONS_PENDING","page already has redaction annotations","use --apply-existing")
        for pno,_ in rects:
            if pno>=doc.page_count: raise PdfError("BAD_PAGE","redaction rectangle page is outside the document","use a valid 1-based page")
        records=[]; boxes=[]; missing=[]
        if args.apply_existing:
            for i in selected:
                page=doc[i]
                for annot in page.annots() or []:
                    if annot.type[0]==pymupdf.PDF_ANNOT_REDACT:
                        boxes.append({"page":i+1,"box":tuple(coords.annotation_to_pymupdf(page,annot))})
                        records.append({"page":i+1,"target":"<existing>","text":"","occurrence":None,"distance":0,"fuzzy":False,"source":"rect"})
        ocr_by_page={}
        if args.ocr:
            for i in selected:
                page=doc[i]
                if not page.get_text("words") or page.get_images(full=True):
                    ocr_by_page[i]=_ocr_matches(page,args.lang,context,context.temp)
        else:
            missing=[i+1 for i in selected if not doc[i].get_text("words") or doc[i].get_images(full=True) or len(doc[i].get_drawings())>=25]
        for target_text,is_regex in targets:
            candidates=[]
            widget_boxes=[(page_no+1,coords.annotation_to_pymupdf(page,widget)) for page_no,page in enumerate(doc)
                          for widget in page.widgets() or []
                          if _contains_target(widget.field_value or "",[(target_text,is_regex)])]
            try: matches=textmatch.find_matches(doc,target_text,regex=is_regex,pages=selected)
            except re.error as exc: raise PdfError("BAD_ARG",f"invalid regular expression: {exc}","provide a valid regex pattern") from exc
            for item in matches["matches"]:
                if is_regex: rects_for_match=[pymupdf.Rect(q.rect) for q in item["quads"]]
                else: rects_for_match=[pymupdf.Rect(q.rect) for q in item["quads"]]
                rects_for_match=[rect for rect in rects_for_match if not any(page_no==item["page"] and widget_box.intersects(rect)
                                    for page_no,widget_box in widget_boxes)]
                if not rects_for_match: continue
                candidates.append({"page":item["page"],"text":item["text"],"rects":rects_for_match,
                    "distance":0,"fuzzy":False,"source":"text"})
            if not is_regex:
                L=len(_norm(target_text));bound=args.max_distance if args.max_distance is not None else (0 if L<=3 else 1 if L<=7 else 2 if L<=14 else 3)
                for i in selected:
                    page=doc[i]; trace=page.get_texttrace()
                    ocr_font=any(span.get("font")=="GlyphLessFont" or span.get("type")==3 for span in trace)
                    words=[(w[4],pymupdf.Rect(w[:4])) for w in page.get_text("words")]
                    if ocr_font:
                        fuzzy_words=words
                        source="ocr"
                    else:
                        fuzzy_words=[];source="ocr"
                    for distance,start,end,text,match_boxes in _fuzzy_words(fuzzy_words,target_text,bound):
                        candidates.append({"page":i+1,"text":text,"rects":match_boxes,"distance":distance,
                            "fuzzy":distance>0,"source":source})
                    ocr_words=ocr_by_page.get(i,[])
                    if ocr_words:
                        for distance,start,end,text,match_boxes in _fuzzy_words([(w,r) for w,r,_ in ocr_words],target_text,bound):
                            candidates.append({"page":i+1,"text":text,"rects":match_boxes,"distance":distance,
                                "fuzzy":distance>0,"source":"ocr"})
            else:
                for i,words in ocr_by_page.items():
                    for text,rect in _ocr_regex_lines(words,target_text):
                        candidates.append({"page":i+1,"text":text,"rects":[rect],"distance":0,"fuzzy":False,"source":"ocr"})
            candidates.sort(key=lambda row:(row["page"],min(r.y0 for r in row["rects"]),min(r.x0 for r in row["rects"]),row["distance"]))
            dedup=[]
            for candidate in candidates:
                if any(old["page"]==candidate["page"] and any(a.intersects(b) for a in old["rects"] for b in candidate["rects"]) for old in dedup):
                    continue
                dedup.append(candidate)
            chosen=set(args.occurrence or [])
            for number,item in enumerate(dedup,1):
                if chosen and number not in chosen: continue
                for rect in item["rects"]: boxes.append({"page":item["page"],"box":tuple(rect)})
                records.append({"page":item["page"],"target":target_text,"text":item["text"],"occurrence":number,
                    "distance":item["distance"],"fuzzy":item["fuzzy"],"source":item["source"]})
        for pno,mmrect in rects:
            if pno in selected:
                box=coords.display_to_pymupdf(doc[pno],mmrect); boxes.append({"page":pno+1,"box":tuple(box)})
                records.append({"page":pno+1,"target":"<rect>","text":"","occurrence":None,"distance":0,"fuzzy":False,"source":"rect"})
        for i,page in enumerate(doc):
            chosen=[pymupdf.Rect(b["box"]) for b in boxes if b["page"]==i+1]
            if not chosen and not (args.apply_existing and i in selected): continue
            for box in chosen: page.add_redact_annot(box,fill=False,cross_out=False)
            page.apply_redactions(images=pymupdf.PDF_REDACT_IMAGE_PIXELS,
                graphics=pymupdf.PDF_REDACT_LINE_ART_REMOVE_IF_TOUCHED if args.graphics=="touched" else pymupdf.PDF_REDACT_LINE_ART_REMOVE_IF_COVERED,
                text=pymupdf.PDF_REDACT_TEXT_REMOVE)
            for box in chosen: _append_black_fill(page,box)
        for page in doc:
            page.parent.xref_set_key(page.xref,"Thumb","null")
        # Clear matching strings from metadata and document structures.
        cleared={}
        doc.save(target_path,garbage=4,deflate=True)
        with pikepdf.open(target_path) as pdf:
            def scrub(value, mask=False):
                text=str(value)
                for pat,rx in targets:
                    try: text=re.sub(pat,lambda m:"█"*len(m.group(0)) if mask else "",text,flags=re.I) if rx else re.sub(re.escape(pat),lambda m:"█"*len(m.group(0)) if mask else "",text,flags=re.I)
                    except re.error: pass
                return text
            def matches(value):
                value=str(value)
                for pat,rx in targets:
                    try:
                        if re.search(pat,value,re.I) if rx else _norm(pat) in _norm(value):return True
                    except re.error:continue
                return False
            allowed_text_boxes=[]
            for page_no,page in enumerate(doc,1):
                for widget in page.widgets() or []:
                    if matches(widget.field_value or ""):
                        allowed_text_boxes.append({"page":page_no,"box":tuple(coords.annotation_to_pymupdf(page,widget))})
            for key,val in list(pdf.docinfo.items()):
                if matches(val): del pdf.docinfo[key];cleared["info"]=cleared.get("info",0)+1
            for obj in list(_walk_pdf(pdf.Root)):
                if isinstance(obj,pikepdf.Dictionary) and "/Metadata" in obj:
                    try:
                        hit=_xmp_contains(obj["/Metadata"].read_bytes(),targets)
                    except Exception: hit=True
                    if hit: del obj["/Metadata"];cleared["xmp"]=cleared.get("xmp",0)+1
            for page in pdf.pages:
                annots=page.get("/Annots",pikepdf.Array());kept=pikepdf.Array()
                for annot in annots:
                    if any(matches(annot.get(key,"")) for key in ("/Contents","/RC")):
                        cleared["annotations"]=cleared.get("annotations",0)+1
                    else:kept.append(annot)
                if kept:page["/Annots"]=kept
                elif "/Annots" in page:del page["/Annots"]
            visited=set()
            def scrub_tree(obj,parent_key="",grand_key=""):
                try:
                    gen=obj.objgen if hasattr(obj,"objgen") else (0,0)
                    if gen!=(0,0):
                        if gen in visited:return
                        visited.add(gen)
                    if isinstance(obj,pikepdf.Dictionary):
                        for key,value in list(obj.items()):
                            k=str(key)
                            if matches(k):
                                del obj[key]
                                kind="named_destinations" if parent_key=="/Dests" else "attachments" if parent_key=="/EmbeddedFiles" else None
                                if kind:cleared[kind]=cleared.get(kind,0)+1
                                continue
                            if isinstance(value,pikepdf.String) and matches(value):
                                kind={"/Title":"outline","/Contents":"annotations","/RC":"annotations","/V":"forms","/DV":"forms","/Alt":"structure","/ActualText":"structure","/T":"structure","/UF":"attachments","/F":"attachments","/P":"labels"}.get(k)
                                if kind is None: scrub_tree(value,k);continue
                                if k in {"/V","/DV"}:
                                    obj[key]=pikepdf.String(scrub(value,True))
                                    _remove_widget_appearances(obj)
                                else: del obj[key]
                                cleared[kind]=cleared.get(kind,0)+1
                            else:scrub_tree(value,k,parent_key)
                    elif isinstance(obj,pikepdf.Array):
                        values=list(obj)
                        if parent_key=="/Names":
                            kept=[];index=0
                            while index<len(values):
                                name=values[index]
                                if isinstance(name,pikepdf.String) and matches(name):
                                    kind="named_destinations" if grand_key=="/Dests" else "attachments" if grand_key=="/EmbeddedFiles" else None
                                    if kind:cleared[kind]=cleared.get(kind,0)+1
                                    index+=2
                                    continue
                                kept.append(name)
                                if index+1<len(values):kept.append(values[index+1])
                                index+=2
                            obj.clear();obj.extend(kept);values=list(obj)
                        for value in values:scrub_tree(value,parent_key,grand_key)
                except Exception:pass
            scrub_tree(pdf.Root)
            clean_path=Path(str(target_path)+".clean")
            pdf.save(clean_path)
        clean_path.replace(target_path)
        operation=OperationResult(measurements=[{"boxes":boxes,"allowed_text_boxes":allowed_text_boxes,
            "all_occurrences":not args.occurrence and selected==set(range(doc.page_count))}])
        operation._targets=targets
        operation._run_password=context.owner_password or context.password or ""
        operation.expected.regions=[{"page":b["page"],"rect":tuple(coords.pymupdf_to_display(doc[b["page"]-1],b["box"]))} for b in boxes]
        operation.expected.regions.extend({"page":b["page"],"rect":tuple(coords.pymupdf_to_display(doc[b["page"]-1],b["box"]))} for b in allowed_text_boxes)
        # Return op plus records for result annotation in wrapper.
        operation._matches=records
        operation._pages_without_text=missing
        operation._cleared=cleared
        last_operation["value"]=operation
        return operation
    def run_one(source_path):
        args.file=source_path
        record=write_pdf(source_path,get("redact"),args,context,writer,security=True)
        if isinstance(record,dict) and record.get("type")=="result":
            op=last_operation.get("value")
            matches=getattr(op,"_matches",[]) if op else []
            pages={item["page"] for item in matches}
            record["matches"]=matches
            record["cleared"]=getattr(op,"_cleared",{}) if op else {}
            fuzzy=sum(bool(m.get("fuzzy")) for m in matches)
            area_count=len(op.measurements[0]["boxes"]) if op else 0
            record["note"]=f"{area_count} areas redacted on {len(pages)} pages ({fuzzy} fuzzy); crop is not redaction, redact is"
            if op and getattr(op,"_pages_without_text",[]) and record.get("verdict")=="ok":
                record.setdefault("warnings",[]).append("UNVERIFIED_REPRESENTATION")
                record["verdict"]="warn"
                import clierr
                clierr.warn("pdf redact","UNVERIFIED_REPRESENTATION",
                            "image or vector content was not searched without OCR",
                            "use --ocr or redact known areas with --rect")
        return record
    if args.each:
        from ..pipeline import run_each
        files=sorted(path.parent.glob(path.name)) if any(c in str(path) for c in "*?[") else sorted(path.glob("*.pdf")) if path.is_dir() else [path]
        return run_each(files,run_one,"redact",context)
    return run_one(path)
