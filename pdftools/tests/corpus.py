"""Pinned public and optional private corpus runner. Invoke manually, never by unittest discovery."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tarfile
import io

ROOT = Path(__file__).resolve().parents[1]
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "common"))
PUBLIC = Path(os.environ.get("PDFTOOLS_CORPUS_PUBLIC", HERE / "corpus_public.json"))  # override: tests
CORPUS = HERE / "corpus" / "pdfjs"
BASELINE = HERE / "corpus" / "baseline.json"
URL = "https://github.com/mozilla/pdf.js"


def digest(path):
    value = hashlib.sha256()
    with open(path, "rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def fetch(commit=None, *, url=URL, destination=CORPUS, manifest=PUBLIC):
    manifest = Path(manifest); destination = Path(destination)
    if commit is None:
        if not manifest.is_file():
            raise ValueError("public corpus manifest is missing; run corpus.py fetch --commit <sha>")
        data = json.loads(manifest.read_text(encoding="utf-8"))
        commit = data["commit"]
        if not data.get("files"):
            raise ValueError("public corpus manifest has no files; run corpus.py fetch --commit <sha>")
        def valid():
            for row in data["files"]:
                path = destination / row["path"]
                if not path.is_file() or path.stat().st_size != row["bytes"] or digest(path) != row["sha256"]:
                    return False
            return True
        if not valid():
            _clone(commit, url, destination)
        for row in data["files"]:
            path = destination / row["path"]
            if not path.is_file() or path.stat().st_size != row["bytes"] or digest(path) != row["sha256"]:
                raise ValueError(f"corpus digest mismatch: {row['path']}")
        return data
    _clone(commit, url, destination)
    files = []
    for path in sorted((destination / "test" / "pdfs").rglob("*.pdf"), key=lambda p: p.relative_to(destination).as_posix()):
        size = path.stat().st_size
        if size <= 2 * 1024 * 1024:
            files.append({"path": path.relative_to(destination).as_posix(), "sha256": digest(path), "bytes": size})
            if len(files) == 200: break
    data = {"commit": commit, "files": files}
    manifest.parent.mkdir(parents=True, exist_ok=True)
    manifest.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    return data


def _clone(commit, url, destination):
    shutil.rmtree(destination, ignore_errors=True)
    local = Path(url)
    if local.exists():
        destination.mkdir(parents=True, exist_ok=True)
        archive = subprocess.run(["git", "-C", str(local), "archive", "--format=tar", commit, "test/pdfs"],
                                 check=True, capture_output=True).stdout
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as bundle:
            bundle.extractall(destination, filter="data")
        return
    destination.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "clone", "--filter=blob:none", "--no-checkout", url, str(destination)], check=True)
    subprocess.run(["git", "-C", str(destination), "sparse-checkout", "init", "--cone"], check=True)
    subprocess.run(["git", "-C", str(destination), "sparse-checkout", "set", "test/pdfs"], check=True)
    subprocess.run(["git", "-C", str(destination), "checkout", commit], check=True)


SEVERITY = {"ok": 0, "report": 0, "warn": 1, "unchecked": 2, "error": 3, "timeout": 4, "crash": 5}


def categorize(returncode, stdout, stderr=""):
    try:
        rows = [json.loads(line) for line in stdout.splitlines() if line.strip()]
        record = next((row for row in reversed(rows) if row.get("type") in {"result", "report", "error"}), None)
    except Exception:
        record = None
    if "Traceback (most recent call last)" in (stdout + stderr):
        return "crash"
    if (record and record.get("type") == "error" and record.get("code") == "TIMEOUT") or \
            any(line.startswith("ERROR pdf ") and " TIMEOUT:" in line for line in stderr.splitlines()):
        return "timeout"
    if record is None:
        return "crash"
    if record["type"] == "error":
        if returncode not in (1, 2): return "crash"
        return "error:" + str(record.get("code", "UNKNOWN"))
    verdict = record.get("verdict")
    if verdict in {"ok", "warn", "report"} and returncode == 0: return verdict
    if verdict == "unchecked" and returncode == 3: return verdict
    return "crash"


def _rank(category):
    return SEVERITY.get(category.split(":", 1)[0], 3)


def compare_baselines(old, new):
    regressions, changed, improvements = [], [], []
    for key, category in new.items():
        previous = old.get(key)
        if previous is None:
            changed.append((key, category))
        elif category != previous:
            if _rank(category) > _rank(previous) or (category.startswith("error:") and previous.startswith("error:") and category != previous):
                regressions.append((key, previous, category))
            elif _rank(category) < _rank(previous): improvements.append((key, previous, category))
    for key in old.keys() - new.keys(): improvements.append((key, old[key], "removed"))
    return regressions, changed, improvements


def _files():
    files = []
    if not PUBLIC.is_file():
        raise ValueError("public corpus manifest is missing; run corpus.py fetch --commit <sha>")
    data = json.loads(PUBLIC.read_text(encoding="utf-8"))
    if not data.get("files"):
        raise ValueError("public corpus manifest is empty; run corpus.py fetch --commit <sha>")
    files.extend((CORPUS / row["path"], row["sha256"], row["bytes"]) for row in data["files"])
    local = HERE / "corpus_local.json"
    if local.is_file():
        for path in json.loads(local.read_text(encoding="utf-8")):
            path = Path(path).resolve(); files.append((path, digest(path), path.stat().st_size))
    return files


# (command, args after the input file); test_pH_corpus parses each through the real CLI parser
COMMANDS = [("info",), ("text",), ("organize", "rev"), ("rotate", "--pages", "1-", "--by", "90"),
            ("numbers",), ("compress", "--level", "lossless")]


def sweep(update=False):
    commands = COMMANDS
    from pdftools.registry import discover, get
    discover()
    outcomes = {}
    files = _files()
    for path, sha, size in files:
        if not path.is_file() or path.stat().st_size != size or digest(path) != sha:
            raise ValueError(f"corpus digest mismatch or missing file: {path}")
    with tempfile.TemporaryDirectory(prefix="pdftools-corpus-") as temp:
        for path, sha, _ in files:
            for argv in commands:
                try: get(argv[0])
                except KeyError: continue
                key = sha + ":" + " ".join(argv)
                output = Path(temp) / hashlib.sha256(key.encode()).hexdigest()
                command_argv = [sys.executable, str(ROOT / "pdf.py"), "--json", argv[0], str(path), *argv[1:]]
                if argv[0] not in {"info", "text"}:
                    output.mkdir()
                    command_argv.extend(["-o", str(output) + os.sep])
                child = subprocess.Popen(command_argv, stdout=subprocess.PIPE,
                                         stderr=subprocess.PIPE, text=True)
                try:
                    stdout, stderr = child.communicate(timeout=120)
                    category = categorize(child.returncode, stdout, stderr)
                except subprocess.TimeoutExpired:
                    try:
                        import psutil
                        proc = psutil.Process(child.pid)
                        for sub in proc.children(recursive=True): sub.kill()
                        proc.kill(); psutil.wait_procs(proc.children(recursive=True) + [proc], timeout=2)
                    except Exception: child.kill()
                    child.communicate(); category = "timeout"
                outcomes[key] = category
    old = json.loads(BASELINE.read_text()) if BASELINE.is_file() else {}
    regressions, changed, improvements = compare_baselines(old, outcomes)
    new_bad = [(key, category) for key, category in changed
               if category.split(":", 1)[0] in {"crash", "timeout", "error"}]
    if update:
        merged = {**old, **outcomes}
        BASELINE.parent.mkdir(parents=True, exist_ok=True)
        BASELINE.write_text(json.dumps(merged, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print("CORPUS " + " ".join(f"{name}={sum(v == name or v.startswith(name + ':') for v in outcomes.values())}"
                              for name in ("ok", "report", "warn", "unchecked", "error", "timeout", "crash")))
    for key, oldcat, newcat in regressions: print(f"WORSE {key}: {oldcat} -> {newcat}")
    for key, category in changed: print(f"NEW {key}: {category}")
    for key, oldcat, newcat in improvements: print(f"IMPROVED {key}: {oldcat} -> {newcat}")
    return 1 if regressions or new_bad else 0


def main(argv=None):
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="action", required=True)
    f = sub.add_parser("fetch"); f.add_argument("--commit")
    s = sub.add_parser("sweep"); s.add_argument("--update-baseline", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.action == "fetch": fetch(args.commit); return 0
        return sweep(args.update_baseline)
    except Exception as exc:
        import clierr
        return clierr.fail("corpus", exc, log_name="corpus")


if __name__ == "__main__": raise SystemExit(main())
