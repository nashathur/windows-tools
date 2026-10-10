"""MCP server that lets Claude Code hand bulk work to a local Ollama model.

Run (Claude Code starts it automatically once registered):
    conda run --no-capture-output -n llm python C:\\Users\\thurs\\tools\\local-llm\\server.py

Settings (environment variables, all optional):
    LOCAL_LLM_MODEL  model tag in Ollama        (default: see DEFAULT_MODEL)
    LOCAL_LLM_CTX    context window in tokens   (default: 16384; more spills to RAM, ~5x slower)
    OLLAMA_HOST      Ollama URL                 (default: http://127.0.0.1:11434)
    LOCAL_LLM_EMBED  embedding model for local_search (default: qwen3-embedding:0.6b)
    LOCAL_LLM_WHISPER  faster-whisper model for local_transcribe (default: large-v3-turbo)

To add a tool: write a function below, decorate it with @mcp.tool(), give it a
clear docstring (Claude reads it to decide when to call it), and restart Claude
Code. Use chat() to talk to the model. See README.md.
"""
import base64
import bisect
import difflib
import glob
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import httpx
import numpy as np
import pymupdf
from mcp.server.mcpserver import MCPServer

DEFAULT_MODEL = "qwen3.5:9b"
MODEL = os.environ.get("LOCAL_LLM_MODEL", DEFAULT_MODEL)
CTX = int(os.environ.get("LOCAL_LLM_CTX", "16384"))
HOST = os.environ.get("OLLAMA_HOST", "http://127.0.0.1:11434")
if not HOST.startswith("http"):
    HOST = "http://" + HOST
CHUNK_CHARS = CTX * 2  # ~4 chars/token; leaves half the window for prompt + answer
EMBED_MODEL = os.environ.get("LOCAL_LLM_EMBED", "qwen3-embedding:0.6b")
INDEX_DIR = Path(__file__).parent / "index"  # local_search caches, one pair per folder
INDEX_VERSION = 2  # bump when search_chunks changes, so old indexes are rebuilt
IMAGE_EXTS = {".png", ".jpg", ".jpeg", ".webp", ".bmp", ".gif"}
TEXT_EXTS = {".md", ".txt", ".csv", ".tsv", ".json", ".tex", ".py", ".log", ".html",
             ".xml", ".yml", ".yaml", ".ini", ".bib", ".rst"}
MEDIA_EXTS = {".mp4", ".mkv", ".webm", ".mov", ".avi", ".m4v", ".wmv", ".flv", ".ts", ".3gp",
              ".mp3", ".wav", ".m4a", ".aac", ".flac", ".ogg", ".opus", ".wma"}
WHISPER_MODEL = os.environ.get("LOCAL_LLM_WHISPER", "large-v3-turbo")

mcp = MCPServer("local-llm")


def image_b64(img: str | bytes) -> str:
    """Base64 of an image path or bytes; photos over 1.5 MB are shrunk to <= 1600 px."""
    if isinstance(img, bytes):
        return base64.b64encode(img).decode()
    data = Path(img).read_bytes()
    if len(data) > 1_500_000:
        try:
            pix = pymupdf.Pixmap(str(img))
            if pix.n - pix.alpha >= 4:  # CMYK -> RGB
                pix = pymupdf.Pixmap(pymupdf.csRGB, pix)
            while max(pix.width, pix.height) > 1600:
                pix.shrink(1)
            data = pix.tobytes("png")
        except Exception:
            pass  # send as is
    return base64.b64encode(data).decode()


def chat(prompt: str, system: str = "", json_mode: bool = False,
         think: bool = False, temperature: float = 0.2,
         images: list[str | bytes] | None = None) -> str:
    """Send one prompt (plus optional images: paths or bytes) to the local model and return its reply."""
    user = {"role": "user", "content": prompt}
    if images:
        user["images"] = [image_b64(i) for i in images]
    messages = ([{"role": "system", "content": system}] if system else []) + [user]
    body = {"model": MODEL, "messages": messages, "stream": False, "think": think,
            "options": {"num_ctx": CTX, "temperature": temperature}}
    if json_mode:
        body["format"] = "json"
    r = httpx.post(f"{HOST}/api/chat", json=body, timeout=600)
    r.raise_for_status()
    return r.json()["message"]["content"].strip()


def is_path(path_or_text: str) -> bool:
    return len(path_or_text) < 500 and "\n" not in path_or_text and Path(path_or_text).is_file()


def read_text(path_or_text: str) -> str:
    """Accept a file path (text or PDF) or literal text."""
    if not is_path(path_or_text):
        return path_or_text
    p = Path(path_or_text)
    if p.suffix.lower() == ".pdf":
        with pymupdf.open(p) as doc:
            return "\n\n".join(page.get_text() for page in doc)
    return p.read_text(encoding="utf-8", errors="replace")


def read_records(path_or_text: str) -> list[tuple[str, str]]:
    """Lines with their location: ("p12", line) for PDFs, ("L340", line) otherwise."""
    if is_path(path_or_text) and Path(path_or_text).suffix.lower() == ".pdf":
        with pymupdf.open(path_or_text) as doc:
            return [(f"p{i + 1}", ln) for i, page in enumerate(doc)
                    for ln in page.get_text().splitlines()]
    return [(f"L{i + 1}", ln) for i, ln in enumerate(read_text(path_or_text).splitlines())]


