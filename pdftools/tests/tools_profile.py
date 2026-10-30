"""Run every test with external tools required and record the capability snapshot."""
import datetime
import json
import os
from pathlib import Path
import sys
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))
import clierr
from pdftools.commands.p2_selftest import _capabilities
from pdftools.check import TEXT_ORDER_MINIMUMS


def _record_tool_usage(tools, usage):
    for name, tool in tools.items():
        tool["executed_tests"] = len(usage.get(name, {}).get("tests", ()))
    return tools


def main():
    started = time.perf_counter()
    os.environ["PDFTOOLS_REQUIRE_TOOLS"] = "1"
    suite = unittest.defaultTestLoader.discover(str(ROOT / "tests"), top_level_dir=str(ROOT))
    result = unittest.TextTestRunner(verbosity=1).run(suite)
    elapsed = time.perf_counter() - started
    tools = _capabilities()["tools"]
    from tests.tooling import TOOL_USAGE
    _record_tool_usage(tools, TOOL_USAGE)
    intentional = [(test.id(), reason) for test, reason in result.skipped if "PDFTOOLS_BENCH" in reason]
    missing_skips = [(test.id(), reason) for test, reason in result.skipped if "PDFTOOLS_BENCH" not in reason]
    summary = {"tools": tools, "tests_run": result.testsRun,
               "failures": len(result.failures), "errors": len(result.errors),
               "skips": len(result.skipped), "intentional_skips": intentional,
               "missing_tool_skips": missing_skips, "wall_seconds": round(elapsed, 3),
               "text_order_minimums": dict(TEXT_ORDER_MINIMUMS)}
    reports = ROOT / "reports"
    reports.mkdir(exist_ok=True)
    name = "tools-profile-" + datetime.datetime.now().strftime("%Y%m%d-%H%M%S") + ".json"
    path = reports / name
    path.write_text(json.dumps(summary, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"TOOLS_PROFILE tests={summary['tests_run']} failures={summary['failures']} errors={summary['errors']} skips={summary['skips']} wall={summary['wall_seconds']}s report={path}")
    return 0 if not (result.failures or result.errors or missing_skips) else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SystemExit:
        raise
    except Exception as exc:
        raise SystemExit(clierr.fail("tools-profile", exc, log_name="tools-profile"))
