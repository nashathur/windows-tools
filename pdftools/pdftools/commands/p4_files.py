"""File naming, blank-page removal, and PDF repair commands."""
import hashlib
import io
import json
import os
import re
import time
from pathlib import Path

import pymupdf

from ..check import OperationResult, Placement
from ..errors import PdfError
from ..io import rename_exclusive
from ..pipeline import open_input, write_pdf
from ..registry import command


def _add_arguments(parser, name):
    if name=="rename":
        parser.add_argument("file",type=Path,nargs="?"); parser.add_argument("--apply",action="store_true"); parser.add_argument("--undo",type=Path)
        parser.add_argument("--title"); parser.add_argument("--recursive",action="store_true")
    elif name=="rmblank":
        parser.add_argument("file",type=Path); parser.add_argument("--include-uncertain",action="store_true")
    else:
        parser.add_argument("file",type=Path)
    if name == "protect":
        parser.add_argument("--new-password-file", type=Path)
        parser.add_argument("--new-password-env")
        parser.add_argument("--new-owner-password-file", type=Path)
        parser.add_argument("--new-owner-password-env")
        parser.add_argument("--no-print", action="store_true")
        parser.add_argument("--no-copy", action="store_true")
        parser.add_argument("--no-modify", action="store_true")
        parser.add_argument("--no-assemble", action="store_true")
    parser.add_argument("-o","--output",type=Path)
    parser.add_argument("--password-file",type=Path);parser.add_argument("--password-env")
    parser.add_argument("--owner-password-file",type=Path);parser.add_argument("--owner-password-env")
    parser.add_argument("--ignore-restrictions",action="store_true");parser.add_argument("--break-signature",action="store_true")


def add_arguments_rename(parser): _add_arguments(parser, "rename")
def add_arguments_rmblank(parser): _add_arguments(parser, "rmblank")
def add_arguments_repair(parser): _add_arguments(parser, "repair")
def add_arguments_protect(parser): _add_arguments(parser, "protect")
def add_arguments_unlock(parser): _add_arguments(parser, "unlock")


def _spec(name):
    from ..registry import get
    return get(name)


def _is_encrypted(path):
    import pikepdf
    try:
        with pikepdf.open(path) as pdf:
            return bool(pdf.is_encrypted)
    except pikepdf.PasswordError:
        return True


def _protect_policy(source, output, operation):
    from ..check import compare, Finding
    findings = compare(source, output, "placements", operation)
    try:
        encrypted=_is_encrypted(output.name)
    except Exception as exc:
        findings.append(Finding("invariant","ENCRYPTION_READBACK","protected output encryption could not be verified",details={"error":str(exc)}))
        encrypted=False
    if not encrypted:
        findings.append(Finding("invariant", "ENCRYPTION_MISSING", "protected output is not encrypted"))
    return findings


def _unlock_policy(source, output, operation):
    from ..check import compare, Finding
    findings = compare(source, output, "placements", operation)
    try:
        encrypted=_is_encrypted(output.name)
    except Exception as exc:
        findings.append(Finding("invariant","ENCRYPTION_READBACK","unlocked output encryption could not be verified",details={"error":str(exc)}))
        encrypted=True
    if encrypted:
        findings.append(Finding("invariant", "ENCRYPTION_REMAINS", "unlocked output is still encrypted"))
    return findings


def _digest(path):
    h=hashlib.sha256()
    with open(path,"rb") as stream:
        for chunk in iter(lambda:stream.read(1024*1024),b""):h.update(chunk)
    return h.hexdigest()


def _safe_name(value):
    name=re.sub(r'[<>:"/\\|?*\x00-\x1f]',"_",str(value)).rstrip(" .")[:100]
    if not name:name="untitled"
    if name.split(".",1)[0].upper() in {"CON","PRN","AUX","NUL",*(f"COM{i}" for i in range(1,10)),*(f"LPT{i}" for i in range(1,10))}:
        stem, dot, suffix = name.partition(".")
        name = stem + "_" + dot + suffix
    return name