def group_records(records: list[tuple[str, str]], size: int) -> list[list[tuple[str, str]]]:
    """Group consecutive lines into pieces of ~size chars."""
    groups, cur, n = [], [], 0
    for r in records:
        if cur and n + len(r[1]) > size:
            groups.append(cur)
            cur, n = [], 0
        cur.append(r)
        n += len(r[1]) + 1
    if cur:
        groups.append(cur)
    return groups


def norm(s: str) -> str:
    return " ".join(s.lower().split())


def locator(records: list[tuple[str, str]]):
    """Return a function mapping a quote to the location where it appears verbatim (or None)."""
    starts, parts, pos = [], [], 0
    for _, ln in records:
        starts.append(pos)
        parts.append(norm(ln) + " ")
        pos += len(parts[-1])
    full = "".join(parts)

    def find(quote: str) -> str | None:
        q = norm(quote)
        idx = full.find(q) if q else -1
        if idx < 0 and len(q) > 60:
            idx = full.find(q[:60])
        return records[bisect.bisect_right(starts, idx) - 1][0] if idx >= 0 else None
    return find


def parse_json(reply: str) -> dict:
    try:
        d = json.loads(reply)
        return d if isinstance(d, dict) else {}
    except json.JSONDecodeError:
        return {}


def chunks(text: str, size: int = CHUNK_CHARS) -> list[str]:
    """Split on paragraph boundaries into pieces of at most ~size chars."""
    out, cur = [], ""
    for para in text.split("\n\n"):
        if len(cur) + len(para) > size and cur:
            out.append(cur)
            cur = ""
        while len(para) > size:
            out.append(para[:size])
            para = para[size:]
        cur += para + "\n\n"
    if cur.strip():
        out.append(cur)
    return out


@mcp.tool()
def local_ask(prompt: str, system: str = "", think: bool = False) -> str:
    """Ask the local model anything (free, runs on the user's GPU). Good for simple,
    self-contained jobs: rewording, short drafts, simple code snippets, quick
    explanations. Not reliable for hard reasoning or facts. Set think=True for a
    slower, more careful answer. Prompt must fit ~10k tokens."""
    return chat(prompt, system, think=think)


@mcp.tool()
def local_summarize(path_or_text: str, focus: str = "", max_words: int = 300) -> str:
    """Summarize a file (give its path) or text of any length with the local model.
    Long input is split into chunks, summarized, then merged. Use to save Claude
    tokens on reading long documents, logs or transcripts. Optional focus says
    what to pay attention to."""
    text = read_text(path_or_text)
    f = f" Focus on: {focus}." if focus else ""
    parts = chunks(text)
    if len(parts) == 1:
        return chat(f"Summarize in at most {max_words} words.{f}\n\n{text}")
    notes = [chat(f"Summarize this part (part {i + 1} of {len(parts)}) as dense "
                  f"bullet points.{f}\n\n{p}") for i, p in enumerate(parts)]
    return chat(f"Merge these partial notes into one summary of at most {max_words} "
                f"words.{f}\n\n" + "\n\n".join(notes))


@mcp.tool()
def local_classify(items: list[str], labels: list[str], instructions: str = "") -> str:
    """Label each item with one of the given labels using the local model.
    Returns JSON lines {"item": ..., "label": ..., "reason": ...}. Good for sorting
    files, emails, QA lines, data rows. Items are sent one at a time."""
    out = []
    for it in items:
        reply = chat(
            f"Choose exactly one label from {json.dumps(labels)} for the item below. "
            f"{instructions}\nReply as JSON: {{\"label\": \"...\", \"reason\": \"short\"}}"
            f"\n\nITEM:\n{it}", json_mode=True)
        try:
            d = json.loads(reply)
        except json.JSONDecodeError:
            d = {"label": "?", "reason": reply[:200]}
        out.append(json.dumps({"item": it[:120], **d}, ensure_ascii=False))
    return "\n".join(out)


@mcp.tool()
def local_extract(path_or_text: str, fields: str) -> str:
    """Pull structured data out of a file or text as JSON, e.g. fields="station name,
    date, max temperature, units". Long input is processed in chunks and the
    results are returned as a JSON list (one object per chunk)."""
    text = read_text(path_or_text)
    res = []
    for p in chunks(text):
        reply = chat(f"Extract these fields as a JSON object: {fields}. Use null when "
                     f"absent. Copy values exactly; do not guess.\n\n{p}", json_mode=True)
        try:
            res.append(json.loads(reply))
        except json.JSONDecodeError:
            res.append({"_raw": reply})
    return json.dumps(res[0] if len(res) == 1 else res, ensure_ascii=False, indent=1)


@mcp.tool()
def local_batch(file_glob: str, instruction: str, output_file: str,
                json_mode: bool = False) -> str:
    """Run the same instruction on every file matching a glob (e.g.
    "D:\\notes\\*.md") and write all answers to output_file (Markdown, one section
    per file). Returns only a short status, so Claude doesn't spend tokens on the
    results; read output_file afterwards if needed. Files longer than one chunk are
    truncated to the first chunk."""
    files = sorted(glob.glob(file_glob, recursive=True))
    if not files:
        return f"No files match {file_glob}"
    done = 0
    with open(output_file, "w", encoding="utf-8") as out:
        for fp in files:
            try:
                text = read_text(fp)[:CHUNK_CHARS]
                ans = chat(f"{instruction}\n\nFILE: {Path(fp).name}\n\n{text}",
                           json_mode=json_mode)
                done += 1
            except Exception as e:  # keep going on one bad file
                ans = f"ERROR: {e}"
            out.write(f"## {fp}\n\n{ans}\n\n")
            out.flush()
    return f"{done}/{len(files)} files processed -> {output_file}"


