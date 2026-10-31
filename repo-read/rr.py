"""Reads a remote git repo for an agent in few calls (repo-read skill).

    rr.py map <url> [--branch B] [--name N] [--depth 2] [--all]
        clone (on GitHub, partial over 200 MB) and print estimated digest tokens per folder
        (a /tree/ URL gives the first segment as branch; branch names with / need --branch)
    rr.py digest <name|path> [-i P ...] [-e P ...] [--out LABEL] [--all]
        gitingest digest of the clone; a partial clone first checks out what -i names
    rr.py checkout <name|path> <pattern> ...
        add folders or patterns to a partial clone's checkout, for grepping them
    rr.py grep <name|path> <regex> [path ...] [-i] [--max 200]
        grep the clone where it lives; each match prints as <path>:<line>:<text>, path readable by the caller

Patterns are gitignore-style, the language gitingest and git's sparse checkout both read: a bare
name (tests) matches at any depth, one with a slash (src/*) is anchored at the repo root, /x is the
root entry x (./x too). rr's default excludes (tests, translations, snapshots) stay out unless --all or named in -i;
gitingest itself always skips lock, minified and binary files. Runs in the workshop (gitingest 0.3.1
in the dev env); Windows calls it through the rr wrapper, which sets RR_WIN_PREFIX so paths print as
\\\\wsl.localhost\\workshop\\... for Windows tools.
"""
import argparse
import fnmatch
import json
import os
import re
import shutil
import subprocess
import sys
import urllib.parse
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "common"))
import clierr  # noqa: E402

TOOL = "rr"
ROOT = Path(os.environ.get("RR_ROOT") or Path.home() / "gi")
PARTIAL_KB = 200_000  # repo size above which only what a digest names is downloaded
READ_BUDGET = 12_000  # gitingest tokens that still fit one Read call (Read counts ~1.6x more and stops at 25k)
MAX_FILE_BYTES = 10 * 1024 * 1024  # gitingest's default max_file_size
DEFAULT_EXCLUDES = ["tests", "test", "__tests__", "__snapshots__", "e2e", "cypress", "fixtures",
                    "testdata", "locales", "i18n", "l10n", "translations", "snapshots", "*.test.*", "*.spec.*", "*.snap"]
# Binary files gitingest drops by content, which the map cannot read before cloning.
BINARY = ["*.ttf", "*.otf", "*.woff", "*.woff2", "*.eot", "*.png", "*.jpg", "*.jpeg", "*.gif", "*.bmp",
          "*.ico", "*.webp", "*.tif", "*.tiff", "*.pdf", "*.zip", "*.gz", "*.tgz", "*.7z", "*.jar", "*.onnx",
          "*.bin", "*.dll", "*.so", "*.dylib", "*.exe", "*.mp3", "*.mp4", "*.wav", "*.webm", "*.wasm", "*.bcmap"]
FILE_RULE = "=" * 48


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


def token_count(summary):
    """gitingest's 'Estimated tokens: 36.6k' -> 36600, or None."""
    m = re.search(r"Estimated tokens:\s*([\d.]+)\s*([kKmM]?)", summary or "")
    if not m:
        return None
    return int(float(m.group(1)) * {"k": 1e3, "m": 1e6}.get(m.group(2).lower(), 1))


def fmt(n):
    return f"{n / 1e6:.1f}M" if n >= 1e6 else f"{n / 1e3:.1f}k" if n >= 1e3 else str(int(n))


def split_patterns(values):
    """-i a,b -i c -> [a, b, c]; a space would be split again by gitingest, so it is refused.
    ./x is the root anchor /x: Git Bash rewrites arguments that start with / into Windows paths."""
    out = [p.strip() for v in values or [] for p in v.split(",") if p.strip()]
    out = ["/" + p[2:] if p.startswith("./") else p for p in out]
    for p in out:
        if re.search(r"\s", p):
            raise clierr.Fail("BAD_ARG", f"pattern '{p}' contains a space, which gitingest splits", "use ? or * in place of the space")
    return out


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


