"""Mine past widget edits into benchmark candidates for local_patch.

Sources: consecutive src_backup_* snapshots of each crate (and the last one vs HEAD), plus git
commits. Each diff hunk whose new text is still in HEAD becomes a candidate: undo it to get the
task's starting state; HEAD is the expected result.

    conda run -n llm python mine.py            -> candidates.json (+ summary)
"""
import difflib
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(r"C:\Users\thurs\tools")
OUT = Path(__file__).parent / "candidates.json"
CRATES = {"taskbar-dock": "taskbar-dock", "taskbar-weather": "taskbar-weather",
          "taskbar-media": "taskbar-media", "taskbar-discord": "taskbar-discord",
          "buswidget/rust": "buswidget/rust"}


def git(*args) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True,
                          encoding="utf-8", errors="replace").stdout


def head_file(rel: str) -> str | None:
    r = subprocess.run(["git", "show", f"HEAD:{rel}"], cwd=ROOT, capture_output=True)
    return r.stdout.decode("utf-8", "replace").replace("\r\n", "\n") if r.returncode == 0 else None


def snapshots(crate: str) -> list[Path]:
    """src_backup_* folders of a crate (bus: also the old top-level ones), oldest first."""
    base = ROOT / crate
    dirs = [d for d in base.glob("src_backup*") if d.is_dir()]
    if crate == "buswidget/rust":
        dirs += [d for d in (ROOT / "buswidget").glob("src_backup*") if d.is_dir()]
    return sorted(dirs, key=lambda d: d.stat().st_mtime)


def hunks(old: str, new: str):
    """(old_lines, new_lines, context_before, context_after) per change, 3 lines of context."""
    a, b = old.split("\n"), new.split("\n")
    for grp in difflib.SequenceMatcher(None, a, b, autojunk=False).get_grouped_opcodes(3):
        i1, i2, j1, j2 = grp[0][1], grp[-1][2], grp[0][3], grp[-1][4]
        yield a[i1:i2], b[j1:j2]


def changed(old: list[str], new: list[str]) -> int:
    return sum(1 for op in difflib.ndiff(old, new) if op[:1] in "+-")


def main():
    cands, seen = [], set()

    def consider(rel: str, old_text: str, new_text: str, source: str):
        head = head_file(rel)
        if head is None:
            return
        for o, n in hunks(old_text, new_text):
            k = changed(o, n)
            if not 2 <= k <= 40:
                continue
            nb = "\n".join(n)
            if head.count(nb) != 1:  # the new text must still be in HEAD, exactly once
                continue
            key = (rel, nb)
            if key in seen:
                continue
            seen.add(key)
            start = head[:head.index(nb)].count("\n") + 1
            cands.append({"id": f"c{len(cands) + 1:03d}", "file": rel, "source": source,
                          "line": start, "old": "\n".join(o), "new": nb, "changed_lines": k,
                          "diff": "\n".join(difflib.unified_diff(o, n, "before", "after", lineterm="", n=0))})

    for crate in CRATES:
        snaps = snapshots(crate)
        for s_old, s_new in zip(snaps, snaps[1:] + [None]):
            for f in sorted(s_old.rglob("*.rs")):
                sub = f.relative_to(s_old).as_posix()
                rel = f"{crate}/src/{sub}"
                if s_new is None:
                    new_text = head_file(rel)
                else:
                    g = s_new / sub
                    new_text = g.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n") if g.exists() else None
                if new_text is None:
                    continue
                old_text = f.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n")
                consider(rel, old_text, new_text, f"{s_old.name} -> {s_new.name if s_new else 'HEAD'}")
    for c in git("log", "--format=%H", "--", "*.rs").split():
        for rel in git("diff", "--name-only", f"{c}^", c, "--", "*.rs").split():
            o = subprocess.run(["git", "show", f"{c}^:{rel}"], cwd=ROOT, capture_output=True)
            n = subprocess.run(["git", "show", f"{c}:{rel}"], cwd=ROOT, capture_output=True)
            if o.returncode == 0 and n.returncode == 0:
                consider(rel, o.stdout.decode("utf-8", "replace").replace("\r\n", "\n"),
                         n.stdout.decode("utf-8", "replace").replace("\r\n", "\n"), f"commit {c[:7]}")
    OUT.write_text(json.dumps({"head": git("rev-parse", "HEAD").strip(), "candidates": cands},
                              indent=1, ensure_ascii=False), encoding="utf-8")
    by = {}
    for c in cands:
        by[c["file"].split("/src/")[0]] = by.get(c["file"].split("/src/")[0], 0) + 1
    print(f"{len(cands)} candidates -> {OUT}\nper crate: {by}")
    sizes = sorted(c["changed_lines"] for c in cands)
    if sizes:
        print(f"changed lines: min {sizes[0]}, median {sizes[len(sizes) // 2]}, max {sizes[-1]}")


if __name__ == "__main__":
    sys.exit(main())
