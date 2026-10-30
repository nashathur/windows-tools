"""Remove active content and selected metadata from PDFs."""
from pathlib import Path

import pymupdf
import pikepdf

from .. import coords
from ..check import ExpectedChanges, Finding, OperationResult, feature_records, page_words, render_rgb, tile_differences
from ..errors import PdfError
from ..pipeline import write_pdf
from ..registry import command, get
import clierr


def _dictlike(value):
    return isinstance(value,(pikepdf.Dictionary,pikepdf.Stream))


def add_arguments_sanitize(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("--external", action="store_true")
    parser.add_argument("--multimedia", action="store_true")
    parser.add_argument("--keep", default="")
    parser.add_argument("--keep-failed", action="store_true")
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--each", action="store_true")
    parser.add_argument("--password-file", type=Path); parser.add_argument("--password-env")
    parser.add_argument("--owner-password-file", type=Path); parser.add_argument("--owner-password-env")
    parser.add_argument("--ignore-restrictions", action="store_true"); parser.add_argument("--break-signature", action="store_true")


def _walk(obj, seen=None):
    seen = set() if seen is None else seen
    try:
        generation = obj.objgen if hasattr(obj, "objgen") else (0, 0)
        if generation != (0, 0):
            if generation in seen: return
            seen.add(generation)
        yield obj
        if isinstance(obj, (pikepdf.Dictionary, pikepdf.Stream, pikepdf.Array)):
            values = obj.values() if isinstance(obj, (pikepdf.Dictionary, pikepdf.Stream)) else obj
            for item in list(values):
                yield from _walk(item, seen)
    except Exception:
        return


def _attachment_specs(pdf):
    specs = {}
    def add(spec):
        if spec is None:return
        generation=getattr(spec,"objgen",(0,0))
        identity=("indirect",generation) if generation!=(0,0) else ("direct",str(spec))
        specs[identity]=spec
    names = pdf.Root.get("/Names", {}).get("/EmbeddedFiles", {}).get("/Names", [])
    for i in range(1, len(names), 2): add(names[i])
    for item in _walk(pdf.Root):
        if isinstance(item, pikepdf.Dictionary) and str(item.get("/Type", "")) == "/Filespec": add(item)
    return list(specs.values())


def _counts(pdf, external=False, multimedia=False):
    js = ext = media = af = 0
    for item in _walk(pdf.Root):
        if not _dictlike(item): continue
        action = str(item.get("/S", ""))
        if action == "/JavaScript": js += 1
        if action in {"/Launch", "/URI", "/SubmitForm", "/ImportData", "/GoToR", "/GoToE"}: ext += 1
        if action in {"/Rendition", "/RichMediaExecute", "/Movie", "/Sound"}: media += 1
        if str(item.get("/Type", "")) == "/Rendition": media += 1
        subtype=str(item.get("/Subtype", ""))
        if ((subtype=="/Movie" and "/Movie" in item) or (subtype=="/Sound" and "/Sound" in item) or
            (subtype=="/RichMedia" and ("/RichMediaContent" in item or "/RichMediaSettings" in item)) or
            (subtype=="/3D" and "/3DD" in item)): media += 1
        af += len(item.get("/AF", []))
    tree = pdf.Root.get("/Names", {}).get("/JavaScript", {}).get("/Names", [])
    js = max(js, (len(tree) // 2))
    embedded = len(pdf.Root.get("/Names", {}).get("/EmbeddedFiles", {}).get("/Names", [])) // 2
    annots = 0
    for page in pdf.pages:
        for annot in page.get("/Annots", []):
            if str(annot.get("/Subtype", "")) == "/FileAttachment": annots += 1
    attachments = max(embedded, len(_attachment_specs(pdf))) + annots
    thumbs = sum(1 for page in pdf.pages if page.get("/Thumb"))
    renditions = pdf.Root.get("/Names", {}).get("/Renditions", {}).get("/Names", [])
    media += len(renditions)//2
    info = len(pdf.docinfo)
    xmp = sum(1 for item in _walk(pdf.Root) if isinstance(item, pikepdf.Stream) and
              str(item.get("/Type", "")) == "/Metadata")
    return {"javascript": js, "attachments": attachments, "thumbnails": thumbs,
            "info": info, "xmp": xmp, "external": ext if external else 0,
            "multimedia": media if multimedia else 0}


def _remove_af_and_attachment_annots(pdf):
    for item in _walk(pdf.Root):
        if _dictlike(item):
            if "/AF" in item: del item["/AF"]
    for page in pdf.pages:
        annots = page.get("/Annots", pikepdf.Array())
        kept = pikepdf.Array()
        for annot in annots:
            if str(annot.get("/Subtype", "")) == "/FileAttachment":
                # remove_attachments defangs these too; remove the annotation so
                # neither its file specification nor its icon remains reachable.
                continue
            else: kept.append(annot)
        if len(kept): page["/Annots"] = kept
        elif "/Annots" in page: del page["/Annots"]


def _remove_action_kinds(pdf, kinds):
    visited=set()
    def clean(value):
        if _dictlike(value):
            gen=value.objgen
            if gen!=(0,0):
                if gen in visited:return value
                visited.add(gen)
            if str(value.get("/S","")) in kinds:return None
            for key,item in list(value.items()):
                replacement=clean(item)
                if replacement is None:del value[key]
                elif replacement is not item:value[key]=replacement
            return value
        if isinstance(value,pikepdf.Array):
            kept=[]
            for item in list(value):
                replacement=clean(item)
                if replacement is not None:kept.append(replacement)
            value.clear()
            value.extend(kept)
        return value
    clean(pdf.Root)
    names=pdf.Root.get("/Names")
    if names is not None and "/JavaScript" in names:names.pop("/JavaScript")


def _run_sanitize(pdf, keep, external, multimedia):
    before = _counts(pdf, external, multimedia)
    if "attachments" not in keep:
        pikepdf.sanitize.remove_attachments(pdf)
        _remove_af_and_attachment_annots(pdf)
    if "thumbnails" not in keep: pikepdf.sanitize.remove_thumbnails(pdf)
    pikepdf.sanitize.remove_javascript(pdf)
    _remove_action_kinds(pdf,{"/JavaScript"})
    if external: pikepdf.sanitize.remove_external_access(pdf)
    if external:_remove_action_kinds(pdf,{"/Launch","/URI","/SubmitForm","/ImportData","/GoToR","/GoToE"})
    if multimedia: pikepdf.sanitize.remove_multimedia(pdf)
    if "meta" not in keep:
        for key in list(pdf.docinfo.keys()): del pdf.docinfo[key]
        for item in list(_walk(pdf.Root)):
            if _dictlike(item) and "/Metadata" in item:
                del item["/Metadata"]
    return before


def _hidden(doc):
    result = {}
    for i, page in enumerate(doc, 1):
        count = 0
        for span in page.get_texttrace():
            if span.get("font") == "GlyphLessFont" or span.get("type") == 3 or span.get("opacity", 1) == 0:
                count += len(span.get("chars", []))
        if count: result[i] = count
    return result


def _off_layers(doc):
    try:
        off = doc.layer_ui_configs()
        return [str(row.get("text", row.get("name", "layer"))) for row in off if not bool(row.get("on", True))]
    except Exception: return []


def _nonattachment_annots(doc):
    return sum(1 for page in doc for annot in (page.annots() or [])
               if annot.type[0] != pymupdf.PDF_ANNOT_FILE_ATTACHMENT)


def _policy(source, output, operation):
    findings = []
    settings = operation.measurements[0] if operation and operation.measurements else {}
    if source.page_count != output.page_count:
        findings.append(Finding("invariant", "PAGE_COUNT", "page count changed"))
    for i, (a, b) in enumerate(zip(source, output), 1):
        if page_words(a) != page_words(b): findings.append(Finding("invariant", "TEXT_CHANGED", "page words changed", i))
        changed = tile_differences(render_rgb(a, unrotated=True),
                                  render_rgb(b, unrotated=True), i, page=a)
        allowed = [r["rect"] for r in operation.expected.regions if r["page"] == i]
        for tile in changed:
            if not any(pymupdf.Rect(tile["box_mm"]).intersects(pymupdf.Rect(rect)) for rect in allowed):
                findings.append(Finding("invariant", "TILE_CHANGED", "page content changed outside attachment icons", i)); break
    try:
        with pikepdf.open(output.name, password=getattr(operation, "_run_password", "") or "") as pdf:
            after = _counts(pdf, bool(settings.get("external_enabled")), bool(settings.get("multimedia_enabled")))
        enabled = settings
        for kind, count in after.items():
            if kind in enabled.get("keep", []): continue
            if kind in {"info", "xmp"} and "meta" in enabled.get("keep", []): continue
            if kind == "external" and not enabled.get("external_enabled", False): continue
            if kind == "multimedia" and not enabled.get("multimedia_enabled", False): continue
            if count: findings.append(Finding("invariant", "SANITIZE_REMAINS", f"{kind} remains after sanitize", details={"kind": kind, "count": count}))
    except Exception as exc:
        findings.append(Finding("invariant", "STRUCTURE_READBACK", "sanitized structure could not be re-read", details={"error": str(exc)}))
    before_features, after_features = feature_records(source), feature_records(output)
    for kind in ("links", "widgets", "bookmarks"):
        expected = len(before_features[kind])
        expected_rows = list(before_features[kind])
        if kind == "links" and settings.get("external_enabled"):
            expected_rows = [x for x in expected_rows if x["kind"] != "uri"]
            expected = len(expected_rows)
        if after_features[kind] != expected_rows:
            findings.append(Finding("invariant", "FEATURE_CHANGED", f"{kind} count changed", details={"before": expected, "after": len(after_features[kind])}))
    before_annots,after_annots=_nonattachment_annots(source),_nonattachment_annots(output)
    if before_annots!=after_annots:
        findings.append(Finding("invariant","FEATURE_CHANGED","non-attachment annotation count changed",
                                details={"before":before_annots,"after":after_annots}))
    return findings


@command("sanitize", "7", "pdf", "pdf", "_sanitized", True, _policy)
def sanitize(args, context):
    path = args.file.resolve()
    keep = {x.strip() for x in args.keep.split(",") if x.strip()}
    invalid = keep - {"meta", "attachments", "thumbnails"}
    if invalid: raise PdfError("BAD_ARG", f"unknown --keep value: {sorted(invalid)[0]}", "use meta, attachments, thumbnails")
    run_data = {}
    def writer(doc, target):
        source = getattr(doc, "name", path)
        with pikepdf.open(source, password=context.owner_password or context.password or "") as pdf:
            before = _run_sanitize(pdf, keep, args.external, args.multimedia)
            pdf.save(target, linearize=False, compress_streams=True,
                     object_stream_mode=pikepdf.ObjectStreamMode.generate)
            after = _counts(pdf, args.external, args.multimedia)
        with pymupdf.open(source) as original:
            areas = []
            for i, page in enumerate(original, 1):
                for annot in page.annots() or []:
                    if annot.type[0] == pymupdf.PDF_ANNOT_FILE_ATTACHMENT:
                        rect=coords.annotation_to_pymupdf(page,annot)
                        areas.append({"page": i, "rect": tuple(coords.pymupdf_to_display(page,rect))})
            hidden, layers = _hidden(original), _off_layers(original)
        removed = {k: max(0, before[k]-after[k]) for k in before if before[k] and before[k] != after[k]}
        run_data.update(removed=removed, listed={"hidden_text": hidden, "layers_off": layers})
        op = OperationResult(expected=ExpectedChanges(regions=areas),
                             measurements=[{"before": before, "after": after, "external_enabled": args.external,
                                            "multimedia_enabled": args.multimedia, "keep": sorted(keep),
                                            "listed": run_data["listed"]}])
        op._run_password = context.owner_password or context.password or ""
        return op
    def run_one(source_path):
        args.file = source_path
        run_data.clear()
        record = write_pdf(source_path, get("sanitize"), args, context, writer, security=True)
        if isinstance(record, dict) and record.get("type") == "result":
            record["removed"] = run_data.get("removed", {})
            listed = run_data.get("listed", {"hidden_text": {}, "layers_off": []})
            record["listed"] = listed
            labels={"javascript":"JavaScript","attachments":"attachment","thumbnails":"thumbnail",
                    "info":"Info key","xmp":"XMP packet","external":"external action","multimedia":"multimedia object"}
            removed_text = ", ".join(f"{v} {labels[k]}{'s' if v != 1 and labels[k] not in {'JavaScript'} else ''}"
                                      for k, v in record["removed"].items() if v) or "nothing to remove"
            record["note"] = f"removed: {removed_text}; hidden text and OFF layers are listed, not removed"
            if (listed["hidden_text"] or listed["layers_off"]) and record.get("verdict") == "ok":
                record.setdefault("warnings", []).append("HIDDEN_CONTENT")
                record["verdict"] = "warn"
                clierr.warn("pdf sanitize", "HIDDEN_CONTENT", "hidden text or optional-content layers are listed but remain",
                            "review listed pages and layers")
        return record
    if args.each:
        from ..pipeline import run_each
        paths = sorted(path.parent.glob(path.name)) if any(c in str(path) for c in "*?[") else sorted(path.glob("*.pdf")) if path.is_dir() else [path]
        return run_each(paths, run_one, "sanitize", context)
    return run_one(path)