def gitingest_ignores():
    from gitingest.utils.ignore_patterns import DEFAULT_IGNORE_PATTERNS
    return set(DEFAULT_IGNORE_PATTERNS)


def spec(patterns):
    import pathspec
    return pathspec.PathSpec.from_lines("gitwildmatch", patterns)


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


def cmd_map(a):
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
        # HEAD can move (checkout, detach) after the clone; digest and grep read what is checked out.
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
            # Root files only; digest adds what its -i names.
            git("-C", str(dest), "sparse-checkout", "set", "--no-cone", "/*", "!/*/")
            git("-C", str(dest), "checkout")
        git("-C", str(dest), "config", "rr.head", current_branch(dest))
        how = "partial clone (root files only until digest)" if partial else "shallow clone"
    sha = git("-C", str(dest), "rev-parse", "HEAD")
    if github:
        # The tree of the checkout's own commit, so sizes match what the digest reads.
        tree = github_api(f"repos/{ref['owner']}/{ref['repo']}/git/trees/{sha}?recursive=1", token)
        if tree.get("truncated"):
            clierr.warn(TOOL, "TRUNCATED", "GitHub listed only part of the tree", "folder sizes are lower bounds")
        files = [(t["path"], t.get("size", 0)) for t in tree["tree"] if t["type"] == "blob"]
    else:
        # Other hosts: sizes from the full shallow clone (a partial clone would fetch each file to size it).
        files = []
        for line in git("-C", str(dest), "ls-tree", "-r", "-l", "HEAD").splitlines():
            m = re.match(r"^\S+ blob \S+\s+(\d+)\t(.+)$", line)
            if m:
                files.append((git_unquote(m.group(2)), int(m.group(1))))

    skip = spec(gitingest_ignores() | set(BINARY))
    default = spec(DEFAULT_EXCLUDES)
    rows, total, n_files, dropped, dropped_bytes = {}, 0, 0, 0, 0
    for path, size in files:
        if size > MAX_FILE_BYTES or skip.match_file(path):
            continue
        if not a.all and default.match_file(path):
            dropped += 1
            dropped_bytes += size
            continue
        segs = path.split("/")
        key = "(root files)" if len(segs) == 1 else "/".join(segs[:min(a.depth, len(segs) - 1)]) + "/"
        r = rows.setdefault(key, [0, 0])
        r[0] += 1
        r[1] += size
        total += size
        n_files += 1
    print(f"repo:   {ref['clone_url']} @ {sha[:7]} ({how})")
    print(f"clone:  {shown(dest)}")
    print(f"digest candidates: {fmt(total / 4)} tokens in {n_files} files (bytes/4)")
    if dropped:
        print(f"excluded by default (tests, translations, snapshots): {dropped} files, {fmt(dropped_bytes / 4)} tokens; --all keeps them")
    print()
    for key in sorted(rows):
        print(f"{fmt(rows[key][1] / 4):>8} {rows[key][0]:>6} files  {key}")
    print()
    print(f"next: rr digest {name} -i <patterns> (keep each digest under {fmt(READ_BUDGET)} tokens)")


def find_clone(name_or_path):
    for c in (Path(name_or_path), ROOT / "src" / name_or_path):
        if (c / ".git").exists():
            return c.resolve()
    raise clierr.Fail("NOT_FOUND", f"no clone named {name_or_path}", "run 'rr.py map <url>' first, or pass the clone's folder")


def cmd_checkout(a):
    """Adds folders or patterns to a partial clone's checkout, for grepping them."""
    clone = find_clone(a.source)
    patterns = split_patterns(a.patterns)
    if git("-C", str(clone), "config", "--bool", "--default", "false", "core.sparseCheckout") != "true":
        print(f"{shown(clone)} is a full clone; every file is already there")
        return
    git("-C", str(clone), "sparse-checkout", "add", *patterns, fix="check the patterns against rr map")
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


