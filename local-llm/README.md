# local-llm

MCP server that lets Claude Code hand bulk, repetitive work to a local model
running on the RTX 3070 (free, no tokens). Claude stays in charge; the local
model does the grunt work.

## Setup (done 2026-10-10)

- **Ollama** 0.40.2, starts at login (Startup folder). Models in `D:\ollama\models`
  (user env var `OLLAMA_MODELS`).
- **Model:** `qwen3.5:9b` (q4_K_M, 6.6 GB). Unloads from VRAM after 5 min idle.
- **Env:** conda `llm` (`C:\Users\thurs\conda-envs\llm.yml`): python 3.12, mcp 2.x, httpx,
  ffmpeg, faster-whisper + CUDA 12 cuBLAS/cuDNN wheels (video tools, added 2026-10-10).
- **Registered** for all projects (user scope, `~/.claude.json`):
  `conda.exe run --no-capture-output -n llm python C:\Users\thurs\tools\local-llm\server.py`

Measured speed (RTX 3070, 8 GB):

| Context | Write | Read | Notes |
|---|---|---|---|
| 12k | 41 tok/s | — | |
| **16k (default)** | 34 tok/s | ~2100 tok/s | best balance |
| 24k | 19 tok/s | ~1950 tok/s | spills to RAM |
| 32k | 15 tok/s | ~1800 tok/s | |
| 64k | 2 tok/s | ~1300 tok/s | unusable |

Don't run alongside Marker (book.py / convert.py): both need the GPU memory.

## Tools

