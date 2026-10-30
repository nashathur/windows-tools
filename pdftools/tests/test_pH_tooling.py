import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))
from tests.tools_profile import _record_tool_usage


class ToolProfileTest(unittest.TestCase):
    def test_all_versions_get_the_executed_test_count(self):
        tools = {"used": {"version": "1"}, "unused": {"version": "2"}}
        usage = {"used": {"tests": {"test.a", "test.b"}}}
        self.assertEqual(_record_tool_usage(tools, usage), {
            "used": {"version": "1", "executed_tests": 2},
            "unused": {"version": "2", "executed_tests": 0},
        })


if __name__ == "__main__": unittest.main()