def cmd_digest(a):
    clone = find_clone(a.source)
    include, exclude = split_patterns(a.include), split_patterns(a.exclude)
    if any(p.startswith("!") for p in include + exclude):
        # gitingest keeps patterns in a set, so the order "!" relies on is lost.
        raise clierr.Fail("BAD_ARG", "negation patterns (!) are not supported in digest", "list what to include, and exclude with -e")
    sparse = git("-C", str(clone), "config", "--bool", "--default", "false", "core.sparseCheckout") == "true"
    if sparse:
        if not include:
            raise clierr.Fail("BAD_ARG", "a partial clone needs -i", "name the folders or files to digest; rr.py map shows them")
        git("-C", str(clone), "sparse-checkout", "add", *include, fix="check the -i patterns")

    ignored = any(l.startswith("!! ") for l in git("-C", str(clone), "status", "--porcelain", "--ignored").splitlines())
    if ignored:
        clierr.warn(TOOL, "IGNORED_FILES", "the checkout holds git-ignored files; they are digested too",
                    "digest a fresh clone (rr.py map) to read only tracked files")

    import codecs
    from gitingest.config import MAX_FILES
    from gitingest.ingestion import ingest_query
    from gitingest.output_formatter import _format_token_count
    from gitingest.query_parser import parse_local_dir_path
    from gitingest.schemas import filesystem
    from gitingest.utils.pattern_utils import process_patterns
    from loguru import logger
    logger.remove()  # gitingest logs every step at INFO; nothing may reach stderr but our one-line errors

    # gitingest calls a file binary when its first 1024-byte chunk fails a strict UTF-8 decode, so a
    # multi-byte character cut at the chunk edge turned text into "[Binary file]"; let a cut tail pass.
    original_decodes = filesystem._decodes

    def decodes(chunk, encoding):
        if codecs.lookup(encoding).name != "utf-8":
            return original_decodes(chunk, encoding)
        try:
            codecs.getincrementaldecoder("utf-8")().decode(chunk, final=False)
        except UnicodeDecodeError:
            return False
        return True

    filesystem._decodes = decodes
    # A default exclude that -i names (or sits under a named path) stays in; -e excludes stay.
    comps = [c for p in include for c in p.strip("/").split("/") if c]
    defaults = [] if a.all else [d for d in DEFAULT_EXCLUDES if not any(fnmatch.fnmatchcase(c, d) for c in comps)]
    excludes = defaults + exclude

    # Local ingestion, not ingest(): its source dispatcher takes any path containing github.com or gitlab.com
    # for a remote URL and strips a trailing .git; its .gitignore loader keeps patterns unordered (it drops
    # tracked folders) and opens .gitignore/.gitingestignore symlinks that point outside the clone.
    query = parse_local_dir_path(str(clone))
    query.max_file_size = MAX_FILE_BYTES
    query.ignore_patterns, query.include_patterns = process_patterns(exclude_patterns=set(excludes) or None,
                                                                     include_patterns=set(include) or None)
    summary, tree, content = ingest_query(query)
    count = int(m.group(1)) if (m := re.search(r"Files analyzed:\s*(\d+)", summary)) else 0
    if count >= MAX_FILES:
        clierr.warn(TOOL, "TRUNCATED", f"gitingest stops at {MAX_FILES} files", "narrow -i")

    # gitingest's built-in ignores win over -i (build.gradle under *.gradle). That is the only omission
    # repaired: a tracked file -i names outright (a glob-free pattern with the file's own name), that
    # gitingest skipped, and the excludes allow. process_patterns drops from its ignores the entries equal to an include.
    added = []
    named = [p for p in include if not re.search(r"[*?\[]", p)]
    if named:
        built_in = spec(gitingest_ignores() - set(include))
        excluded = spec([p for p in excludes if p not in include])  # gitingest drops an exclude named in -i
        listing = clierr.run_tool(["git", "-C", str(clone), "ls-files", "-z"]).stdout
        for rel in listing.split("\0"):
            if not rel or not built_in.match_file(rel) or excluded.match_file(rel):
                continue
            name = rel.rsplit("/", 1)[-1]
            if not any(spec([p]).match_file(rel) and name == p.rstrip("/").rsplit("/", 1)[-1] for p in named):
                continue
            f = clone / rel
            if f.is_symlink() or not f.is_file() or not f.resolve().is_relative_to(clone) or f.stat().st_size > MAX_FILE_BYTES:
                continue
            try:
                text = f.read_bytes().decode("utf-8")
            except UnicodeDecodeError:
                continue
            if "\x00" not in text:
                content += f"{FILE_RULE}\nFILE: {rel}\n{FILE_RULE}\n{text}\n\n"
                added.append(rel)
    final = tree + "\n" + content
    label = f"-{a.out}" if a.out else ""
    out = ROOT / f"{clone.name}{label}.txt"
    out.write_text(final, encoding="utf-8")

    counted = _format_token_count(final)
    tokens, note = (token_count("Estimated tokens: " + counted) if counted else None), ""
    if tokens is None:
        tokens = len(final.encode()) // 4
        note = " (estimated from size)"
        clierr.warn(TOOL, "NO_ESTIMATE", "token recount failed", "the count is file size / 4")
    if added:
        note += f" (+{len(added)} named files gitingest skips: {', '.join(added)})"
    print(f"digest: {shown(out)}  files={count + len(added)}  tokens={fmt(tokens)}{note}")
    if count + len(added) == 0:
        clierr.warn(TOOL, "EMPTY", "the patterns matched no files", "check -i against rr.py map; --all if they sit under a default exclude")
    elif tokens > READ_BUDGET:
        clierr.warn(TOOL, "TOO_BIG", f"digest is {fmt(tokens)} tokens, over the {fmt(READ_BUDGET)} one-Read budget",
                    "drop the largest item, or split into one digest per folder with --out")


