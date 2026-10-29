from pathlib import Path
import json

from ..registry import command
import clierr
from .p3a_core import check_file, run_safely


def add_arguments(parser):
    parser.add_argument("file", type=Path)
    parser.add_argument("--against", type=Path)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--map")
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--password-env")


@command("check", "3a", "pdf", "report", "", False, "report")
def check_command(args, context):
    if args.source:
        if args.against or args.map:
            from ..errors import PdfError
            raise PdfError("BAD_ARG", "--source cannot be combined with --against or --map", "use one check mode")
        from .p6_convert import check_markdown_source
        try:
            return check_markdown_source(args.file, args.source, context)
        except Exception as exc:
            from ..errors import PdfError
            if not isinstance(exc, PdfError) or exc.code != "CHECK_FAILED":
                raise
            try:
                detail = json.loads(exc.detail) if exc.detail else {}
                if isinstance(detail, dict):
                    findings = detail.get("findings", [{"code": "RAW_TEX", "message": exc.what}])
                    source_warnings = detail.get("warnings", [])
                else:
                    findings, source_warnings = detail, []
            except (TypeError, ValueError):
                findings = [{"code": "SOURCE_CHECK_FAILED", "message": exc.what}]
                source_warnings = []
            warnings = source_warnings
            for item in findings:
                if isinstance(item, dict):
                    clierr.warn("pdf check", item.get("code", "SOURCE_CHECK_FAILED"), item.get("message", exc.what), "review the PDF against its source")
            for item in warnings:
                if isinstance(item, dict):
                    clierr.warn("pdf check", item.get("code", "SOURCE_WARNING"), json.dumps(item, ensure_ascii=False), "review the PDF against its source")
            return {"type": "report", "command": "check", "input": str(args.file), "verdict": "warn",
                    "data": {"source": str(args.source), "findings": findings, "warnings": warnings}}
    if args.map and not args.against:
        from ..errors import PdfError
        raise PdfError("BAD_ARG", "--map requires --against", "use --map with check FILE --against ORIG")
    return run_safely(check_file, args, context)
