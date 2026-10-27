"""Multi-file assembly and splitting commands for phase 3b."""

from pathlib import Path
from copy import copy
import os

import pymupdf

from .. import io
from ..errors import BadArg
from ..pipeline import open_input, write_pdf
from ..registry import command
from ..secrets import strip_one_newline
from . import p3b_organize as org


def _plain(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("files", nargs="*")
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")
    parser.add_argument("--owner-password-file", type=Path)
    parser.add_argument("--owner-password-env")
    parser.add_argument("--ignore-restrictions", action="store_true")
    parser.add_argument("--break-signature", action="store_true")


def _register(name, func, builder, suffix=None, policy="placements"):
    globals()["add_arguments"] = builder
    command(name, "3b", "pdf", "pdf", suffix or "_" + name, True, policy)(func)


def _build_merge(p):
    _plain(p); p.add_argument("--toc", action="store_true")


def _build_interleave(p):
    p.add_argument("file", type=Path); p.add_argument("other", type=Path)
    p.add_argument("-o", "--output", type=Path)
    p.add_argument("--allow-uneven", action="store_true"); p.add_argument("--reverse-back", action="store_true")
    p.add_argument("--password-file", type=Path); p.add_argument("--password-env")
    p.add_argument("--owner-password-file", type=Path); p.add_argument("--owner-password-env")
    p.add_argument("--ignore-restrictions", action="store_true"); p.add_argument("--break-signature", action="store_true")


def _build_split(p):
    p.add_argument("file", type=Path); p.add_argument("-o", "--output")
    group = p.add_mutually_exclusive_group(required=True)
    group.add_argument("--every", type=int); group.add_argument("--ranges", nargs="+"); group.add_argument("--bookmarks", action="store_true")
    p.add_argument("--password-file", type=Path); p.add_argument("--password-env")
    p.add_argument("--owner-password-file", type=Path); p.add_argument("--owner-password-env")
    p.add_argument("--ignore-restrictions", action="store_true"); p.add_argument("--break-signature", action="store_true")


def merge(args, context):
    base = Path(getattr(args, "job_base", Path.cwd()))
    paths = [Path(args.file).resolve()]
    input_password_files = [None]
    for value in args.files:
        if isinstance(value, dict):
            if set(value) - {"file", "password_file"} or not isinstance(value.get("file"), str):
                raise BadArg("merge input entries need a file path and optional password_file")
            raw_path, password_file = value["file"], value.get("password_file")
            if password_file is not None and not isinstance(password_file, str):
                raise BadArg("merge input password_file must be a path string")
        else:
            raw_path, password_file = value, None
        p = Path(raw_path)
        paths.append((base / p if not p.is_absolute() else p).resolve())
        input_password_files.append(password_file)
    if len(paths) < 2: raise BadArg("merge needs at least two input PDFs")
    recipe = []
    for path, password_file in zip(paths, input_password_files):
        item = {"file": str(path), "pages": "1-"}
        input_password = context.password or context.owner_password
        if password_file:
            password_path = Path(password_file)
            if not password_path.is_absolute(): password_path = base / password_path
            item["password_file"] = str(password_path.resolve())
            input_password = context.secret_store.add(strip_one_newline(password_path.read_text(encoding="utf-8-sig")))
        with open_input(path, input_password) as doc:
            item["pages"] = f"1-{len(doc)}"
        recipe.append(item)
    return org._execute(args, context, recipe, extra_inputs=paths[1:], command_name="merge")


def interleave(args, context):
    first, second = Path(args.file).resolve(), Path(args.other).resolve()
    with open_input(first, context.password or context.owner_password) as a, open_input(second, context.password or context.owner_password) as b:
        if len(a) != len(b) and not args.allow_uneven:
            raise BadArg("interleave inputs have unequal page counts", [f"first: {len(a)} pages", f"second: {len(b)} pages", "use --allow-uneven to append leftovers"])
        back = list(range(len(b) - 1, -1, -1)) if args.reverse_back else list(range(len(b)))
        recipe = []
        for i in range(max(len(a), len(b))):
            if i < len(a): recipe.append({"file": str(first), "pages": str(i + 1)})
            if i < len(b): recipe.append({"file": str(second), "pages": str(back[i] + 1)})
    return org._execute(args, context, recipe, extra_inputs=[second], command_name="interleave")


def _safe_title(text):
    value = "".join("_" if c in '<>:"/\\|?*' or ord(c) < 32 else c for c in str(text)).strip(" .")
    return value[:60] or "part"


def _split_parts(doc, args):
    if args.every is not None:
        if args.every < 1: raise BadArg("--every must be positive")
        return [(f"part{i // args.every + 1:02d}", list(range(i, min(i + args.every, len(doc))))) for i in range(0, len(doc), args.every)]
    if args.ranges:
        parts = []
        for value in args.ranges:
            pages, bad = org._page_list(value, len(doc))
            if bad: raise BadArg(f"invalid split range: {bad[0]}", bad)
            label = value.replace(",", "-")
            parts.append(("p" + label, pages))
        return parts
    toc = doc.get_toc()
    top = [entry for entry in toc if entry[0] == 1]
    if not top: raise BadArg("PDF has no top-level bookmarks to split")
    parts = []
    for i, entry in enumerate(top):
        start = 0 if i == 0 else entry[2] - 1
        end = (top[i + 1][2] - 1) if i + 1 < len(top) else len(doc)
        parts.append((_safe_title(entry[1]), list(range(start, end))))
    return parts


def split(args, context):
    source = Path(args.file).resolve()
    password = context.owner_password or context.password
    with open_input(source, password) as doc:
        parts = _split_parts(doc, args)
        if not parts or any(not pages for _, pages in parts): raise BadArg("split produced an empty part")
        page_count = len(doc)
        labels, used = [], set()
        for label, _ in parts:
            base_label, suffix = label, 2
            while label.lower() in used:
                label = f"{base_label}_{suffix}"; suffix += 1
            used.add(label.lower()); labels.append(label)
        parts = [(labels[i], pages) for i, (_, pages) in enumerate(parts)]
    raw_output = args.output
    output_text = str(raw_output) if raw_output is not None else None
    directory_intent = bool(output_text and output_text.endswith(("/", "\\")))
    if raw_output is None:
        target = source.parent / f"{source.stem}_split"
    else:
        output_path = Path(raw_output)
        if not output_path.is_absolute(): output_path = Path(getattr(args, "job_base", Path.cwd())) / output_path
        output_path = output_path.resolve()
        if output_path.is_dir() or directory_intent:
            target = output_path / f"{source.stem}_split"
        else:
            raise BadArg("split output requires a folder", ["use -o with an existing or trailing-separator folder"])
    if raw_output is not None and (not Path(output_path).is_dir() and not directory_intent):
        raise BadArg("split output requires a folder", ["use -o with an existing or trailing-separator folder"])
    if args.dry_run:
        return {"type": "result", "command": "split", "path": str(target), "kind": "folder", "pages": page_count,
                "bytes": 0, "verdict": "report", "provisional": True,
                "page_map": [f"{name}: {','.join(str(n+1) for n in pages)}" for name, pages in parts], "warnings": []}
    target.parent.mkdir(parents=True, exist_ok=True)
    if len(str(target)) > 240: raise BadArg(f"output path exceeds 240 characters: {target}", ["use a shorter -o"])
    with io.temp_folder() as temp:
        io.disk_budget([source], target, temp, estimate=source.stat().st_size * 3)
        with io.Staging(target, temp) as stage:
            stage.path.mkdir(parents=True, exist_ok=False)
            staged_names, findings, warnings, log_paths = [], [], [], []
            for label, pages in parts:
                name = f"{source.stem}_{label}.pdf"
                staged = stage.path / name
                with open_input(source, password) as original:
                    op_rows = [{"source": str(source), "page": n, "rotation": None, "absolute": False} for n in pages]
                    operation = org._operation(op_rows, {str(source): (original, source)})
                    operation.features.must_survive.pop("labels", None)
                    label_count = len(original.get_page_labels())
                    if label_count: operation.features.dropped["labels"] = label_count
                    restore_catalog = org._catalog_features_present(original)
                part_args = copy(args)
                part_args.output = staged
                def writer(src, destination):
                    src.select(pages)
                    src.set_page_labels([])
                    src.save(destination, garbage=3, deflate=True)
                    if restore_catalog:
                        org._restore_catalog_features(source, destination, password, op_rows)
                    return operation
                result = write_pdf(source, _spec(), part_args, context, writer, operation=operation)
                staged_names.append(Path(result["path"]).name)
                findings.extend(result.get("findings", []))
                warnings.extend(result.get("warnings", []))
                if result.get("log"): log_paths.append(result["log"])
            published = stage.publish()
    if published != target:
        warnings.append("NAME_TAKEN")
        __import__("clierr").warn("pdf split", "NAME_TAKEN", "split output folder already existed; published under a numbered name", "review the output path")
    __import__("clierr").warn("pdf split", "LABELS_DROPPED", "split parts do not retain page labels", "review the output")
    warnings.append("LABELS_DROPPED")
    warnings = list(dict.fromkeys(warnings))
    if "LABELS_DROPPED" not in warnings:
        __import__("clierr").warn("pdf split", "LABELS_DROPPED", "split parts do not retain page labels", "review the output")
        warnings.append("LABELS_DROPPED")
    return {"type": "result", "command": "split", "path": str(published), "kind": "folder", "pages": page_count,
            "bytes": sum((published / name).stat().st_size for name in staged_names),
            "verdict": "unchecked" if context.exit_code == 3 else ("warn" if warnings else "ok"),
            "warnings": warnings, "findings": findings, "log_paths": log_paths, "page_map": staged_names}


_register("merge", merge, _build_merge)
_register("interleave", interleave, _build_interleave)
_register("split", split, _build_split, "_split")


def _spec():
    from ..registry import get
    return get("split")