@mcp.tool()
def local_describe_image(path_or_glob: str, instruction: str = "",
                         output_file: str = "") -> str:
    """Describe images with the local vision model (free, saves Claude's image
    tokens). Give one image path, or a glob (e.g. "D:\\figs\\*.png") plus
    output_file to describe many: answers go to output_file (Markdown, one section
    per image) and only a short status is returned. instruction says what to look
    for, e.g. "transcribe all text", "what does this plot show; axes, units,
    trend". Good for gists, sorting, alt text and reading clear text; not reliable
    for exact numbers, equations or fine detail."""
    files = sorted(f for f in glob.glob(path_or_glob, recursive=True)
                   if Path(f).suffix.lower() in IMAGE_EXTS)
    if not files:
        return f"No images match {path_or_glob}"
    ask = instruction or "Describe this image concisely: what it shows and any text in it."
    if len(files) == 1 and not output_file:
        return chat(ask, images=files)
    if not output_file:
        return f"{len(files)} images match; give output_file to describe them all."
    done = 0
    with open(output_file, "w", encoding="utf-8") as out:
        for fp in files:
            try:
                ans = chat(ask, images=[fp])
                done += 1
            except Exception as e:  # keep going on one bad image
                ans = f"ERROR: {e}"
            out.write(f"## {fp}\n\n{ans}\n\n")
            out.flush()
    return f"{done}/{len(files)} images described -> {output_file}"


def embed(texts: list[str]) -> np.ndarray:
    """Unit-length embedding vectors, one row per text."""
    out = []
    for i in range(0, len(texts), 32):
        r = httpx.post(f"{HOST}/api/embed", json={"model": EMBED_MODEL, "input": texts[i:i + 32]},
                       timeout=600)
        r.raise_for_status()
        out += r.json()["embeddings"]
    v = np.array(out, dtype=np.float32)
    return v / np.maximum(np.linalg.norm(v, axis=1, keepdims=True), 1e-9)


def search_chunks(path: str) -> list[dict]:
    """~1500-char passages of a file, split at Markdown headings, with location + heading."""
    md = Path(path).suffix.lower() == ".md"
    out, heading, cur, n, fence = [], "", [], 0, False

    def flush():
        text = "\n".join(ln for _, ln in cur).strip()
        if text:
            out.append({"file": path, "loc": cur[0][0], "heading": heading, "text": text})

    for loc, ln in read_records(path):
        if ln.lstrip().startswith("```"):
            fence = not fence
        is_head = md and not fence and re.match(r"#{1,6} ", ln)
        if cur and (n + len(ln) > 1500 or is_head):
            flush()
            cur, n = [], 0
        if is_head:
            heading = ln.lstrip("#").strip()
        cur.append((loc, ln))
        n += len(ln) + 1
    if cur:
        flush()
    return out


@mcp.tool()
def local_search(query: str, folder: str, k: int = 5,
                 pattern: str = "**/*.md,**/*.txt,**/*.pdf", reindex: bool = False) -> str:
    """Search a folder of notes/books/PDFs by meaning (not exact words), e.g. "where
    is Ekman pumping derived?" or "aturan peringatan dini hujan lebat". Returns the
    k best passages with file, line/page, heading and a snippet, so Claude can read
    only those spots. The first call on a folder builds an index (about a minute per
    book); later calls re-index only changed files. pattern is comma-separated globs
    relative to folder. Point it at clean text (e.g. a pdf2md *_project export
    folder, not the raw chapter folders with QA reports)."""
    root = Path(folder)
    if not root.is_dir():
        return f"Not a folder: {folder}"
    files = sorted({str(Path(f)) for pat in pattern.split(",") if pat.strip()
                    for f in glob.glob(str(root / pat.strip()), recursive=True)
                    if Path(f).is_file()})
    INDEX_DIR.mkdir(exist_ok=True)
    key = hashlib.sha1(f"{INDEX_VERSION}|{root.resolve()}|{pattern}|{EMBED_MODEL}"
                       .lower().encode()).hexdigest()[:12]
    meta_p, vec_p = INDEX_DIR / f"{key}.json", INDEX_DIR / f"{key}.npy"
    meta, vecs = {"folder": str(root), "pattern": pattern, "files": {}, "chunks": []}, None
    if not reindex and meta_p.exists() and vec_p.exists():
        meta, vecs = json.loads(meta_p.read_text(encoding="utf-8")), np.load(vec_p)

    mtimes = {f: os.path.getmtime(f) for f in files}
    stale = {f for f, m in meta["files"].items() if mtimes.get(f) != m}  # changed or deleted
    new = [f for f in files if meta["files"].get(f) != mtimes[f]]
    if stale or new or vecs is None:
        keep = [i for i, c in enumerate(meta["chunks"]) if c["file"] not in stale]
        chunks_ = [meta["chunks"][i] for i in keep]
        vecs = vecs[keep] if vecs is not None and len(keep) else None
        add, failed = [], []
        for f in new:
            try:
                add += search_chunks(f)
            except Exception:  # unreadable file: skip it, keep indexing the rest
                failed.append(f)
        if add:
            v = embed([f"{c['heading']}\n{c['text']}" for c in add])
            vecs = v if vecs is None else np.vstack([vecs, v])
        meta["files"] = {f: mtimes[f] for f in files if f not in failed}
        meta["chunks"] = chunks_ + add
        if vecs is None:
            return f"No text found in {len(files)} files matching {pattern} under {folder}."
        meta_p.write_text(json.dumps(meta, ensure_ascii=False), encoding="utf-8")
        np.save(vec_p, vecs)
    else:
        failed = []

    q = embed([f"Instruct: Given a search query, retrieve relevant passages that answer "
               f"the query\nQuery: {query}"])[0]
    scores = vecs @ q
    lines = [f"{len(files)} files, {len(meta['chunks'])} passages indexed "
             f"({len(new)} files (re)indexed now"
             + (f", {len(failed)} unreadable" if failed else "") + ")."]
    for rank, i in enumerate(np.argsort(-scores)[:k], 1):
        c = meta["chunks"][i]
        rel = os.path.relpath(c["file"], root)
        head = f" | {c['heading']}" if c["heading"] else ""
        lines.append(f"{rank}. [{scores[i]:.2f}] {rel} {c['loc']}{head}\n   "
                     f"{' '.join(c['text'].split())[:300]}")
    return "\n".join(lines)