def _manifest_write(path,data):
    temp=path.with_suffix(path.suffix+".part")
    with open(temp,"w",encoding="utf-8",newline="\n") as f:
        json.dump(data,f,ensure_ascii=False,indent=2);f.write("\n");f.flush();os.fsync(f.fileno())
    os.replace(temp,path)
    try:
        fd=os.open(path.parent,os.O_RDONLY);os.fsync(fd);os.close(fd)
    except OSError:pass


@command("rename",4,"any","report","",False,"report")
def rename(args,context):
    if args.undo:
        manifest=args.undo.resolve()
        try:data=json.loads(manifest.read_text(encoding="utf-8"))
        except Exception as exc:raise PdfError("BAD_ARG",f"cannot read rename manifest: {exc}","provide a valid manifest file")
        if getattr(args, "dry_run", False):
            return {"type":"report","command":"rename","input":str(manifest),"verdict":"report",
                    "data":{"undo_preview":[row.get("old") for row in reversed(data.get("files", []))]}}
        rows=data.get("files",[]); restored=[]
        for row in reversed(rows):
            folder=manifest.parent; old=folder/row["old"]; temp=folder/row["temp"]; new=folder/row["new"]
            current=next((p for p in (old,temp,new) if p.exists()),None)
            if current is None:continue
            if current.stat().st_size!=row["size"] or _digest(current)!=row["sha256"]:
                raise PdfError("BAD_INPUT",f"file identity changed: {current.name}","restore only files whose size and SHA-256 match the manifest")
            if old.exists() and old.resolve()!=current.resolve():
                raise PdfError("NAME_TAKEN",f"original name is now taken: {old.name}","move the conflicting file, then retry --undo")
            rename_exclusive(current,old) if current!=old else None;row["state"]="restored";_manifest_write(manifest,data);restored.append(str(old))
        return {"type":"report","command":"rename","input":str(manifest),"verdict":"report","data":{"restored":restored}}
    if args.file is None:raise PdfError("BAD_ARG","rename requires a file or folder unless --undo is used","provide a PDF path")
    root=args.file.resolve()
    files=[root] if root.is_file() else sorted(root.rglob("*.pdf") if args.recursive else root.glob("*.pdf")) if root.is_dir() else []
    if not files:raise PdfError("NOT_FOUND",f"no PDF files found: {root}","check the input path")
    plan=[]
    reserved=set()
    for path in files:
        try:
            with pymupdf.open(path) as doc:
                title=args.title or doc.metadata.get("title") or next((line.strip() for line in doc[0].get_text().splitlines() if line.strip()),path.stem)
        except Exception as exc:raise PdfError("BAD_INPUT",f"cannot read title from {path.name}","use --title or check the PDF",detail=str(exc))
        base=_safe_name(title); candidate=path.with_name(base+path.suffix); n=2
        while ((candidate.name.casefold() in reserved) or
               (candidate.exists() and candidate.resolve()!=path.resolve())):
            candidate=path.with_name(f"{base}_{n}{path.suffix}");n+=1
        reserved.add(candidate.name.casefold())
        plan.append({"old":path.relative_to(root.parent).as_posix(),"new":candidate.relative_to(root.parent).as_posix(),
                     "size":path.stat().st_size,"sha256":_digest(path),"state":"old"})
    if not args.apply or getattr(args, "dry_run", False):
        return {"type":"report","command":"rename","input":str(root),"verdict":"report","data":[{"old":r["old"],"new":r["new"]} for r in plan]}
    stamp=time.strftime("%Y%m%d-%H%M%S")+f"-{os.getpid()}"; manifest=root.parent/f"rename_manifest_{stamp}.json"
    for i,row in enumerate(plan):
        parent=Path(row["old"]).parent
        row["temp"]=(parent/f".pdftools-rename-{stamp}-{i}.tmp").as_posix()
    data={"version":1,"files":plan};_manifest_write(manifest,data)
    try:
        for i,row in enumerate(plan):
            old=root.parent/row["old"];temp=root.parent/row["temp"]
            rename_exclusive(old,temp);row["state"]="temp";_manifest_write(manifest,data)
        for row in plan:
            temp=root.parent/row["temp"];new=root.parent/row["new"]
            if new.exists():raise PdfError("NAME_TAKEN",f"target name is taken: {new.name}","choose another title or resolve the collision")
            rename_exclusive(temp,new);row["state"]="new";_manifest_write(manifest,data)
    except Exception:
        raise
    return {"type":"report","command":"rename","input":str(root),"verdict":"report","data":{"renamed":[r["new"] for r in plan],"manifest":str(manifest)}}


