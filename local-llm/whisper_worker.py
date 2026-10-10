"""Transcribe media files with faster-whisper; run by server.py in its own process so
the GPU memory is freed afterwards and a CUDA crash can't take down the MCP server.

    python whisper_worker.py <job.json>

job.json: {"files": [...], "model": "large-v3-turbo", "language": "" | "id", "task":
"transcribe" | "translate", "device": "cuda" | "cpu", "out": "<result.jsonl>"}
Writes one JSON line per file: {"file", "language", "prob", "duration", "segments":
[[start, end, text], ...]} or {"file", "error"}.
"""
import json
import os
import subprocess
import sys
from pathlib import Path

# ctranslate2 bundles libiomp5md.dll, the env's OpenBLAS loads libomp.dll; both in one
# process abort with "OMP: Error #15" unless allowed.
os.environ["KMP_DUPLICATE_LIB_OK"] = "TRUE"
import numpy as np  # noqa: E402

job = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
# CUDA 12 cuBLAS/cuDNN DLLs come from the nvidia-* pip wheels.
for b in Path(sys.prefix, "Lib", "site-packages", "nvidia").glob("*/bin"):
    os.add_dll_directory(str(b))
    os.environ["PATH"] = str(b) + os.pathsep + os.environ["PATH"]

from faster_whisper import WhisperModel  # noqa: E402


def load_audio(path: str) -> np.ndarray:
    """16 kHz mono float32 via ffmpeg (faster-whisper's own PyAV decoder breaks on av>=15)."""
    r = subprocess.run(["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-i", path,
                        "-vn", "-ac", "1", "-ar", "16000", "-f", "f32le", "-"],
                       stdin=subprocess.DEVNULL, capture_output=True)
    if r.returncode:
        raise RuntimeError(r.stderr.decode(errors="replace").strip()[-300:])
    return np.frombuffer(r.stdout, np.float32)


cuda = job["device"] == "cuda"
model = WhisperModel(job["model"], device=job["device"],
                     compute_type="int8_float16" if cuda else "int8")
with open(job["out"], "w", encoding="utf-8") as out:
    for f in job["files"]:
        try:
            segs, info = model.transcribe(load_audio(f), language=job["language"] or None, task=job["task"],
                                          vad_filter=True, beam_size=5)
            rec = {"file": f, "language": info.language, "prob": info.language_probability,
                   "duration": info.duration,
                   "segments": [[s.start, s.end, s.text.strip()] for s in segs]}
        except Exception as e:  # keep going on one bad file
            rec = {"file": f, "error": f"{type(e).__name__}: {e}"}
        out.write(json.dumps(rec, ensure_ascii=False) + "\n")
        out.flush()
