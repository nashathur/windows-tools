import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1].parent / "common"))
import clierr

PART = re.compile(r"^p(\d+)([a-z]?)\.md$")
DEFAULT = Path(r"C:\Users\thurs\.claude\skills\pdftools\SKILL.md")


def phase_key(path):
    match = PART.match(path.name)
    return int(match.group(1)), match.group(2)


def assemble(parts_dir):
    paths = list(Path(parts_dir).glob("*.md"))
    malformed = [path for path in paths if not PART.match(path.name)]
    if malformed:
        raise clierr.Fail("BAD_INPUT", f"malformed skill part name: {malformed[0].name}", "use p<number><phase-letter>.md")
    paths.sort(key=phase_key)
    required = {"p0.md", "p1.md", "p2.md"}
    missing = sorted(required - {path.name for path in paths})
    if missing:
        raise clierr.Fail("BAD_INPUT", f"missing skill part: {missing[0]}", "restore the missing phase part")
    for path in paths:
        if not PART.match(path.name):
            raise clierr.Fail("BAD_INPUT", f"malformed skill part name: {path.name}", "use p<number><phase-letter>.md")
        text = path.read_text(encoding="utf-8").strip()
        if not text or (path.name == "p0.md" and
                        (not text.startswith("---\n") or "\n---\n" not in text or "name:" not in text or "description:" not in text)):
            raise clierr.Fail("BAD_INPUT", f"empty or malformed skill part: {path.name}", "add valid Markdown and p0 frontmatter")
        if path.name != "p0.md" and not text.startswith("## "):
            raise clierr.Fail("BAD_INPUT", f"skill part lacks a section heading: {path.name}", "start the part with a level-two heading")
    return "\n\n".join(path.read_text(encoding="utf-8").strip() for path in paths) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(allow_abbrev=False)
    parser.add_argument("--out", type=Path, default=DEFAULT)
    args = parser.parse_args(argv)
    try:
        content = assemble(Path(__file__).resolve().parents[1] / "skill_parts")
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(content, encoding="utf-8")
    except clierr.Fail as exc:
        return clierr.fail("pdftools", exc, display_tool="pdf build_skill")
    except (OSError, UnicodeError) as exc:
        fail = clierr.Fail("BAD_INPUT", f"cannot assemble skill: {exc}", "check the skill parts and output path")
        return clierr.fail("pdftools", fail, display_tool="pdf build_skill")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