@mcp.tool()
def local_ask_file(path_or_text: str, question: str, max_quotes: int = 8) -> str:
    """Answer a question about one long file (text, Markdown or PDF) or text, e.g.
    "what rainfall threshold triggers a warning?". The local model reads it part by
    part, keeps relevant quotes, then answers. Each quote is matched back to the
    file to give its line (L123) or PDF page (p12); quotes it can't find verbatim
    are marked unverified. Use instead of reading a whole long file for one fact."""
    records = read_records(path_or_text)
    groups = group_records(records, CHUNK_CHARS)
    notes, quotes = [], []
    for i, g in enumerate(groups):
        text = "\n".join(ln for _, ln in g)
        d = parse_json(chat(
            f"Question: {question}\n\nBelow is part {i + 1} of {len(groups)} of a document. "
            f"Does it contain information that helps answer the question? Reply as JSON: "
            f"{{\"relevant\": true, \"quotes\": [\"sentences copied word for word\"], "
            f"\"note\": \"what this part says about the question, 1-2 sentences\"}} with at "
            f"most 3 quotes, or {{\"relevant\": false}}.\n\nTEXT:\n{text}", json_mode=True))
        if not d.get("relevant"):
            continue
        if d.get("note"):
            notes.append(f"(part {i + 1}) {d['note']}")
        for q in d.get("quotes") or []:
            q = q if isinstance(q, str) else str(q.get("text", "")) if isinstance(q, dict) else ""
            if q.strip():
                quotes.append(q.strip())
    if not notes and not quotes:
        return f"Nothing relevant found ({len(groups)} parts checked)."
    find, sources, seen = locator(records), [], set()
    for q in quotes:
        if norm(q) in seen:
            continue
        seen.add(norm(q))
        sources.append(f"- {find(q) or 'unverified'}: \"{q[:300]}\"")
    answer = chat(f"Question: {question}\n\nAnswer using only these notes and quotes from "
                  f"the document. If they don't answer it, say so.\n\nNOTES:\n"
                  + "\n".join(notes) + "\n\nQUOTES:\n" + "\n".join(quotes))
    return (f"{answer}\n\nSources ({len(groups)} parts read; location = exact match in "
            f"the file):\n" + "\n".join(sources[:max_quotes]))


