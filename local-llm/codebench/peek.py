"""Print the model's diff vs the real edit for tasks:  python peek.py <label> id1,id2"""
import json, sys
from common import HERE
res = {r["id"]: r for r in json.loads((HERE / "results" / f"{sys.argv[1]}.json").read_text(encoding="utf-8"))["rows"]}
tasks = {t["id"]: t for t in json.loads((HERE / "tasks.json").read_text(encoding="utf-8"))["tasks"]}
for i in sys.argv[2].split(","):
    print(f"##### {i} overlap {res[i]['overlap']}\n--- REAL:\n" + "\n".join(tasks[i]["diff"].split("\n")[2:]))
    print("--- MODEL:\n" + "\n".join(l for l in res[i].get("diff", "").split("\n") if l[:1] in "+-" and not l.startswith(("+++", "---"))))