| Tool | Use |
|---|---|
| `local_ask` | any self-contained prompt (`think=True` for slower, careful answers) |
| `local_summarize` | file path or text, any length (chunked + merged) |
| `local_classify` | label a list of items from a fixed label set → JSON lines |
| `local_extract` | pull named fields out of a file/text → JSON |
| `local_batch` | same instruction on every file in a glob → results written to a file |
| `local_describe_image` | describe/transcribe one image, or a glob of images → file (qwen3.5 has vision; ~5 s/image) |
| `local_search` | search a folder (md/txt/pdf) by meaning → best passages with file, line/page, heading. Uses embedding model `qwen3-embedding:0.6b`; index cached in `index\` (one .json + .npy per folder), only changed files re-indexed |
| `local_ask_file` | one question about a long file/PDF → answer + quotes, each matched back to its line (L…) or page (p…); unmatched quotes marked unverified |
| `local_translate` | translate file/PDF/text (e.g. Indonesian ↔ English), keeps Markdown/LaTeX/numbers; `output_file` to save tokens |
| `local_compare` | plain-words summary of what changed between two versions (exact difflib diff; sentences for PDFs, lines otherwise) |
| `local_rename_suggest` | suggested names + subfolders for messy files (PDF first page, vision for images/scans) → plan table; renames nothing |
| `local_video_info` | ffprobe facts: duration, resolution, fps, audio/subtitle tracks + languages, chapters (no model) |
| `local_transcribe` | speech → transcript for video/audio (one file or glob), Whisper `large-v3-turbo` on the GPU; `.md` (paragraphs with [mm:ss]), `.srt` or `.txt`; default `<name>.transcript.md` next to the file; returns only a status |
| `local_video_frames` | frames at scene changes + even fill-ins, and contact sheets (16 numbered, time-stamped thumbnails per PNG) for Claude to view; default `<name>_frames\` next to the video; `start`/`end` to zoom in (no model) |
| `local_describe_video` | transcript + vision descriptions of key frames (with nearby speech as context) → `<name>.timeline.md`, returns a summary |
| `local_status` | model, installed models, VRAM use |
| `local_code_search` | search a code project by meaning; one hit per fn / struct / impl method (tree-sitter), with file:line range; `full=N` returns the code of the top N |
| `local_docs` | search a Rust crate's dependency APIs (windows-sys features enabled in the build, ureq, serde, chrono, …); an exact identifier is looked up in ALL windows-sys features → which feature it needs and whether it's on |
| `local_patch` | small, mechanical edit by the coding model in a scratch git worktree, `cargo check` after each round, errors fed back; returns PASS/FAIL + diff (`D:\lp\last.patch`) |
| `local_widget_check` | screenshot a running widget's windows (docked strip + open panels) → PNGs in `shots\`; pixel diff vs a saved baseline; optional hover/click first; optional vision question |

PDF input works in every tool that takes a path (text layer via PyMuPDF; scanned PDFs have none, so convert them first).

### Video notes

- Whisper runs in `whisper_worker.py`, a separate process: its VRAM is fully freed
  afterwards, and a CUDA crash can't kill the server (the remaining files are retried on
  CPU). Ollama models are unloaded first (Whisper + qwen3.5:9b don't fit in 8 GB
  together). Job/result files go to `_whisper\`.
- Speed (RTX 3070): ~35x real time (10 min of audio in 17 s, incl. ~4 s model load);
  frame extraction ~1 min per hour of video; `local_describe_video` adds ~6 s per frame.
- Audio is decoded by ffmpeg, not faster-whisper's PyAV path (`open()` broke with av 19).
  The worker sets `KMP_DUPLICATE_LIB_OK=TRUE`: ctranslate2's bundled OpenMP clashes with
  the env's, which otherwise aborts with "OMP: Error #15".
- Model: `large-v3-turbo` (multilingual, good for Indonesian), cached in
  `~\.cache\huggingface`. Change with env var `LOCAL_LLM_WHISPER` (e.g. `medium`).
- Online videos (YouTube etc.) aren't downloaded by the server; download first, then
  point the tools at the file.

### Code tools notes (`code_tools.py`, added 2026-10-10)

- `C:\Users\thurs\tools` is one git repo (widgets, taskbar-dock, local-llm, pdf2md), so the
  widgets' `../taskbar-dock` path dependencies also work in a worktree. `.gitignore` skips
  `target\`, `src_backup*`, `*.bak*`, exes, logs.
- **local_patch** uses the coding model `qwen2.5-coder:7b` at 32k context (`LOCAL_LLM_CODE_MODEL`,
  `LOCAL_LLM_CODE_CTX`). Scratch lives in `D:\lp` (`LOCAL_LLM_SCRATCH`): `wt\` (worktree of the
  current working tree, recreated each call at the same path so builds stay incremental),
  `target\` (shared cargo target dir), `sccache\`, `last.patch`. The real files change only
  with `apply=True` (after a clean `git apply --check`). Errors that already existed before
  the edit are ignored. When a taskbar-dock file changes, every widget is checked too.
- **local_docs** reads crate sources via `cargo metadata`. From `windows` it keeps only COM
  interface methods when `windows-sys` is also used (the rest duplicates it); error-code
  constants (HRESULT / WIN32_ERROR / NTSTATUS) are skipped in the meaning search.
- **local_code_search / local_docs** indexes live in `index\` (`code_*`, `docs_*`).
- **local_widget_check** finds strips as children of `Shell_TrayWnd` and panels as top-level
  windows; captures are screen copies at real pixels (per-monitor DPI). `hover`/`click`/`rightclick`
  move the real mouse (put back afterwards). Baselines in `shots\baseline\`.
- Also installed for direct use by Claude: `sccache` and `ast-grep` (`sg`) via winget, both on PATH.
  `ast-grep` does exact structural search/rewrite (e.g. `sg -p 'dock::slot($$$)' -l rust`), better
  than any model when a pattern fits.

## Adding a tool

1. In `server.py`, add a function decorated with `@mcp.tool()`. Call `chat(prompt,
   system="", json_mode=False, think=False)` to talk to the model; `read_text()` and
   `chunks()` help with files.
2. Write a clear docstring: Claude reads it to decide when to use the tool.
3. Test: `conda run -n llm python -c "import server; print(server.my_tool(...))"`
   (or add it to `selftest.py`).
4. Restart Claude Code (new session) to pick it up.

Or just ask Claude: "add a tool to local-llm that …".

## Changing model / context

Set env vars in the server registration, e.g. re-register with
`claude mcp add --scope user local-llm -e LOCAL_LLM_MODEL=qwen3.5:4b -- …`, or
change `DEFAULT_MODEL` / `LOCAL_LLM_CTX` in `server.py`. Pull new models with
`ollama pull <tag>`. `bench.py <ctx>` measures speed at a context size.

## Files

- `server.py`: the MCP server
- `code_tools.py`: the four code tools (registered by server.py)
- `shots\`: local_widget_check screenshots and baselines (safe to delete)
- `whisper_worker.py`: Whisper transcription, run by server.py in its own process
- `_whisper\`: worker job/result files, selftest video (safe to delete)
- `index\`: local_search caches (safe to delete; rebuilt on next search)
- `selftest.py`: runs each tool once
- `bench.py`: speed test at a given context size
- `install_ollama.ps1`, `install.log`: one-off installer (ran via scheduled task,
  outside the Claude app's MSIX sandbox)

## Later: fine-tuning

Only for a narrow task the stock model keeps getting wrong. Plan: QLoRA with
Unsloth on a 4–9B model, overnight run, examples generated from past work, then
import the result into Ollama as a new model tag. Would need PyTorch (CUDA) added
to the `llm` env.
