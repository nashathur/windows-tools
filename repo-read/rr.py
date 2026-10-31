"""Reads a remote git repo for an agent in few calls (repo-read skill).

    rr.py clone <url> [--branch B] [--name N] [--depth 2]
        clone (on GitHub, partial over 200 MB) and print a structure map: files per folder with their
        top extensions, key files, CI workflows
        (a /tree/ URL gives the first segment as branch; branch names with / need --branch)
    rr.py grep <name|path> <regex> [path ...] [-i] [--max 200]
        grep the clone where it lives; each match prints as <path>:<line>:<text>, path readable by the caller
    rr.py checkout <name|path> <pattern> ...
        add folders or patterns to a partial clone's checkout, for grepping them

Checkout patterns are gitignore-style (git sparse-checkout, no-cone): /x is the root entry x (./x too),
a bare name (tests) matches at any depth. Runs in the workshop; Windows calls it through the rr
wrapper, which sets RR_WIN_PREFIX so paths print as \\\\wsl.localhost\\workshop\\... for Windows tools.
"""
import argparse
import fnmatch
import json
import os
import re
import shutil
import subprocess
import sys
import urllib.request
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "common"))
import clierr  # noqa: E402

TOOL = "rr"
ROOT = Path(os.environ.get("RR_ROOT") or Path.home() / "gi")
PARTIAL_KB = 200_000  # repo size above which only the root files are downloaded until checkout
MAX_ROWS, MAX_KEY, MAX_WORKFLOWS, KEY_DEPTH = 60, 40, 15, 4
# Folders whose files are noise in the key-file list (vendored, generated or test copies of manifests).
NOISE_DIRS = {"node_modules", "vendor", "dist", "build", "test", "tests", "__tests__", "e2e", "fixtures", "testdata"}
KEY_FILES = ["README*", "AGENTS.md", "CLAUDE.md", "CONTRIBUTING*",
             "package.json", "pyproject.toml", "setup.py", "Cargo.toml", "go.mod", "pom.xml", "build.gradle*", "settings.gradle*",
             "Makefile", "Taskfile.y*ml", "justfile", "Dockerfile*", "docker-compose*.y*ml", "compose*.y*ml",
             "main.*", "App.*", "app.py", "index.ts", "index.tsx", "index.js", "server.*", "cli.*", "__main__.py",
             "*Application.java", "*Application.kt", "AndroidManifest.xml", "capacitor.config.*", "vite.config.*", "tauri.conf.json"]


def shown(path):
    """A path as the caller reads it: \\\\wsl.localhost\\workshop\\home\\... when the Windows wrapper calls."""
    prefix = os.environ.get("RR_WIN_PREFIX")
    return prefix + str(path).replace("/", "\\") if prefix else str(path)


def repo_ref(url):
    """https://host/owner/repo[.git][/tree/<branch>[/sub]] or git@host:owner/repo.git -> dict, or None."""
    u = url.strip().rstrip("/")
    m = re.match(r"^git@([^:]+):(.+)$", u) or re.match(r"^https?://([^/]+)/(.+)$", u)
    if not m:
        return None
    host, path = m.group(1), m.group(2)
    branch = sub = None
    t = re.match(r"^(.+?)/(?:-/)?tree/(.+)$", path)
    if t:
        path, rest = t.group(1), t.group(2)
        branch, _, sub = rest.partition("/")
        sub = sub or None
    path = re.sub(r"\.git$", "", path)
    parts = [p for p in path.split("/") if p]
    if len(parts) < 2:
        return None
    # GitHub repos are owner/repo; GitLab subgroups nest, so the repo is the last segment there.
    owner, repo = (parts[0], parts[1]) if host == "github.com" else ("-".join(parts[:-1]), parts[-1])
    clone_url = u if u.startswith("git@") else f"https://{host}/{path}"
    return {"host": host, "owner": owner, "repo": repo, "branch": branch, "sub": sub, "clone_url": clone_url}


def repo_key(url):
    """Repo identity for comparing origins: no .git, host lowercased, owner/repo case kept."""
    u = re.sub(r"\.git$", "", url.strip())
    m = re.match(r"^([A-Za-z][\w+.-]*://)([^/]+)(.*)$", u)
    if m:
        return m.group(1) + m.group(2).lower() + m.group(3)
    m = re.match(r"^([^@/]+@)([^:]+):(.*)$", u)
    return m.group(1) + m.group(2).lower() + ":" + m.group(3) if m else u


