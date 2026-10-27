from pathlib import Path

import pymupdf

from ..registry import command


def add_arguments(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")
    parser.add_argument("--password", help="refused; use --password-file or --password-env")
    parser.add_argument("--owner-password-file", type=Path)
    parser.add_argument("--owner-password-env")
    parser.add_argument("--owner-password", help="refused; use --owner-password-file or --owner-password-env")


@command("info", 1, "pdf", "report", "", False, "report")
def info(args, context):
    path = args.file
    if path.is_dir():
        try:
            return _folder_info(path, context)
        except Exception as e:
            from ..errors import PdfError
            if isinstance(e, PdfError):
                raise
            raise PdfError("BAD_INPUT", f"cannot scan PDF folder: {path}", "check that the folder is readable", detail=str(e))
    try:
        doc = pymupdf.open(path)
    except (FileNotFoundError, pymupdf.FileNotFoundError):
        from ..errors import PdfError
        raise PdfError("NOT_FOUND", f"PDF not found: {path}", "check the path")
    except (PermissionError, pymupdf.FileDataError) as e:
        from ..errors import PdfError
        if isinstance(e, PermissionError):
            raise PdfError("LOCKED", f"PDF is locked or not readable: {path}", "close the program holding it, then rerun")
        raise PdfError("BAD_INPUT", f"cannot read PDF: {path}", "check that the PDF is valid", detail=str(e))
    except Exception as e:
        from ..errors import PdfError
        raise PdfError("BAD_INPUT", f"cannot read PDF: {path}", "check that the PDF is valid", detail=str(e))
    try:
        password = context.password or context.owner_password
        needs_password = doc.needs_pass
        encrypted_initially = bool(needs_password or doc.is_encrypted)
        auth_level = doc.authenticate(password or "") if needs_password else 0
        if needs_password and not auth_level:
            from ..errors import PdfError
            raise PdfError("WRONG_PASSWORD", "PDF requires a valid password", "supply --password-file or --password-env")
        pages = []
        for page in doc:
            text = page.get_text("text").strip()
            pages.append({"number": page.number + 1, "text_layer": bool(text), "scanned": not bool(text)})
        import pikepdf
        with pikepdf.open(path, password=password or "") as pdf:
            def has_signature(fields, inherited=None):
                for field in fields:
                    kind = field.get("/FT", inherited)
                    if str(kind) == "/Sig" and field.get("/V"):
                        return True
                    if has_signature(field.get("/Kids", []), kind):
                        return True
                return False
            form = pdf.Root.get("/AcroForm", {})
            signed = has_signature(form.get("/Fields", []))
            names = pdf.Root.get("/Names", {})
            def has_javascript(action, seen=None):
                if seen is None:
                    seen = set()
                if isinstance(action, pikepdf.Array):
                    return any(has_javascript(item, seen) for item in action)
                if not isinstance(action, pikepdf.Dictionary):
                    return False
                try:
                    key = tuple(action.objgen)
                except Exception:
                    key = id(action)
                if key == (0, 0):
                    key = id(action)
                if key in seen:
                    return False
                seen.add(key)
                if str(action.get("/S", "")) == "/JavaScript":
                    return True
                return has_javascript(action.get("/Next"), seen)

            javascript = bool(names.get("/JavaScript") or has_javascript(pdf.Root.get("/OpenAction")))
            font_stream_xrefs = set()
            def mark_font_streams(font):
                fonts_to_check = [font, *list(font.get("/DescendantFonts", []))]
                for font_item in fonts_to_check:
                    descriptor = font_item.get("/FontDescriptor")
                    if isinstance(descriptor, pikepdf.Dictionary):
                        for key in ("/FontFile", "/FontFile2", "/FontFile3"):
                            stream = descriptor.get(key)
                            if isinstance(stream, pikepdf.Stream):
                                font_stream_xrefs.add(stream.objgen[0])
            for page_obj in pdf.pages:
                resources = page_obj.obj.get("/Resources", {})
                fonts_dict = resources.get("/Font", {})
                if isinstance(fonts_dict, pikepdf.Dictionary):
                    for font in fonts_dict.values():
                        mark_font_streams(font)
            for page_obj in pdf.pages:
                javascript = javascript or has_javascript(page_obj.obj.get("/AA"))
                for annotation in page_obj.obj.get("/Annots", []):
                    javascript = javascript or has_javascript(annotation.get("/A")) or has_javascript(annotation.get("/AA"))
            def fields_have_javascript(fields):
                for field in fields:
                    if has_javascript(field.get("/A")) or has_javascript(field.get("/AA")):
                        return True
                    if fields_have_javascript(field.get("/Kids", [])):
                        return True
                return False
            javascript = javascript or fields_have_javascript(pdf.Root.get("/AcroForm", {}).get("/Fields", []))
            def outline_has_javascript(node, seen=None):
                if seen is None:
                    seen = set()
                while isinstance(node, pikepdf.Dictionary):
                    try:
                        key = tuple(node.objgen)
                    except Exception:
                        key = id(node)
                    if key == (0, 0):
                        key = id(node)
                    if key in seen:
                        return False
                    seen.add(key)
                    if has_javascript(node.get("/A")) or outline_has_javascript(node.get("/First"), seen):
                        return True
                    node = node.get("/Next")
                return False
            outlines = pdf.Root.get("/Outlines")
            if isinstance(outlines, pikepdf.Dictionary):
                javascript = javascript or outline_has_javascript(outlines.get("/First"))
            restricted = [name for name in ("modify_other", "modify_assembly", "modify_annotation", "modify_form",
                                             "extract", "print_lowres", "print_highres") if not getattr(pdf.allow, name, False)]
            restrictions_only = bool(pdf.is_encrypted and not password and not pdf.owner_password_matched
                                     and pdf.user_password_matched)
            encryption = {
                "encrypted": bool(pdf.is_encrypted),
                "restrictions_only": restrictions_only,
                "owner_password_set": bool(pdf.is_encrypted),
                "authenticated_as": "owner" if pdf.owner_password_matched else ("user" if pdf.user_password_matched else "none"),
                "restrictions": restricted,
                "permissions": {name: bool(getattr(pdf.allow, name, False)) for name in
                                ("accessibility", "extract", "modify_annotation", "modify_assembly", "modify_form", "modify_other", "print_lowres", "print_highres")},
            }
            pike_warnings = pdf.get_warnings()
        resource_kind = {xref: "fonts" for xref in font_stream_xrefs}
        mupdf_warnings = pymupdf.TOOLS.mupdf_warnings()
        from ..errors import library_warnings
        library_warnings.extend(str(w) for w in pike_warnings)
        if mupdf_warnings:
            library_warnings.append(str(mupdf_warnings))
        fonts = set()
        for page in doc:
            for image in page.get_images(full=True):
                resource_kind[image[0]] = "images"
            for font in page.get_fonts(full=True):
                resource_kind[font[0]] = "fonts"
            for xref in page.get_contents():
                resource_kind[xref] = "content streams"
        largest = ("unknown", 0)
        for xref in range(1, doc.xref_length()):
            try:
                size = len(doc.xref_stream_raw(xref) or b"")
                if size > largest[1]:
                    largest = (resource_kind.get(xref, "other streams"), size)
            except Exception:
                pass
        for page in doc:
            for item in page.get_fonts(full=True):
                font_xref = item[0]
                font_type, descriptor = doc.xref_get_key(font_xref, "FontDescriptor")
                if font_type == "xref":
                    descriptor_xref = int(descriptor.split()[0])
                    for key in ("FontFile", "FontFile2", "FontFile3"):
                        stream_type, stream_xref = doc.xref_get_key(descriptor_xref, key)
                        if stream_type == "xref":
                            resource_kind[int(stream_xref.split()[0])] = "fonts"
                elif font_type == "null":
                    child_type, child = doc.xref_get_key(font_xref, "DescendantFonts")
                    if child_type == "array":
                        child_xref = int(child.strip("[] ").split()[0])
                        child_type, descriptor = doc.xref_get_key(child_xref, "FontDescriptor")
                        if child_type == "xref":
                            descriptor_xref = int(descriptor.split()[0])
                            for key in ("FontFile", "FontFile2", "FontFile3"):
                                stream_type, stream_xref = doc.xref_get_key(descriptor_xref, key)
                                if stream_type == "xref":
                                    resource_kind[int(stream_xref.split()[0])] = "fonts"
                try:
                    embedded = bool(doc.extract_font(item[0])[3])
                except Exception:
                    embedded = False
                fonts.add((item[3], embedded))
        toc = doc.get_toc()
        result = {
            "pages": len(doc), "bytes": path.stat().st_size,
            "page_sizes_pt": [[round(p.rect.width, 2), round(p.rect.height, 2)] for p in doc],
            "pdf_version": doc.metadata.get("format"), "encrypted": encrypted_initially,
            "encryption": encryption,
            "permissions": int(doc.permissions), "signed": signed,
            "text_layer": any(p["text_layer"] for p in pages), "scanned_pages": [p["number"] for p in pages if p["scanned"]],
            "fonts": [{"name": n, "embedded": embedded} for n, embedded in sorted(fonts)],
            "forms": sum(1 for page in doc for _ in (page.widgets() or [])),
            "attachments": doc.embfile_count(), "javascript": javascript,
            "bookmarks": len(toc), "largest_space_user": {"kind": largest[0], "bytes": largest[1]},
        }
        result["next"] = ["scanned → ocr before text"] if result["scanned_pages"] else []
        if signed:
            result["next"].append("signed → PDF rewrites need --break-signature")
        return {"type": "report", "command": "info", "input": str(path), "verdict": "report", "data": result}
    finally:
        doc.close()


def _folder_info(folder, context):
    rows = []
    for path in sorted(folder.glob("*.pdf"), key=lambda item: item.name.casefold()):
        try:
            doc = pymupdf.open(path)
        except Exception as e:
            from ..errors import PdfError
            raise PdfError("BAD_INPUT", f"cannot read PDF: {path}", "check that the PDF is valid", detail=str(e))
        try:
            password = context.password or context.owner_password
            import pikepdf
            with pikepdf.open(path, password=password or "") as pdf:
                encrypted = bool(pdf.is_encrypted)
            if doc.needs_pass and not doc.authenticate(password or ""):
                from ..errors import PdfError
                raise PdfError("WRONG_PASSWORD", f"PDF requires a valid password: {path}",
                               "supply --password-file or --password-env")
            has_text = any(bool(page.get_text("text").strip()) for page in doc)
            title = (doc.metadata or {}).get("title") or ""
            rows.append(f"{path.name}: {doc.page_count} pages, {path.stat().st_size} bytes; "
                        f"title={title!r}; encrypted={'yes' if encrypted else 'no'}; "
                        f"text layer={'yes' if has_text else 'no'}")
        finally:
            doc.close()
    data = "\n".join(rows) if rows else "No PDF files found."
    return {"type": "report", "command": "info", "input": str(folder), "verdict": "report", "data": data}
