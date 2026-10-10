"""Benchmark the local model at a given context size: python bench.py [ctx]"""
import sys

import httpx

ctx = int(sys.argv[1]) if len(sys.argv) > 1 else 32768
text = open(r"C:\Users\thurs\tools\pdf2md\README.md", encoding="utf-8").read()
text = (text * (ctx * 3 // len(text) + 1))[: int(ctx * 2.5)]  # ~ctx*0.7 tokens
r = httpx.post("http://127.0.0.1:11434/api/chat", timeout=900, json={
    "model": "qwen3.5:9b", "stream": False, "think": False,
    "options": {"num_ctx": ctx},
    "messages": [{"role": "user", "content": "Summarize in 2 sentences:\n\n" + text}]}).json()
print(f"ctx {ctx}: prompt {r['prompt_eval_count']} tok, read "
      f"{r['prompt_eval_count'] / (r['prompt_eval_duration'] / 1e9):.0f} tok/s, write "
      f"{r['eval_count'] / (r['eval_duration'] / 1e9):.1f} tok/s")
for m in httpx.get("http://127.0.0.1:11434/api/ps").json()["models"]:
    print(f"VRAM {m['size_vram'] / 1e9:.1f}/{m['size'] / 1e9:.1f} GB")
