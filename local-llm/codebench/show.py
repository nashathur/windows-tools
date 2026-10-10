"""Print selected tasks' diffs (to write instructions):  python show.py [selected.json]"""
import json
import sys

from common import HERE

src = sys.argv[1] if len(sys.argv) > 1 else "selected.json"
for t in json.loads((HERE / src).read_text(encoding="utf-8"))["tasks"]:
    print(f"##### {t['id']} {t['file']}:{t['line']} ({t['changed_lines']} lines, {t['source']})")
    print("\n".join(t["diff"].split("\n")[2:]))
