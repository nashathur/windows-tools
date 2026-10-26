"""Clean up Marker's Markdown: turn equations that Marker rendered as Markdown tables back into display math.

Marker often outputs aligned/numbered equations as tables, e.g.
    | $x$ | $=$ | $+X \\cos \\Omega t$ | $+Y \\sin \\Omega t$ | $(2.3a)$ |
    |-----|-----|-----|-----|-----|
which this rewrites as
    $$x = +X \\cos \\Omega t +Y \\sin \\Omega t \\quad (2.3a)$$

A table is converted only if every non-empty row contains math, no cell is prose, and it either carries an
equation number or every row is a relation (=, <, ≤, …). Each new equation is checked with KaTeX; if any row
fails, the table is left as it was. Real tables (headers, data, prose) are not touched.

convert.py runs this automatically. To run it on an existing file (edits in place):
    conda run -n pdfmath python C:\\Tools\\pdf2md\\postprocess.py chapter.md
"""

import html
import re
import sys

from rapidfuzz import fuzz

from qa import validate_math

COMMENT = re.compile(r"<!--.*?-->")
TABLE_LINE = re.compile(r"^\s*\|.*\|\s*(<!--.*?-->\s*)*$")
SEPARATOR = re.compile(r"^\s*\|[\s\-:|]+\|\s*$")
EQ_NUMBER_CELL = re.compile(r"^\$?\s*\(\s*\d+(?:\.\d+)?[a-z]?\s*\)\s*\$?$")
RELATION = re.compile(r"=|<|>|\\leq?|\\geq?|\\neq?|\\approx|\\sim|\\equiv|\\propto|\\rightarrow|\\to\b")
MAX_TEXT_CELL = 25  # longer plain-text cells mean a real table


def cell_to_latex(cell):
    """'$a$ for $b$' -> 'a \\text{ for } b'. Returns None if the cell is prose or its $ are unbalanced."""
    if cell.count("$") % 2:
        return None
    parts = cell.split("$")
    out = []
    for i, part in enumerate(parts):
        if i % 2:  # inside $...$
            if part.strip():
                out.append(part.strip())
        elif part.strip():
            if len(part.strip()) > MAX_TEXT_CELL:
                return None
            out.append(r"\text{ " + part.strip() + " }")
    return " ".join(out)


def row_to_latex(row):
    cells = [c.strip() for c in row.strip().strip("|").split("|")]
    pieces, number, has_math = [], None, False
    for cell in cells:
        if not cell:
            continue
        if EQ_NUMBER_CELL.match(cell):
            number = cell.strip("$ ")
            continue
        latex = cell_to_latex(cell)
        if latex is None:
            return None
        has_math |= "$" in cell
        # Two equations side by side ("| $j =$ | $…$ | $J =$ | $…$ |"): separate them.
        if pieces and latex.rstrip().endswith("=") and any("=" in p for p in pieces):
            pieces.append(r"\qquad")
        pieces.append(latex)
    if not pieces:
        return ""  # empty row
    if not has_math:
        return None  # text-only row: a real table
    latex = " ".join(pieces)
    return latex + (rf" \quad {number}" if number else ""), number is not None


def convert_table(lines):
    """Return display-math lines for an equation table, or None to keep it. HTML comments
    (e.g. review markers) are kept: they're returned in the first element as a prefix."""
    comments = [c for l in lines for c in COMMENT.findall(l)]
    rows = [COMMENT.sub("", l) for l in lines]
    rows = [l for l in rows if not SEPARATOR.match(l)]
    converted, numbered = [], False
    for row in rows:
        result = row_to_latex(row)
        if result is None:
            return None
        if result == "":
            continue
        latex, has_number = result
        numbered |= has_number
        converted.append(latex)
    if not converted:
        return None
    if not numbered and not all(RELATION.search(c) for c in converted):
        return None
    return converted, comments


def fix_equation_tables(text):
    """Returns (new_text, tables_converted, tables_kept_after_failed_check)."""
    lines = text.split("\n")
    runs, i = [], 0
    while i < len(lines):
        if TABLE_LINE.match(lines[i]):
            j = i
            while j < len(lines) and TABLE_LINE.match(lines[j]):
                j += 1
            runs.append((i, j))
            i = j
        else:
            i += 1

    candidates = []
    for start, end in runs:
        result = convert_table(lines[start:end])
        if result:
            candidates.append((start, end, *result))

    items = [{"id": f"{n}/{k}", "latex": eq, "display": True}
             for n, (_, _, eqs, _) in enumerate(candidates) for k, eq in enumerate(eqs)]
    failed = {item_id.split("/")[0] for item_id in validate_math(items)}

    kept = 0
    for n, (start, end, eqs, comments) in reversed(list(enumerate(candidates))):
        if str(n) in failed:
            kept += 1
            continue
        block = "\n\n".join(f"$${eq}$$" for eq in eqs)
        lines[start:end] = (" ".join(comments) + "\n" + block if comments else block).split("\n")
    return "\n".join(lines), len(candidates) - kept, kept


