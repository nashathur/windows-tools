"""Code tools for local-llm (Rust widgets and other projects), registered by server.py:

    local_code_search   search a project's source by meaning, one hit per fn/impl/struct
    local_docs          search the Rust dependencies' APIs (windows-sys features in use, etc.)
    local_patch         small, mechanical edits by the coding model, compile-checked in a scratch worktree
    local_widget_check  screenshot a running widget's windows, compare with a baseline, ask about it

Settings (environment variables, all optional):
    LOCAL_LLM_CODE_MODEL  model for local_patch     (default: qwen3.5:9b; 16k ctx, 24k for big jobs)
    LOCAL_LLM_CODE_CTX    context for a "coder" model (default: 32768; a 7B fits that in 8 GB)
    LOCAL_LLM_SCRATCH     worktree, cargo target dir, sccache cache, last.patch (default: D:\\lp)
"""
import ctypes
import difflib
import fnmatch
import glob
import hashlib
import json
import os
import re
import shutil
import subprocess
import threading
import time
from ctypes import wintypes
from pathlib import Path

import numpy as np
import pymupdf
import tree_sitter
import tree_sitter_rust

# qwen3.5:9b beat qwen2.5-coder:7b in tests (2026-10-10): it follows the SEARCH/REPLACE format;
# the coder rewrote whole ranges or replied with diffs. The coder stays for >24k-token jobs.
CODE_MODEL = os.environ.get("LOCAL_LLM_CODE_MODEL", "qwen3.5:9b")
CODE_CTX = int(os.environ.get("LOCAL_LLM_CODE_CTX", "32768"))  # for a "coder" model only
SCRATCH = Path(os.environ.get("LOCAL_LLM_SCRATCH", r"D:\lp"))
SHOTS = Path(__file__).resolve().parent / "shots"
CODE_INDEX_VERSION = 1  # bump when the chunkers change, so old code/docs indexes are rebuilt
SKIP_DIRS = {"target", "node_modules", ".git", "__pycache__"}
PARSER = tree_sitter.Parser(tree_sitter.Language(tree_sitter_rust.language()))
PATCH_LOCK = threading.Lock()


def sh(cmd: list[str], cwd=None, timeout=None, env=None) -> subprocess.CompletedProcess:
    """Run a program; stdin closed so it can't read the MCP stdio stream."""
    return subprocess.run(cmd, cwd=cwd, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                          encoding="utf-8", errors="replace", timeout=timeout, env=env,
                          creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))


def git(args: list[str], cwd) -> str:
    r = sh(["git", *args], cwd=cwd)
    if r.returncode:
        raise RuntimeError(f"git {' '.join(args)}: {r.stderr.strip()[:400]}")
    return r.stdout


def repo_root(path) -> Path | None:
    r = sh(["git", "rev-parse", "--show-toplevel"], cwd=path)
    return Path(r.stdout.strip()) if r.returncode == 0 else None


def list_files(folder: Path, pattern: str) -> list[str]:
    """Files under folder matching comma-separated globs: git-tracked + untracked-not-ignored when
    inside a repo (so target/, backups and logs are skipped), else a glob skipping build dirs."""
    pats = [p.strip().replace("\\", "/") for p in pattern.split(",") if p.strip()]

    def match(rel: str) -> bool:
        return any(fnmatch.fnmatch(rel, p) or (p.startswith("**/") and fnmatch.fnmatch(rel, p[3:]))
                   for p in pats)

    if repo_root(folder):
        out = sh(["git", "ls-files", "-co", "--exclude-standard", "-z", "."], cwd=folder).stdout
        rels = [r for r in out.split("\0") if r]
    else:
        rels = [str(p.relative_to(folder)).replace("\\", "/") for p in folder.rglob("*")
                if p.is_file() and not (set(p.relative_to(folder).parts) & SKIP_DIRS)
                and not any(x.startswith("src_backup") for x in p.relative_to(folder).parts)]
    return sorted(str(folder / r) for r in rels if match(r) and (folder / r).is_file())


# ---------- Rust chunking ----------

COMMENTS = {"line_comment", "block_comment", "attribute_item", "inner_attribute_item"}


def with_leading(node) -> int:
    """First row of a node including the doc comments/attributes right above it."""
    row, prev = node.start_point[0], node.prev_sibling
    while prev is not None and prev.type in COMMENTS and prev.end_point[0] >= row - 1:
        row, prev = prev.start_point[0], prev.prev_sibling
    return row


def name_of(node, src: bytes) -> str:
    def text(n):
        return src[n.start_byte:n.end_byte].decode("utf-8", "replace") if n else ""
    if node.type == "impl_item":
        t, tr = node.child_by_field_name("type"), node.child_by_field_name("trait")
        return f"impl {text(tr) + ' for ' if tr else ''}{text(t)}"
    kind = node.type.replace("_signature_item", "").replace("_item", "").replace("_definition", "")
    kind = {"function": "fn", "macro": "macro_rules!"}.get(kind, kind)
    return f"{kind} {text(node.child_by_field_name('name'))}".strip()


def rust_chunks(path: str) -> list[dict]:
    """One chunk per fn / struct / enum / trait / small impl or mod, with its doc comments; big
    impls and mods are split into their fns (named "impl X::fn"), huge fns into ~3000-char parts.
    Small leftovers (consts, statics, type aliases) are grouped. use-lines are skipped."""
    src = Path(path).read_bytes()
    lines = src.decode("utf-8", "replace").splitlines()
    out, group = [], []

    def add(a: int, b: int, sym: str):
        text = "\n".join(lines[a:b + 1])
        if len(text) <= 4000:
            out.append({"file": path, "a": a + 1, "b": b + 1, "sym": sym, "text": text})
            return
        cur, start, n, part = [], a, 0, 1
        for i in range(a, b + 1):
            cur.append(lines[i])
            n += len(lines[i]) + 1
            if n > 3000 or i == b:
                out.append({"file": path, "a": start + 1, "b": i + 1,
                            "sym": f"{sym} (part {part})", "text": "\n".join(cur)})
                cur, start, n, part = [], i + 1, 0, part + 1

    def flush_group():
        if group:
            add(group[0][0], group[-1][1], "items: " + ", ".join(s for _, _, s in group)[:120])
            group.clear()

    def walk(container, prefix: str):
        for node in container.named_children:
            body = node.child_by_field_name("body")
            if node.type in COMMENTS or node.type in ("use_declaration", "extern_crate_declaration") \
                    or (node.type == "mod_item" and body is None):  # `mod foo;`
                continue
            a, b = with_leading(node), node.end_point[0]
            sym = prefix + name_of(node, src)
            big = b - a > 80 or len("\n".join(lines[a:b + 1])) > 4000
            if node.type in ("impl_item", "trait_item", "mod_item") and body is not None and big:
                flush_group()
                walk(body, sym + "::")
            elif node.type in ("function_item", "impl_item", "trait_item", "mod_item", "struct_item",
                               "enum_item", "union_item", "macro_definition",
                               "function_signature_item"):
                flush_group()
                add(a, b, sym)
            else:  # const, static, type alias, stray statements: group them
                group.append((a, b, sym))
                if sum(len(lines[i]) for g in group for i in range(g[0], g[1] + 1)) > 1500:
                    flush_group()
        flush_group()

    walk(PARSER.parse(src).root_node, "")
    return out