@mcp.tool()
def local_translate(path_or_text: str, target: str = "English", output_file: str = "") -> str:
    """Translate a file (text, Markdown or PDF) or text into target language (e.g.
    Indonesian <-> English), keeping Markdown, LaTeX, code, numbers and units intact.
    Long input is translated in parts. With output_file the result is written there
    and only a short status is returned (saves Claude tokens). Check key technical
    or legal terms in anything official."""
    parts = chunks(read_text(path_or_text), CHUNK_CHARS // 3)  # output needs room too
    out = [chat(f"Translate the text below into {target}. Keep Markdown, LaTeX ($...$), "
                f"code, numbers, units, names and line breaks unchanged; translate only the "
                f"words. Output only the translation.\n\n{p}", temperature=0.1)
           for p in parts]
    result = "\n\n".join(out)
    if output_file:
        Path(output_file).write_text(result, encoding="utf-8")
        return f"{len(parts)} parts translated -> {output_file} ({len(result)} chars)"
    return result


def compare_units(path_or_text: str) -> list[str]:
    """Sentences for PDFs (line wrapping differs between versions), else non-empty lines."""
    text = read_text(path_or_text)
    if is_path(path_or_text) and Path(path_or_text).suffix.lower() == ".pdf":
        return [s for s in re.split(r"(?<=[.!?;:])\s+", " ".join(text.split())) if s]
    return [ln.rstrip() for ln in text.splitlines() if ln.strip()]


@mcp.tool()
def local_compare(old: str, new: str, focus: str = "") -> str:
    """Explain in plain words what changed between two versions of a document (file
    paths, text/Markdown/PDF, or literal text): additions, removals, rewording,
    changed numbers/dates. The diff itself is exact (Python difflib); the local model
    only summarizes it. Optional focus, e.g. "thresholds and deadlines"."""
    a, b = compare_units(old), compare_units(new)
    diff = list(difflib.unified_diff(a, b, "old", "new", n=1, lineterm=""))[2:]
    if not diff:
        return "No differences in the text."
    added = sum(ln.startswith("+") for ln in diff)
    removed = sum(ln.startswith("-") for ln in diff)
    f = f" Pay most attention to: {focus}." if focus else ""
    ask = ("This is a diff between an old and a new version of a document (lines starting "
           "with '-' were removed, '+' added, others are context). Summarize what changed "
           "in plain words: additions, removals, rewording, changed numbers or dates. Ignore "
           f"pure formatting or whitespace changes.{f}\n\nDIFF:\n")
    groups = group_records([("", ln) for ln in diff], CHUNK_CHARS)
    notes = [chat(ask + "\n".join(ln for _, ln in g)) for g in groups]
    summary = notes[0] if len(notes) == 1 else chat(
        "Merge these notes on the changes in one document into one clear summary.\n\n"
        + "\n\n".join(notes))
    return f"{added} added / {removed} removed lines or sentences.\n\n{summary}"


def file_sample(p: Path) -> tuple[str, str | bytes | None]:
    """What the model sees of a file for naming: text excerpt and/or an image."""
    ext = p.suffix.lower()
    if ext in IMAGE_EXTS:
        return "(the image is attached)", str(p)
    if ext == ".pdf":
        with pymupdf.open(p) as doc:
            if not len(doc):
                return "(empty PDF)", None
            text = doc[0].get_text()
            if len(text.strip()) > 50:
                return (f"PDF, {len(doc)} pages, title metadata: "
                        f"{doc.metadata.get('title') or '-'}\nFirst page text:\n{text[:2500]}"), None
            return (f"Scanned PDF, {len(doc)} pages; first page attached as image",
                    doc[0].get_pixmap(dpi=100).tobytes("png"))
    if ext in TEXT_EXTS:
        return p.read_text(encoding="utf-8", errors="replace")[:2500], None
    return f"(binary file, {p.stat().st_size // 1024} KB; no content preview)", None


def clean_name(s) -> str:
    return re.sub(r'[<>:"/\\|?*\x00-\x1f]', "", str(s or "")).strip(" .")[:80]


@mcp.tool()
def local_rename_suggest(folder: str, output_file: str, pattern: str = "*",
                         instructions: str = "") -> str:
    """Suggest clear file names and category subfolders for messy files (e.g.
    Downloads), from their content: PDF first page (scans via vision), images via
    vision, text excerpts. Writes a plan table to output_file (Markdown) and renames
    NOTHING; show the plan to the user and apply only what they approve.
    instructions can set the naming style or the folder set to use."""
    files = sorted(Path(f) for f in glob.glob(str(Path(folder) / pattern), recursive=True)
                   if Path(f).is_file())
    if not files:
        return f"No files match {pattern} in {folder}"
    style = instructions or ("Style: concise and descriptive, at most 60 characters, words "
                             "separated by spaces; papers/books as 'Author Year Title'; add a "
                             "date (YYYY-MM-DD) when the document shows one.")
    # Pass 1: name each file from its content.
    names, about = [], []
    for p in files:
        try:
            sample, img = file_sample(p)
        except Exception as e:
            sample, img = f"(could not read: {e})", None
        d = parse_json(chat(
            f"Suggest a better file name (without extension) for this file. {style} If the "
            f"current name is already good, keep it. Reply as JSON: {{\"name\": \"...\", "
            f"\"about\": \"what the file is and its subject, in a few words\"}}"
            f"\n\nCurrent name: {p.name}\n\nCONTENT:\n{sample}",
            json_mode=True, images=[img] if img else None))
        names.append(clean_name(d.get("name")) or p.stem)
        about.append(str(d.get("about", "")).replace("\n", " ")[:150])
    # Pass 2: choose folders seeing all files at once, so similar files land together.
    subs = [""] * len(files)
    for s in range(0, len(files), 80):
        listing = "\n".join(f"{i}. {names[i]} -- {about[i]}"
                            for i in range(s, min(s + 80, len(files))))
        d = parse_json(chat(
            f"Group these files into a few subfolders by topic or purpose (e.g. a course, "
            f"a project, a research field, receipts). Use broad, clear folder names; files "
            f"on different topics go to different folders. {instructions}\nReply as JSON "
            f"mapping each number to a folder name: {{\"{s}\": \"...\", ...}}\n\n{listing}",
            json_mode=True))
        for k, v in d.items():
            if str(k).isdigit() and int(k) < len(files):
                subs[int(k)] = clean_name(v)
    rows, used = [], set()
    cell = lambda s: str(s).replace("|", "/").replace("\n", " ")
    for p, name, sub, ab in zip(files, names, subs, about):
        new, n = name + p.suffix, 2
        while (sub.lower(), new.lower()) in used:
            new, n = f"{name} ({n}){p.suffix}", n + 1
        used.add((sub.lower(), new.lower()))
        rows.append(f"| {cell(p.name)} | {cell(new)} | {cell(sub)} | {cell(ab)} |")
    Path(output_file).write_text(
        f"# Rename plan for {folder}\n\nNothing has been renamed. Suggested by the local "
        f"model; review before applying.\n\n| Current | Suggested name | Folder | Reason |\n"
        f"|---|---|---|---|\n" + "\n".join(rows) + "\n", encoding="utf-8")
    return f"{len(files)} files -> plan in {output_file}. Nothing was renamed or moved."


# ---------- Video and audio ----------

def run(cmd: list[str], timeout: float | None = None) -> subprocess.CompletedProcess:
    """Run a helper program; stdin closed so it can't read the MCP stdio stream."""
    return subprocess.run(cmd, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                          encoding="utf-8", errors="replace", timeout=timeout,
                          creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))


def hms(t: float) -> str:
    t = int(t)
    return f"{t // 3600}:{t // 60 % 60:02d}:{t % 60:02d}" if t >= 3600 else f"{t // 60:02d}:{t % 60:02d}"


