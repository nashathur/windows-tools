"""Add a searchable text layer with OCRmyPDF."""
import os
import re
import shutil
import sys
import time
from pathlib import Path

import cv2
import numpy as np
import pymupdf

from ..check import OperationResult, Placement
from ..errors import PdfError
from .. import io
from ..pipeline import announce_estimate, open_input, run_each, write_pdf
from ..registry import command
from ..tools import dev_tool_env, run_tool
from .p5_compress import _password_args, _tool


_MEASURE_DPI = 100
_DESKEW_THRESHOLD = 1.5  # Enable document-wide deskew when any page exceeds this angle.
_LOW_DPI_THRESHOLD = 250  # Oversample pages whose effective source resolution is below this dpi.
_WARN_DPI_THRESHOLD = 150  # Warn when a source scan is below this effective resolution.
_UNEVEN_SPREAD_THRESHOLD = 40  # Sauvola is useful when estimated background spread exceeds 40 levels.
_DARK_MEAN_THRESHOLD = 100  # Record pages with mean grayscale brightness below 100 levels.
_BACKGROUND_KERNEL_MM = 15  # Estimate illumination with a median window about 15 mm wide.


def add_arguments_ocr(parser):
    parser.add_argument("file",type=Path,nargs="?");parser.add_argument("--lang",default="eng+ind")
    parser.add_argument("--pdfa",action="store_true");parser.add_argument("-o","--output",type=Path)
    parser.add_argument("--clean",action="store_true",help="clean scanned pages with unpaper (Linux only)")
    parser.add_argument("--each",type=Path);_password_args(parser)