@command("rmblank",4,"pdf","pdf","_rmblank",True,"rmblank")
def rmblank(args,context):
    path=args.file.resolve()
    def writer(doc,target):
        keep=[];removed=[];uncertain=[];ink_values=[]
        import cv2
        import numpy as np
        for i,page in enumerate(doc):
            pix=page.get_pixmap(matrix=pymupdf.Matrix(50/72,50/72),colorspace=pymupdf.csGRAY,alpha=False,annots=False)
            gray=np.frombuffer(pix.samples,dtype=np.uint8).reshape(pix.height,pix.width)
            ink=float(np.mean(gray<245));n,labels,stats,cent=cv2.connectedComponentsWithStats((gray<245).astype(np.uint8),8)
            ink_values.append(ink)
            max_mm=max((max(stats[j,cv2.CC_STAT_WIDTH],stats[j,cv2.CC_STAT_HEIGHT])*25.4/50 for j in range(1,n)),default=0)
            has_features=bool(page.get_links() or list(page.annots() or []) or list(page.widgets() or []))
            no_text=not page.get_text("words")
            maybe=(not has_features and no_text and (.001<=ink<.005 or max_mm>3))
            candidate=no_text and not has_features and ink<.005 and max_mm<=3 and not maybe
            if candidate:removed.append(i)
            elif maybe:
                uncertain.append({"page":i+1,"ink_percent":round(ink*100,4)})
                if args.include_uncertain:removed.append(i)
                else:keep.append(i)
            else:keep.append(i)
        if not keep:raise PdfError("BAD_PAGE","blank-page removal would remove every page","keep at least one page or use a different input")
        had_labels=bool(doc.get_page_labels())
        labels=[doc[i].get_label() for i in keep]
        doc.select(keep)
        if had_labels:
            doc.set_page_labels([{"startpage":i,"prefix":label,"style":""} for i,label in enumerate(labels)])
        doc.save(target)
        context.rmblank_report={"removed":[{"page":i+1,"ink_percent":round(ink_values[i]*100,4)} for i in removed],
                                "uncertain":uncertain}
        op=OperationResult(pages=[[Placement(None,i)] for i in keep])
        return op
    result=write_pdf(path,_spec("rmblank"),args,context,writer)
    if hasattr(context,"rmblank_report"):
        result.update(context.rmblank_report)
    return result


