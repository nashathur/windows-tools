import os
import time
from contextlib import ExitStack
from pathlib import Path

import pymupdf

import clierr

from . import crypt, io
from .check import OperationResult, compare
from .errors import PdfError


def _signed(path, password=None):
    import pikepdf
    try:
        with pikepdf.open(path, password=password or "") as pdf:
            def walk(fields, inherited=None):
                for field in fields:
                    kind = field.get("/FT", inherited)
                    if str(kind) == "/Sig" and field.get("/V"):
                        return True
                    if walk(field.get("/Kids", []), kind):
                        return True
                return False
            return walk(pdf.Root.get("/AcroForm", {}).get("/Fields", []))
    except Exception:
        return False


def _encryption_state(path, password=None, document=None):
    import pikepdf
    with pikepdf.open(path, password=password or "") as pdf:
        encrypted = bool(pdf.is_encrypted)
        allow = pdf.allow
        return crypt.Encryption(encrypted=encrypted, user_password=password,
                                owner_password=None, permissions=document.permissions if document else 0,
                                restrictions_only=encrypted and not password and pdf.user_password_matched and
                                not pdf.owner_password_matched), {
                                    "modify": bool(allow.modify_other),
                                    "assemble": bool(allow.modify_assembly),
                                    "owner_matched": bool(pdf.owner_password_matched)}


def open_input(path, password=None):
    try:
        doc = pymupdf.open(path)
    except Exception as exc:
        raise PdfError("BAD_INPUT", f"cannot open input: {path}", "check that the file is valid", detail=str(exc))
    if doc.needs_pass and not doc.authenticate(password or ""):
        doc.close()
        raise PdfError("WRONG_PASSWORD", "PDF requires a valid password", "supply --password-file or --password-env")
    return doc


def estimate_seconds(page_count, seconds_per_page, *, sample_pages=2):
    return max(0.0, float(page_count) * float(seconds_per_page))


def announce_estimate(pages, seconds_per_page):
    seconds = estimate_seconds(pages, seconds_per_page)
    if seconds > 10:
        print(f"estimate: ~{round(seconds)} s for {pages} pages", file=__import__("sys").stderr)
    return seconds


