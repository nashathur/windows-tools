"""Metadata, bookmarks, and page-label editing commands."""
import argparse
import datetime as dt
import json
from pathlib import Path

import pikepdf
import pymupdf

from ..check import OperationResult, Features
from ..errors import PdfError, BadArg
from ..pipeline import open_input, run_each, write_pdf
from ..registry import command


def _add_arguments_meta(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("--title")
    parser.add_argument("--author")
    parser.add_argument("--subject")
    parser.add_argument("--keywords")
    parser.add_argument("--creator")
    parser.add_argument("--producer")
    parser.add_argument("--created")
    parser.add_argument("--modified")
    parser.add_argument("--lang")
    parser.add_argument("--custom", action="append", default=[])
    parser.add_argument("--clear", nargs="?", const="all")
    parser.add_argument("--to", type=Path)
    parser.add_argument("--from", dest="from_file", type=Path)
    parser.add_argument("--replace", action="store_true")
    parser.add_argument("--each", action="store_true")
    parser.add_argument("--output", "-o", type=Path)
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")
    parser.add_argument("--owner-password-file", type=Path)
    parser.add_argument("--owner-password-env")
    parser.add_argument("--ignore-restrictions", action="store_true")
    parser.add_argument("--break-signature", action="store_true")


def add_arguments_meta(parser): _add_arguments_meta(parser)


STANDARD = {"title": "/Title", "author": "/Author", "subject": "/Subject",
            "keywords": "/Keywords", "creator": "/Creator", "producer": "/Producer",
            "created": "/CreationDate", "modified": "/ModDate"}


def _roman(number):
    parts=[]
    for value,symbol in ((1000,"M"),(900,"CM"),(500,"D"),(400,"CD"),(100,"C"),(90,"XC"),(50,"L"),(40,"XL"),(10,"X"),(9,"IX"),(5,"V"),(4,"IV"),(1,"I")):
        while number>=value: parts.append(symbol);number-=value
    return "".join(parts)


def _date(value):
    if isinstance(value,str) and value.startswith("D:"):
        if not __import__("re").fullmatch(r"D:\d{14}(?:[+-]\d{2}'\d{2}'?|Z)?",value):
            raise PdfError("BAD_ARG", f"invalid date: {value}", "use YYYY-MM-DD or ISO 8601")
        return value
    try:
        date = dt.datetime.fromisoformat(value)
    except (ValueError, TypeError):
        try:
            date = dt.date.fromisoformat(value)
        except (ValueError, TypeError):
            raise PdfError("BAD_ARG", f"invalid date: {value}", "use YYYY-MM-DD or ISO 8601")
        date = dt.datetime.combine(date, dt.time())
    if date.tzinfo is None:
        date = date.astimezone()
    offset = date.utcoffset() or dt.timedelta()
    total = int(offset.total_seconds() // 60)
    sign = "+" if total >= 0 else "-"
    hours, minutes = divmod(abs(total), 60)
    return date.strftime("D:%Y%m%d%H%M%S") + f"{sign}{hours:02d}'{minutes:02d}'"


def _read_meta(pdf):
    info = {str(k).lstrip("/"): str(v) for k, v in pdf.docinfo.items()}
    with pdf.open_metadata(set_pikepdf_as_editor=False) as meta:
        xmp = {str(k): str(v) for k, v in meta.items()}
    return {"info": info, "xmp": xmp, "lang": str(pdf.Root.get("/Lang", ""))}


@command("meta", 4, "pdf", "pdf", "_meta", True, "structure")
def meta(args, context):
    if not args.each:
        return _meta_one(args, context)
    pattern = Path(args.file)
    paths = (sorted(pattern.glob("*.pdf"), key=lambda p: p.name.casefold()) if pattern.is_dir()
             else sorted(pattern.parent.glob(pattern.name), key=lambda p: p.name.casefold()))
    if not paths:
        raise PdfError("NOT_FOUND", f"no PDF matches {pattern}", "give a folder or a glob such as D:\\docs\\*.pdf")
    if args.output:
        raise PdfError("BAD_ARG", "--each cannot take -o", "drop -o; each output is named after its input")

    def one(path):
        per_file = argparse.Namespace(**{**vars(args), "file": path, "custom": list(args.custom)})
        return _meta_one(per_file, context)
    return run_each(paths, one, "meta", context)


def _meta_one(args, context):
    try:
        return _meta_one_impl(args, context)
    except PdfError:
        raise
    except pikepdf.PasswordError as exc:
        raise PdfError("WRONG_PASSWORD", "PDF requires a valid password", "supply --password-file or --password-env") from exc
    except Exception as exc:
        raise PdfError("BAD_INPUT", f"metadata operation failed: {exc}", "check the PDF and metadata values") from exc


def _meta_one_impl(args, context):
    path = args.file.resolve()
    edits = {key: getattr(args, key) for key in STANDARD if getattr(args, key) is not None}
    xmp_patch={}
    if args.from_file:
        try:
            patch = json.loads(args.from_file.read_text(encoding="utf-8-sig"))
        except Exception as exc:
            raise PdfError("BAD_ARG", f"cannot read metadata JSON: {exc}", "provide a valid JSON object")
        if not isinstance(patch, dict):
            raise PdfError("BAD_ARG", "metadata JSON must be an object", "use an object with info, xmp, or lang")
        values = patch.get("info", patch)
        if not isinstance(values, dict):
            raise PdfError("BAD_ARG", "metadata info must be an object", "use key/value pairs")
        for key, value in values.items():
            k = str(key).lstrip("/").lower()
            aliases = {"creationdate":"created", "moddate":"modified"}
            dest = aliases.get(k) or next((name for name in STANDARD if name.lower() == k), None)
            if dest and dest not in edits:
                edits[dest] = value
            elif not dest:
                args.custom.append(f"{str(key).lstrip('/') }={value}")
        custom = patch.get("custom", {})
        if isinstance(custom, dict):
            args.custom.extend(f"{k}={v}" for k, v in custom.items())
        if "lang" in patch and args.lang is None:
            args.lang = patch["lang"]
        xmp_patch=patch.get("xmp",{})
        if not isinstance(xmp_patch,dict):
            raise PdfError("BAD_ARG","metadata XMP patch must be an object","use XMP field names and string values")
        if args.replace:
            args.clear = "all"
    clearing = set(str(args.clear).split(",")) if args.clear else set()
    custom = {}
    for item in args.custom:
        if "=" not in item:
            raise PdfError("BAD_ARG", f"invalid custom metadata: {item}", "use --custom KEY=VALUE")
        key, value = item.split("=", 1)
        custom[key.lstrip("/")] = value
    with pikepdf.open(path,password=context.password or context.owner_password or "") as original_pdf:
        original_info_keys={str(key) for key in original_pdf.docinfo.keys()}
        with original_pdf.open_metadata(set_pikepdf_as_editor=False) as original_metadata:
            original_xmp_keys=set(original_metadata.keys())
    if args.to:
        destination=Path(args.to).resolve()
        if destination == path:
            raise PdfError("BAD_ARG", "metadata export cannot replace its input PDF", "choose a separate JSON output path")
        if getattr(args,"dry_run",False):
            return {"type":"report","command":"meta","input":str(path),"verdict":"report","data":{"planned_export":str(destination)}}
        from .. import io as pdfio
        with pdfio.temp_folder() as temp:
            destination.parent.mkdir(parents=True,exist_ok=True)
            with pdfio.Staging(destination,temp) as stage:
                with pikepdf.open(path, password=context.password or context.owner_password or "") as pdf:
                    stage.path.write_text(json.dumps(_read_meta(pdf), ensure_ascii=False, indent=2), encoding="utf-8")
                published=stage.publish()
        return {"type": "report", "command": "meta", "input": str(path), "verdict": "report", "data": str(published)}
    if not edits and not clearing and not custom and not xmp_patch and args.lang is None and not args.from_file:
        with pikepdf.open(path, password=context.password or context.owner_password or "") as pdf:
            return {"type": "report", "command": "meta", "input": str(path), "verdict": "report", "data": _read_meta(pdf)}
    def writer(doc, target):
        temp = Path(context.temp) / "meta-edit.pdf"
        try:
            doc.save(temp)
            with pikepdf.open(temp) as pdf:
                info = pdf.docinfo
                for key in clearing:
                    if key == "all":
                        for entry in list(info.keys()): del info[entry]
                    else:
                        name = STANDARD.get(key.lower(), "/" + key.lstrip("/"))
                        if name in info: del info[name]
                if args.replace:
                    for entry in list(info.keys()): del info[entry]
                    if "/Lang" in pdf.Root: del pdf.Root["/Lang"]
                    if "/Metadata" in pdf.Root: del pdf.Root["/Metadata"]
                for key, value in edits.items():
                    info[STANDARD[key]] = _date(str(value)) if key in {"created", "modified"} else str(value)
                for key, value in custom.items():
                    info["/" + key] = str(value)
                if args.lang is not None:
                    pdf.Root.Lang = str(args.lang)
                with pdf.open_metadata(set_pikepdf_as_editor=False) as metadata:
                    metadata.load_from_docinfo(info, delete_missing=True)
                    metadata.update(xmp_patch)
                pdf.save(target)
        finally:
            temp.unlink(missing_ok=True)
        return OperationResult()
    result=write_pdf(path, _spec(), args, context, writer, operation=OperationResult())
    if result.get("type")=="result" and Path(result.get("path","")).is_file():
        try:
            with pikepdf.open(result["path"]) as pdf:
                for key,value in edits.items():
                    actual=str(pdf.docinfo.get(STANDARD[key],""))
                    expected=_date(str(value)) if key in {"created","modified"} else str(value)
                    matches=(actual==expected or (key in {"created","modified"} and actual.startswith(expected[:10])))
                    if not matches: raise ValueError(f"Info {key} did not read back")
                for key,value in custom.items():
                    if str(pdf.docinfo.get("/"+key,""))!=str(value): raise ValueError(f"custom Info {key} did not read back")
                for key in clearing:
                    targets=(original_info_keys if key=="all" else {STANDARD.get(key.lower(), "/"+key.lstrip("/"))})
                    targets-=set(STANDARD[k] for k in edits) | {"/"+k for k in custom}
                    if args.replace: targets.discard("/Producer")
                    remaining=[name for name in targets if name in pdf.docinfo]
                    if remaining: raise ValueError(f"cleared Info keys remain: {remaining}")
                if args.lang is not None and str(pdf.Root.get("/Lang",""))!=str(args.lang): raise ValueError("language did not read back")
                if args.replace and args.lang is None and pdf.Root.get("/Lang") is not None: raise ValueError("replaced language remains")
                with pdf.open_metadata(set_pikepdf_as_editor=False) as metadata:
                    for key,value in xmp_patch.items():
                        if str(metadata.get(key,""))!=str(value): raise ValueError(f"XMP {key} did not read back")
                    info_mapped={"dc:title","dc:creator","dc:description","pdf:Keywords","xmp:CreatorTool","xmp:CreateDate","xmp:ModifyDate",
                                 "{http://purl.org/dc/elements/1.1/}title","{http://purl.org/dc/elements/1.1/}creator",
                                 "{http://purl.org/dc/elements/1.1/}description","{http://ns.adobe.com/pdf/1.3/}Keywords",
                                 "{http://ns.adobe.com/xap/1.0/}CreatorTool","{http://ns.adobe.com/xap/1.0/}CreateDate",
                                 "{http://ns.adobe.com/xap/1.0/}ModifyDate"}
                    remaining_xmp=sorted(key for key in original_xmp_keys if key in metadata and key not in xmp_patch and key not in info_mapped)
                    if args.replace and remaining_xmp:
                        raise ValueError(f"replaced XMP properties remain: {remaining_xmp}")
        except Exception as exc:
            Path(result["path"]).unlink(missing_ok=True)
            raise PdfError("CHECK_FAILED",f"metadata readback failed: {exc}","review the metadata patch") from exc
    return result


def _spec():
    from ..registry import get
    return get("meta")


def add_arguments_bookmarks(parser):
    parser.add_argument("file", type=Path)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--list", action="store_true")
    group.add_argument("--set", dest="items", type=json.loads)
    group.add_argument("--clear", action="store_true")
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--password-file", type=Path); parser.add_argument("--password-env")
    parser.add_argument("--owner-password-file", type=Path); parser.add_argument("--owner-password-env")
    parser.add_argument("--ignore-restrictions", action="store_true"); parser.add_argument("--break-signature", action="store_true")


@command("bookmarks", 4, "pdf", "pdf", "_bookmarks", True, "structure")
def bookmarks(args, context):
    path = args.file.resolve()
    if args.list:
        with open_input(path, context.password or context.owner_password) as doc:
            return {"type": "report", "command": "bookmarks", "input": str(path), "verdict": "report", "data": doc.get_toc()}
    try:
        toc = [] if args.items is None else args.items
        if not isinstance(toc, list): raise ValueError("expected a list")
        previous = 0
        for row in toc:
            if not isinstance(row, dict) or not {"level","title","page"} <= set(row) or int(row["level"]) < 1 or int(row["page"]) < 1 or not isinstance(row["title"], str):
                raise ValueError("each item needs level, title, page")
            level=int(row["level"])
            if (not toc or row is toc[0]) and level != 1 or level > previous + 1:
                raise ValueError("bookmark levels must start at 1 and cannot skip a level")
            previous=level
    except Exception as exc:
        raise PdfError("BAD_ARG", f"invalid bookmark list: {exc}", "use --set with a JSON list of level/title/page objects")
    def writer(doc, target):
        if any(int(r["page"])>doc.page_count for r in toc):
            raise PdfError("BAD_PAGE","bookmark target is outside the document","choose a page within the input document")
        doc.set_toc([] if args.clear else [[int(r["level"]), r["title"], int(r["page"])] for r in toc])
        doc.save(target)
        return OperationResult(features=Features(must_survive={"bookmarks":0 if args.clear else len(toc)}))
    result=write_pdf(path, _spec_named("bookmarks"), args, context, writer, operation=OperationResult())
    if result.get("type")=="result" and Path(result.get("path","")).is_file():
        with pymupdf.open(result["path"]) as check_doc:
            actual=[row[:3] for row in check_doc.get_toc()]
        expected=[] if args.clear else [[int(r["level"]),r["title"],int(r["page"])] for r in toc]
        if actual!=expected:
            Path(result["path"]).unlink(missing_ok=True)
            raise PdfError("CHECK_FAILED","bookmark structure did not read back","review bookmark levels, titles, and targets")
    return result


def add_arguments_labels(parser):
    parser.add_argument("file", type=Path); parser.add_argument("--set", dest="items", type=json.loads, required=True)
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--password-file", type=Path); parser.add_argument("--password-env")
    parser.add_argument("--owner-password-file", type=Path); parser.add_argument("--owner-password-env")
    parser.add_argument("--ignore-restrictions", action="store_true"); parser.add_argument("--break-signature", action="store_true")


@command("labels", 4, "pdf", "pdf", "_labels", True, "structure")
def labels(args, context):
    mapping = {"roman": "r", "Roman": "R", "arabic": "D", "alpha": "a", "Alpha": "A", "none": ""}
    try:
        entries = []
        for row in args.items:
            if not isinstance(row, dict) or row["style"] not in mapping: raise ValueError("invalid label item")
            entries.append({"startpage": int(row["from"]) - 1, "style": mapping[row["style"]],
                            "prefix": str(row.get("prefix", "")), "firstpagenum": int(row.get("start", 1))})
    except Exception as exc:
        raise PdfError("BAD_ARG", f"invalid page labels: {exc}", "use --set with a JSON list of from/style/prefix/start objects")
    def writer(doc, target):
        if any(row["startpage"]<0 or row["startpage"]>=doc.page_count for row in entries):
            raise PdfError("BAD_PAGE","page-label start is outside the document","choose a from page within the input document")
        doc.set_page_labels(entries); doc.save(target); return OperationResult()
    result=write_pdf(args.file.resolve(), _spec_named("labels"), args, context, writer, operation=OperationResult())
    if result.get("type")=="result" and Path(result.get("path","")).is_file():
        with pymupdf.open(result["path"]) as check_doc:
            actual=[check_doc[i].get_label() for i in range(check_doc.page_count)]
        expected=[]
        rules=sorted(entries,key=lambda item:item["startpage"])
        for page_no in range(len(actual)):
            rule=next((rule for rule in reversed(rules) if rule["startpage"]<=page_no),None)
            if rule is None: expected.append(str(page_no+1));continue
            number=rule["firstpagenum"]+page_no-rule["startpage"]
            style=rule["style"]
            if style=="r": number_label=_roman(number).lower()
            elif style=="R": number_label=_roman(number).upper()
            elif style in ("a","A"):
                number_label=chr((number-1)%26+ord("a" if style=="a" else "A"))
            elif style=="D": number_label=str(number)
            else: number_label=""
            expected.append(rule["prefix"]+number_label)
        if actual!=expected:
            Path(result["path"]).unlink(missing_ok=True)
            raise PdfError("CHECK_FAILED","page labels did not read back","review the label rules")
    return result


def _spec_named(name):
    from ..registry import get
    return get(name)
