"""Reads a remote git repo for an agent through gitingest (repo-read skill).

    rr.py ingest <url> [-i P ...] [-e P ...] [--max-kb 100] [--out LABEL] [--local]
        one gitingest digest of the repo, or of one folder with a .../tree/<branch>/<folder> URL;
        writes ~/gi/<owner>-<repo>[-LABEL].txt (file tree first, then file contents) and prints its size

The hosted API (gitingest.com) clones and filters on its side, so nothing is cloned here. It takes one
kind of pattern per call (-i or -e) and 10 calls a minute. --local runs the same gitingest library in the
workshop instead: when the API is down or rate-limited, for private repos (GITHUB_TOKEN), or to combine
-i with -e. Patterns are gitignore-style: a bare name matches at any depth, /x is the root entry x (./x
too), src/* is anchored at the root. gitingest always skips lock, minified and binary files and *.gradle,
*.svg and the like, unless -i names that exact pattern (-i '*.gradle'). Windows calls rr.py through the
rr wrapper, which sets RR_WIN_PREFIX so paths print as \\\\wsl.localhost\\workshop\\... for Windows tools.
"""
import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "common"))
import clierr  # noqa: E402

TOOL = "rr"
ROOT = Path(os.environ.get("RR_ROOT") or Path.home() / "gi")
API = "https://gitingest.com/api/ingest"
READ_BUDGET = 12_000  # gitingest tokens that still fit one Read call (Read counts ~1.6x more and stops at 25k)
CROPPED = "(Files content cropped"  # the API's marker when content passes 300k characters


def shown(path):
    """A path as the caller reads it: \\\\wsl.localhost\\workshop\\home\\... when the Windows wrapper calls."""
    prefix = os.environ.get("RR_WIN_PREFIX")
    return prefix + str(path).replace("/", "\\") if prefix else str(path)


def slug(url):
    """https://host/owner/repo[/tree/...] -> owner-repo, the digest's file name."""
    m = re.match(r"^(?:https?://[^/]+/|git@[^:]+:)?([^/\s]+)/([^/\s]+?)(?:\.git)?(?:/.*)?$", url.strip())
    if not m:
        raise clierr.Fail("BAD_ARG", f"not a repo URL: {url}", "pass https://github.com/<owner>/<repo>[/tree/<branch>/<folder>]")
    return f"{m.group(1)}-{m.group(2)}"


def split_patterns(values):
    """["a,b", "c"] -> [a, b, c]. gitingest splits patterns on spaces too, so a space is refused.
    ./x is the root anchor /x: Git Bash rewrites arguments that start with / into Windows paths."""
    out = [p.strip() for v in values or [] for p in v.split(",") if p.strip()]
    for p in out:
        if re.search(r"\s", p):
            raise clierr.Fail("BAD_ARG", f"pattern '{p}' contains a space, which gitingest splits", "use ? or * in place of the space")
    return ["/" + p[2:] if p.startswith("./") else p for p in out]


def token_count(summary):
    """'Estimated tokens: 12.3k' -> 12300, or None."""
    m = re.search(r"Estimated tokens:\s*([\d.]+)\s*([kM]?)", summary or "")
    return int(float(m.group(1)) * {"": 1, "k": 1e3, "M": 1e6}[m.group(2)]) if m else None


def via_api(url, include, exclude, max_kb):
    kind, pattern = ("include", include) if include else ("exclude", exclude)
    body = json.dumps({"input_text": url, "max_file_size": max_kb, "pattern_type": kind, "pattern": ",".join(pattern)}).encode()
    req = urllib.request.Request(API, body, {"Content-Type": "application/json", "User-Agent": "repo-read"})
    try:
        with urllib.request.urlopen(req, timeout=300) as r:
            data = json.load(r)
    except urllib.error.HTTPError as e:
        if e.code == 429:
            raise clierr.Fail("RATE_LIMITED", "gitingest.com allows 10 calls a minute", "wait a minute, or pass --local")
        detail = e.read().decode("utf-8", "replace")[:300]
        raise clierr.Fail("TOOL_FAILED", f"gitingest.com HTTP {e.code}: {detail}", "check the URL and patterns, or pass --local")
    except (urllib.error.URLError, TimeoutError) as e:
        raise clierr.Fail("TOOL_FAILED", f"gitingest.com unreachable: {e}", "pass --local")
    if data.get("error"):
        raise clierr.Fail("BAD_INPUT", f"gitingest.com: {data['error']}", "check the URL, branch and folder")
    content = data.get("content", "")
    if content.startswith(CROPPED):
        # The JSON holds only the first 300k characters; digest_url serves the whole digest (tree + content).
        with urllib.request.urlopen(urllib.parse.urljoin(API, data["digest_url"]), timeout=300) as r:
            return data["summary"], r.read().decode("utf-8", "replace")
    return data["summary"], data["tree"] + "\n" + content


def via_library(url, include, exclude, max_kb):
    from gitingest import ingest
    from loguru import logger
    logger.remove()  # gitingest logs every step at INFO; nothing may reach stderr but our one-line errors
    # include_gitignored, as the hosted API does: gitingest's .gitignore loader keeps patterns unordered
    # and drops tracked folders (Stirling-PDF: every folder), and a fresh clone has nothing to hide.
    summary, tree, content = ingest(url, max_file_size=max_kb * 1024, include_patterns=set(include) or None,
                                    exclude_patterns=set(exclude) or None, include_gitignored=True,
                                    token=os.environ.get("GITHUB_TOKEN"))
    return summary, tree + "\n" + content


def cmd_ingest(a):
    name = slug(a.source)
    include, exclude = split_patterns(a.include), split_patterns(a.exclude)
    if include and exclude and not a.local:
        raise clierr.Fail("BAD_ARG", "the gitingest API takes one kind of pattern per call", "drop -e (or -i), or pass --local")
    summary, text = (via_library if a.local else via_api)(a.source, include, exclude, a.max_kb)
    out = ROOT / f"{name}{'-' + a.out if a.out else ''}.txt"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(text, encoding="utf-8")

    files = m.group(1) if (m := re.search(r"Files analyzed:\s*(\d+)", summary)) else "?"
    tokens = token_count(summary)
    print(f"digest: {shown(out)}  files={files}  tokens={tokens if tokens is not None else '?'}")
    if files == "0":
        clierr.warn(TOOL, "EMPTY", "the patterns matched no files", "check the patterns against the tree of an earlier digest")
    elif tokens and tokens > READ_BUDGET:
        clierr.warn(TOOL, "TOO_BIG", f"digest is {tokens} tokens, over the {READ_BUDGET} one-Read budget",
                    "narrow -i, point the URL at one folder (/tree/<branch>/<folder>), or Read it in parts")


def main():
    p = argparse.ArgumentParser(prog="rr.py", description="gitingest digests of remote git repos for agents")
    sub = p.add_subparsers(dest="command", required=True)
    g = sub.add_parser("ingest", help="one gitingest digest of a repo or one of its folders")
    g.add_argument("source", help="repo URL; .../tree/<branch>/<folder> digests one folder")
    g.add_argument("-i", "--include", action="append", default=[])
    g.add_argument("-e", "--exclude", action="append", default=[])
    g.add_argument("--max-kb", type=int, default=100, help="skip files larger than this (default 100 KB)")
    g.add_argument("--out", help="label appended to the digest file name")
    g.add_argument("--local", action="store_true", help="run gitingest in the workshop instead of gitingest.com")
    a = p.parse_args()
    {"ingest": cmd_ingest}[a.command](a)


if __name__ == "__main__":
    clierr.run(TOOL, main)
