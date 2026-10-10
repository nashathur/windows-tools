"""Run the local_patch benchmark: each task undoes a past edit in a scratch worktree and asks the
model to redo it from a written instruction; the result is compared with what was really written.

    conda run -n llm python run.py --label base [--model qwen3.5:9b] [--attempts 1] [--tries 3]
                                   [--only t01,t05]
-> results/<label>.json and a row in results/summary.md

Scores per task: pass (compiles), exact (file identical to the real edit, ignoring trailing
spaces), overlap (Jaccard of the changed lines vs the real edit's; 1.0 = same lines).
"""
import argparse
import difflib
import json
import time

from common import HERE, crates_for, ct, set_before, worktree_at

import server  # noqa: E402  (chat; imports the MCP server module without running it)

ap = argparse.ArgumentParser()
ap.add_argument("--label", required=True)
ap.add_argument("--model", default="")
ap.add_argument("--attempts", type=int, default=1)
ap.add_argument("--tries", type=int, default=3)
ap.add_argument("--only", default="")
ap.add_argument("--margin", type=int, default=40, help="lines of context shown around the edit")
args = ap.parse_args()

data = json.loads((HERE / "tasks.json").read_text(encoding="utf-8"))
tasks = [t for t in data["tasks"] if not args.only or t["id"] in args.only.split(",")]
out_dir = HERE / "results"
out_dir.mkdir(exist_ok=True)


def norm(text: str) -> list[str]:
    return [ln.rstrip() for ln in text.replace("\r\n", "\n").split("\n")]


def changes(a: list[str], b: list[str]) -> set:
    return {ln[0] + ln[1:].strip() for ln in difflib.unified_diff(a, b, lineterm="", n=0)
            if ln[:1] in "+-" and not ln.startswith(("+++", "---")) and ln[1:].strip()}


rows = []
for t in tasks:
    t0 = time.time()
    wt = worktree_at(data["head"])
    set_before(wt, t)
    before = norm((wt / t["file"]).read_text(encoding="utf-8", errors="replace"))
    truth = norm(ct.sh(["git", "show", f"{data['head']}:{t['file']}"], cwd=wt).stdout)
    n_old = t["old"].count("\n") + 1
    a = max(1, t["line"] - args.margin)
    b = t["line"] + n_old + args.margin
    specs = [(t["file"], a, b)]
    try:
        res = ct.run_patch(server.chat, wt, specs, t["instruction"], t.get("example", ""), "check",
                           crates_for(wt, t["file"]), args.tries, args.attempts, args.model)
    except Exception as e:  # keep the run going
        res = {"error": f"{type(e).__name__}: {e}"}
    if "error" in res:
        row = {"id": t["id"], "pass": False, "exact": False, "overlap": 0.0, "error": res["error"]}
    else:
        after = norm((wt / t["file"]).read_text(encoding="utf-8", errors="replace"))
        want, got = changes(before, truth), changes(before, after)
        overlap = len(want & got) / len(want | got) if want | got else 1.0
        row = {"id": t["id"], "pass": res["ok"], "exact": after == truth, "overlap": round(overlap, 2),
               "calls": res["rounds"], "attempt": res["attempt"], "ctx": res["ctx"],
               "diff": res["diff"][:4000], "errs": [e[:300] for e in res["errs"][:3]]}
    row["secs"] = round(time.time() - t0)
    rows.append(row)
    print(f"{t['id']}: {'PASS' if row['pass'] else 'fail'} exact={row['exact']} overlap={row['overlap']} "
          f"{row['secs']}s", flush=True)

n = len(rows)
summ = {"label": args.label, "model": args.model or ct.CODE_MODEL, "attempts": args.attempts,
        "tries": args.tries, "n": n,
        "pass": sum(r["pass"] for r in rows), "exact": sum(r["exact"] for r in rows),
        "pass_and_close": sum(r["pass"] and r["overlap"] >= 0.8 for r in rows),
        "mean_overlap": round(sum(r["overlap"] for r in rows) / max(n, 1), 2),
        "mean_secs": round(sum(r["secs"] for r in rows) / max(n, 1))}
(out_dir / f"{args.label}.json").write_text(json.dumps({"summary": summ, "rows": rows}, indent=1,
                                                        ensure_ascii=False), encoding="utf-8")
md = out_dir / "summary.md"
if not md.exists():
    md.write_text("| label | model | attempts | tries | tasks | compiles | close (overlap>=0.8) | exact "
                  "| mean overlap | s/task |\n|---|---|---|---|---|---|---|---|---|---|\n", encoding="utf-8")
with open(md, "a", encoding="utf-8") as f:
    f.write(f"| {summ['label']} | {summ['model']} | {summ['attempts']} | {summ['tries']} | {n} | "
            f"{summ['pass']} | {summ['pass_and_close']} | {summ['exact']} | {summ['mean_overlap']} | "
            f"{summ['mean_secs']} |\n")
print(json.dumps(summ))