# ------------------------------------------------------------------------------------------- Markdown clean-up
# Marker's Markdown next to Mathpix's: heading levels follow font size rather than numbering, sentences ending in
# "yields" / "is" before an equation become headings, equation numbers are "\quad (2.3)" instead of \tag, and
# internal anchors (<span id="page-…">) and double spaces around inline math clutter the text.

SPAN_ANCHOR = re.compile(r'<span id="page-\d+-\d+"></span>')
PAGE_LINK = re.compile(r"\[([^\]]*)\]\(#page-\d+-\d+\)")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*$")
CHAPTER = re.compile(r"^(Chapter|Part|Appendix)\s+([0-9IVX]+|[A-Z])\.?$", re.I)
CHAPTER_DONE = re.compile(r"^(Chapter|Part|Appendix)\s+([0-9IVX]+|[A-Z])\.?:\s", re.I)
SUBSECTION = re.compile(r"^(\d+|[A-Z])\.\d+\.\d+\.?\s")
SECTION = re.compile(r"^(\d+|[A-Z])\.\s?\d+\.?\s")
DISPLAY = re.compile(r"\$\$(.+?)\$\$", re.S)
EQ_LABEL_ANY = re.compile(r"\(\s*\d+\.\d+[a-z]?\s*\)")
END_LABEL = re.compile(r"\s*(?:\\q?quad|\\,|\\;|~)?\s*\(\s*(\d+\.\d+[a-z]?)\s*\)\s*$")
INLINE = r"(?<!\$)\$(?!\$)[^$\n]+?(?<!\$)\$(?!\$)"


def heading_text(text):
    return re.sub(r"\*\*|__", "", SPAN_ANCHOR.sub("", text)).strip()


def looks_like_prose(text):
    """A 'heading' that is really a sentence fragment before an equation ("The first time derivative ... yields")."""
    words = text.split()
    return (len(text) < 3 or text[0].islower() or text.rstrip().endswith((":", ",", ".", ";"))
            or re.match(r"^\d+-\d+\.", text) is not None or len(words) > 12
            or (len(words) > 5 and words[-1][0].islower()))


def normalize_headings(md):
    """# Chapter N: Title, ## N.M section, ### N.M.K subsection, ## other headings; prose fragments demoted."""
    lines = md.split("\n")
    out, i = [], 0
    while i < len(lines):
        m = HEADING.match(lines[i])
        if not m:
            out.append(lines[i]); i += 1; continue
        text = heading_text(m.group(2))
        if CHAPTER_DONE.match(text):  # already merged by an earlier run
            out.append(f"# {text}")
        elif CHAPTER.match(text):
            # Merge with the title heading that follows ("# Chapter 2" + "# The Coriolis Force").
            j = i + 1
            while j < len(lines) and not lines[j].strip():
                j += 1
            nxt = HEADING.match(lines[j]) if j < len(lines) else None
            if nxt and not SECTION.match(heading_text(nxt.group(2))):
                out.append(f"# {text}: {heading_text(nxt.group(2))}"); i = j + 1; continue
            out.append(f"# {text}")
        elif SUBSECTION.match(text):
            out.append(f"### {text}")
        elif SECTION.match(text):
            out.append(f"## {text}")
        elif looks_like_prose(text):
            out.append(text)
        else:
            out.append(f"## {text}")
        i += 1
    return "\n".join(out)


def tag_label(latex):
    """'… \\quad (2.31)' -> '… \\tag{2.31}' when the equation carries exactly one number, at its end."""
    if len(EQ_LABEL_ANY.findall(latex)) != 1:
        return latex
    return END_LABEL.sub(lambda m: rf" \tag{{{m.group(1)}}}", latex)


def tag_labels(md):
    """\\tag the numbers of all display equations; an equation whose tagged form fails KaTeX keeps its old form."""
    changed = {}
    for k, m in enumerate(DISPLAY.finditer(md)):
        tagged = tag_label(m.group(1))
        if tagged != m.group(1):
            changed[str(k)] = tagged
    failed = validate_math([{"id": k, "latex": v, "display": True} for k, v in changed.items()])
    counter = iter(range(10 ** 9))

    def sub(m):
        k = str(next(counter))
        return "$$" + changed[k] + "$$" if k in changed and k not in failed else m.group(0)
    return DISPLAY.sub(sub, md)