def probe(path: str) -> dict:
    r = run(["ffprobe", "-v", "error", "-print_format", "json", "-show_format",
             "-show_streams", "-show_chapters", str(path)])
    if r.returncode:
        raise ValueError(f"ffprobe can't read {path}: {r.stderr.strip()[:300]}")
    d = json.loads(r.stdout)
    d["duration"] = float(d.get("format", {}).get("duration") or 0)
    d["has_video"] = any(s.get("codec_type") == "video"
                         and not s.get("disposition", {}).get("attached_pic")
                         for s in d.get("streams", []))
    d["has_audio"] = any(s.get("codec_type") == "audio" for s in d.get("streams", []))
    return d


def media_files(path_or_glob: str) -> list[str]:
    if Path(path_or_glob).is_file():
        return [path_or_glob]
    return sorted(f for f in glob.glob(path_or_glob, recursive=True)
                  if Path(f).suffix.lower() in MEDIA_EXTS)


def unload_ollama():
    """Free VRAM held by Ollama models (Whisper and qwen3.5:9b don't both fit in 8 GB)."""
    try:
        for m in httpx.get(f"{HOST}/api/ps", timeout=10).json().get("models", []):
            httpx.post(f"{HOST}/api/generate", json={"model": m["name"], "keep_alive": 0},
                       timeout=60)
    except Exception:
        pass


def whisper(files: list[str], language: str = "", task: str = "transcribe") -> list[dict]:
    """Transcribe files in whisper_worker.py (GPU; files left after a crash retried on CPU)."""
    unload_ollama()
    tmp = INDEX_DIR.parent / "_whisper"
    tmp.mkdir(exist_ok=True)
    job_p, out_p = tmp / "job.json", tmp / "result.jsonl"
    results, todo = {}, list(files)
    for device in ("cuda", "cpu"):
        out_p.unlink(missing_ok=True)
        job_p.write_text(json.dumps({"files": todo, "model": WHISPER_MODEL, "language": language,
                                     "task": task, "device": device, "out": str(out_p)}),
                         encoding="utf-8")
        r = run([sys.executable, str(Path(__file__).parent / "whisper_worker.py"), str(job_p)])
        if out_p.exists():
            for ln in out_p.read_text(encoding="utf-8").splitlines():
                d = json.loads(ln)
                d["device"] = device
                results[d["file"]] = d
        todo = [f for f in todo if f not in results]
        if not todo:
            break
        err = " | ".join(r.stderr.strip().splitlines()[-2:]) or f"exit code {r.returncode}"
    for f in todo:
        results[f] = {"file": f, "error": f"whisper failed: {err}"}
    return [results[f] for f in files]


def paragraphs(segs: list, span: float = 30.0) -> list[tuple[float, str]]:
    """Merge Whisper segments into ~span-second paragraphs (new one after a 3 s pause)."""
    out, buf, start, last = [], [], 0.0, 0.0
    for a, b, t in segs:
        if buf and (a - start >= span or a - last > 3):
            out.append((start, " ".join(buf)))
            buf = []
        if not buf:
            start = a
        buf.append(t)
        last = b
    if buf:
        out.append((start, " ".join(buf)))
    return out


def srt_time(t: float) -> str:
    ms = int(round(t * 1000))
    return f"{ms // 3600000:02d}:{ms // 60000 % 60:02d}:{ms // 1000 % 60:02d},{ms % 1000:03d}"


@mcp.tool()
def local_video_info(path: str) -> str:
    """Quick facts about a video or audio file (no model, instant): duration, size,
    resolution, fps, codecs, audio tracks and languages, embedded subtitle tracks,
    chapters. Use first to decide how to handle a video."""
    d = probe(path)
    f = d.get("format", {})
    lines = [f"{Path(path).name}: {hms(d['duration'])} ({d['duration']:.1f} s), "
             f"{int(f.get('size', 0)) / 1e6:.1f} MB, {f.get('format_long_name', '?')}"]
    for s in d.get("streams", []):
        kind, tags = s.get("codec_type"), s.get("tags", {})
        extra = " ".join(x for x in (tags.get("language", ""), tags.get("title", "")) if x)
        if kind == "video" and s.get("disposition", {}).get("attached_pic"):
            lines.append(f"- cover image ({s.get('codec_name')})")
        elif kind == "video":
            num, den = (s.get("avg_frame_rate") or "0/1").split("/")
            fps = float(num) / float(den) if float(den) else 0
            lines.append(f"- video #{s['index']}: {s.get('codec_name')} {s.get('width')}x"
                         f"{s.get('height')}, {fps:.2f} fps")
        elif kind == "audio":
            lines.append(f"- audio #{s['index']}: {s.get('codec_name')}, {s.get('channels')} ch, "
                         f"{s.get('sample_rate')} Hz {extra}".rstrip())
        elif kind == "subtitle":
            lines.append(f"- subtitles #{s['index']}: {s.get('codec_name')} {extra}".rstrip())
    for c in d.get("chapters", []):
        lines.append(f"- chapter {hms(float(c['start_time']))} {c.get('tags', {}).get('title', '')}")
    return "\n".join(lines)


