"""Pick benchmark tasks from candidates.json: 4-25 changed lines, real code (not only comments),
not pure deletions, spread over the crates; keep those whose 'before' state compiles.

    conda run -n llm python pick.py [n=40]   -> selected.json
"""
import json
import random
import re
import sys
import time

from common import HERE, crates_for, ct, set_before, worktree_at

n_want = int(sys.argv[1]) if len(sys.argv) > 1 else 40
data = json.loads((HERE / "candidates.json").read_text(encoding="utf-8"))


def code_lines(s: str) -> list[str]:
    return [ln for ln in s.split("\n") if ln.strip() and not re.match(r"\s*//", ln)]


pool = [c for c in data["candidates"]
        if 4 <= c["changed_lines"] <= 25 and code_lines(c["new"])
        and code_lines(c["new"]) != code_lines(c["old"])
        and "src_backup" not in c["file"]]
random.seed(7)
random.shuffle(pool)
by_crate = {}
for c in pool:
    by_crate.setdefault(c["file"].split("/src/")[0], []).append(c)
order = []  # round-robin over crates
while any(by_crate.values()):
    for k in list(by_crate):
        if by_crate[k]:
            order.append(by_crate[k].pop())
print(f"{len(pool)} candidates after filtering; checking up to {n_want} that compile before", flush=True)

keep = []
for c in order:
    if len(keep) >= n_want:
        break
    t = time.time()
    wt = worktree_at(data["head"])
    if not set_before(wt, c):
        continue
    errs = [e for cr in sorted(crates_for(wt, c["file"])) for e in ct.cargo_errors(cr, "check", wt)]
    print(f"{c['id']} {c['file']}:{c['line']} ({c['changed_lines']} lines): "
          f"{'ok' if not errs else 'before does not compile'} {time.time() - t:.0f}s", flush=True)
    if not errs:
        keep.append(c)
(HERE / "selected.json").write_text(json.dumps({"head": data["head"], "tasks": keep}, indent=1,
                                               ensure_ascii=False), encoding="utf-8")
print(f"{len(keep)} selected -> selected.json")