def split_patterns(values):
    """["a,b", "c"] -> [a, b, c]. ./x is the root anchor /x: Git Bash rewrites arguments that start
    with / into Windows paths."""
    out = [p.strip() for v in values or [] for p in v.split(",") if p.strip()]
    return ["/" + p[2:] if p.startswith("./") else p for p in out]


def git(*args, fix=None):
    return clierr.run_tool(["git", *args], fix=fix).stdout.strip()


def github_token():
    if os.environ.get("GITHUB_TOKEN"):
        return os.environ["GITHUB_TOKEN"]
    if shutil.which("gh"):
        r = subprocess.run(["gh", "auth", "token"], capture_output=True, text=True)
        if r.returncode == 0 and r.stdout.strip():
            return r.stdout.strip()
    return None


def github_api(path, token):
    req = urllib.request.Request(f"https://api.github.com/{path}", headers={"Accept": "application/vnd.github+json", "User-Agent": "rr"})
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            return json.load(resp)
    except urllib.error.HTTPError as e:
        hint = "check the URL; a private repo needs GITHUB_TOKEN or gh auth login" if e.code == 404 else \
            "rate limit: wait an hour, or set GITHUB_TOKEN / gh auth login" if e.code in (403, 429) else "retry later"
        raise clierr.Fail("TOOL_FAILED", f"GitHub API {e.code} for {path}", hint)


def git_unquote(s):
    """git C-quotes paths with non-ASCII bytes: "src/\\303\\251/a.c" -> src/é/a.c."""
    if len(s) < 2 or not (s.startswith('"') and s.endswith('"')):
        return s
    raw = s[1:-1].encode("latin-1", "backslashreplace").decode("unicode_escape")
    return raw.encode("latin-1").decode("utf-8", "replace")