def main():
    p = argparse.ArgumentParser(prog="rr.py", description="map and digest remote git repos for agents")
    sub = p.add_subparsers(dest="command", required=True)
    m = sub.add_parser("map", help="clone and print estimated digest tokens per folder")
    m.add_argument("source", help="repo URL")
    m.add_argument("--branch")
    m.add_argument("--name", help="clone folder name (default <owner>-<repo>)")
    m.add_argument("--depth", type=int, default=2, choices=range(1, 11), metavar="1-10")
    m.add_argument("--all", action="store_true", help="count tests, translations and snapshots too")
    d = sub.add_parser("digest", help="gitingest digest of a clone")
    d.add_argument("source", help="clone name (from map) or folder")
    d.add_argument("-i", "--include", action="append", default=[])
    d.add_argument("-e", "--exclude", action="append", default=[])
    d.add_argument("--out", help="label appended to the digest file name")
    d.add_argument("--all", action="store_true", help="drop rr's default excludes")
    c = sub.add_parser("checkout", help="add folders or patterns to a partial clone, for grepping")
    c.add_argument("source", help="clone name (from map) or folder")
    c.add_argument("patterns", nargs="+")
    g = sub.add_parser("grep", help="grep a clone inside the workshop; paths print readable from the caller")
    g.add_argument("source", help="clone name (from map) or folder")
    g.add_argument("regex", help="extended regex")
    g.add_argument("paths", nargs="*", help="folders or files inside the clone (default: all)")
    g.add_argument("-i", "--ignore-case", action="store_true")
    g.add_argument("--max", type=int, default=200, help="matches to print (default 200)")
    a = p.parse_args()
    {"map": cmd_map, "digest": cmd_digest, "checkout": cmd_checkout, "grep": cmd_grep}[a.command](a)


if __name__ == "__main__":
    clierr.run(TOOL, main)