@command("ocr","5","pdf","pdf","_ocr",True,"ocr")
def ocr(args,context):
    if bool(getattr(args,"file",None)) == bool(getattr(args,"each",None)):
        raise PdfError("BAD_ARG","give exactly one of FILE and --each","provide a PDF file or --each folder/glob")
    if args.each:
        from .p5_batch import _glob_files
        return run_each(_glob_files(args.each),lambda file:ocr(_per_file(args,file),context),"ocr",context)
    path=Path(args.file).resolve()
    tool=_tool("ocrmypdf");sidecar_holder={}
    def writer(doc,target):
        sidecar=Path(context.temp)/"sidecar.txt"
        output_type="pdfa" if args.pdfa else "pdf"
        io.disk_budget([path],target,context.temp,multiplier=20)
        env=dev_tool_env(context.temp,args.lang)
        measurements=[_measure_page(page,index+1) for index,page in enumerate(doc)
                      if not page.get_text("text").strip()]
        clean=bool(getattr(args,"clean",False))
        if clean and not shutil.which("unpaper",path=env.get("PATH")):
            raise PdfError("NOT_FOUND","unpaper is not installed (needed by --clean)",
                           "run without --clean, or install unpaper (conda-forge has it for Linux only)")
        preprocess={"rotate_pages":True,
                    "deskew":any(abs(item["skew"])>_DESKEW_THRESHOLD for item in measurements),
                    "oversample":300 if any(item["dpi"] is not None and item["dpi"]<_LOW_DPI_THRESHOLD for item in measurements) else None,
                    "thresholding":"sauvola" if any(item["uneven"] for item in measurements) else None,
                    "clean":clean}
        options=["--rotate-pages"]
        if preprocess["deskew"]:options.append("--deskew")
        if preprocess["oversample"]:options.extend(["--oversample",str(preprocess["oversample"])])
        if preprocess["thresholding"]:options.extend(["--tesseract-thresholding",preprocess["thresholding"]])
        if clean:options.append("--clean")
        argv=[tool,"--skip-text",*options,"--output-type",output_type,"-l",args.lang,"--sidecar",str(sidecar),
              "--jobs",str(min(max(1,os.cpu_count() or 1),4))]
        if args.break_signature:argv.append("--invalidate-digital-signatures")
        if doc.page_count>10:
            sample=Path(context.temp)/"ocr-sample.pdf";sample_out=Path(context.temp)/"ocr-sample-out.pdf"
            with pymupdf.open(doc.name) as source,pymupdf.open() as first_two:
                first_two.insert_pdf(source,from_page=0,to_page=1);first_two.save(sample)
            started=time.perf_counter()
            sample_argv=[tool,"--skip-text",*options,"--output-type","pdf","-l",args.lang,"--jobs","1",str(sample),str(sample_out)]
            run_tool(sample_argv,timeout=120,cwd=context.temp,env=env,temp=context.temp,
                     secret_env=context.secret_env,secret_values=context.secret_store.values if context.secret_store else ())
            announce_estimate(doc.page_count,(time.perf_counter()-started)/2)
        argv.extend([doc.name,str(target)])
        run_tool(argv,timeout=max(120,60*doc.page_count),cwd=context.temp,env=env,temp=context.temp,
                 secret_env=context.secret_env,secret_values=context.secret_store.values if context.secret_store else ())
        sidecar_holder["text"]=sidecar.read_text(encoding="utf-8",errors="replace") if sidecar.exists() else ""
        sidecar_holder["measurements"]=measurements
        sidecar_holder["preprocess"]=preprocess
        return OperationResult(pages=[[Placement(None,i)] for i in range(doc.page_count)])
    result=write_pdf(path,_spec(),args,context,writer)
    if getattr(args,"dry_run",False):return result
    if result.get("path")==str(path):return result
    if result.get("type")=="result":
        with open_input(path,context.owner_password or context.password or "") as source, \
             open_input(result["path"],context.owner_password or context.password or "") as out:
            source_text=[bool(page.get_text("text").strip()) for page in source]
            output_text=[bool(page.get_text("text").strip()) for page in out]
            rotated=[index+1 for index,(before,after) in enumerate(zip(source,out)) if before.rotation!=after.rotation]
        page_text,skipped_markers=_sidecar_pages(sidecar_holder.get("text",""))
        ocred=sum(1 for i,has_text in enumerate(output_text) if i<len(source_text) and not source_text[i]
                  and has_text and i not in skipped_markers and page_text.get(i,""))
        skipped=len({i for i,has_text in enumerate(source_text) if has_text}|skipped_markers)
        empty=[i+1 for i,has_text in enumerate(output_text) if not has_text]
        measurements=sidecar_holder.get("measurements",[])
        low_dpi=[item["page"] for item in measurements if item["dpi"] is not None and item["dpi"]<_WARN_DPI_THRESHOLD]
        if empty:
            import clierr
            if "OCR_EMPTY" not in result.get("warnings",[]):
                clierr.warn("pdf ocr","OCR_EMPTY",f"pages still have no text: {', '.join(map(str,empty))}","check scan quality or add a language model")
                result.setdefault("warnings",[]).append("OCR_EMPTY")
            if result.get("verdict")=="ok":result["verdict"]="warn"
        if low_dpi:
            import clierr
            if "OCR_LOW_QUALITY" not in result.get("warnings",[]):
                clierr.warn("pdf ocr","OCR_LOW_QUALITY",f"pages below 150 dpi effective: {', '.join(map(str,low_dpi))}","use a higher-resolution scan when available")
                result.setdefault("warnings",[]).append("OCR_LOW_QUALITY")
            if result.get("verdict")=="ok":result["verdict"]="warn"
        preprocess=sidecar_holder.get("preprocess",{"rotate_pages":True,"deskew":False,"oversample":None,"thresholding":None,"clean":False})
        skewed=[{"page":item["page"],"angle":item["skew"]} for item in measurements
                if preprocess["deskew"] and abs(item["skew"])>0.1]
        notes=[]
        if preprocess["deskew"]:notes.append(f"deskew {len(skewed)} pages")
        if preprocess["oversample"]:notes.append("oversample 300")
        if preprocess["thresholding"]:notes.append("sauvola")
        if preprocess["clean"]:notes.append("clean")
        result["note"]=f"{ocred} pages OCRed, {skipped} had text"+(f", OCR_EMPTY: {', '.join(map(str,empty))}" if empty else "")
        if notes:result["note"]+=", "+", ".join(notes)
        result["ocr_pages"]=ocred;result["text_pages_skipped"]=skipped
        result["preprocess"]=preprocess;result["page_measurements"]=measurements
        result["rotated"]=rotated;result["deskewed"]=skewed
    return result


