"""Page recipes and the small commands that compile to them."""

import math
import re
import sys
import time
from collections import Counter
from pathlib import Path

import pymupdf

from .. import io
from ..check import Features, OperationResult, Placement, compare, feature_records
from ..errors import BadArg, PdfError
from ..pipeline import announce_estimate, open_input, write_pdf
from ..registry import command
from ..secrets import strip_one_newline


def add_arguments(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("recipe", nargs="?", help="comma-separated page recipe")
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--keep-rest", action="store_true")
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")
    parser.add_argument("--owner-password-file", type=Path)
    parser.add_argument("--owner-password-env")
    parser.add_argument("--ignore-restrictions", action="store_true")
    parser.add_argument("--break-signature", action="store_true")


def _page_list(spec, count, *, allow_rev=True):
    if isinstance(spec, (list, tuple)):
        raw = spec
    else:
        raw = str(spec).split(",")
    pages = []
    problems = []
    for bit in raw:
        text = str(bit).strip().lower()
        if allow_rev and text == "rev":
            pages.extend(range(count - 1, -1, -1)); continue
        if text in {"odd", "even"}:
            start = 0 if text == "odd" else 1
            pages.extend(range(start, count, 2)); continue
        match = re.fullmatch(r"(\d*)-(\d*)", text)
        if match:
            left, right = match.groups()
            a, b = int(left) if left else 1, int(right) if right else count
            if not 1 <= a <= count or not 1 <= b <= count:
                problems.append(f"range endpoint {a if not 1 <= a <= count else b} is outside 1-{count}")
                continue
            if not left or not right or a <= b:
                values = range(a, b + 1)
            else:
                values = range(a, b - 1, -1)
        elif text.isdigit():
            values = (int(text),)
        else:
            problems.append(f"invalid page selector: {bit}"); continue
        for n in values:
            if not 1 <= n <= count:
                problems.append(f"page {n} is outside 1-{count}")
            else:
                pages.append(n - 1)
    return pages, problems


def _validate_recipe(recipe, docs, keep_rest=False, job_base=None):
    problems, rows, listed = [], [], set()
    previous_size = None
    if not isinstance(recipe, list):
        return [], ["recipe must be a list of objects"]
    for i, item in enumerate(recipe):
        if not isinstance(item, dict):
            problems.append(f"recipe[{i}] must be an object"); continue
        keys = set(item)
        allowed = {"pages", "blank", "rotate", "rotate_abs", "times", "file", "password_file"}
        for key in sorted(keys - allowed):
            problems.append(f"recipe[{i}] has unknown key {key!r}")
        if "file" in item and not isinstance(item["file"], str):
            problems.append(f"recipe[{i}].file must be a path string")
            continue
        if "password_file" in item and not isinstance(item["password_file"], str):
            problems.append(f"recipe[{i}].password_file must be a path string")
            continue
        if ("pages" in item) == ("blank" in item):
            problems.append(f"recipe[{i}] must contain exactly one of pages or blank"); continue
        if "blank" in item:
            size = item["blank"]
            if size is True or size is None:
                first_doc = next(iter(docs.values()))[0]
                size = previous_size or (first_doc[0].rect.width, first_doc[0].rect.height)
            elif isinstance(size, str) and size.lower() in {"a4", "letter"}:
                w, h = (210, 297) if size.lower() == "a4" else (215.9, 279.4)
                size = (w / (25.4 / 72), h / (25.4 / 72))
            elif isinstance(size, str):
                m = re.fullmatch(r"\s*(\d+(?:\.\d+)?)\s*x\s*(\d+(?:\.\d+)?)\s*", size.lower())
                if not m:
                    problems.append(f"recipe[{i}].blank must be a4, letter, or WxH in mm"); continue
                size = tuple(float(v) / (25.4 / 72) for v in m.groups())
            elif isinstance(size, (list, tuple)) and len(size) == 2:
                try:
                    size = tuple(float(v) / (25.4 / 72) for v in size)
                except (TypeError, ValueError):
                    problems.append(f"recipe[{i}].blank must have positive dimensions"); continue
            else:
                problems.append(f"recipe[{i}].blank has invalid size"); continue
            if not all(math.isfinite(v) and v > 0 for v in size):
                problems.append(f"recipe[{i}].blank dimensions must be positive"); continue
            try:
                raw_times = item.get("times", 1)
                if isinstance(raw_times, bool) or not re.fullmatch(r"\d+", str(raw_times)): raise ValueError
                times = int(raw_times)
                if times < 1: raise ValueError
            except (ValueError, TypeError):
                problems.append(f"recipe[{i}].times must be a positive integer"); continue
            rotation, bad = _rotation(item, i)
            problems.extend(bad)
            if bad: continue
            for _ in range(times):
                rows.append({"source": "blank", "page": "blank", "occurrence": i, "rotation": rotation,
                             "absolute": "rotate_abs" in item, "size": size})
            previous_size = size
            continue
        first_path = next(iter(docs.values()))[1]
        path_value = item.get("file") or first_path
        path = Path(path_value)
        if not path.is_absolute(): path = Path(job_base or Path.cwd()) / path
        path = path.resolve()
        doc_pair = docs.get(str(path))
        if doc_pair is None:
            problems.append(f"recipe[{i}].file is not in the input set: {path}"); continue
        doc, _ = doc_pair
        pages, bad = _page_list(item["pages"], len(doc))
        problems.extend(f"recipe[{i}]: {message}" for message in bad)
        if bad: continue
        try:
            raw_times = item.get("times", 1)
            if isinstance(raw_times, bool) or not re.fullmatch(r"\d+", str(raw_times)): raise ValueError
            times = int(raw_times)
            if times < 1: raise ValueError
        except (ValueError, TypeError):
            problems.append(f"recipe[{i}].times must be a positive integer"); continue
        rotation, bad = _rotation(item, i)
        problems.extend(bad)
        if bad: continue
        for _ in range(times):
            for page_no in pages:
                listed.add((str(path), page_no))
                rows.append({"source": str(path), "page": page_no, "occurrence": i, "rotation": rotation,
                             "absolute": "rotate_abs" in item,
                             "size": (doc[page_no].rect.width, doc[page_no].rect.height)})
                out_rotation = (rotation if "rotate_abs" in item and rotation is not None else
                                (doc[page_no].rotation + (rotation or 0)) % 360)
                previous_size = ((doc[page_no].rect.height, doc[page_no].rect.width)
                                 if out_rotation % 180 else (doc[page_no].rect.width, doc[page_no].rect.height))
    if keep_rest:
        primary = next(iter(docs.values()))[1]
        for page_no in range(len(docs[str(primary)][0])):
            if (str(primary), page_no) not in listed:
                rows.append({"source": str(primary), "page": page_no, "occurrence": len(recipe), "rotation": None, "absolute": False,
                             "size": (docs[str(primary)][0][page_no].rect.width,
                                      docs[str(primary)][0][page_no].rect.height)})
    if not rows:
        problems.append("recipe produces no output pages")
    return rows, problems


def _rotation(item, index):
    problems = []
    rel, absolute = item.get("rotate"), item.get("rotate_abs")
    if rel is not None and absolute is not None:
        problems.append(f"recipe[{index}] cannot combine rotate and rotate_abs")
    value = absolute if absolute is not None else rel
    if value is None: return None, problems
    if isinstance(value, bool) or not isinstance(value, (int, float)) or value % 90:
        problems.append(f"recipe[{index}] rotation must be a multiple of 90")
        return None, problems
    return int(value) % 360, problems


def _shorthand(text):
    recipe, problems = [], []
    for token in text.split(","):
        token = token.strip().lower()
        if token.startswith("blank="):
            recipe.append({"blank": token.split("=", 1)[1]}); continue
        if token == "blank":
            recipe.append({"blank": True}); continue
        m = re.fullmatch(r"(odd|even|rev|\d+(?:-\d*)?|-\d+)(?:@(90|180|270))?(?:x(\d+))?", token)
        if not m:
            problems.append(f"invalid organize shorthand item: {token}")
            continue
        pages, rotation, times = m.groups()
        item = {"pages": pages}
        if rotation: item["rotate"] = int(rotation)
        if times: item["times"] = int(times)
        recipe.append(item)
    return recipe, problems


def _feature_counts(doc):
    return {"bookmarks": len(doc.get_toc()), "labels": len(doc.get_page_labels()),
            "links": sum(len(p.get_links()) for p in doc),
            "widgets": sum(sum(1 for _ in (p.widgets() or [])) for p in doc)}


def _operation(rows, docs):
    placements, expected, dropped = [], Counter(), {}
    for row in rows:
        if row["page"] == "blank":
            placements.append([Placement(None, "blank", rotation=0 if row["rotation"] is None else row["rotation"])])
            continue
        src = docs[row["source"]][0]
        page = src[row["page"]]
        rot = (row["rotation"] if row.get("absolute") and row["rotation"] is not None
               else (page.rotation + (row["rotation"] or 0)) % 360)
        placements.append([Placement(row["source"], row["page"], rotation=rot)])
        expected["widgets"] += sum(1 for _ in (page.widgets() or []))
        expected["annotations"] += sum(1 for _ in (page.annots() or []))
        expected["links"] += sum(1 for link in page.get_links()
                                  if link.get("kind") != pymupdf.LINK_GOTO or
                                  any(r["source"] == row["source"] and r["page"] == link.get("page") for r in rows))
    for path, (doc, _) in docs.items():
        retained = {r["page"] for r in rows if r["source"] == path and r["page"] != "blank"}
        expected["bookmarks"] += sum(1 for entry in doc.get_toc() if entry[2] - 1 in retained)
    path = next(iter(docs))
    source = docs[path][0]
    original_order = list(range(len(source)))
    row_order = [r["page"] for r in rows if r["page"] != "blank"]
    if len(docs) == 1 and row_order == original_order and all(r["page"] != "blank" for r in rows):
        expected["labels"] = len(source.get_page_labels())
    elif source.get_page_labels():
        dropped["labels"] = len(source.get_page_labels())
    records = {"bookmarks": [], "links": [], "widgets": []}
    page_map = {}
    for out_page, row in enumerate(rows, 1):
        if row["page"] != "blank":
            page_map.setdefault((row["source"], row["page"]), []).append(out_page)
    source_records_by_path = {}
    for path, (doc, _) in docs.items():
        source_records = feature_records(doc)
        source_records_by_path[path] = source_records
        for bookmark in source_records["bookmarks"]:
            targets = page_map.get((path, bookmark["page"] - 1), [])
            if targets:
                records["bookmarks"].append({**bookmark, "page": targets[0]})
    used_names = set()
    source_names = {}
    for path, source_records in source_records_by_path.items():
        for widget in source_records["widgets"]:
            original = widget["name"]
            if (path, original) in source_names:
                continue
            name, suffix = original, 2
            while name in used_names:
                name = f"{original}#{suffix}"; suffix += 1
            source_names[(path, original)] = name
            used_names.add(name)
    page_occurrences = Counter()
    copied_names = {}
    for out_page, row in enumerate(rows, 1):
        if row["page"] == "blank":
            continue
        path = row["source"]
        page_key = (path, row["page"])
        page_occurrences[page_key] += 1
        copy_no = page_occurrences[page_key]
        source_records = source_records_by_path[path]
        for link in source_records["links"]:
            if link["page"] != row["page"] + 1:
                continue
            mapped = dict(link, page=out_page)
            if link["kind"] == "page":
                targets = page_map.get((path, link["target"] - 1), [])
                if not targets:
                    continue
                mapped["target"] = targets[0]
            records["links"].append(mapped)
        for widget in source_records["widgets"]:
            if widget["page"] == row["page"] + 1:
                record = dict(widget, page=out_page)
                base = source_names[(path, widget["name"])]
                if copy_no > 1:
                    copy_key = (path, row["page"], widget["name"], copy_no)
                    if copy_key not in copied_names:
                        candidate = f"{base}#{copy_no}"
                        suffix = copy_no
                        while candidate in used_names:
                            suffix += 1
                            candidate = f"{base}#{suffix}"
                        copied_names[copy_key] = candidate
                        used_names.add(candidate)
                    record["name"] = copied_names[copy_key]
                else:
                    record["name"] = base
                records["widgets"].append(record)
    previous = 0
    for bookmark in records["bookmarks"]:
        bookmark["level"] = max(1, min(bookmark["level"], previous + 1))
        previous = bookmark["level"]
    return OperationResult(pages=placements, features=Features(must_survive=dict(expected), dropped=dropped,
                                                               records=records))


def _page_map(rows):
    grouped, start = [], 0
    while start < len(rows):
        row = rows[start]
        end = start + 1
        step = None
        if row["page"] != "blank" and end < len(rows):
            nxt = rows[end]
            if nxt["source"] == row["source"] and nxt["page"] != "blank" and nxt["rotation"] == row["rotation"]:
                step = nxt["page"] - row["page"]
                if step not in (-1, 1): step = None
        if step is not None:
            while end < len(rows):
                prev, nxt = rows[end - 1], rows[end]
                if nxt["source"] != row["source"] or nxt["page"] == "blank" or nxt["rotation"] != row["rotation"] or nxt["page"] - prev["page"] != step:
                    break
                end += 1
        elif row["page"] == "blank":
            while end < len(rows) and rows[end]["page"] == "blank": end += 1
        out_range = str(start + 1) if end == start + 1 else f"{start + 1}-{end}"
        if row["page"] == "blank":
            source = "blank"
        else:
            page_range = str(row["page"] + 1) if end == start + 1 else f"{row['page'] + 1}-{rows[end - 1]['page'] + 1}"
            source = page_range if len({r["source"] for r in rows if r["page"] != "blank"}) == 1 else f"{Path(row['source']).name}:{page_range}"
        suffix = f" @{row['rotation']}" if row["rotation"] else ""
        grouped.append(f"out {out_range} ← {source}{suffix}")
        start = end
    return ", ".join(grouped)


def _normalize_toc_levels(toc):
    normalized, previous = [], 0
    for level, *rest in toc:
        level = max(1, min(level, previous + 1))
        normalized.append([level, *rest])
        previous = level
    return normalized


def _clone_repeated_widget_fields(path, rows):
    """Detach repeated-page widgets so each copied occurrence has an independent value."""
    import pikepdf
    repeated = Counter((row["source"], row["page"]) for row in rows if row["page"] != "blank")
    if not any(count > 1 for count in repeated.values()): return
    occurrences = Counter()
    path = Path(path)
    temporary = path.with_name("." + path.stem + ".fields.part.pdf")
    try:
        with pikepdf.open(path) as pdf:
            acro = pdf.Root.get("/AcroForm")
            if acro is None:
                acro = pdf.make_indirect(pikepdf.Dictionary())
                pdf.Root["/AcroForm"] = acro
            fields = acro.get("/Fields")
            if fields is None:
                fields = pikepdf.Array()
                acro["/Fields"] = fields
            known = {tuple(field.objgen) for field in fields if getattr(field, "is_indirect", False)}
            for page in pdf.pages:
                for annot in page.obj.get("/Annots", []):
                    if str(annot.get("/Subtype", "")) != "/Widget": continue
                    objgen = tuple(annot.objgen) if getattr(annot, "is_indirect", False) else None
                    if objgen and objgen in known: continue
                    if not annot.get("/Parent"):
                        fields.append(annot)
                        if objgen: known.add(objgen)
            used_names = set()
            def field_names(items):
                for field in items:
                    name = str(field.get("/T", ""))
                    if name: used_names.add(name)
                    for child in field.get("/Kids", []):
                        field_names([child])
            field_names(fields)
            for page_no, row in enumerate(rows):
                if row["page"] == "blank": continue
                key = (row["source"], row["page"])
                occurrences[key] += 1
                if occurrences[key] <= 1: continue
                annots = pdf.pages[page_no].obj.get("/Annots", pikepdf.Array())
                for annot_no, annot in enumerate(list(annots)):
                    if str(annot.get("/Subtype", "")) != "/Widget": continue
                    original_name = str(annot.get("/T", "field"))
                    clone = pdf.make_indirect(pikepdf.Dictionary({key: value for key, value in annot.items()}))
                    suffix = 2
                    while f"{original_name}#{suffix}" in used_names: suffix += 1
                    clone_name = f"{original_name}#{suffix}"
                    used_names.add(clone_name)
                    clone["/T"] = pikepdf.String(clone_name)
                    if annot.get("/Parent"):
                        parent = annot["/Parent"]
                        parent_clone = pdf.make_indirect(pikepdf.Dictionary({key: value for key, value in parent.items()}))
                        parent_name = str(parent.get("/T", "field"))
                        parent_suffix = suffix
                        while f"{parent_name}#{parent_suffix}" in used_names: parent_suffix += 1
                        parent_clone["/T"] = pikepdf.String(f"{parent_name}#{parent_suffix}")
                        used_names.add(f"{parent_name}#{parent_suffix}")
                        parent_clone["/Kids"] = pikepdf.Array([clone])
                        clone["/Parent"] = parent_clone
                        fields.append(parent_clone)
                    else:
                        fields.append(clone)
                    annots[annot_no] = clone
            pdf.save(temporary)
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def _normalize_field_collisions(path):
    """Give every AcroForm field a stable unique name after multi-source assembly."""
    import pikepdf
    path = Path(path)
    temporary = path.with_name("." + path.stem + ".names.part.pdf")
    try:
        with pikepdf.open(path) as pdf:
            acro = pdf.Root.get("/AcroForm")
            fields = acro.get("/Fields", []) if acro else []
            seen, changes = set(), False
            def visit(field):
                nonlocal changes
                raw = str(field.get("/T", ""))
                if raw:
                    base = re.sub(r"\s+\[\d+\]$", "", raw)
                    candidate, suffix = base, 2
                    while candidate in seen:
                        candidate = f"{base}#{suffix}"; suffix += 1
                    seen.add(candidate)
                    if candidate != raw:
                        field["/T"] = pikepdf.String(candidate); changes = True
                for child in field.get("/Kids", []): visit(child)
            for field in fields: visit(field)
            if changes:
                pdf.save(temporary)
        if temporary.exists(): temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def _catalog_features_present(source_doc):
    catalog = source_doc.pdf_catalog()
    for key in ("OpenAction", "AF", "StructTreeRoot", "MarkInfo"):
        if source_doc.xref_get_key(catalog, key)[0] != "null": return True
    for key in ("Names/JavaScript", "Names/EmbeddedFiles"):
        if source_doc.xref_get_key(catalog, key)[0] != "null": return True
    return False


def _restore_catalog_features(source_path, output_path, password=None, rows=None):
    """PyMuPDF select can omit catalog-level actions and embedded files on save."""
    import pikepdf
    def clone_value(value, output):
        if getattr(value, "is_indirect", False):
            return output.copy_foreign(value)
        if isinstance(value, pikepdf.Array):
            return pikepdf.Array([clone_value(item, output) for item in value])
        if isinstance(value, pikepdf.Dictionary):
            return pikepdf.Dictionary({key: clone_value(item, output) for key, item in value.items()})
        return value
    source_path, output_path = Path(source_path), Path(output_path)
    temp_path = output_path.with_name("." + output_path.stem + ".catalog.part.pdf")
    try:
        with pikepdf.open(source_path, password=password or "") as source, pikepdf.open(output_path) as output:
            src_root, out_root = source.Root, output.Root
            rows = rows or [{"source": str(source_path), "page": i} for i in range(len(source.pages))]
            page_map = {}
            for out_no, row in enumerate(rows):
                if row.get("page") == "blank": continue
                if str(Path(row["source"]).resolve()) != str(Path(source_path).resolve()): continue
                page_map.setdefault(tuple(source.pages[row["page"]].obj.objgen), output.pages[out_no].obj)
            if src_root.get("/OpenAction"):
                out_root["/OpenAction"] = clone_value(src_root["/OpenAction"], output)
            src_names = src_root.get("/Names")
            if src_names:
                out_names = out_root.get("/Names")
                if out_names is None:
                    out_names = pikepdf.Dictionary()
                    out_root["/Names"] = out_names
                for key in ("/JavaScript", "/EmbeddedFiles"):
                    if src_names.get(key): out_names[key] = clone_value(src_names[key], output)
            if src_root.get("/AF"):
                out_root["/AF"] = clone_value(src_root["/AF"], output)
            if src_root.get("/MarkInfo"):
                out_root["/MarkInfo"] = clone_value(src_root["/MarkInfo"], output)
            if src_root.get("/StructTreeRoot"):
                source_tree = src_root["/StructTreeRoot"]
                structure_memo = {}
                def remap_node(node):
                    if isinstance(node, pikepdf.Array):
                        return pikepdf.Array([mapped for item in node
                                              if (mapped := remap_node(item)) is not None])
                    if not isinstance(node, pikepdf.Dictionary): return clone_value(node, output)
                    objgen = tuple(node.objgen) if getattr(node, "is_indirect", False) else None
                    if objgen in page_map: return page_map[objgen]
                    if objgen in structure_memo: return structure_memo[objgen]
                    pg = node.get("/Pg")
                    if pg is not None:
                        replacement = page_map.get(tuple(pg.objgen) if getattr(pg, "is_indirect", False) else None)
                        if replacement is None: return None
                    mapped_node = output.make_indirect(pikepdf.Dictionary()) if objgen else pikepdf.Dictionary()
                    if objgen: structure_memo[objgen] = mapped_node
                    for key, value in node.items():
                        if key == "/Pg": mapped_node[key] = replacement
                        elif key == "/P":
                            parent = remap_node(value)
                            if parent is not None: mapped_node[key] = parent
                        elif key == "/K":
                            child = remap_node(value)
                            if child is not None: mapped_node[key] = child
                        elif key != "/ParentTree": mapped_node[key] = clone_value(value, output)
                    return mapped_node
                tree = remap_node(source_tree)
                if isinstance(tree, pikepdf.Dictionary):
                    parent_tree = source_tree.get("/ParentTree")
                    if parent_tree and parent_tree.get("/Nums"):
                        nums = parent_tree["/Nums"]
                        retained_keys = set()
                        for row in rows:
                            if row.get("page") != "blank" and str(Path(row["source"]).resolve()) == str(Path(source_path).resolve()):
                                key = source.pages[row["page"]].obj.get("/StructParents")
                                if key is not None: retained_keys.add(int(key))
                        copied = pikepdf.Dictionary({k: clone_value(v, output) for k, v in parent_tree.items() if k != "/Nums"})
                        copied["/Nums"] = pikepdf.Array([item for i in range(0, len(nums), 2) if int(nums[i]) in retained_keys
                                                         for item in (clone_value(nums[i], output), remap_node(nums[i + 1]))])
                        tree["/ParentTree"] = copied
                out_root["/StructTreeRoot"] = tree
            output.save(temp_path)
        temp_path.replace(output_path)
    finally:
        temp_path.unlink(missing_ok=True)


def _write_rows(docs, rows, target, *, top_level_toc=False, input_occurrences=(), passwords=None):
    # select shares one page object between repeated pages, so a per-occurrence top-level bookmark would
    # resolve to the first copy; that case takes the insert_pdf route below.
    repeated = len({row["page"] for row in rows}) < len(rows)
    if len(docs) == 1 and all(row["page"] != "blank" for row in rows) and not (top_level_toc and repeated):
        path = next(iter(docs))
        source = docs[path][0]
        restore_catalog = _catalog_features_present(source)
        original_labels = source.get_page_labels()
        preserve_labels = [row["page"] for row in rows] == list(range(len(source)))
        source.select([row["page"] for row in rows])
        source.set_page_labels(original_labels if preserve_labels else [])
        for page, row in zip(source, rows):
            if row["rotation"] is not None:
                page.set_rotation(row["rotation"] if row.get("absolute") else (page.rotation + row["rotation"]) % 360)
        source.save(target, garbage=3, deflate=True)
        _clone_repeated_widget_fields(target, rows)
        _normalize_field_collisions(target)
        if top_level_toc:
            toc = []
            seen_occurrences = set()
            for i, row in enumerate(rows):
                marker = row.get("occurrence", i)
                if row["page"] == "blank" or marker in seen_occurrences: continue
                seen_occurrences.add(marker)
                toc.append([1, Path(row["source"]).stem, i + 1])
            if toc: source.set_toc(toc)
            source.save(target, garbage=3, deflate=True)
        if restore_catalog:
            _restore_catalog_features(path, target, (passwords or {}).get(str(Path(path).resolve())), rows)
        return
    out = pymupdf.open()
    for row in rows:
        if row["page"] == "blank":
            page = out.new_page(width=row["size"][0], height=row["size"][1])
            if row["rotation"] is not None: page.set_rotation(row["rotation"])
            continue
        src = docs[row["source"]][0]
        # insert_pdf preserves links, annotations and widgets; select is used for one-source sequences.
        out.insert_pdf(src, from_page=row["page"], to_page=row["page"], links=True,
                       annots=True, widgets=True, final=0, join_duplicates=False)
        page = out[-1]
        rotation = (row["rotation"] if row.get("absolute") and row["rotation"] is not None
                    else (src[row["page"]].rotation + (row["rotation"] or 0)) % 360)
        page.set_rotation(rotation)
    output_map = {}
    for out_no, row in enumerate(rows):
        if row["page"] != "blank":
            output_map.setdefault((row["source"], row["page"]), out_no)
    for out_no, row in enumerate(rows):
        if row["page"] == "blank": continue
        for existing in out[out_no].get_links():
            out[out_no].delete_link(existing)
        source_page = docs[row["source"]][0][row["page"]]
        for original in source_page.get_links():
            link = {key: original[key] for key in ("kind", "from", "uri", "file", "page", "to", "zoom", "nameddest", "name") if key in original}
            if link.get("kind") == pymupdf.LINK_GOTO:
                link_target = output_map.get((row["source"], link.get("page", -1)))
                if link_target is None: continue
                link["page"] = link_target
            try:
                out[out_no].insert_link(link)
            except Exception:
                continue
    toc = []
    if top_level_toc:
        groups = []
        for out_no, row in enumerate(rows, 1):
            if row["page"] == "blank": continue
            marker = row.get("occurrence", out_no)
            group = next((g for g in groups if g[0] == marker), None)
            if group is None:
                group = (marker, row["source"], {})
                groups.append(group)
            group[2].setdefault(row["page"], out_no)
        for _, path, destination in groups:
            src = docs[path][0]
            toc.append([1, Path(path).stem, min(destination.values())])
            for level, title, page_no, *rest in src.get_toc():
                if page_no - 1 in destination: toc.append([level + 1, title, destination[page_no - 1], *rest])
    else:
        for path, (src, _) in docs.items():
            destination = {}
            for out_no, row in enumerate(rows, 1):
                if row["source"] == path and row["page"] != "blank":
                    destination.setdefault(row["page"], out_no)
            for level, title, page_no, *rest in src.get_toc():
                if page_no - 1 in destination: toc.append([level, title, destination[page_no - 1], *rest])
    if top_level_toc or toc:
        if toc: toc = _normalize_toc_levels(toc)
        out.set_toc([])
        if toc: out.set_toc(toc)
    # select is used when available to preserve structure and form-field identity on a single source.
    out.save(target, garbage=3, deflate=True)
    out.close()
    _clone_repeated_widget_fields(target, rows)
    _normalize_field_collisions(target)
    if len(docs) == 1 and _catalog_features_present(docs[next(iter(docs))][0]):
        _restore_catalog_features(next(iter(docs)), target, (passwords or {}).get(next(iter(docs))), rows)


def _execute(args, context, recipe, *, extra_inputs=(), command_name="organize"):
    path = Path(args.file).resolve()
    paths = [path, *(Path(p).resolve() for p in extra_inputs)]
    password = context.owner_password or context.password
    docs = {}
    per_file_passwords = {}
    base = Path(getattr(args, "job_base", Path.cwd()))
    try:
        shorthand_problems = []
        if isinstance(recipe, str): recipe, shorthand_problems = _shorthand(recipe)
        if not isinstance(recipe, list): raise BadArg("recipe must be a list of objects", ["recipe must be a list of objects"])
        for item in recipe:
            if isinstance(item, dict) and isinstance(item.get("file"), str):
                source = Path(item["file"])
                if not source.is_absolute(): source = base / source
                source = source.resolve()
                if source not in paths: paths.append(source)
        occurrences = paths[:]
        paths = list(dict.fromkeys(paths))
        for source in paths:
            item_password = password
            for item in recipe:
                if not isinstance(item, dict) or not isinstance(item.get("file"), str): continue
                item_path = Path(item["file"])
                if not item_path.is_absolute(): item_path = base / item_path
                if item_path.resolve() != source: continue
                if isinstance(item.get("password_file"), str):
                    password_path = Path(item["password_file"])
                    if not password_path.is_absolute(): password_path = base / password_path
                    item_password = context.secret_store.add(strip_one_newline(password_path.read_text(encoding="utf-8-sig")))
                break
            doc = open_input(source, item_password)
            docs[str(source)] = (doc, source)
            per_file_passwords[str(source)] = item_password or ""
        rows, problems = _validate_recipe(recipe, docs, getattr(args, "keep_rest", False), base)
        problems = shorthand_problems + problems
        if problems: raise BadArg(f"recipe has {len(problems)} problem(s): {problems[0]}", problems)
        if len(rows) > 100:
            source_doc = docs[str(path)][0]
            sample_doc = pymupdf.open()
            sample_pages = list(range(min(2, len(source_doc))))
            for page_no in sample_pages:
                sample_doc.insert_pdf(source_doc, from_page=page_no, to_page=page_no)
            started = time.perf_counter()
            sample_operation = OperationResult(pages=[[Placement(str(path), number)] for number in sample_pages])
            compare(sample_doc, sample_doc, _spec(command_name).check_policy, sample_operation,
                    source_docs={str(path): sample_doc})
            sample_doc.close()
            announce_estimate(len(rows), (time.perf_counter() - started) / max(1, len(sample_pages)))
        operation = _operation(rows, docs)
        source_link_total = sum(len(docs[row["source"]][0][row["page"]].get_links())
                                for row in rows if row["page"] != "blank")
        labels_dropped = len(paths) > 1 or any(r["page"] == "blank" for r in rows) or [r["page"] for r in rows] != list(range(len(docs[str(path)][0])))
        if command_name == "extract" and (getattr(args, "output", None) is None or Path(args.output).is_dir()):
            page_spec = str(recipe[0].get("pages", "")).replace(",", "-")
            base = Path(args.output) if getattr(args, "output", None) is not None else path.parent
            name = f"{path.stem}_p{page_spec}.pdf"
            args.output = base / name
        if getattr(args, "toc", False):
            occurrence_paths = [str(p) for p in occurrences if any(row["source"] == str(p) for row in rows)]
            operation.features.must_survive["bookmarks"] = operation.features.must_survive.get("bookmarks", 0) + len(occurrence_paths)
            bookmarks = []
            groups = []
            previous_source, previous_page = None, None
            for out_no, row in enumerate(rows, 1):
                if row["page"] == "blank": continue
                if row["source"] != previous_source or (previous_page is not None and row["page"] <= previous_page):
                    group = [len(groups), row["source"], {}]; groups.append(group)
                previous_source, previous_page = row["source"], row["page"]
                group[2].setdefault(row["page"], out_no)
            for _, source_path, destinations in groups:
                bookmarks.append({"title": Path(source_path).stem, "level": 1,
                                  "page": min(destinations.values())})
                for level, title, page_no in docs[source_path][0].get_toc():
                    if page_no - 1 in destinations:
                        bookmarks.append({"title": " ".join(str(title).split()), "level": level + 1,
                                          "page": destinations[page_no - 1]})
            previous = 0
            for bookmark in bookmarks:
                bookmark["level"] = max(1, min(bookmark["level"], previous + 1))
                previous = bookmark["level"]
            operation.features.records["bookmarks"] = bookmarks
        spec = _spec(command_name)
        def writer(_doc, staged):
            _write_rows(docs, rows, staged, top_level_toc=getattr(args, "toc", False),
                        input_occurrences=occurrences, passwords=per_file_passwords)
            return operation
        result = write_pdf(path, spec, args, context, writer, operation=operation, inputs=paths,
                           passwords=per_file_passwords)
        result["page_map"] = _page_map(rows)
        if labels_dropped:
            result["warnings"].append("LABELS_DROPPED")
            __import__("clierr").warn(f"pdf {command_name}", "LABELS_DROPPED", "page order changed or files were assembled; page labels were dropped", "review the output")
        if source_link_total > operation.features.must_survive.get("links", 0):
            result["warnings"].append("LINKS_DROPPED")
            __import__("clierr").warn(f"pdf {command_name}", "LINKS_DROPPED", "cross-file or removed-target links were dropped", "review the output")
        if len(paths) > 1:
            result["warnings"].append("WARN LINKS_DROPPED")
            for code, detail in _multi_feature_warnings(paths, per_file_passwords):
                result["warnings"].append(code)
                __import__("clierr").warn("pdf organize", code, detail, "merge files as a single document first")
                if code == "FEATURES_DROPPED" and result.get("verdict") == "ok": result["verdict"] = "warn"
        print(result["page_map"], file=sys.stderr)
        return result
    finally:
        for doc, _ in docs.values(): doc.close()


def _multi_feature_warnings(paths, passwords=None):
    # PyMuPDF does not expose these counters, so inspect catalog-level structures with pikepdf.
    import pikepdf
    found = []
    def has_js(value, seen=None):
        if seen is None: seen = set()
        if isinstance(value, pikepdf.Array): return any(has_js(item, seen) for item in value)
        if not isinstance(value, pikepdf.Dictionary): return False
        try: key = tuple(value.objgen)
        except Exception: key = id(value)
        if key in seen: return False
        seen.add(key)
        return (str(value.get("/S", "")) == "/JavaScript" or bool(value.get("/JS")) or
                has_js(value.get("/Next"), seen))
    for path in paths:
        with pikepdf.open(path, password=((passwords or {}).get(str(Path(path).resolve())) or "")) as pdf:
            root = pdf.Root
            if root.get("/StructTreeRoot"): found.append("structure tree")
            names = root.get("/Names", {})
            javascript = bool(names.get("/JavaScript")) or has_js(root.get("/OpenAction"))
            for page in pdf.pages:
                javascript = javascript or has_js(page.obj.get("/AA"))
                for annot in page.obj.get("/Annots", []):
                    javascript = javascript or has_js(annot.get("/A")) or has_js(annot.get("/AA"))
            if javascript: found.append("JavaScript")
            if root.get("/AF") or names.get("/EmbeddedFiles"): found.append("attachments")
    return [("FEATURES_DROPPED", "multi-file assembly drops " + ", ".join(sorted(set(found))))] if found else []


def _spec(name):
    from ..registry import get
    return get(name)


def organize(args, context):
    if args.recipe is None:
        raise BadArg("organize requires a recipe", ["missing recipe; supply a shorthand recipe or JSON recipe"])
    return _execute(args, context, args.recipe)


def extract(args, context): return _execute(args, context, [{"pages": args.pages}], command_name="extract")


def remove(args, context):
    with open_input(args.file, context.password or context.owner_password) as doc:
        selected, bad = _page_list(args.pages, len(doc))
        if bad: raise BadArg(f"invalid pages: {bad[0]}", bad)
        keep = [i + 1 for i in range(len(doc)) if i not in selected]
    if not keep: raise PdfError("BAD_PAGE", "remove cannot remove every page", "select at least one page to keep")
    return _execute(args, context, [{"pages": ",".join(map(str, keep))}], command_name="remove")


def reorder(args, context):
    with open_input(args.file, context.password or context.owner_password) as doc:
        pages, bad = _page_list(args.order, len(doc))
        if bad: raise BadArg(f"invalid order: {bad[0]}", bad)
        if Counter(pages) != Counter(range(len(doc))): raise BadArg("order must contain every page exactly once")
    return _execute(args, context, [{"pages": args.order}], command_name="reorder")


def move(args, context):
    with open_input(args.file, context.password or context.owner_password) as doc:
        selected, bad = _page_list(args.pages, len(doc));
        if bad: raise BadArg(f"invalid pages: {bad[0]}", bad)
        block = sorted(set(selected))
        rest = [i for i in range(len(doc)) if i not in block]
        if args.after is not None and args.after - 1 in block: raise PdfError("BAD_PAGE", "move target is inside the selected block", "choose a page outside the selected block")
        if args.to == "start": index = 0
        elif args.to == "end": index = len(rest)
        elif args.after is not None:
            targets, bad = _page_list(args.after, len(doc))
            if bad or len(targets) != 1: raise PdfError("BAD_PAGE", "--after must identify exactly one original page", "choose one existing page")
            index = rest.index(targets[0]) + 1
        else: raise BadArg("move requires --after or --to start|end")
        rest[index:index] = block
        order = ",".join(str(n + 1) for n in rest)
    return _execute(args, context, [{"pages": order}], command_name="move")


def swap(args, context):
    with open_input(args.file, context.password or context.owner_password) as doc:
        pages, bad = _page_list(args.pages, len(doc))
        if bad or len(pages) != 2 or len(set(pages)) != 2: raise BadArg("swap requires exactly two distinct pages", bad)
        order = list(range(len(doc))); a,b=pages; order[a],order[b]=order[b],order[a]
    return _execute(args, context, [{"pages": ",".join(str(n+1) for n in order)}], command_name="swap")


def reverse(args, context):
    with open_input(args.file, context.password or context.owner_password) as doc:
        order = list(range(len(doc)))
        if args.pages:
            pages,bad=_page_list(args.pages,len(doc));
            if bad: raise BadArg(f"invalid pages: {bad[0]}",bad)
            if pages != list(range(min(pages),max(pages)+1)): raise BadArg("reverse --pages must be one contiguous range")
            order[min(pages):max(pages)+1]=reversed(order[min(pages):max(pages)+1])
        else: order.reverse()
    return _execute(args, context, [{"pages": ",".join(str(n+1) for n in order)}], command_name="reverse")


def duplicate(args, context):
    with open_input(args.file, context.password or context.owner_password) as doc:
        selected, bad = _page_list(args.pages, len(doc))
        if bad: raise BadArg(f"invalid pages: {bad[0]}", bad)
        recipe = [{"pages": str(i + 1), **({"times": args.times} if i in selected else {})} for i in range(len(doc))]
    return _execute(args, context, recipe, command_name="duplicate")


def blank(args, context):
    with open_input(args.file, context.password or context.owner_password) as doc:
        count = len(doc)
        if args.at == "end": positions = [count]
        else:
            positions,bad=_page_list(args.at,count)
            if bad: raise BadArg(f"invalid insertion positions: {bad[0]}",bad)
        result=[]
        for i in range(count+1):
            for _ in range(positions.count(i)): result.append({"blank": args.size or True})
            if i<count: result.append({"pages": str(i+1)})
    return _execute(args, context, result, command_name="blank")


def rotate(args, context):
    with open_input(args.file, context.password or context.owner_password) as doc:
        selected, bad = _page_list(args.pages, len(doc))
        if bad: raise BadArg(f"invalid pages: {bad[0]}", bad)
        recipe = []
        for i in range(len(doc)):
            item = {"pages": str(i + 1)}
            if i in selected:
                if args.set is None: item["rotate"] = args.by
                else: item["rotate_abs"] = args.set
            recipe.append(item)
    return _execute(args, context, recipe, command_name="rotate")


def insert(args, context):
    if args.after is None and args.to is None: raise BadArg("insert requires --after N or --to start|end")
    with open_input(args.file, context.password or context.owner_password) as first, open_input(args.other, context.password or context.owner_password) as other:
        pages,bad=_page_list(args.pages,len(other));
        if bad: raise BadArg(f"invalid pages: {bad[0]}",bad)
        order=[{"pages":str(i+1)} for i in range(len(first))]
        if args.to == "start": insert_at = 0
        elif args.to == "end": insert_at = len(first)
        else: insert_at = args.after
        if not 0<=insert_at<=len(first): raise BadArg("insert position is outside the primary PDF")
        order[insert_at:insert_at]=[{"pages":str(i+1),"file":str(Path(args.other).resolve())} for i in pages]
    return _execute(args, context, order, extra_inputs=[args.other], command_name="insert")
def _plain(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("-o", "--output", type=Path)
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")
    parser.add_argument("--owner-password-file", type=Path)
    parser.add_argument("--owner-password-env")
    parser.add_argument("--ignore-restrictions", action="store_true")
    parser.add_argument("--break-signature", action="store_true")


def _register(name, func, builder, suffix=None, policy="placements"):
    # The registry intentionally reads the module-level argument builder at registration.
    globals()["add_arguments"] = builder
    command(name, "3b", "pdf", "pdf", suffix or "_" + name, True, policy)(func)


def _build_organize(p):
    _plain(p); p.add_argument("recipe", nargs="?"); p.add_argument("--keep-rest", action="store_true")


def _build_pages(p): _plain(p); p.add_argument("--pages", required=True)
def _build_order(p): _plain(p); p.add_argument("--order", required=True)
def _build_move(p): _plain(p); p.add_argument("--pages", required=True); p.add_argument("--after", type=int); p.add_argument("--to", choices=("start", "end"))
def _build_reverse(p): _plain(p); p.add_argument("--pages")
def _build_duplicate(p): _plain(p); p.add_argument("--pages", required=True); p.add_argument("--times", type=int, default=2)
def _build_blank(p): _plain(p); p.add_argument("--at", required=True); p.add_argument("--size")
def _build_rotate(p): _plain(p); p.add_argument("--pages", required=True); p.add_argument("--by", type=int, default=90); p.add_argument("--set", type=int)
def _build_insert(p):
    _plain(p); p.add_argument("other", type=Path); p.add_argument("--pages", required=True); p.add_argument("--after", type=int); p.add_argument("--to", choices=("start", "end"))


_register("organize", organize, _build_organize, "_organized")
_register("extract", extract, _build_pages, "")
_register("remove", remove, _build_pages)
_register("reorder", reorder, _build_order)
_register("move", move, _build_move)
_register("swap", swap, _build_pages)
_register("reverse", reverse, _build_reverse)
_register("duplicate", duplicate, _build_duplicate)
_register("blank", blank, _build_blank)
_register("rotate", rotate, _build_rotate, "_rotated", policy="rotate")
_register("insert", insert, _build_insert)
