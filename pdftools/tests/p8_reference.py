"""Windows-only pdf2md reference regression (the script is intentionally not unittest-collected)."""
from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BASELINE = Path(r"C:\p8ref")
PDF = Path(r"C:\Users\thurs\Downloads\introduction to gfd (CRB Full).pdf")
PLAN_LINE = re.compile(r"^\s*[＊*]?\s*(\d+)\.\s*(.*?)\s+PDF\s+p(\d+)[-–](\d+)\s+\((\d+)\s+pp?\)\s*$")


def plan_rows(text):
    rows = []
    for line in text.splitlines():
        match = PLAN_LINE.match(line)
        if match:
            n, title, first, last, _ = match.groups()
            rows.append((int(n), " ".join(title.split()), int(first), int(last)))
    return rows


def box_close(left, right):
    return len(left) == len(right) == 4 and all(abs(float(a) - float(b)) <= 0.5 for a, b in zip(left, right))


def inside_picture(issue, page_data):
    page = next((p for p in page_data.get("pages", []) if int(p["id"].split("/")[2]) + 1 == issue.get("page")), None)
    if not page:
        return False
    x0, top, x1, bottom = issue.get("box", (0, 0, 0, 0))
    return any(child.get("block_type") in {"Picture", "Figure", "Diagram"}
               and child.get("bbox", [1, 1, 0, 0])[0] <= x0 and child.get("bbox", [1, 1, 0, 0])[1] <= top
               and child.get("bbox", [1, 1, 0, 0])[2] >= x1 and child.get("bbox", [1, 1, 0, 0])[3] >= bottom
               for child in page.get("children", []))


def compare_part(name, baseline_folder, actual_folder):
    differences = []
    try:
        expected_eqs = json.loads((baseline_folder / "eqs.json").read_text(encoding="utf-8"))
        actual_eqs = json.loads((actual_folder / "eqs.json").read_text(encoding="utf-8"))
    except Exception as exc:
        return [f"{name}: could not read equation files: {exc}"]
    old = {e["id"]: e for e in expected_eqs}
    new = {e["id"]: e for e in actual_eqs}
    if set(old) != set(new):
        differences.append(f"{name}: equation ids differ (baseline {len(old)}, actual {len(new)})")
    for eid in sorted(set(old) & set(new)):
        a, b = old[eid], new[eid]
        if not box_close(a.get("box", []), b.get("box", [])):
            differences.append(f"{name} {eid}: box differs (baseline {a.get('box')}, actual {b.get('box')})")
        try:
            import numpy as np
            from PIL import Image
            with Image.open(baseline_folder / a["crop"]) as expected_image, \
                 Image.open(actual_folder / b["crop"]) as actual_image:
                expected_pixels = expected_image.convert("RGB")
                actual_pixels = actual_image.convert("RGB")
                if actual_pixels.size != expected_pixels.size:
                    # Crops are only ever downscaled from pdf2md's 200 dpi crop: shrink the baseline to the new size.
                    # Enlarging the small crop instead blurs it and reports differences that are not there.
                    expected_pixels = expected_pixels.resize(actual_pixels.size, Image.LANCZOS)
                difference = float(np.abs(np.asarray(expected_pixels, dtype=np.float32) -
                                          np.asarray(actual_pixels, dtype=np.float32)).mean())
            if difference > 1:
                differences.append(f"{name} {eid}: crop pixels differ (mean absolute difference {difference:.3f}/255)")
        except Exception as exc:
            differences.append(f"{name} {eid}: could not compare crop pixels: {exc}")
    try:
        old_qa = json.loads((baseline_folder / "qa_report.json").read_text(encoding="utf-8"))
        new_qa = json.loads((actual_folder / "qa_report.json").read_text(encoding="utf-8"))
    except Exception as exc:
        return differences + [f"{name}: could not read QA reports: {exc}"]
    old_counts, new_counts = old_qa.get("counts", {}), new_qa.get("counts", {})
    if new_counts.get("high", 0) > old_counts.get("high", 0):
        differences.append(f"{name}: QA high count increased ({old_counts.get('high', 0)} -> {new_counts.get('high', 0)})")
    old_lost = [i for i in old_qa.get("issues", [])
                if "PDF text missing from Marker" in i.get("reason", "")]
    actual_pages = json.loads(next(actual_folder.glob("*.pages.json")).read_text(encoding="utf-8"))
    new_lost = [i for i in new_qa.get("issues", []) if "PDF text missing from Marker" in i.get("reason", "")
                and not inside_picture(i, actual_pages)]
    if len(new_lost) > len(old_lost):
        differences.append(f"{name}: lost-text issues increased ({len(old_lost)} -> {len(new_lost)})")
    return differences


def main():
    plan_file = BASELINE / "plan.txt"
    if not plan_file.is_file():
        print(f"Reference regression unavailable: baseline plan is absent ({plan_file}).")
        return 0
    if not PDF.is_file():
        print(f"Reference regression unavailable: reference PDF is absent ({PDF}).")
        return 0
    baseline_plan = plan_rows(plan_file.read_text(encoding="utf-8"))
    if not baseline_plan:
        print(f"Reference regression baseline has no parseable plan lines: {plan_file}.")
        return 1
    env = os.environ.copy()
    env.setdefault("NODE_PATH", str(ROOT / "node_modules"))
    plan = subprocess.run([sys.executable, str(ROOT / "pdf.py"), "--json", "md", str(PDF), "--list", "--level", "auto"],
                          cwd=ROOT, env=env, text=True, capture_output=True)
    if plan.returncode:
        print(f"md --list failed ({plan.returncode}): {plan.stderr.strip()}")
        return plan.returncode
    current_plan = plan_rows(json.loads(plan.stdout).get("data", ""))
    differences = []
    if current_plan != baseline_plan:
        differences.append(f"plan differs: baseline {len(baseline_plan)} rows, actual {len(current_plan)} rows")
    with tempfile.TemporaryDirectory(prefix="p8-reference-") as temp:
        output = Path(temp) / "crb_md"
        run = subprocess.run([sys.executable, str(ROOT / "pdf.py"), "--json", "md", str(PDF), "-o", str(output),
                              "--parts", "2,3", "--no-read"], cwd=ROOT, env=env, text=True, capture_output=True)
        if run.returncode not in (0, 4):
            print(f"md --parts 2,3 --no-read failed ({run.returncode}): {run.stderr.strip()}")
            return run.returncode
        for n, folder_name in ((2, "02_the_coriolis_force"), (3, "03_equations_of_fluid_motion")):
            baseline_folder = BASELINE / "crb" / folder_name
            actual = next((p for p in output.iterdir() if p.is_dir() and p.name.startswith(f"{n:02d}_")), None)
            if not baseline_folder.is_dir():
                differences.append(f"{folder_name}: baseline folder is absent ({baseline_folder})")
            elif actual is None:
                differences.append(f"{folder_name}: converted part folder is absent")
            else:
                differences.extend(compare_part(folder_name, baseline_folder, actual))
    for difference in differences:
        print(difference)
    if differences:
        return 1
    print("Reference regression passed: plan, equation ids/boxes/crops, QA high counts, and lost-text counts match.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