def _measure_page(page,number):
    pix=page.get_pixmap(matrix=pymupdf.Matrix(_MEASURE_DPI/72,_MEASURE_DPI/72),colorspace=pymupdf.csGRAY,alpha=False)
    gray=np.frombuffer(pix.samples,dtype=np.uint8).reshape(pix.height,pix.width).copy()
    del pix
    dpi=_effective_dpi(page)
    _,binary=cv2.threshold(gray,0,255,cv2.THRESH_BINARY_INV|cv2.THRESH_OTSU)
    skew=_skew_angle(binary)
    kernel=max(3,int(round(_BACKGROUND_KERNEL_MM/25.4*_MEASURE_DPI))|1)
    background=cv2.medianBlur(gray,kernel)
    spread=float(np.percentile(background,95)-np.percentile(background,5))
    mean=float(gray.mean())
    count,_,stats,_=cv2.connectedComponentsWithStats(binary,connectivity=8)
    blobs=int(sum(1 for area in stats[1:count,cv2.CC_STAT_AREA] if area<=2))
    return {"page":number,"dpi":round(dpi,2) if dpi is not None else None,"skew":round(skew,2),
            "spread":round(spread,2),"mean":round(mean,2),"blobs":blobs,
            "uneven":spread>_UNEVEN_SPREAD_THRESHOLD,"dark":mean<_DARK_MEAN_THRESHOLD}


def _effective_dpi(page):
    images=page.get_image_info()
    if not images:return None
    def area(image):
        width,height=image.get("width",0),image.get("height",0)
        return width*height
    image=max(images,key=area)
    rect=pymupdf.Rect(image["bbox"])
    if rect.width<=0 or rect.height<=0:return None
    return min(image["width"]/(rect.width/72),image["height"]/(rect.height/72))


def _skew_angle(binary):
    if cv2.countNonZero(binary)<10:return 0.0
    def score(angle):
        height,width=binary.shape
        matrix=cv2.getRotationMatrix2D((width/2,height/2),angle,1.0)
        rotated=cv2.warpAffine(binary,matrix,(width,height),flags=cv2.INTER_NEAREST,borderValue=0)
        projection=rotated.sum(axis=1,dtype=np.float64)
        return float(projection.var())
    coarse=np.arange(-10.0,10.0001,0.25)
    best=float(max(coarse,key=score))
    fine=np.arange(max(-10,best-0.25),min(10,best+0.25)+0.0001,0.05)
    return float(max(fine,key=score))


def _spec():
    from ..registry import get
    return get("ocr")


def _sidecar_pages(text):
    pages=text.split("\f")
    if pages and not pages[-1].strip():pages.pop()
    skipped=set();page_text={}
    marker_pattern=re.compile(r"\[OCR skipped on page\(s\)\s+([^\]]+)\]",re.I)
    page=0
    for value in pages:
        markers=marker_pattern.findall(value)
        if not markers:
            page_text[page]=value.strip();page+=1;continue
        for marker in markers:
            for first,last in re.findall(r"(\d+)(?:\s*-\s*(\d+))?",marker):
                start=int(first);end=int(last or first)
                skipped.update(range(start-1,end));page=max(page,end)
    return page_text,skipped


def _per_file(args,file):
    import argparse
    values=vars(args).copy();values["file"]=file;values["each"]=None
    return argparse.Namespace(**values)
