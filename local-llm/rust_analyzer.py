"""Minimal rust-analyzer (LSP) client for exact find-references and rename (used by code_tools.py).

One rust-analyzer process per git repo, loaded with every crate in it (linkedProjects), kept
running between calls (loading the crates takes a while) and stopped after IDLE_S seconds
unused. It watches the files itself, so it follows edits made outside it.
"""
import json
import re
import shutil
import subprocess
import threading
import time
from pathlib import Path
from urllib.parse import quote, unquote, urlparse

IDLE_S = 600
_servers: dict = {}
_lock = threading.Lock()


def uri_of(p: Path) -> str:
    return "file:///" + quote(str(p.resolve()).replace("\\", "/"), safe="/:")


def path_of(uri: str) -> Path:
    return Path(unquote(urlparse(uri).path).lstrip("/"))


def u16(line: str, i: int) -> int:
    """Python index -> LSP character (UTF-16 code units)."""
    return len(line[:i].encode("utf-16-le")) // 2


def py_index(line: str, c16: int) -> int:
    """LSP character (UTF-16 code units) -> Python index."""
    n = 0
    for i, ch in enumerate(line):
        if n >= c16:
            return i
        n += 2 if ord(ch) > 0xFFFF else 1
    return len(line)


class RA:
    def __init__(self, root: Path, manifests: list[Path]):
        exe = shutil.which("rust-analyzer")
        if not exe:
            raise RuntimeError("rust-analyzer not found (rustup component add rust-analyzer)")
        self.root, self.next_id, self.pending = root, 0, {}
        self.config = {"linkedProjects": [str(m) for m in manifests], "files": {"watcher": "server"},
                       "checkOnSave": False, "cargo": {"buildScripts": {"enable": True}},
                       "procMacro": {"enable": True}}
        self.quiescent = threading.Event()
        self.used = time.time()
        self.p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, cwd=root,
                                  creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        threading.Thread(target=self._read, daemon=True).start()
        self.request("initialize", {
            "processId": None, "rootUri": uri_of(root),
            "capabilities": {"experimental": {"serverStatusNotification": True},
                             "workspace": {"workspaceEdit": {"documentChanges": True}},
                             "window": {"workDoneProgress": True}},
            "initializationOptions": self.config}, timeout=120)
        self.notify("initialized", {})

    def _send(self, msg: dict):
        body = json.dumps(msg).encode("utf-8")
        self.p.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        self.p.stdin.flush()

    def _read(self):
        f = self.p.stdout
        while True:
            n = 0
            while True:
                line = f.readline()
                if not line:
                    return  # process ended
                line = line.strip()
                if not line:
                    break
                if line.lower().startswith(b"content-length:"):
                    n = int(line.split(b":")[1])
            msg = json.loads(f.read(n))
            if "id" in msg and "method" in msg:  # server -> client request: answer blandly
                self._send({"jsonrpc": "2.0", "id": msg["id"],
                            "result": [self.config] * len(msg.get("params", {}).get("items", []))
                            if msg["method"] == "workspace/configuration" else None})
            elif "id" in msg:
                slot = self.pending.pop(msg["id"], None)
                if slot:
                    slot[1] = msg
                    slot[0].set()
            elif msg.get("method") == "experimental/serverStatus":
                if msg["params"].get("quiescent"):
                    self.quiescent.set()
                else:
                    self.quiescent.clear()

    def notify(self, method: str, params):
        self._send({"jsonrpc": "2.0", "method": method, "params": params})

    def request(self, method: str, params, timeout: float = 120):
        self.next_id += 1
        slot = [threading.Event(), None]
        self.pending[self.next_id] = slot
        self._send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        if not slot[0].wait(timeout):
            raise TimeoutError(f"rust-analyzer: no answer to {method} in {timeout:.0f} s")
        msg = slot[1]
        if "error" in msg:
            raise RuntimeError(f"rust-analyzer {method}: {msg['error'].get('message')}")
        return msg.get("result")

    def ask(self, method: str, params, wait: float = 600):
        """Request once the crates are loaded; retries while rust-analyzer is still busy."""
        self.used = time.time()
        if not self.quiescent.wait(wait):
            raise TimeoutError(f"rust-analyzer still loading after {wait:.0f} s")
        for i in range(5):
            try:
                return self.request(method, params)
            except RuntimeError as e:
                if "content modified" not in str(e).lower() or i == 4:
                    raise
                time.sleep(1)
                self.quiescent.wait(120)

    def alive(self) -> bool:
        return self.p.poll() is None

    def stop(self):
        try:
            self.request("shutdown", None, timeout=10)
            self.notify("exit", None)
        except Exception:
            pass
        try:
            self.p.kill()
        except Exception:
            pass


def server(root: Path, manifests: list[Path]) -> tuple["RA", bool]:
    """The running rust-analyzer for root (started if needed). Returns (ra, was_running)."""
    with _lock:
        key = str(root.resolve()).lower()
        ra = _servers.get(key)
        if ra and ra.alive():
            return ra, True
        ra = RA(root, manifests)
        _servers[key] = ra

        def reaper():
            while ra.alive():
                time.sleep(30)
                if time.time() - ra.used > IDLE_S and ra.quiescent.is_set():
                    ra.stop()
                    _servers.pop(key, None)
        threading.Thread(target=reaper, daemon=True).start()
        return ra, False


DEF_RE = r"\b(?:fn|struct|enum|trait|type|const|static|mod|union|macro_rules!|let(?:\s+mut)?)\s+{}\b"


def locate(file: Path, symbol: str, line: int = 0) -> tuple[int, int]:
    """(0-based line, UTF-16 column) of symbol in file: on the given 1-based line if any, else
    its definition (fn/struct/let/...), else its first use."""
    lines = file.read_text(encoding="utf-8", errors="replace").splitlines()
    word = re.compile(rf"\b{re.escape(symbol)}\b")
    if line:
        for ln in sorted(range(len(lines)), key=lambda k: abs(k - (line - 1)))[:20]:
            m = word.search(lines[ln])
            if m:
                return ln, u16(lines[ln], m.start())
        raise ValueError(f"{symbol!r} not found near line {line} of {file.name}")
    d = re.compile(DEF_RE.format(re.escape(symbol)))
    for pat in (d, word):
        for ln, text in enumerate(lines):
            m = pat.search(text)
            if m:
                s = word.search(text, m.start())
                return ln, u16(text, s.start())
    raise ValueError(f"{symbol!r} not found in {file.name}")


def apply_edits(edits_by_file: dict[Path, list], to_root: Path, from_root: Path) -> list[str]:
    """Apply LSP TextEdits (computed on from_root's files) to the same files under to_root.
    Returns the repo-relative paths changed."""
    changed = []
    for path, edits in edits_by_file.items():
        rel = path.resolve().relative_to(from_root.resolve())
        f = to_root / rel
        raw = f.read_bytes().decode("utf-8")
        crlf = "\r\n" in raw
        lines = raw.replace("\r\n", "\n").split("\n")
        starts, pos = [], 0
        for ln in lines:
            starts.append(pos)
            pos += len(ln) + 1
        text = "\n".join(lines)

        def off(p):
            return starts[p["line"]] + py_index(lines[p["line"]], p["character"])

        for e in sorted(edits, key=lambda e: off(e["range"]["start"]), reverse=True):
            a, b = off(e["range"]["start"]), off(e["range"]["end"])
            text = text[:a] + e["newText"] + text[b:]
        f.write_bytes((text.replace("\n", "\r\n") if crlf else text).encode("utf-8"))
        changed.append(rel.as_posix())
    return changed