def _json_page_map(value):
    if isinstance(value, Path):
        return str(value)
    if hasattr(value, "__dataclass_fields__"):
        return {key: _json_page_map(item) for key, item in vars(value).items()}
    if isinstance(value, dict):
        return {str(key): _json_page_map(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_json_page_map(item) for item in value]
    return value


def measure_seconds_per_page(path, render=True):
    start = time.perf_counter()
    with open_input(path) as doc:
        for page in list(doc)[:2]:
            if render:
                pix = page.get_pixmap(matrix=pymupdf.Matrix(0.25, 0.25), alpha=False)
                del pix
            else:
                page.get_text("words")
        count = min(2, len(doc))
    return (time.perf_counter() - start) / max(1, count)


def run_each(paths, run_one, command, context):
    """--each: run_one(path) per file; a failing file becomes an error record and is skipped; exit 4 if any failed."""
    records, failed = [], 0
    for path in paths:
        try:
            records.append(run_one(Path(path)))
        except clierr.Fail as exc:
            failed += 1
            records.append({"type": "error", "command": command, "input": str(path), "code": exc.code,
                            "message": exc.what, "fix": exc.fix})
    if failed:
        context.exit_code = 4
    records.append({"type": "summary", "command": command, "files": len(records), "ok": len(records) - failed,
                    "failed": failed})
    return records


def _warn(code, detail, fix="review the output"):
    clierr.warn("pdf", code, detail, fix)


def write_pdf(source, spec, args, context, writer, *, operation=None, inputs=None, can_modify=True,
              can_assemble=True, security=False, encrypt_output=None, decrypt_output=False, opener=None,
              passwords=None):
    """Run a registered PDF writer under the shared open/check/stage/publish contract.

    writer(source_doc, staged_path) writes the candidate. It may also return an OperationResult.
    encrypt_output: {"user_password", "owner_password", "permissions"} replaces the encryption matrix (protect).
    decrypt_output: publish without encryption (unlock).
    opener(path, password) -> pymupdf.Document replaces open_input for inputs that need recovery (repair).
    passwords: {input path: password} for inputs whose password differs from the command's (organize/merge files).
    """
    source = Path(source).resolve()
    inputs = [Path(p).resolve() for p in (inputs or [source])]
    password = context.owner_password or context.password
    source_pdf = spec.input_kind == "pdf" or source.suffix.lower() == ".pdf"
    open_source = opener or open_input
    passwords = {str(Path(k).resolve()): v for k, v in (passwords or {}).items()}
    pw = lambda path: passwords.get(str(path), password)
    try:
        docs = [open_source(path, pw(path)) for path in inputs] if source_pdf else []
    except PdfError as exc:
        if context.owner_password:
            raise PdfError("WRONG_PASSWORD", "owner password did not authenticate",
                           "supply a valid --owner-password-file") from exc
        raise
    doc = docs[0] if docs else None
    context.input_docs = {str(path): item for path, item in zip(inputs, docs)}
    warnings = []
    try:
        try:
            states = [_encryption_state(path, pw(path), item) for path, item in zip(inputs, docs)]
        except Exception as exc:
            if opener and not any(item.is_encrypted for item in docs):
                states = [(crypt.Encryption(), {"modify": True, "assemble": True, "owner_matched": False})
                          for _ in inputs]
            elif context.owner_password:
                raise PdfError("WRONG_PASSWORD", "owner password did not authenticate",
                               "supply a valid --owner-password-file") from exc
            else:
                raise
        encryptions = [state[0] for state in states]
        combined = crypt.combine_decision(encryptions) if encryptions else {"encryption": crypt.Encryption(), "warnings": []}
        encrypted_input = combined["encryption"]
        encrypted = encrypted_input.encrypted
        input_permissions = encrypted_input.permissions
        owner_secret = context.owner_password
        restricted_inputs = [permissions for encryption, permissions in states
                             if encryption.encrypted and encryption.restrictions_only and
                             ((can_modify and not permissions["modify"]) or
                              (can_assemble and not permissions["assemble"]))]
        if context.owner_password:
            if any(encryption.encrypted and not permissions["owner_matched"] for encryption, permissions in states):
                raise PdfError("WRONG_PASSWORD", "owner password did not authenticate",
                               "supply a valid --owner-password-file")
        if restricted_inputs and not getattr(args, "ignore_restrictions", False):
            raise PdfError("ENCRYPTED", "PDF restrictions prohibit modification", "give --owner-password-file, or --ignore-restrictions")
        if source_pdf and spec.rewrites_pdf and any(_signed(path, pw(path)) for path in inputs):
            if not getattr(args, "break_signature", False):
                raise PdfError("SIGNED", "input contains a signed signature field", "add --break-signature if a modified copy is fine")
            warnings.append("SIGNATURE_INVALIDATED")
        if restricted_inputs and getattr(args, "ignore_restrictions", False):
            warnings.append("RESTRICTIONS_IGNORED")
        warnings.extend(combined["warnings"])
        output = io.resolve_output(source, spec.name, getattr(args, "output", None), spec.suffix,
                                   ".pdf", multi=False)
        if output.parent.exists():
            io.sweep_stale(output.parent)
        io.sweep_temp()
        estimate = sum(path.stat().st_size for path in inputs) * 3
        with io.temp_folder() as temp:
            context.temp = Path(temp)  # run temp folder for writer scratch files such as OperationResult.baseline
            io.disk_budget(inputs, output, temp, multiplier=3, estimate=estimate)
            op = operation or OperationResult()
            if getattr(args, "dry_run", False):
                return {"type": "result", "command": spec.name, "path": str(output), "kind": "pdf",
                        "pages": doc.page_count if doc else None, "bytes": 0, "verdict": "report",
                        "provisional": True, "warnings": warnings, "page_map": _json_page_map(op.pages)}
            output.parent.mkdir(parents=True, exist_ok=True)
            with io.Staging(output, temp) as stage:
                if encrypted:
                    for index, input_doc in enumerate(docs):
                        clear_input = Path(temp) / f"input-{index + 1}.pdf"
                        input_doc.close()
                        import pikepdf
                        with pikepdf.open(inputs[index], password=pw(inputs[index]) or "") as encrypted_pdf:
                            encrypted_pdf.save(clear_input)
                        docs[index] = open_input(clear_input)
                    context.input_docs = {str(path): item for path, item in zip(inputs, docs)}
                    doc = docs[0]
                candidate_op = writer(doc, stage.path)
                if isinstance(candidate_op, OperationResult):
                    op = candidate_op
                source_pages = doc.page_count if doc else None
                for item in docs:
                    item.close()
                if op.keep_input:
                    stage.cleanup()
                    warnings.append(op.keep_input)
                    for warning in warnings:
                        clierr.warn(f"pdf {spec.name}", warning, f"{spec.name} produced warning {warning}; output is the input",
                                    "nothing to do; the input is already the best result")
                    return {"type": "result", "command": spec.name, "path": str(source), "kind": "pdf",
                            "pages": source_pages, "bytes": source.stat().st_size,
                            "verdict": "warn", "warnings": warnings, "findings": []}
                check_password = password
                if decrypt_output:
                    check_password = None
                elif encrypt_output:
                    user_pw = encrypt_output.get("user_password") or ""
                    owner_pw = encrypt_output.get("owner_password") or user_pw
                    if not owner_pw:
                        raise PdfError("BAD_ARG", "protect needs a user or owner password", "give --password-file or --owner-password-file")
                    encrypted_path = stage.path.with_name(stage.path.stem + ".encrypted.part.pdf")
                    with pymupdf.open(stage.path) as candidate:
                        candidate.save(encrypted_path, encryption=pymupdf.PDF_ENCRYPT_AES_256,
                                       owner_pw=owner_pw, user_pw=user_pw,
                                       permissions=encrypt_output.get("permissions", -1), garbage=3, deflate=True)
                    stage.path.unlink(missing_ok=True)
                    stage.path = encrypted_path
                    check_password = owner_pw
                elif encrypted:
                    decision = crypt.write_decision(crypt.Encryption(encrypted=True,
                        user_password=password, permissions=input_permissions,
                        restrictions_only=encrypted_input.restrictions_only), can_modify=can_modify,
                        can_assemble=can_assemble, ignore_restrictions=getattr(args, "ignore_restrictions", False),
                        owner_password=owner_secret,
                        secret_store=getattr(context, "secret_store", None))
                    if decision.get("error"):
                        raise PdfError(decision["error"], "PDF restrictions prohibit modification", decision["fix"])
                    warnings.extend(decision["warnings"])
                    clear_path = stage.path.with_name(stage.path.stem + ".clear.part.pdf")
                    with open_input(stage.path, password) as candidate:
                        candidate.save(clear_path, garbage=3, deflate=True)
                    stage.path.unlink(missing_ok=True)
                    stage.path = clear_path
                    encrypted_path = stage.path.with_name(stage.path.stem + ".encrypted.part.pdf")
                    with pymupdf.open(stage.path) as candidate:
                        candidate.save(encrypted_path, encryption=pymupdf.PDF_ENCRYPT_AES_256,
                                       owner_pw=decision["owner_password"], user_pw=decision["user_password"] or "",
                                       permissions=decision["permissions"], garbage=3, deflate=True)
                    stage.path.unlink(missing_ok=True)
                    stage.path = encrypted_path
                with ExitStack() as stack:
                    check_docs = {str(path): stack.enter_context(open_source(path, pw(path))) for path in inputs} if source_pdf else {}
                    if source_pdf and op.baseline:
                        check_docs[str(source)] = stack.enter_context(open_input(Path(op.baseline)))
                    outdoc = stack.enter_context(open_input(stage.path, check_password))
                    result_pages = outdoc.page_count
                    if source_pdf:
                        findings = compare(check_docs[str(source)], outdoc, spec.check_policy, op,
                                           source_docs=check_docs)
                    else:
                        findings = []
                invariant_failures = [f for f in findings if f.tier == "invariant"]
                warnings.extend(f.code for f in findings if f.tier == "new_warning")
                finding_codes = set()
                for finding in findings:
                    if finding.tier in {"invariant", "new_warning"}:
                        finding_codes.add(finding.code)
                        clierr.warn(f"pdf {spec.name}", finding.code,
                                    (f"page {finding.page}: " if finding.page else "") + finding.message,
                                    "review the output and measurement log")
                if getattr(args, "verbose", getattr(args, "v", False)):
                    for finding in findings:
                        if finding.tier == "pre_existing":
                            print(f"PRE-EXISTING {finding.code} page {finding.page}: {finding.message}", file=__import__("sys").stderr)
                if invariant_failures:
                    warnings.extend(f.code for f in invariant_failures)
                    import json
                    log = clierr.write_log("pdf", "WARN", f"{spec.name} invariant check failed",
                        "review the measurements", sections={"findings": json.dumps([
                            {"tier": f.tier, "code": f.code, "page": f.page, "message": f.message,
                             "details": f.details} for f in findings], ensure_ascii=False, default=str)},
                        log_name=spec.name)
                if invariant_failures:
                    if security or spec.name in {"sanitize", "redact"}:
                        context.exit_code = 3
                        if getattr(args, "keep_failed", False):
                            stage.target = output.with_name(output.stem + "_FAILED_UNSAFE" + output.suffix)
                            final_path = stage.publish()
                        else:
                            stage.cleanup()
                            return {"type": "result", "command": spec.name, "path": None, "kind": "pdf",
                                    "pages": result_pages, "bytes": 0, "verdict": "failed", "warnings": warnings,
                                    "findings": [{"tier": f.tier, "code": f.code, "page": f.page,
                                                  "message": f.message, "details": f.details} for f in findings],
                                    "log": log}
                    else:
                        stage.target = output.with_name(output.stem + "_UNCHECKED" + output.suffix)
                        final_path = stage.publish()
                        context.exit_code = 3
                else:
                    final_path = stage.publish()
                if not invariant_failures and not security and final_path != output and getattr(args, "output", None):
                    warnings.append("NAME_TAKEN")
                for warning in warnings:
                    if warning not in finding_codes:
                        clierr.warn(f"pdf {spec.name}", warning, f"{spec.name} produced warning {warning}", "review the output")
                size = final_path.stat().st_size
                verdict = ("failed" if invariant_failures and security else
                           "unchecked" if invariant_failures else "warn" if warnings else "ok")
                return {"type": "result", "command": spec.name, "path": str(final_path), "kind": "pdf",
                        "pages": result_pages, "bytes": size, "verdict": verdict, "warnings": warnings,
                        "findings": [{"tier": f.tier, "code": f.code, "page": f.page,
                                      "message": f.message, "details": f.details} for f in findings],
                        **({"log": log} if invariant_failures else {})}
    finally:
        for item in docs:
            if not item.is_closed:
                item.close()


def write_non_pdf(source, spec, args, writer, *, kind=None, password=None):
    source = Path(source).resolve()
    output_arg = getattr(args, "output", None)
    extension = Path(output_arg).suffix if output_arg else (".png" if spec.output_kind == "image" else "." + spec.output_kind)
    output = io.resolve_output(source, spec.name, getattr(args, "output", None), spec.suffix,
                              extension or ".png")
    if output.parent.exists():
        io.sweep_stale(output.parent)
    warnings = []
    try:
        import pikepdf
        with pikepdf.open(source, password=password or "") as pdf:
            if pdf.is_encrypted:
                warnings.extend(crypt.export_warnings(True))
    except Exception:
        pass
    if getattr(args, "dry_run", False):
        return {"type": "result", "command": spec.name, "path": str(output), "kind": kind or spec.output_kind,
                "pages": None, "bytes": 0, "verdict": "provisional", "warnings": warnings}
    with io.temp_folder() as temp:
        output.parent.mkdir(parents=True, exist_ok=True)
        io.disk_budget([source], output, temp, estimate=max(source.stat().st_size * 3, 800 * 800 * 4))
        with io.Staging(output, temp) as stage:
            writer(stage.path)
            path = stage.publish()
    for warning in warnings:
        clierr.warn("pdf", warning, f"{spec.name} exported encrypted input to a non-PDF file", "review the output")
    return {"type": "result", "command": spec.name, "path": str(path), "kind": kind or spec.output_kind,
            "pages": None, "bytes": path.stat().st_size, "verdict": "warn" if warnings else "ok", "warnings": warnings}