@command("repair",4,"pdf","pdf","_repaired",True,"report")
def repair(args,context):
    path = args.file.resolve()
    report = {"pages": 0, "warnings": []}

    def opener(source, password):
        try:
            doc = pymupdf.open(source)
            if doc.needs_pass and not doc.authenticate(password or ""):
                doc.close()
                raise PdfError("WRONG_PASSWORD", "PDF requires a valid password", "supply --password-file or --password-env")
            if doc.page_count < 1:
                doc.close()
                raise ValueError("missing page tree")
            report["pages"] = doc.page_count
            if getattr(doc, "is_repaired", False):
                report["warnings"].append("PyMuPDF rebuilt the cross-reference table")
            return doc
        except PdfError:
            raise
        except Exception as pymupdf_error:
            try:
                import pikepdf
                with pikepdf.open(source, password=password or "") as recovered:
                    if not recovered.pages:
                        raise ValueError("missing page tree")
                    warnings = list(recovered.get_warnings()) if hasattr(recovered, "get_warnings") else []
                    if any("Pages tree includes non-dictionary object" in str(item) for item in warnings):
                        raise ValueError("missing page tree; unrecoverable damaged PDF")
                    stream = io.BytesIO()
                    recovered.save(stream)
                    data = stream.getvalue()
                    doc = pymupdf.open(stream=data, filetype="pdf")
                    if doc.page_count < 1:
                        doc.close()
                        raise ValueError("missing page tree")
                    report["pages"] = doc.page_count
                    report["warnings"].append("Recovered with pikepdf/qpdf after PyMuPDF could not open the input")
                    report["warnings"].extend(str(item) for item in warnings)
                    return doc
            except PdfError:
                raise
            except Exception as recovery_error:
                raise PdfError("DAMAGED", f"unrecoverable PDF damage: {recovery_error}",
                               "use an intact copy or a PDF recovery tool") from pymupdf_error

    def writer(doc, target):
        try:
            doc.save(target, garbage=3, deflate=True)
        except Exception as pymupdf_error:
            try:
                import pikepdf
                with pikepdf.open(path, password=context.password or context.owner_password or "") as recovered:
                    if not recovered.pages:
                        raise ValueError("missing page tree")
                    recovered.save(target)
                report["warnings"].append("Recovered with pikepdf/qpdf after PyMuPDF could not save the input")
            except Exception as recovery_error:
                raise PdfError("DAMAGED", f"cannot save repaired PDF: {recovery_error}",
                               "use an intact copy or a PDF recovery tool") from pymupdf_error
        return OperationResult(pages=[[Placement(None, i)] for i in range(doc.page_count)])

    result = write_pdf(path, _spec("repair"), args, context, writer, opener=opener)
    result["recovered_pages"] = report["pages"]
    result["recovery_warnings"] = list(dict.fromkeys(report["warnings"]))
    return result


def _new_secret(args, file_option, env_option, context):
    from ..secrets import strip_one_newline
    import os
    file_path = getattr(args, file_option, None)
    env_name = getattr(args, env_option, None)
    if file_path and env_name:
        raise PdfError("BAD_ARG", f"choose either --{file_option.replace('_', '-')} or --{env_option.replace('_', '-')}",
                       "provide one new-password secret source")
    if file_path:
        try:
            value = strip_one_newline(Path(file_path).read_text(encoding="utf-8-sig"))
        except Exception as exc:
            raise PdfError("BAD_ARG", f"cannot read new password file: {exc}", "provide a readable UTF-8 secret file")
    elif env_name:
        value = os.environ.get(env_name)
        if context.secret_store is not None:
            context.secret_store.env_names.append(env_name)
    else:
        return None
    if value is None:
        raise PdfError("BAD_ARG", "new password environment variable is not set", "set the named environment variable")
    if context.secret_store is not None:
        context.secret_store.add(value)
    return value


@command("protect",4,"pdf","pdf","_protected",True,_protect_policy)
def protect(args,context):
    user_password = _new_secret(args, "new_password_file", "new_password_env", context)
    owner_password = _new_secret(args, "new_owner_password_file", "new_owner_password_env", context)
    if user_password is None and owner_password is None:
        raise PdfError("BAD_ARG", "protect requires a new user or owner password", "use --new-password-file or --new-owner-password-file (or their -env forms)")
    permissions = -1
    for option, bit in (("no_print", pymupdf.PDF_PERM_PRINT), ("no_copy", pymupdf.PDF_PERM_COPY),
                        ("no_modify", pymupdf.PDF_PERM_MODIFY), ("no_assemble", pymupdf.PDF_PERM_ASSEMBLE)):
        if getattr(args, option, False):
            permissions &= ~bit
    return write_pdf(args.file.resolve(), _spec("protect"), args, context,
                     lambda doc, target: doc.save(target, garbage=3, deflate=True),
                     encrypt_output={"user_password": user_password, "owner_password": owner_password,
                                     "permissions": permissions})


@command("unlock",4,"pdf","pdf","_unlocked",True,_unlock_policy)
def unlock(args,context):
    return write_pdf(args.file.resolve(), _spec("unlock"), args, context,
                     lambda doc, target: doc.save(target, garbage=3, deflate=True), decrypt_output=True)
