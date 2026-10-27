import re

import pymupdf


def _chars(page):
    text, boxes, lines = [], [], []
    offset = 0
    for block in page.get_text("rawdict").get("blocks", []):
        for line in block.get("lines", []):
            start = offset
            for span in line.get("spans", []):
                for char in span.get("chars", []):
                    text.append(char["c"])
                    boxes.append(pymupdf.Rect(char["bbox"]))
                    offset += len(char["c"])
            lines.append((start, offset))
            text.append(" ")
            boxes.append(None)
            offset += 1
    return "".join(text), boxes, lines


def _quads_for_span(boxes, start, end):
    groups = []
    group = []
    for box in boxes[start:end]:
        if box is None:
            if group:
                groups.append(group)
                group = []
        else:
            group.append(box)
    if group:
        groups.append(group)
    result = []
    for group in groups:
        rect = pymupdf.Rect(group[0])
        for box in group[1:]:
            rect |= box
        result.append(pymupdf.Quad(rect.tl, rect.tr, rect.bl, rect.br))
    return result


def find_matches(doc, pattern, *, regex=False, pages=None, occurrence=None, flags=0):
    """Find text and return per-page match counts and one quad per matched text line."""
    selected = set(pages) if pages is not None else set(range(doc.page_count))
    wanted = set(occurrence) if isinstance(occurrence, (list, tuple, set)) else ({occurrence} if occurrence is not None else None)
    expression = str(pattern) if regex else r"\s+".join(re.escape(part) for part in re.split(r"\s+", str(pattern).strip()))
    match_flags = flags if regex else flags | re.IGNORECASE
    matches, counts, occurrence_no = [], {}, 0
    for page_no in sorted(selected):
        page = doc[page_no]
        page_matches = []
        text, boxes, _ = _chars(page)
        for match in re.finditer(expression, text, match_flags):
            quads = _quads_for_span(boxes, match.start(), match.end())
            if quads:
                page_matches.append({"page": page_no + 1, "text": match.group(0), "quads": quads})
        counts[page_no + 1] = len(page_matches)
        for item in page_matches:
            occurrence_no += 1
            if wanted is None or occurrence_no in wanted:
                item["occurrence"] = occurrence_no
                matches.append(item)
    return {"matches": matches, "counts": counts, "total": occurrence_no}


literal_matches = find_matches
regex_matches = lambda doc, pattern, **kwargs: find_matches(doc, pattern, regex=True, **kwargs)