@mcp.tool()
def local_transcribe(path_or_glob: str, output_file: str = "", language: str = "",
                     translate_to_english: bool = False) -> str:
    """Transcribe speech in video or audio files (mp4, mkv, webm, mp3, wav, m4a, ...)
    with Whisper on the user's GPU (free). language e.g. "id" or "en" (default:
    auto-detect). Writes the transcript to a file and returns only a short status
    (saves Claude tokens); then use local_summarize, local_ask_file or local_search on
    it, or read just the part needed. output_file ending: .md = paragraphs with
    [mm:ss] times (default), .srt = subtitles, .txt = plain text. Default: next to the
    media as <name>.transcript.md. For a glob, output_file is a folder. Names and
    technical terms may be misheard. Unloads the Ollama model while it runs."""
    files = media_files(path_or_glob)
    if not files:
        return f"No audio/video files match {path_or_glob}"
    if len(files) > 1 and output_file:
        Path(output_file).mkdir(parents=True, exist_ok=True)
    report, run_files = [], []
    for fp in files:
        try:
            if probe(fp)["has_audio"]:
                run_files.append(fp)
            else:
                report.append(f"{Path(fp).name}: no audio track")
        except ValueError as e:
            report.append(f"{Path(fp).name}: {e}")
    for d in whisper(run_files, language, "translate" if translate_to_english else "transcribe"):
        p = Path(d["file"])
        if "error" in d:
            report.append(f"{p.name}: ERROR {d['error']}")
            continue
        if len(files) > 1:
            out = Path(output_file or p.parent) / f"{p.stem}.transcript.md"
        else:
            out = Path(output_file or p.with_name(f"{p.stem}.transcript.md"))
        segs = d["segments"]
        if out.suffix.lower() == ".srt":
            text = "\n".join(f"{i}\n{srt_time(a)} --> {srt_time(b)}\n{t}\n"
                             for i, (a, b, t) in enumerate(segs, 1))
        elif out.suffix.lower() == ".txt":
            text = "\n".join(t for _, _, t in segs)
        else:
            text = (f"# Transcript: {p.name}\n\nLanguage {d['language']} ({d['prob']:.2f}), "
                    f"{hms(d['duration'])}, Whisper {WHISPER_MODEL}. Machine transcript: names "
                    f"and technical terms may be misheard.\n\n"
                    + "\n\n".join(f"[{hms(a)}] {t}" for a, t in paragraphs(segs)))
        out.write_text(text + "\n", encoding="utf-8")
        words = sum(len(t.split()) for _, _, t in segs)
        report.append(f"{p.name}: {hms(d['duration'])}, language {d['language']} "
                      f"({d['prob']:.2f}), {words} words -> {out}"
                      + (" (CPU: GPU run failed)" if d["device"] == "cpu" else ""))
    return "\n".join(report)


def scene_times(path: str, start: float, end: float, threshold: float = 0.3) -> list[float]:
    """Times (s) where the picture changes a lot: cuts, new slides."""
    rng = (["-ss", str(start)] if start else []) + (["-to", str(end)] if end else [])
    r = run(["ffmpeg", "-hide_banner", "-nostats", *rng, "-i", str(path), "-an", "-sn", "-dn",
             "-vf", f"scale=160:-2,select='gt(scene,{threshold})',showinfo", "-f", "null", "-"])
    return [start + float(t) for t in re.findall(r"pts_time:\s*([\d.]+)", r.stderr)]


def pick_times(start: float, end: float, scenes: list[float], n: int) -> list[float]:
    """Up to n frame times: the start, scene changes, and fill-ins so no gap > (end-start)/n."""
    gap = (end - start) / n
    pts = sorted({start, *[s for s in scenes if start < s < end - 0.5]})
    times = []
    for a, b in zip(pts, pts[1:] + [end]):
        k = int((b - a) // gap)
        times += [a + (b - a) * j / (k + 1) for j in range(k + 1)]
    if len(times) > n:
        times = [times[i] for i in sorted(set(np.linspace(0, len(times) - 1, n).round().astype(int)))]
    # a moment after the cut, so the transition has finished
    return [min(t + 0.4, max(end - 0.2, start)) for t in times]


def contact_sheets(frames: list[tuple[float, str]], out_dir: Path, per_sheet: int = 16) -> list[str]:
    """Grids of numbered, time-stamped thumbnails (one PNG per per_sheet frames)."""
    pix = pymupdf.Pixmap(frames[0][1])
    ar = pix.height / pix.width
    cols, cw = (4, 400) if ar <= 1 else (6, 260)
    ch, lab, pad = cw * ar, 18, 6
    sheets = []
    for s in range(0, len(frames), per_sheet):
        part = frames[s:s + per_sheet]
        rows = -(-len(part) // cols)
        doc = pymupdf.open()
        page = doc.new_page(width=cols * (cw + pad) + pad, height=rows * (ch + lab + pad) + pad)
        for i, (t, png) in enumerate(part):
            r, c = divmod(i, cols)
            x, y = pad + c * (cw + pad), pad + r * (ch + lab + pad)
            page.insert_text((x, y + 14), f"{s + i + 1}.  {hms(t)}", fontsize=14)
            page.insert_image(pymupdf.Rect(x, y + lab, x + cw, y + lab + ch), filename=png)
        sheet = out_dir / f"sheet_{s // per_sheet + 1}.png"
        page.get_pixmap(dpi=72).save(str(sheet))
        sheets.append(str(sheet))
    return sheets


def grab_frames(path: str, out_dir: str, max_frames: int, start: float, end: float,
                width: int) -> tuple[list[tuple[float, str]], list[str], str]:
    """Extract frames at scene changes (+ fill-ins) and contact sheets. Returns
    ([(time, png)], sheets, out_dir)."""
    d = probe(path)
    if not d["has_video"]:
        raise ValueError(f"{Path(path).name} has no video track")
    end = min(end or d["duration"], d["duration"])
    if end <= start:
        raise ValueError(f"empty range: start {start} >= end {end}")
    max_frames = max(1, max_frames)
    p = Path(path)
    od = Path(out_dir or p.with_name(f"{p.stem}_frames"))
    od.mkdir(parents=True, exist_ok=True)
    for old in list(od.glob("frame_*.png")) + list(od.glob("sheet_*.png")):
        old.unlink()
    frames = []
    for i, t in enumerate(pick_times(start, end, scene_times(path, start, end), max_frames), 1):
        png = od / f"frame_{i:02d}_{hms(t).replace(':', '-')}.png"
        run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-ss", f"{t:.2f}", "-i", str(path),
             "-frames:v", "1", "-vf", f"scale='min({width},iw)':-2", "-y", str(png)])
        if png.exists():
            frames.append((t, str(png)))
    if not frames:
        raise ValueError(f"ffmpeg extracted no frames from {path}")
    return frames, contact_sheets(frames, od), str(od)