def tidy_line(line):
    """Marker writes '  $x$  ': one space around inline math, none before ,.;:) or after (."""
    parts = re.split(f"({INLINE})", line)  # odd items are inline math
    if len(parts) == 1:
        return line
    for k in range(1, len(parts), 2):
        before, after = parts[k - 1], parts[k + 1]
        before = before.rstrip(" ") + (" " if before.strip(" ") and not before.rstrip(" ").endswith("(") else "")
        stripped = after.lstrip(" ")
        after = stripped if not stripped or stripped[0] in ",.;:)" else " " + stripped
        parts[k - 1], parts[k + 1] = before, after
    for k in range(2, len(parts) - 1, 2):  # between two formulas: never nothing ("$a$$b$" would read as "$$")
        if not parts[k].strip():
            parts[k] = " "
    return "".join(parts)


def tidy_inline_spacing(md):
    # Line by line, skipping tables (cell padding) and display math; a regex over the whole text backtracks badly
    # on long runs of spaces.
    out, in_display = [], False
    for line in md.split("\n"):
        if line.count("$$") % 2:
            in_display = not in_display
        out.append(line if in_display or "$$" in line or line.lstrip().startswith("|") else tidy_line(line))
    return "\n".join(out)


def html_to_markdown(fragment):
    """Marker's block HTML (paragraphs, inline and display <math>, equation tables, b/i) as Markdown."""
    def table_row(m):
        cells = re.findall(r"<t[dh][^>]*>(.*?)</t[dh]>", m.group(1), flags=re.S)
        maths = [x.strip() for c in cells for x in re.findall(r"<math[^>]*>(.*?)</math>", c, flags=re.S)]
        labels = [l for c in cells for l in EQ_LABEL_ANY.findall(re.sub(r"<[^>]+>", " ", c))]
        if not maths:
            return " ".join(re.sub(r"<[^>]+>", " ", c).strip() for c in cells) + "\n\n"
        return "\n\n$$" + " ".join(maths) + "".join(rf" \quad {l}" for l in labels[:1]) + "$$\n\n"
    t = re.sub(r"<tr[^>]*>(.*?)</tr>", table_row, fragment, flags=re.S)
    t = re.sub(r'<math display="block">(.*?)</math>', lambda m: "\n\n$$" + m.group(1).strip() + "$$\n\n", t, flags=re.S)
    t = re.sub(r"<math>(.*?)</math>", lambda m: "$" + m.group(1).strip() + "$", t, flags=re.S)
    t = re.sub(r"</?b>", "**", t)
    t = re.sub(r"</?i>", "*", t)
    t = re.sub(r"<br\s*/?>|</p>|</h\d>", "\n\n", t)
    t = re.sub(r"<[^>]+>", "", t)
    t = html.unescape(t)
    return re.sub(r"\n{3,}", "\n\n", "\n".join(l.strip() for l in t.split("\n"))).strip()


def unwrap_fake_tables(md):
    """A margin note (C-R&B: "JMB from ↓ / to ↑") makes Marker treat a whole page as a table: the page's
    paragraphs and equations end up as HTML in one huge cell, followed by rows of garbled equation fragments.
    Such a table is replaced by the cell's content as Markdown; a garbage row is kept (as display math) only if
    it carries an equation number the cell doesn't have."""
    lines, out, i = md.split("\n"), [], 0
    while i < len(lines):
        if not TABLE_LINE.match(lines[i]):
            out.append(lines[i]); i += 1; continue
        j = i
        while j < len(lines) and TABLE_LINE.match(lines[j]):
            j += 1
        rows = lines[i:j]
        cells = [c.strip() for r in rows for c in r.strip().strip("|").split("|")]
        # Distinct cells only: Marker repeats the same page HTML across several columns (C-R&B ch. 18: 10 copies).
        prose = list(dict.fromkeys(c for c in cells if len(c) > 300 and re.search(r"<p>|<math display", c)))
        if not prose:
            out += rows; i = j; continue
        body = "\n\n".join(html_to_markdown(c) for c in prose)
        have = set(EQ_LABEL_ANY.findall(body))
        extra = []
        for r in rows:
            if SEPARATOR.match(r) or any(c in r for c in prose):
                continue
            labels = set(EQ_LABEL_ANY.findall(r)) - have
            result = row_to_latex(r) if labels else None
            if result:
                extra.append(f"$${result[0]}$$")
                have |= labels
        out += ["", body, ""] + [x for e in extra for x in (e, "")]
        i = j
    return "\n".join(out)


