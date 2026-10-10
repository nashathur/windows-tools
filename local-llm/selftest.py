"""Quick check that each tool works: python selftest.py"""
import os
import subprocess
import time

import server

# 10 s test video (colour bars + tone) for the video tools; no speech, so no transcribe test.
VIDEO = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_whisper", "selftest.mp4")
os.makedirs(os.path.dirname(VIDEO), exist_ok=True)
subprocess.run(["ffmpeg", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc=duration=10:size=640x360",
                "-f", "lavfi", "-i", "sine=duration=10", "-shortest", "-y", VIDEO],
               stdin=subprocess.DEVNULL, check=True)

for name, call in [
    ("status", lambda: server.local_status()),
    ("ask", lambda: server.local_ask("Give one sentence on what ENSO is.")),
    ("summarize", lambda: server.local_summarize(
        r"C:\Users\thurs\tools\pdf2md\README.md", max_words=60)),
    ("classify", lambda: server.local_classify(
        ["Rainfall 120 mm in 24 h at Station A", "Invoice #443 due Friday"],
        ["weather", "finance"])),
    ("extract", lambda: server.local_extract(
        "Station Kemayoran recorded 34.2 °C on 3 Oct 2026.", "station, date, max_temp_c")),
    ("ask_file", lambda: server.local_ask_file(
        r"C:\Users\thurs\tools\pdf2md\README.md", "Which script applies review fixes?")),
    ("translate", lambda: server.local_translate(
        "Hujan lebat berpotensi terjadi di Jawa Barat.", "English")),
    ("compare", lambda: server.local_compare(
        "Threshold: 50 mm/day.\nIssued daily.", "Threshold: 100 mm/day.\nIssued daily.")),
    ("search", lambda: server.local_search(
        "how to apply corrections", r"C:\Users\thurs\tools\pdf2md", k=2, pattern="*.md")),
    ("video_info", lambda: server.local_video_info(VIDEO)),
    ("video_frames", lambda: server.local_video_frames(VIDEO, max_frames=4)),
]:
    t = time.time()
    print(f"--- {name} ({{:.1f}} s)".format(0), end="")
    out = call()
    print(f"\r--- {name} ({time.time() - t:.1f} s)\n{out}\n")