@mcp.tool()
def local_video_frames(path: str, output_dir: str = "", max_frames: int = 16,
                       start: float = 0, end: float = 0, width: int = 1280) -> str:
    """Pull still frames out of a video so Claude can see it cheaply (no model; ~1 min
    per hour of video). Frames are taken at scene changes (cuts, new slides), filled in
    evenly so no long stretch is missed, and laid out on contact sheets (numbered
    thumbnails with timestamps, 16 per sheet). Look at the contact sheet first, then
    open only the full frames needed (e.g. to read slide text or equations). start/end
    in seconds zoom into one part. output_dir default: <video>_frames next to the
    video (old frames there are replaced)."""
    try:
        frames, sheets, od = grab_frames(path, output_dir, max_frames, start, end, width)
    except ValueError as e:
        return str(e)
    return (f"{len(frames)} frames -> {od}\nContact sheet(s), view first: " + ", ".join(sheets)
            + "\nFrames (number time): " + ", ".join(f"{i} {hms(t)}" for i, (t, _) in
                                                    enumerate(frames, 1))
            + "\nFile names: frame_NN_<mm-ss>.png")


@mcp.tool()
def local_describe_video(path: str, instruction: str = "", output_file: str = "",
                         max_frames: int = 16, transcribe: bool = True, language: str = "") -> str:
    """Let the local models watch a whole video (free): Whisper transcribes the speech,
    the vision model describes key frames (scene changes) using the speech around
    them as context, and both are merged into a timeline file (output_file, default
    <video>.timeline.md; frames kept in <video>_frames). Returns a short summary.
    instruction says what to look for, e.g. "topics covered and any charts or
    equations shown". Slow: ~6 s per frame plus transcription. Frame descriptions are
    gists; for exact text, numbers or equations view the frames yourself."""
    try:
        frames, _, od = grab_frames(path, "", max_frames, 0, 0, 1280)
    except ValueError as e:
        return str(e)
    speech, note = [], ""
    if transcribe and probe(path)["has_audio"]:
        d = whisper([path], language)[0]
        if "error" in d:
            note = f"\n(No speech: {d['error']})"
        else:
            speech = paragraphs(d["segments"], span=20)
    look = instruction or "what is shown, including any visible text, charts or people"
    events = [(a, f"SPEECH: {t}") for a, t in speech]
    for t, png in frames:
        near = " ".join(s for a, s in speech if t - 25 <= a <= t + 10)[:1500]
        ctx = f"\nSpeech around this moment (context only): {near}" if near else ""
        try:
            desc = chat(f"This is a frame at {hms(t)} of a video. Describe it in 1-3 sentences: "
                        f"{look}.{ctx}", images=[png])
        except Exception as e:
            desc = f"ERROR: {e}"
        events.append((t, f"FRAME {Path(png).name}: {' '.join(desc.split())}"))
    events.sort(key=lambda e: e[0])
    p = Path(path)
    out = Path(output_file or p.with_name(f"{p.stem}.timeline.md"))
    timeline = "\n\n".join(f"[{hms(t)}] {e}" for t, e in events)
    out.write_text(f"# Timeline: {p.name}\n\nMachine-made: speech by Whisper, frames described "
                   f"by {MODEL}. Frames in {od}\n\n{timeline}\n", encoding="utf-8")
    summary = local_summarize(timeline, focus=instruction or "what the video is about and how "
                              "it unfolds", max_words=250)
    return (f"{summary}\n\nTimeline ({len(frames)} frames, {len(speech)} speech paragraphs) "
            f"-> {out}{note}")


@mcp.tool()
def local_status() -> str:
    """Show the local model in use, installed models, and what is loaded in VRAM."""
    try:
        tags = httpx.get(f"{HOST}/api/tags", timeout=10).json()["models"]
        ps = httpx.get(f"{HOST}/api/ps", timeout=10).json()["models"]
    except Exception as e:
        return f"Ollama not reachable at {HOST}: {e}. Start the Ollama app."
    inst = ", ".join(f"{m['name']} ({m['size'] / 1e9:.1f} GB)" for m in tags)
    loaded = ", ".join(f"{m['name']} ({m.get('size_vram', 0) / 1e9:.1f} GB VRAM)"
                       for m in ps) or "none"
    return f"Model in use: {MODEL}, ctx {CTX}\nInstalled: {inst}\nLoaded: {loaded}"


if __name__ == "__main__":
    mcp.run()