def text_chunks(path: str) -> list[dict]:
    """Non-Rust files: ~1500-char line windows."""
    lines = Path(path).read_text(encoding="utf-8", errors="replace").splitlines()
    out, start, n = [], 0, 0
    for i, ln in enumerate(lines):
        n += len(ln) + 1
        if n > 1500 or i == len(lines) - 1:
            text = "\n".join(lines[start:i + 1]).strip()
            if text:
                out.append({"file": path, "a": start + 1, "b": i + 1, "sym": "", "text": text})
            start, n = i + 1, 0
    return out


# ---------- shared index cache ----------

def safe_embed(embed, texts: list[str], index_dir: Path) -> np.ndarray:
    """embed() in batches; a batch Ollama rejects is split until the bad text is found, which
    gets a zero vector (never matches) and is logged to index\\embed_errors.log."""
    def go(part: list[str]) -> np.ndarray:
        try:
            return embed(part)
        except Exception as e:
            if len(part) == 1:
                with open(index_dir / "embed_errors.log", "a", encoding="utf-8") as f:
                    f.write(f"{e}: {part[0][:300]!r}\n")
                return None
            a, b = go(part[:len(part) // 2]), go(part[len(part) // 2:])
            dim = next((x.shape[1] for x in (a, b) if x is not None), 0)
            if not dim:
                return None
            return np.vstack([x if x is not None else np.zeros((n, dim), np.float32)
                              for x, n in ((a, len(part) // 2), (b, len(part) - len(part) // 2))])
    out = [go(texts[i:i + 256]) for i in range(0, len(texts), 256)]
    dim = next((x.shape[1] for x in out if x is not None), 1024)
    return np.vstack([x if x is not None else np.zeros((min(256, len(texts) - i * 256), dim), np.float32)
                      for i, x in enumerate(out)])

def cached_index(index_dir: Path, embed, key: str, files: list[str], chunker, text_of,
                 reindex: bool = False):
    """Embedding index over files, cached as <key>.json/.npy; only changed files are re-chunked.
    Returns (chunks, vectors, n_reindexed, failed_files)."""
    index_dir.mkdir(exist_ok=True)
    meta_p, vec_p = index_dir / f"{key}.json", index_dir / f"{key}.npy"
    meta, vecs = {"files": {}, "chunks": []}, None
    if not reindex and meta_p.exists() and vec_p.exists():
        meta, vecs = json.loads(meta_p.read_text(encoding="utf-8")), np.load(vec_p)
    mtimes = {f: os.path.getmtime(f) for f in files}
    stale = {f for f, m in meta["files"].items() if mtimes.get(f) != m}
    new = [f for f in files if meta["files"].get(f) != mtimes[f]]
    failed = []
    if stale or new or vecs is None:
        keep = [i for i, c in enumerate(meta["chunks"]) if c["file"] not in stale]
        kept = [meta["chunks"][i] for i in keep]
        vecs = vecs[keep] if vecs is not None and keep else None
        add = []
        for f in new:
            try:
                add += chunker(f)
            except Exception:  # unreadable/unparsable file: skip it
                failed.append(f)
        if add:
            v = safe_embed(embed, [text_of(c)[:6000] for c in add], index_dir)
            vecs = v if vecs is None else np.vstack([vecs, v])
        meta = {"files": {f: mtimes[f] for f in files if f not in failed}, "chunks": kept + add}
        if vecs is not None:
            meta_p.write_text(json.dumps(meta, ensure_ascii=False), encoding="utf-8")
            np.save(vec_p, vecs)
    return meta["chunks"], vecs, len(new), failed


def query_vec(embed, query: str, task: str):
    return embed([f"Instruct: {task}\nQuery: {query}"])[0]


def name_boost(query: str, syms: list[str]) -> np.ndarray:
    """Small bonus when identifier-like words of the query appear in the item's name."""
    words = [w.lower() for w in re.findall(r"[A-Za-z_][A-Za-z0-9_]{3,}", query)]
    return np.array([0.04 * sum(w in s.lower() for w in words) for s in syms], dtype=np.float32)


# ---------- dependency docs ----------

LINK_RE = re.compile(r'link!\("([\w.\-]+)"\s+"\w+"\s+(?:#\[doc\s*=\s*"((?:[^"\\]|\\.)*)"\]\s*)?'
                     r'fn\s+(\w+)\s*\((.*?)\)\s*(?:->\s*(.*?))?\);')
ERROR_CONST_RE = re.compile(r"pub const \w+: [\w:]*?\b(HRESULT|WIN32_ERROR|NTSTATUS)\b")
NAME_RE = re.compile(r"(?:\bfn|\bconst|\bstruct|\btype|\bunion|\benum|\btrait|define_interface!\()\s*([A-Za-z_]\w*)")
FEATURE_RE = re.compile(r'feature\s*=\s*\\?"(\w+)\\?"')
DOC_ITEMS = {"function_item", "function_signature_item", "struct_item", "enum_item", "union_item",
             "const_item", "static_item", "type_item", "trait_item", "macro_definition"}


def cargo_dir(project: str) -> Path | None:
    p = Path(project)
    for c in (p, p / "rust"):
        if (c / "Cargo.toml").is_file():
            return c
    return None


def dep_files(project: str) -> list[tuple[str, str]]:
    """(file, label) for the APIs a crate can use: for windows / windows-sys only the feature
    modules enabled in the build; other direct dependencies (of the crate and its path
    dependencies, e.g. taskbar-dock) whole, skipping generated files > 400 KB."""
    cd = cargo_dir(project)
    if cd is None:
        raise ValueError(f"No Cargo.toml in {project}")
    r = sh(["cargo", "metadata", "--format-version", "1"], cwd=cd, timeout=300)
    if r.returncode:
        raise ValueError(f"cargo metadata failed: {r.stderr.strip()[:400]}")
    m = json.loads(r.stdout)
    pkgs = {p["id"]: p for p in m["packages"]}
    nodes = {n["id"]: n for n in m["resolve"]["nodes"]}
    local = [i for i, p in pkgs.items() if p["source"] is None]  # this crate + path deps
    direct = {d["pkg"] for i in local for d in nodes[i]["deps"]} - set(local)
    out = []
    has_sys = any(pkgs[i]["name"] == "windows-sys" for i in direct)
    for pid in sorted(direct, key=lambda i: pkgs[i]["name"]):
        p = pkgs[pid]
        src = Path(p["manifest_path"]).parent / "src"
        tag = f"{p['name']} {p['version']}"
        if p["name"] == "windows" and has_sys:
            tag += " COM"  # api_items keeps only interface methods; the rest duplicates windows-sys
        if p["name"] in ("windows", "windows-sys"):
            for f in sorted(nodes[pid]["features"]):
                mod = src / "Windows" / Path(*f.split("_")) / "mod.rs"
                if mod.is_file():
                    out.append((str(mod), f"{tag} [{f}]"))
        else:
            for f in sorted(src.rglob("*.rs")):
                if f.stat().st_size <= 400_000:
                    out.append((str(f), f"{tag} {f.relative_to(src).as_posix()}"))
    return out


def api_items(path: str, label: str) -> list[dict]:
    """Public API items of one dependency source file: signature (no body), doc comment,
    features its #[cfg] needs, line. windows-sys link!() lines become fn items."""
    src = Path(path).read_bytes()
    out = []
    com_only = " COM [" in label

    def text(n):
        return src[n.start_byte:n.end_byte].decode("utf-8", "replace")

    def visit(node, owner: str, in_trait: bool):
        for ch in node.named_children:
            t = ch.type
            if t in ("impl_item", "trait_item", "mod_item"):
                body = ch.child_by_field_name("body")
                if body is not None:
                    own = name_of(ch, src).split(" ", 1)[-1] if t != "mod_item" else owner
                    visit(body, own, t == "trait_item")
                if t != "trait_item":
                    continue
            if t not in DOC_ITEMS:
                continue
            pub = in_trait or any(c.type == "visibility_modifier" for c in ch.children)
            if not pub or (com_only and not owner):
                continue
            body = ch.child_by_field_name("body")
            end = body.start_byte if body is not None and t != "const_item" else ch.end_byte
            sig = " ".join(src[ch.start_byte:end].decode("utf-8", "replace").split())[:300]
            if t == "const_item" and ERROR_CONST_RE.match(sig):
                continue
            sig = sig.replace("super::super::", "").replace("super::", "")
            doc, needs, prev = [], [], ch.prev_sibling
            while prev is not None and prev.type in COMMENTS:
                tx = text(prev)
                if tx.startswith("///"):
                    doc.insert(0, tx[3:].strip())
                needs += FEATURE_RE.findall(tx)
                prev = prev.prev_sibling
            out.append({"file": path, "label": label, "line": ch.start_point[0] + 1,
                        "sig": (f"{owner}::" if owner and t.startswith("function") else "") + sig,
                        "doc": " ".join(doc)[:400], "needs": sorted(set(needs))})

    visit(PARSER.parse(src).root_node, "", False)
    if label.startswith("windows-sys"):  # functions are link!() macro lines, not fn items
        full = src.decode("utf-8", "replace")
        for mm in LINK_RE.finditer(full):
            dll, doc, name, args, ret = mm.groups()
            sig = f"fn {name}({' '.join(args.split())})" + (f" -> {' '.join(ret.split())}" if ret else "")
            out.append({"file": path, "label": label, "line": full.count("\n", 0, mm.start()) + 1,
                        "sig": sig.replace("super::super::", "").replace("super::", "")[:300],
                        "doc": dll, "needs": re.findall(r'\\"(\w+)\\"', doc or "")})
    return out


def name_table(index_dir: Path, base: Path, tag: str) -> dict:
    """{identifier: [[feature, file, line], ...]} for a whole windows / windows-sys crate, built
    once (~10 s) and cached in index\\names_<tag>.json (registry sources never change)."""
    p = index_dir / f"names_{re.sub(r'[^\w.-]', '_', tag)}.json"
    if p.exists():
        return json.loads(p.read_text(encoding="utf-8"))
    table: dict = {}
    for f in base.rglob("mod.rs"):
        t = f.read_text(encoding="utf-8", errors="replace")
        feat = "_".join(f.parent.relative_to(base).parts)
        for mm in NAME_RE.finditer(t):
            table.setdefault(mm.group(1), []).append([feat, str(f), t.count("\n", 0, mm.start()) + 1])
    index_dir.mkdir(exist_ok=True)
    p.write_text(json.dumps(table), encoding="utf-8")
    return table


def exact_lookup(project: str, name: str, index_dir: Path) -> list[str]:
    """Find an exact identifier anywhere in the windows / windows-sys versions this crate uses
    directly (all features, not only enabled ones): "which feature do I need for X"."""
    r = sh(["cargo", "metadata", "--format-version", "1"], cwd=cargo_dir(project), timeout=300)
    m = json.loads(r.stdout)
    pkgs = {p["id"]: p for p in m["packages"]}
    nodes = {n["id"]: n for n in m["resolve"]["nodes"]}
    local = [i for i, p in pkgs.items() if p["source"] is None]
    direct = {d["pkg"] for i in local for d in nodes[i]["deps"]}
    hits = []
    for pid in sorted(direct):
        p = pkgs[pid]
        if p["name"] not in ("windows", "windows-sys"):
            continue
        base = Path(p["manifest_path"]).parent / "src" / "Windows"
        enabled = set(nodes[pid]["features"])
        found = {}
        for feat, f, ln in name_table(index_dir, base, f"{p['name']}-{p['version']}").get(name, []):
            found.setdefault(feat, (f, ln))  # first per feature (windows repeats fns in link!)
        for feat, (f, ln) in list(found.items())[:3]:
            line = Path(f).read_text(encoding="utf-8", errors="replace").splitlines()[ln - 1]
            line = re.sub(r"#\[doc = .*?\] ", "", " ".join(line.split()))
            on = "enabled" if feat in enabled else "NOT enabled: add it to Cargo.toml"
            hits.append(f"{p['name']} {p['version']}, feature {feat} ({on}):\n   {line[:280]}  "
                        f"[{p['name']}-{p['version']}\\src\\Windows\\{Path(f).relative_to(base)}:{ln}]")
    return hits


# ---------- local_patch helpers ----------

BLOCK_RE = re.compile(r"<{5,9} ?SEARCH\s*\n(.*?)\n?={5,9}\s*\n(.*?)\n?>{5,9} ?REPLACE", re.S)

PATCH_SYSTEM = """You make small, exact code edits. Reply ONLY with edit blocks, no explanations:

FILE: <path exactly as given>
<<<<<<< SEARCH
<lines copied exactly from the current file, with their indentation>
=======
<the new lines>
>>>>>>> REPLACE

Rules: the SEARCH text must appear exactly once in the file; keep it short (a few lines) but
unique. Several blocks per file are fine; list them in file order. To create a new file, use an
empty SEARCH. Change only what the task asks; keep the surrounding code style."""


LOOSE_RE = re.compile(r"SEARCH:?\s*\n(.*?)\n\s*REPLACE:?\s*\n(.*?)(?=\n\s*SEARCH:?\s*\n|\n```|\Z)", re.S)
FENCE_RE = re.compile(r"```[\w+-]*\n(.*?)\n```", re.S)


def parse_blocks(reply: str) -> list[tuple[str, str, str]]:
    """(path, search, replace) for each edit block; the path is the last non-empty line before it.
    Also accepts the loose "SEARCH:\\n...\\nREPLACE:\\n..." form small models sometimes use."""
    out, pos = [], 0
    if not BLOCK_RE.search(reply):
        return [("", m.group(1).strip("\n"), m.group(2).strip("\n")) for m in LOOSE_RE.finditer(reply)]
    for m in BLOCK_RE.finditer(reply):
        before = [ln.strip() for ln in reply[pos:m.start()].splitlines() if ln.strip()
                  and not ln.strip().startswith("```")]
        path = before[-1] if before else (out[-1][0] if out else "")
        path = re.sub(r"^(FILE|File|file)\s*:\s*", "", path)
        path = re.sub(r"\s+\((lines|new file).*$", "", path).strip("`*# ")  # header copied whole
        out.append((path, m.group(1), m.group(2)))
        pos = m.end()
    return out


def apply_block(text: str, search: str, replace: str) -> str:
    """Replace search with replace in text (LF line ends). Exact match first, then ignoring
    trailing spaces, then ignoring indentation (replacement re-indented by the difference)."""
    if not search.strip():
        if text.strip():
            raise ValueError("empty SEARCH on a non-empty file")
        return replace + "\n"
    n = text.count(search)
    if n == 1:
        return text.replace(search, replace)
    if n > 1:
        raise ValueError("SEARCH text occurs more than once; add a line of context")
    tl, sl = text.split("\n"), search.split("\n")
    while sl and not sl[0].strip():
        sl = sl[1:]
    while sl and not sl[-1].strip():
        sl = sl[:-1]
    for key in (str.rstrip, str.strip):
        want = [key(s) for s in sl]
        hits = [i for i in range(len(tl) - len(sl) + 1)
                if [key(x) for x in tl[i:i + len(sl)]] == want]
        if len(hits) > 1:
            raise ValueError("SEARCH text occurs more than once; add a line of context")
        if hits:
            i = hits[0]
            rl = replace.split("\n")
            if key is str.strip:  # shift the replacement by the indentation difference
                have = len(tl[i]) - len(tl[i].lstrip())
                gave = len(sl[0]) - len(sl[0].lstrip())
                d = have - gave
                rl = [(" " * d + r) if d > 0 else r[min(-d, len(r) - len(r.lstrip())):] for r in rl]
            return "\n".join(tl[:i] + rl + tl[i + len(sl):])
    raise ValueError("SEARCH text not found in the file")


def numbered(text: str, a: int, b: int) -> str:
    return "\n".join(text.split("\n")[a - 1:b])


def crate_of(path: Path, stop: Path) -> Path | None:
    for d in [path.parent, *path.parent.parents]:
        if (d / "Cargo.toml").is_file():
            return d
        if d == stop:
            break
    return None


def dependents(crate: Path, root: Path) -> list[Path]:
    """Crates in the repo with a path dependency on crate (e.g. all widgets for taskbar-dock)."""
    out = []
    for toml in root.rglob("Cargo.toml"):
        if set(toml.relative_to(root).parts) & SKIP_DIRS or "src_backup" in str(toml):
            continue
        for m in re.finditer(r'path\s*=\s*"([^"]+)"', toml.read_text(encoding="utf-8", errors="replace")):
            if (toml.parent / m.group(1)).resolve() == crate.resolve() and toml.parent != crate:
                out.append(toml.parent)
    return out


def cargo_env() -> dict:
    env = dict(os.environ, CARGO_TARGET_DIR=str(SCRATCH / "target"), CARGO_TERM_COLOR="never",
               SCCACHE_DIR=str(SCRATCH / "sccache"))
    sc = shutil.which("sccache") or next(iter(glob.glob(os.path.expandvars(
        r"%LOCALAPPDATA%\Microsoft\WinGet\Packages\Mozilla.sccache*\*\sccache.exe"))), None)
    if sc:
        env["RUSTC_WRAPPER"] = sc
    return env


def cargo_errors(crate: Path, mode: str, wt: Path) -> list[str]:
    """Run cargo check/clippy (all targets) or test in crate; return error messages (rendered,
    worktree paths made relative). Empty list = OK."""
    sub = {"check": ["check", "--all-targets"], "clippy": ["clippy", "--all-targets"],
           "test": ["test", "--no-run"]}[mode]
    r = sh(["cargo", *sub, "--message-format=json"], cwd=crate, timeout=1800, env=cargo_env())
    errs = []
    rel = crate.relative_to(wt).as_posix()

    def repo_paths(text: str) -> str:  # "--> src\x.rs:3:5" (crate-relative) -> "--> taskbar-weather/src/x.rs:3:5"
        text = text.replace(str(wt) + "\\", "")
        return re.sub(r"(-->\s*)([^\s:][^:\n]*)", lambda m: m.group(1) + (
            m.group(2).replace("\\", "/") if m.group(2).replace("\\", "/").startswith(
                tuple(p.name + "/" for p in wt.iterdir())) else f"{rel}/{m.group(2).replace(chr(92), '/')}"), text)

    for ln in r.stdout.splitlines():
        try:
            d = json.loads(ln)
        except json.JSONDecodeError:
            continue
        msg = d.get("message") or {}
        if d.get("reason") == "compiler-message" and msg.get("level") in ("error", "error: internal compiler error"):
            e = repo_paths(msg.get("rendered") or msg.get("message", "")).strip()
            if e not in errs and not e.startswith(("error: aborting", "error: could not compile")):
                errs.append(e)
    if r.returncode and not errs:  # e.g. a manifest error: no JSON messages
        errs.append(r.stderr.strip()[-1500:].replace(str(wt) + "\\", ""))
    if mode == "test" and not errs:
        t = sh(["cargo", "test", "--quiet"], cwd=crate, timeout=1800, env=cargo_env())
        if t.returncode:
            errs.append("tests failed:\n" + (t.stdout + t.stderr).strip()[-2500:])
    return errs


def error_spots(errs: list[str]) -> list[tuple[str, int]]:
    return [(m.group(1).replace("\\", "/"), int(m.group(2)))
            for e in errs for m in [re.search(r"-->\s*([^:\s][^:]*):(\d+):\d+", e)] if m]


def make_worktree(root: Path) -> Path:
    """Fresh scratch worktree at SCRATCH/wt holding the CURRENT working tree (uncommitted edits
    and untracked files included). Fixed path, so the shared target dir stays incremental."""
    wt = SCRATCH / "wt"
    SCRATCH.mkdir(parents=True, exist_ok=True)
    sh(["git", "worktree", "remove", "--force", str(wt)], cwd=root)
    if wt.exists():
        shutil.rmtree(wt, ignore_errors=True)
    sh(["git", "worktree", "prune"], cwd=root)
    snap = git(["stash", "create"], root).strip() or "HEAD"  # commit of tracked edits; no ref touched
    git(["worktree", "add", "--detach", str(wt), snap], root)
    for rel in git(["ls-files", "--others", "--exclude-standard", "-z"], root).split("\0"):
        if rel:
            dst = wt / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(root / rel, dst)
    if git(["status", "--porcelain"], wt).strip():  # commit the untracked copies: diff = model's edits only
        git(["add", "-A"], wt)
        git(["-c", "user.name=lp", "-c", "user.email=lp@local", "commit", "-q", "-m", "untracked"], wt)
    return wt


def wt_diff(wt: Path) -> str:
    git(["add", "-N", "."], wt)  # show new files too
    return git(["diff", "--", ".", ":(exclude)*Cargo.lock"], wt)


# ---------- widget screenshots ----------

user32 = ctypes.WinDLL("user32", use_last_error=True)
gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
WNDENUMPROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
user32.SetThreadDpiAwarenessContext.restype = ctypes.c_void_p
user32.SetThreadDpiAwarenessContext.argtypes = [ctypes.c_void_p]
user32.GetDC.restype = wintypes.HDC
for _f in ("FindWindowW", "FindWindowExW", "GetParent"):
    getattr(user32, _f).restype = wintypes.HWND
gdi32.CreateCompatibleDC.restype = wintypes.HDC
gdi32.CreateCompatibleDC.argtypes = [wintypes.HDC]
gdi32.CreateCompatibleBitmap.restype = wintypes.HBITMAP
gdi32.CreateCompatibleBitmap.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int]
gdi32.SelectObject.restype = wintypes.HGDIOBJ
gdi32.SelectObject.argtypes = [wintypes.HDC, wintypes.HGDIOBJ]
gdi32.BitBlt.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                         wintypes.HDC, ctypes.c_int, ctypes.c_int, wintypes.DWORD]
gdi32.GetDIBits.argtypes = [wintypes.HDC, wintypes.HBITMAP, wintypes.UINT, wintypes.UINT,
                            ctypes.c_void_p, ctypes.c_void_p, wintypes.UINT]
gdi32.DeleteObject.argtypes = [wintypes.HGDIOBJ]
gdi32.DeleteDC.argtypes = [wintypes.HDC]
user32.ReleaseDC.argtypes = [wintypes.HWND, wintypes.HDC]
kernel32.OpenProcess.restype = wintypes.HANDLE


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [("biSize", wintypes.DWORD), ("biWidth", ctypes.c_long), ("biHeight", ctypes.c_long),
                ("biPlanes", wintypes.WORD), ("biBitCount", wintypes.WORD),
                ("biCompression", wintypes.DWORD), ("biSizeImage", wintypes.DWORD),
                ("biXPelsPerMeter", ctypes.c_long), ("biYPelsPerMeter", ctypes.c_long),
                ("biClrUsed", wintypes.DWORD), ("biClrImportant", wintypes.DWORD)]


def exe_of(hwnd) -> str:
    pid = wintypes.DWORD()
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    h = kernel32.OpenProcess(0x1000, False, pid.value)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not h:
        return ""
    buf, n = ctypes.create_unicode_buffer(520), wintypes.DWORD(520)
    ok = kernel32.QueryFullProcessImageNameW(h, 0, buf, ctypes.byref(n))
    kernel32.CloseHandle(h)
    return buf.value if ok else ""


def widget_windows(name: str) -> list[dict]:
    """Visible windows of processes whose exe name contains name: top-level ones (panels) and
    children of the taskbars (docked strips). Strips first, then largest first."""
    found = []

    def cb(hwnd, _):
        if not user32.IsWindowVisible(hwnd):
            return True
        exe = exe_of(hwnd)
        if not exe or name.lower() not in Path(exe).stem.lower():
            return True
        r = wintypes.RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(r))
        w, h = r.right - r.left, r.bottom - r.top
        if w > 1 and h > 1:
            cls = ctypes.create_unicode_buffer(256)
            user32.GetClassNameW(hwnd, cls, 256)
            found.append({"hwnd": hwnd, "exe": Path(exe).name, "class": cls.value,
                          "rect": (r.left, r.top, w, h), "docked": bool(user32.GetParent(hwnd))})
        return True

    proc = WNDENUMPROC(cb)
    user32.EnumWindows(proc, 0)
    for tray_cls in ("Shell_TrayWnd", "Shell_SecondaryTrayWnd"):
        tray = user32.FindWindowW(tray_cls, None)
        while tray:
            user32.EnumChildWindows(tray, proc, 0)
            tray = user32.FindWindowExW(None, tray, tray_cls, None) if tray_cls != "Shell_TrayWnd" else 0
    uniq = {d["hwnd"]: d for d in found}.values()
    return sorted(uniq, key=lambda d: (not d["docked"], -d["rect"][2] * d["rect"][3]))


def grab(rect) -> np.ndarray:
    """Screen pixels of rect (x, y, w, h) as an RGB array (what is on screen, layered windows included)."""
    x, y, w, h = rect
    sdc = user32.GetDC(None)
    mdc = gdi32.CreateCompatibleDC(sdc)
    bmp = gdi32.CreateCompatibleBitmap(sdc, w, h)
    old = gdi32.SelectObject(mdc, bmp)
    gdi32.BitBlt(mdc, 0, 0, w, h, sdc, x, y, 0x00CC0020 | 0x40000000)  # SRCCOPY | CAPTUREBLT
    bi = BITMAPINFOHEADER(ctypes.sizeof(BITMAPINFOHEADER), w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
    buf = ctypes.create_string_buffer(w * h * 4)
    gdi32.GetDIBits(mdc, bmp, 0, h, buf, ctypes.byref(bi), 0)
    gdi32.SelectObject(mdc, old)
    gdi32.DeleteObject(bmp)
    gdi32.DeleteDC(mdc)
    user32.ReleaseDC(None, sdc)
    return np.frombuffer(buf.raw, np.uint8).reshape(h, w, 4)[:, :, 2::-1].copy()


def save_png(arr: np.ndarray, path: Path):
    h, w, _ = arr.shape
    pymupdf.Pixmap(pymupdf.csRGB, w, h, np.ascontiguousarray(arr).tobytes(), 0).save(str(path))


def load_png(path: Path) -> np.ndarray:
    pix = pymupdf.Pixmap(str(path))
    if pix.alpha:
        pix = pymupdf.Pixmap(pix, 0)
    return np.frombuffer(pix.samples, np.uint8).reshape(pix.height, pix.width, pix.n)[:, :, :3]


def mouse(x: int, y: int, action: str):
    user32.SetCursorPos(x, y)
    if action in ("click", "rightclick"):
        down, up = (0x2, 0x4) if action == "click" else (0x8, 0x10)
        time.sleep(0.15)
        user32.mouse_event(down, 0, 0, 0, 0)
        time.sleep(0.05)
        user32.mouse_event(up, 0, 0, 0, 0)


def setup(mcp, chat, embed, index_dir: Path):
    """Register the code tools on the server (chat/embed are server.py's model helpers)."""

    @mcp.tool()
    def local_code_search(query: str, folder: str, k: int = 6, full: int = 0,
                          pattern: str = "**/*.rs", reindex: bool = False) -> str:
        """Search a code project by meaning, e.g. "where does the hover panel start fading
        out?" or "which code decides the strip's dock slot?". Rust files are split into one
        item per fn / struct / enum / impl (big impls into their methods, named "impl X::fn"),
        so each hit is a whole function with its file and line range. Returns the k best hits
        with a short snippet; full=N also returns the complete code of the top N hits (instead
        of opening the files). Inside a git repo only tracked + untracked-not-ignored files are
        searched (no target/, backups). pattern: comma-separated globs, e.g. "**/*.rs,**/*.ps1"
        (non-Rust files are split into ~40-line windows). First call indexes the folder (~1 s
        per file), later calls only changed files. Use Grep for exact names instead."""
        root = Path(folder)
        if not root.is_dir():
            return f"Not a folder: {folder}"
        files = list_files(root, pattern)
        if not files:
            return f"No files match {pattern} under {folder}"
        key = "code_" + hashlib.sha1(f"{CODE_INDEX_VERSION}|{root.resolve()}|{pattern}"
                                     .lower().encode()).hexdigest()[:12]
        chunks, vecs, n_new, failed = cached_index(
            index_dir, embed, key, files,
            lambda f: rust_chunks(f) if f.endswith(".rs") else text_chunks(f),
            lambda c: f"{Path(c['file']).name} {c['sym']}\n{c['text']}", reindex)
        if vecs is None:
            return "Nothing to index."
        q = query_vec(embed, query, "Given a question about a code base, retrieve the code "
                                    "that answers it")
        scores = vecs @ q + name_boost(query, [c["sym"] for c in chunks])
        lines = [f"{len(files)} files, {len(chunks)} items ({n_new} files (re)indexed now"
                 + (f", {len(failed)} unreadable" if failed else "") + ")."]
        for rank, i in enumerate(np.argsort(-scores)[:k], 1):
            c = chunks[i]
            rel = os.path.relpath(c["file"], root)
            head = f"  {c['sym']}" if c["sym"] else ""
            lines.append(f"{rank}. [{scores[i]:.2f}] {rel}:{c['a']}-{c['b']}{head}")
            if rank <= full:
                lines.append("```\n" + c["text"] + "\n```")
            else:
                body = [ln for ln in c["text"].splitlines() if ln.strip()
                        and not ln.strip().startswith(("///", "//", "#["))]
                lines.append("   " + " ".join(" ".join(body[:3]).split())[:220])
        return "\n".join(lines)

    @mcp.tool()
    def local_docs(query: str, project: str, k: int = 8, reindex: bool = False) -> str:
        """Look up APIs in a Rust project's dependencies without web searches, e.g. "make a
        window click-through", "get the monitor work area", "HTTP GET with a timeout".
        project = the crate folder (with Cargo.toml, or its rust\\ subfolder). Searches the
        public items (signature + doc comment) of: windows / windows-sys modules for the
        features ENABLED in this build (each hit shows its feature and any extra features it
        needs), and the other direct dependencies (ureq, serde, chrono, png, ...). If query
        is one exact identifier (e.g. SetLayeredWindowAttributes, WS_EX_TRANSPARENT), it is
        looked up in ALL windows / windows-sys features and tells which feature is needed and
        whether it's enabled. First call indexes the dependencies (a few minutes for
        windows-sys); cached after that."""
        if cargo_dir(project) is None:
            return f"No Cargo.toml in {project} (or {project}\\rust)"
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", query.strip()):
            hits = exact_lookup(project, query.strip(), index_dir)
            if hits:
                return "Exact matches:\n" + "\n".join(hits[:12])
        try:
            deps = dep_files(project)
        except ValueError as e:
            return str(e)
        labels = dict(deps)
        files = list(labels)
        items, parts, n_new = [], [], 0
        for f in files:  # one cache per dependency file: shared by every crate that uses it
            key = "docs_" + hashlib.sha1(f"{CODE_INDEX_VERSION}|{labels[f]}|{f}".lower().encode()).hexdigest()[:12]
            its, v, n, _ = cached_index(index_dir, embed, key, [f], lambda p: api_items(p, labels[p]),
                                        lambda c: f"{c['label']}: {c['sig']} {c['doc']}", reindex)
            if v is not None and its:
                items += its
                parts.append(v)
                n_new += n
        if not parts:
            return "No API items found."
        vecs = np.vstack(parts)
        q = query_vec(embed, query, "Given a programming task, retrieve the API function, "
                                    "type or constant to use")
        scores = vecs @ q + name_boost(query, [c["sig"] for c in items])
        scores[[i for i, c in enumerate(items) if ERROR_CONST_RE.match(c["sig"])]] = -1  # old indexes
        crates = sorted({labels[f].split(" [")[0].rsplit(" ", 1)[0] for f in files})
        lines = [f"{len(items)} API items from {', '.join(crates)} ({n_new} files (re)indexed now)."]
        for rank, i in enumerate(np.argsort(-scores)[:k], 1):
            c = items[i]
            needs = f"  needs: {', '.join(c['needs'])}" if c["needs"] else ""
            doc = f"\n   {c['doc'][:200]}" if c["doc"] else ""
            lines.append(f"{rank}. [{scores[i]:.2f}] {c['label']}: {c['sig'][:240]}{needs}"
                         f"  [{Path(c['file']).name}:{c['line']}]{doc}")
        return "\n".join(lines)

    @mcp.tool()
    def local_patch(task: str, project: str, files: str, example: str = "", check: str = "check",
                    also_check: str = "", max_tries: int = 3, apply: bool = False,
                    model: str = "") -> str:
        """Let the local coding model make a SMALL, MECHANICAL code edit, compile-checked: e.g.
        mirror a change already made in one widget into the others, rename/add a field and
        update its uses, add a settings switch following an existing one, add a test like an
        existing test. Not for design work, new logic or subtle bugs (do those yourself).
        Runs in a scratch git worktree (D:\\lp\\wt) holding the current working tree, so the
        real files are untouched unless apply=True. Loop: the model writes SEARCH/REPLACE
        edits -> cargo check (all targets) of every crate touched (and of the widgets when
        taskbar-dock changes) -> errors fed back -> up to max_tries rounds. Returns PASS/FAIL,
        the diff and remaining errors; the diff is also saved to D:\\lp\\last.patch (apply it
        with `git apply`). Review the diff before applying.
        project: any folder inside the git repo (paths below are relative to it).
        files: comma-separated files the model sees and may edit; add :a-b for a line range
        when a file is big (e.g. "src/main.rs:120-260, ../taskbar-dock/src/menu_panel.rs").
        Keep the total under ~25k tokens. example: text or a file path showing the pattern to
        copy (e.g. a diff of the same change in another widget) - this helps a lot.
        task: precise instructions (what to change, where, names to use).
        check: "check" | "clippy" | "test" (builds and runs the tests). also_check:
        comma-separated extra crate folders to check. model: override the coding model."""
        root = repo_root(project)
        if root is None:
            return f"{project} is not inside a git repository (local_patch needs one)."
        proj = Path(project).resolve()
        specs = []
        for s in [x.strip() for x in files.split(",") if x.strip()]:
            m = re.fullmatch(r"(.+?):(\d+)-(\d+)", s)
            p, a, b = (m.group(1), int(m.group(2)), int(m.group(3))) if m else (s, 0, 0)
            full = (proj / p).resolve()
            try:
                rel = full.relative_to(root.resolve()).as_posix()
            except ValueError:
                return f"{p} is outside the repo {root}"
            specs.append((rel, a, b))
        if not specs:
            return "Give files (comma-separated, relative to project)."
        ex = Path(example).read_text(encoding="utf-8", errors="replace") if example and len(example) < 300 \
            and "\n" not in example and Path(example).is_file() else example

        with PATCH_LOCK:
            t0 = time.time()
            wt = make_worktree(root)

            def view() -> str:
                parts = []
                for rel, a, b in specs:
                    f = wt / rel
                    if not f.exists():
                        parts.append(f"FILE: {rel} (new file, does not exist yet)")
                        continue
                    text = f.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n")
                    n = text.count("\n") + 1
                    rng = f" (lines {a}-{min(b, n)} of {n}; only edit inside these)" if a else ""
                    body = numbered(text, a, b) if a else text
                    parts.append(f"FILE: {rel}{rng}\n```rust\n{body}\n```")
                return "\n\n".join(parts)

            base = f"TASK:\n{task}\n\n" + (f"EXAMPLE of the pattern to follow:\n{ex}\n\n" if ex else "")
            prompt = base + view()
            # qwen3.5:9b: 16k fits the GPU; 24k spills to RAM (~2x slower) but takes bigger jobs.
            # A coder 7B fits 32k.
            need = len(prompt) // 3 + 5000  # ~3 chars/token for code; 5k tokens left for the reply
            ctx = CODE_CTX if "coder" in (model or CODE_MODEL) else (16384 if need <= 16384 else 24576)
            limit = (ctx - 5000) * 3
            if len(prompt) > limit:
                return (f"Too much for the model (~{len(prompt) // 3000}k tokens, limit "
                        f"~{limit // 3000}k): give line ranges (file:a-b) or fewer files.")
            crates = set()
            for rel, _, _ in specs:
                c = crate_of(wt / rel, wt)
                if c:
                    crates.add(c)
            for c in list(crates):
                crates.update(dependents(c, wt))
            for x in [x.strip() for x in also_check.split(",") if x.strip()]:
                c = cargo_dir(str(wt / (proj / x).resolve().relative_to(root.resolve())))
                if c:
                    crates.add(c)
            if not crates:
                return "None of the files is inside a Cargo crate."
            baseline: set[str] | None = None
            errs, tries = [], 0
            for tries in range(1, max_tries + 1):
                reply = chat(prompt, PATCH_SYSTEM, model=model or CODE_MODEL, ctx=ctx,
                             temperature=0.1 if tries == 1 else 0.3)
                with open(SCRATCH / "rounds.log", "w" if tries == 1 else "a", encoding="utf-8") as lg:
                    lg.write(f"===== round {tries} PROMPT\n{prompt}\n===== REPLY\n{reply}\n")
                blocks = parse_blocks(reply)
                problems = []
                if not blocks and len(specs) == 1:
                    # the model rewrote the shown lines as one code block: take it as the new range
                    rel, a, b = specs[0]
                    f = wt / rel
                    fences = sorted(FENCE_RE.findall(reply), key=len)
                    raw = f.read_bytes().decode("utf-8", "replace") if f.exists() else ""
                    old = raw.replace("\r\n", "\n").split("\n")
                    lo, hi = (a - 1, min(b, len(old))) if a else (0, len(old))
                    cand = fences[-1] if fences else ""
                    looks_diff = re.search(r"^(@@ |diff --git|\+\+\+ |--- a/)", cand, re.M)
                    similar = cand and difflib.SequenceMatcher(
                        None, "\n".join(old[lo:hi]), cand, autojunk=False).quick_ratio() >= 0.85
                    if cand and not looks_diff and similar and cand.count("\n") + 1 >= 0.6 * (hi - lo):
                        new = old[:lo] + fences[-1].split("\n") + old[hi:]
                        text = "\n".join(new)
                        f.write_bytes((text.replace("\n", "\r\n") if "\r\n" in raw else text).encode("utf-8"))
                        blocks = [("", "", "")]  # counts as an edit
                    else:
                        problems.append("Your reply had no SEARCH/REPLACE blocks.")
                elif not blocks:
                    problems.append("Your reply had no SEARCH/REPLACE blocks.")
                for path, s, r in blocks:
                    if not (path or s or r):
                        continue  # the whole-range rewrite above
                    rel = path.replace("\\", "/")
                    match = [x for x, _, _ in specs if x == rel or x.endswith("/" + rel)
                             or rel.endswith("/" + x)] if rel else [x for x, _, _ in specs][:1] * (len(specs) == 1)
                    if len(match) != 1:
                        problems.append(f"FILE {path!r} is not one of the given files.")
                        continue
                    f = wt / match[0]
                    raw = f.read_bytes().decode("utf-8", "replace") if f.exists() else ""
                    crlf = "\r\n" in raw
                    try:
                        new = apply_block(raw.replace("\r\n", "\n"), s, r)
                    except ValueError as e:
                        problems.append(f"Edit for {match[0]} failed: {e}. SEARCH was:\n{s[:600]}")
                        continue
                    f.parent.mkdir(parents=True, exist_ok=True)
                    f.write_bytes((new.replace("\n", "\r\n") if crlf else new).encode("utf-8"))
                errs = list(problems)
                if git(["status", "--porcelain"], wt).strip():
                    for c in sorted(crates):
                        errs += cargo_errors(c, check, wt)
                if errs and baseline is None and not problems:
                    # errors that were already there before the edit don't count
                    saved = SCRATCH / "round.patch"
                    saved.write_text(wt_diff(wt), encoding="utf-8", newline="\n")
                    git(["reset", "-q", "--hard"], wt)
                    git(["clean", "-fdq"], wt)
                    baseline = {e.split("\n")[0] for c in sorted(crates) for e in cargo_errors(c, check, wt)}
                    git(["apply", str(saved)], wt)
                if baseline:
                    errs = [e for e in errs if e.split("\n")[0] not in baseline]
                if not errs and blocks:
                    break
                diff = wt_diff(wt)
                spots = []
                for p, ln in error_spots(errs)[:6]:
                    f = wt / p
                    if f.is_file():
                        t = f.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n")
                        spots.append(f"{p} lines {max(1, ln - 12)}-{ln + 12}:\n```rust\n"
                                     f"{numbered(t, max(1, ln - 12), ln + 12)}\n```")
                tail = (f"\n\nYour edits so far are ALREADY APPLIED (diff below). They don't work "
                        f"yet:\n\n```diff\n{diff[:12000]}\n```\n\nPROBLEMS:\n" + "\n\n".join(errs)[:8000]
                        + ("\n\nCurrent code around the errors:\n" + "\n\n".join(spots) if spots else "")
                        + "\n\nReply with new SEARCH/REPLACE blocks against the CURRENT text "
                          "to fix this (or to finish the task).")
                files_now = view()
                if len(base) + len(files_now) + len(tail) > limit:  # drop the full files, keep the spots
                    files_now = "(files omitted for space; edit using the diff and code shown below)"
                prompt = base + "Current files:\n" + files_now + tail
            diff = wt_diff(wt)
            (SCRATCH / "last.patch").write_text(diff, encoding="utf-8", newline="\n")
            ok = not errs and bool(diff.strip())
            names = ", ".join(sorted(c.relative_to(wt).as_posix() for c in crates))
            head = (f"{'PASS' if ok else 'FAIL'} after {tries} round(s), {time.time() - t0:.0f} s, "
                    f"model {model or CODE_MODEL}; cargo {check} on: {names}"
                    + (f" (errors that already existed ignored: {len(baseline)})" if baseline else ""))
            applied = ""
            if apply and ok:
                chk = sh(["git", "apply", "--check", str(SCRATCH / "last.patch")], cwd=root)
                if chk.returncode:
                    applied = f"\nNOT applied (working tree changed meanwhile?): {chk.stderr.strip()[:300]}"
                else:
                    git(["apply", str(SCRATCH / "last.patch")], root)
                    applied = "\nApplied to the working tree."
            elif ok:
                applied = f"\nNot applied. To apply: git -C \"{root}\" apply \"{SCRATCH / 'last.patch'}\""
            rest = ("\n\nRemaining problems:\n" + "\n\n".join(errs)[:4000]) if errs else ""
            return f"{head}{applied}\n\n```diff\n{diff[:20000]}\n```{rest}"

    @mcp.tool()
    def local_widget_check(widget: str, question: str = "", action: str = "",
                           wait: float = 1.2, baseline: str = "compare") -> str:
        """Screenshot a running widget (any app) and optionally compare/ask about it, to check
        UI changes without spending Claude image tokens. widget: part of the exe name, e.g.
        "taskbar-weather", "buswidget", "media". Captures every visible window of the process
        (strip, open panels) exactly as on screen -> PNGs in local-llm\\shots\\ (view them
        yourself when exact pixels matter). action: "" (as is), "hover" (mouse to the strip,
        wait, capture: hover panel), "click" (left-click panel; it stays open), "rightclick"
        (right-click panel; stays open). These MOVE THE USER'S MOUSE (put back afterwards).
        baseline: "compare" (default: pixel diff against the saved baseline of the same
        window + state: changed %, changed box, and a _diff.png with changes in red; live
        values like time/temperature always differ), "save" (store this capture as the new
        baseline), "none". question: asked to the local vision model about each capture
        (images upscaled), e.g. "Is any text cut off or overlapping? Read all text." It may
        misread small text; verify anything important by viewing the PNG."""
        if action not in ("", "hover", "click", "rightclick"):
            return 'action must be "", "hover", "click" or "rightclick"'
        old_ctx = user32.SetThreadDpiAwarenessContext(ctypes.c_void_p(-4))  # per-monitor v2: real pixels
        try:
            wins = widget_windows(widget)
            if not wins:
                return f"No visible window of a process matching {widget!r} (is it running?)"
            if action:
                pt = wintypes.POINT()
                user32.GetCursorPos(ctypes.byref(pt))
                x, y, w, h = wins[0]["rect"]
                mouse(x + w // 2, y + h // 2, action)
                time.sleep(wait)
                wins = widget_windows(widget)
                user32.SetCursorPos(pt.x, pt.y)
            SHOTS.mkdir(exist_ok=True)
            (SHOTS / "baseline").mkdir(exist_ok=True)
            stamp = time.strftime("%H%M%S") + f"{int(time.time() * 1000) % 1000:03d}"
            state = action or "rest"
            lines, seen = [], {}
            for wdw in wins:
                x, y, w, h = wdw["rect"]
                arr = grab(wdw["rect"])
                cls = re.sub(r"[^\w-]", "_", wdw["class"])[:40] or "win"
                seen[cls] = seen.get(cls, 0) + 1
                tag = f"{Path(wdw['exe']).stem}_{state}_{cls}" + (f"_{seen[cls]}" if seen[cls] > 1 else "")
                png = SHOTS / f"{tag}_{stamp}.png"
                save_png(arr, png)
                info = [f"- {wdw['class']} at ({x},{y}) {w}x{h} -> {png}"]
                bl = SHOTS / "baseline" / f"{tag}.png"
                if baseline == "save":
                    shutil.copy(png, bl)
                    info.append("  saved as baseline")
                elif baseline == "compare" and bl.exists():
                    ref = load_png(bl)
                    if ref.shape != arr.shape:
                        info.append(f"  SIZE CHANGED vs baseline: was {ref.shape[1]}x{ref.shape[0]}, "
                                    f"now {w}x{h}")
                    else:
                        changed = np.abs(ref.astype(int) - arr.astype(int)).max(axis=2) > 24
                        frac = changed.mean()
                        if frac == 0:
                            info.append("  identical to baseline")
                        else:
                            ys, xs = np.nonzero(changed)
                            grey = (arr.mean(axis=2, keepdims=True) * 0.5).astype(np.uint8).repeat(3, 2)
                            grey[changed] = (255, 0, 0)
                            dpng = SHOTS / f"{tag}_{stamp}_diff.png"
                            save_png(grey, dpng)
                            info.append(f"  {frac:.1%} of pixels changed vs baseline, in box x {xs.min()}-"
                                        f"{xs.max()}, y {ys.min()}-{ys.max()} -> {dpng}")
                elif baseline == "compare":
                    info.append("  no baseline yet (baseline=\"save\" to store one)")
                if question:
                    s = max(1, min(4, -(-240 // max(h, 1))))
                    big = arr.repeat(s, 0).repeat(s, 1) if s > 1 else arr
                    tmp = SHOTS / "_ask.png"
                    save_png(big, tmp)
                    ans = chat(f"This is a screenshot of a small desktop widget window ({w}x{h} px"
                               f"{', shown ' + str(s) + 'x enlarged' if s > 1 else ''}). {question}",
                               images=[str(tmp)])
                    info.append("  answer: " + " ".join(ans.split()))
                lines += info
            note = {"click": "\nThe left-click panel stays open (click the strip again to close).",
                    "rightclick": "\nThe right-click panel stays open."}.get(action, "")
            return f"{len(wins)} window(s) of {wins[0]['exe']}, state {state}:\n" + "\n".join(lines) + note
        finally:
            user32.SetThreadDpiAwarenessContext(ctypes.c_void_p(old_ctx))