def current_branch(repo):
    """The checked-out branch, or "" when HEAD is detached (a clone of a tag)."""
    r = subprocess.run(["git", "-C", str(repo), "symbolic-ref", "-q", "--short", "HEAD"], capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else ""


def render_map(paths, depth):
    """Structure map lines for a file list: files per folder (to `depth` levels) with the two most common
    extensions, then key files (README, manifests, entry points) and CI workflows."""
    groups = {}
    for p in paths:
        segs = p.split("/")
        key = "(root files)" if len(segs) == 1 else "/".join(segs[:min(depth, len(segs) - 1)]) + "/"
        # No extension: the name stands for the kind (Dockerfile).
        groups.setdefault(key, Counter())[os.path.splitext(segs[-1])[1].lstrip(".") or segs[-1]] += 1
    keys = sorted(groups)
    lines = []
    for key in keys[:MAX_ROWS]:
        top = sorted(groups[key].items(), key=lambda kv: (-kv[1], kv[0]))[:2]
        lines.append(f"{sum(groups[key].values()):>6} files  {key}  {', '.join(f'{e} {n}' for e, n in top)}")
    if len(keys) > MAX_ROWS:
        lines.append(f"... +{len(keys) - MAX_ROWS} more folders (rr clone again with --depth 1, or grep)")

    patterns = [k.lower() for k in KEY_FILES]
    key_files = []
    for p in paths:
        segs = p.split("/")
        if len(segs) <= KEY_DEPTH and not NOISE_DIRS.intersection(segs[:-1]) \
                and any(fnmatch.fnmatchcase(segs[-1].lower(), k) for k in patterns):
            key_files.append(p)
    key_files.sort(key=lambda p: (p.count("/"), p))
    lines += ["", "key files:", *(f"  {p}" for p in key_files[:MAX_KEY])]

    names = sorted(p.rsplit("/", 1)[1] for p in paths if p.startswith(".github/workflows/") and p.count("/") == 2)
    ci = []
    if names:
        more = f", ... +{len(names) - MAX_WORKFLOWS}" if len(names) > MAX_WORKFLOWS else ""
        ci.append(f"{len(names)} in .github/workflows ({', '.join(names[:MAX_WORKFLOWS])}{more})")
    if ".gitlab-ci.yml" in paths:
        ci.append(".gitlab-ci.yml")
    if ci:
        lines.append("workflows: " + "; ".join(ci))
    return lines


def cmd_clone(a):
    ref = repo_ref(a.source)
    if not ref:
        raise clierr.Fail("BAD_ARG", f"not a repo URL: {a.source}", "pass https://<host>/<owner>/<repo> or git@<host>:<owner>/<repo>.git")
    br = a.branch or ref["branch"]
    name = a.name or f"{ref['owner']}-{ref['repo']}"
    dest = ROOT / "src" / name

    # GitHub: the repo's size and default branch first, so a big repo is cloned partially.
    github = ref["host"] == "github.com"
    token, size_kb = None, None
    if github:
        token = github_token()
        meta = github_api(f"repos/{ref['owner']}/{ref['repo']}", token)
        size_kb = meta.get("size", 0)
        br = br or meta["default_branch"]
    partial = bool(size_kb and size_kb > PARTIAL_KB)
    wanted = br or ""

    if dest.exists():
        origin = git("-C", str(dest), "remote", "get-url", "origin", fix="pass --name to clone into a new folder")
        if repo_key(origin) != repo_key(ref["clone_url"]):
            raise clierr.Fail("BAD_INPUT", f"{dest} holds another repo ({origin})", "pass --name <new folder name>")
        # rr.branch is set at clone time; HEAD cannot say which branch a shallow clone was asked for.
        r = subprocess.run(["git", "-C", str(dest), "config", "--get", "rr.branch"], capture_output=True, text=True)
        if r.returncode != 0:
            raise clierr.Fail("BAD_INPUT", f"{dest} has no recorded branch", "delete the folder or pass --name")
        stored = r.stdout.strip()
        if stored != wanted:
            raise clierr.Fail("BAD_INPUT", f"{dest} holds branch '{stored or 'default'}', not '{wanted or 'default'}'",
                              "pass --name <new folder name> to clone that branch alongside")
        # HEAD can move (checkout, detach) after the clone; grep reads what is checked out.
        r = subprocess.run(["git", "-C", str(dest), "config", "--get", "rr.head"], capture_output=True, text=True)
        if r.returncode != 0:
            raise clierr.Fail("BAD_INPUT", f"{dest} has no recorded checkout", "delete the folder or pass --name")
        recorded, current = r.stdout.strip(), current_branch(dest)
        if current != recorded:
            raise clierr.Fail("BAD_INPUT", f"{dest} is checked out at '{current or 'detached HEAD'}', not '{recorded or 'detached HEAD'}'",
                              "delete the folder or pass --name")
        how = "reused"
    else:
        dest.parent.mkdir(parents=True, exist_ok=True)
        cmd = ["clone", "--depth", "1"] + (["--branch", br] if br else []) + (["--filter=blob:none", "--no-checkout"] if partial else [])
        git(*cmd, ref["clone_url"], str(dest), fix="check the URL and your git credentials")
        git("-C", str(dest), "config", "rr.branch", wanted)
        if partial:
            # Root files only; checkout adds folders.
            git("-C", str(dest), "sparse-checkout", "set", "--no-cone", "/*", "!/*/")
            git("-C", str(dest), "checkout")
        git("-C", str(dest), "config", "rr.head", current_branch(dest))
        how = "partial clone" if partial else "shallow clone"
    sha = git("-C", str(dest), "rev-parse", "HEAD")
    if github:
        # The tree of the checkout's own commit; a partial clone has only the root files on disk.
        tree = github_api(f"repos/{ref['owner']}/{ref['repo']}/git/trees/{sha}?recursive=1", token)
        if tree.get("truncated"):
            clierr.warn(TOOL, "TRUNCATED", "GitHub listed only part of the tree", "the map and file count are lower bounds")
        files = [t["path"] for t in tree["tree"] if t["type"] == "blob"]
    else:
        files = [git_unquote(l) for l in git("-C", str(dest), "ls-tree", "-r", "--name-only", "HEAD").splitlines() if l]

    print(f"repo:   {ref['clone_url']} @ {sha[:7]} ({how})")
    print(f"clone:  {shown(dest)}")
    print(f"files:  {len(files)}")
    if partial:
        print(f"partial clone: root files only; rr checkout {name} <folder> ... before grepping or reading other folders")
    print()
    print("\n".join(render_map(files, a.depth)))
    print()
    print(f"next: read the README's opening section and the build manifest, then rr grep {name} <regex> for the operation asked about")


def find_clone(name_or_path):
    for c in (Path(name_or_path), ROOT / "src" / name_or_path):
        if (c / ".git").exists():
            return c.resolve()
    raise clierr.Fail("NOT_FOUND", f"no clone named {name_or_path}", "run 'rr.py clone <url>' first, or pass the clone's folder")


def cmd_checkout(a):
    """Adds folders or patterns to a partial clone's checkout, for grepping them."""
    clone = find_clone(a.source)
    patterns = split_patterns(a.patterns)
    if git("-C", str(clone), "config", "--bool", "--default", "false", "core.sparseCheckout") != "true":
        print(f"{shown(clone)} is a full clone; every file is already there")
        return
    git("-C", str(clone), "sparse-checkout", "add", *patterns, fix="check the patterns against the rr clone map")
    print(f"checked out {', '.join(patterns)} in {shown(clone)}")


def cmd_grep(a):
    """grep inside the workshop: Windows tools grepping \\\\wsl.localhost run ~500x slower (19 s vs 0.04 s on 5.8k files)."""
    clone = find_clone(a.source)
    where = []
    for p in a.paths:
        t = (clone / p.lstrip("/")).resolve()  # resolve() also catches a symlink operand that points outside
        if not t.is_relative_to(clone):
            raise clierr.Fail("BAD_ARG", f"path {p} leaves the clone", "name folders or files inside the clone")
        where.append(str(t))
    cmd = ["grep", "-rnHIE", "--exclude-dir=.git", *(["-i"] if a.ignore_case else []), "-e", a.regex, *(where or [str(clone)])]
    r = subprocess.run(cmd, capture_output=True, text=True, errors="replace")
    if r.returncode not in (0, 1):  # a signal-killed grep has a negative code
        raise clierr.Fail("TOOL_FAILED", f"grep exit {r.returncode}: {clierr.relevant_line(r.stderr)}", "check the regex (extended syntax) and paths", tool_run=r)
    lines = r.stdout.splitlines()
    prefix = str(clone) + "/"
    for line in lines[:a.max]:
        path, _, rest = line.partition(":")
        rel = path[len(prefix):] if path.startswith(prefix) else path
        print(f"{shown(clone / rel)}:{rest[:300]}")
    if len(lines) > a.max:
        clierr.warn(TOOL, "MORE", f"{len(lines)} matches, showing {a.max}", "narrow the regex or the paths, or raise --max")
    elif not lines:
        print("no matches")


def main():
    p = argparse.ArgumentParser(prog="rr.py", description="clone, map and grep remote git repos for agents")
    sub = p.add_subparsers(dest="command", required=True)
    m = sub.add_parser("clone", help="clone and print a structure map: folders, key files, workflows")
    m.add_argument("source", help="repo URL")
    m.add_argument("--branch")
    m.add_argument("--name", help="clone folder name (default <owner>-<repo>)")
    m.add_argument("--depth", type=int, default=2, choices=range(1, 11), metavar="1-10")
    c = sub.add_parser("checkout", help="add folders or patterns to a partial clone, for grepping")
    c.add_argument("source", help="clone name (from clone) or folder")
    c.add_argument("patterns", nargs="+")
    g = sub.add_parser("grep", help="grep a clone inside the workshop; paths print readable from the caller")
    g.add_argument("source", help="clone name (from clone) or folder")
    g.add_argument("regex", help="extended regex")
    g.add_argument("paths", nargs="*", help="folders or files inside the clone (default: all)")
    g.add_argument("-i", "--ignore-case", action="store_true")
    g.add_argument("--max", type=int, default=200, help="matches to print (default 200)")
    a = p.parse_args()
    {"clone": cmd_clone, "checkout": cmd_checkout, "grep": cmd_grep}[a.command](a)


if __name__ == "__main__":
    clierr.run(TOOL, main)
