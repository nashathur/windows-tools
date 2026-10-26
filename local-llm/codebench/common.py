"""Shared helpers for the local_patch benchmark (mine.py, select.py, run.py)."""
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import code_tools as ct  # noqa: E402

ROOT = Path(r"C:\Tools")
HERE = Path(__file__).resolve().parent
ct.SCRATCH = Path(r"D:\lp\bench")  # own worktree/target, so it never collides with live local_patch


def worktree_at(commit: str) -> Path:
    """Fresh scratch worktree of the repo at a fixed commit (not the live working tree)."""
    wt = ct.SCRATCH / "wt"
    ct.SCRATCH.mkdir(parents=True, exist_ok=True)
    ct.sh(["git", "worktree", "remove", "--force", str(wt)], cwd=ROOT)
    if wt.exists():
        shutil.rmtree(wt, ignore_errors=True)
    ct.sh(["git", "worktree", "prune"], cwd=ROOT)
    ct.git(["worktree", "add", "--detach", str(wt), commit], ROOT)
    return wt


def set_before(wt: Path, cand: dict) -> bool:
    """Undo the candidate's change in the worktree (HEAD state -> before state) and commit it,
    so diffs show only the model's edits. False if the new text isn't there."""
    f = wt / cand["file"]
    raw = f.read_bytes().decode("utf-8")
    crlf = "\r\n" in raw
    text = raw.replace("\r\n", "\n")
    if text.count(cand["new"]) != 1:
        return False
    text = text.replace(cand["new"], cand["old"])
    f.write_bytes((text.replace("\n", "\r\n") if crlf else text).encode("utf-8"))
    ct.git(["-c", "user.name=bench", "-c", "user.email=bench@local", "commit", "-q", "-am",
            f"before {cand['id']}"], wt)
    return True


def crates_for(wt: Path, rel: str) -> set:
    c = ct.crate_of(wt / rel, wt)
    out = {c} if c else set()
    for x in list(out):
        out.update(ct.dependents(x, wt))
    return out