def compact_tables(md):
    """Marker pads every table cell to the widest one (C-R&B ch. 13: rows of 67,000 characters)."""
    out = []
    for line in md.split("\n"):
        if line.lstrip().startswith("|"):
            line = re.sub(r"-{3,}", "---", re.sub(r" {2,}", " ", line))
        out.append(line)
    return "\n".join(out)


def drop_adjacent_duplicates(md):
    """Neighbouring display equations (only blank lines between) with the same number and near-identical LaTeX:
    Marker output one printed equation twice. The copy with a properly \\tag-ged number is kept."""
    blocks = list(re.finditer(r"\$\$(.+?)\$\$", md, re.S))
    drop = []
    for a, b in zip(blocks, blocks[1:]):
        if md[a.end():b.start()].strip():
            continue
        la, lb = set(re.findall(r"\d+\.\d+[a-z]?", " ".join(EQ_LABEL_ANY.findall(a.group(1)) + re.findall(r"\\tag\{([^}]*)\}", a.group(1))))), \
            set(re.findall(r"\d+\.\d+[a-z]?", " ".join(EQ_LABEL_ANY.findall(b.group(1)) + re.findall(r"\\tag\{([^}]*)\}", b.group(1)))))
        strip = lambda s: re.sub(r"\\tag\{[^}]*\}|\\q?quad|\(\s*\d+\.\d+[a-z]?\s*\)|\s", "", s)
        if la and la == lb and fuzz.ratio(strip(a.group(1)), strip(b.group(1))) >= 85:
            drop.append(b if "\\tag{" in a.group(1) and "\\tag{" not in b.group(1) else a)
    for m in sorted(set(drop), key=lambda m: m.start(), reverse=True):
        md = md[:m.start()] + md[m.end():]
    return md


def drop_repeated_paragraphs(md, min_chars=60):
    """Exact repeats of a paragraph (60+ characters) after its first occurrence: duplicated content, never needed."""
    seen, out = set(), []
    for para in re.split(r"\n{2,}", md):
        key = para.strip()
        if len(key) >= min_chars and key in seen:
            continue
        seen.add(key)
        out.append(para)
    return "\n\n".join(out)


def separate_adjacent_inline_math(md):
    """"$a$$b$" (two inline formulas side by side) reads as a display-math delimiter and unbalances everything after
    it: put a space between them. Display math sits on its own lines, so a "$$" inside a line between two
    non-space characters is always such a pair."""
    return re.sub(r"(?<=[^\s$])\$\$(?=[^\s$])", "$ $", md)


def bold_numbered_headings(md):
    """'**17.3 Baroclinic instability**' on a line of its own (headings from unwrapped fake tables) -> heading."""
    return re.sub(r"^\*\*((?:\d+|[A-Z])\.\d+(?:\.\d+)?\.?\s[^*\n]{2,80})\*\*\s*$", r"## \1", md, flags=re.M)


def drop_frequent_short_lines(md, max_chars=30, min_count=5):
    """Short lines repeated 5+ times in a chapter are margin notes or placeholders (C-R&B draft: "JMB from ↓"),
    never content; also as headings."""
    def key(line):
        text = re.sub(r"^#+\s*", "", line).strip()
        if not text or len(text) > max_chars or text.startswith(("$$", "|")):
            return None
        # Letters only, so "JMB from ↓" and "JMB from $\downarrow$" count as one.
        return re.sub(r"\\[A-Za-z]+|[^A-Za-z]", "", text) or None
    counts = {}
    for line in md.split("\n"):
        k = key(line)
        if k:
            counts[k] = counts.get(k, 0) + 1
    frequent = {k for k, n in counts.items() if n >= min_count}
    return "\n".join(l for l in md.split("\n") if key(l) not in frequent)


def clean_markdown(md):
    md = unwrap_fake_tables(md)
    md = drop_repeated_paragraphs(md)
    md = separate_adjacent_inline_math(md)
    md = bold_numbered_headings(md)
    md = drop_frequent_short_lines(md)
    md = compact_tables(md)
    md = normalize_headings(md)
    md = SPAN_ANCHOR.sub("", md)
    md = PAGE_LINK.sub(r"\1", md)
    md = tag_labels(md)
    md = drop_adjacent_duplicates(md)
    return tidy_inline_spacing(md)


def main():
    for path in sys.argv[1:]:
        with open(path, encoding="utf-8") as f:
            text = f.read()
        new_text, converted, kept = fix_equation_tables(text)
        new_text = clean_markdown(new_text)
        with open(path, "w", encoding="utf-8") as f:
            f.write(new_text)
        print(f"{path}: converted {converted} equation tables to display math"
              + (f", kept {kept} that failed the KaTeX check" if kept else "") + "; headings, labels and spacing tidied")


if __name__ == "__main__":
    main()
