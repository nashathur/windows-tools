"""Chapter planning from PDF outlines, shared by md and split."""

import re

from .errors import PdfError

TARGET_PAGES = 25


def outline(doc):
    return [(int(level), str(title).strip(), int(page) - 1)
            for level, title, page, *_ in doc.get_toc(simple=True) if page and 1 <= page <= doc.page_count]


def letters(text):
    return re.sub(r"[^a-z]", "", text.lower())


def _page_text(doc, index, limit=None):
    text = doc[index].get_text()
    return text[:limit] if limit else text


def fix_starts(doc, chapters):
    fixed = []
    for title, start, end, original in chapters:
        key = letters(title)
        if key and key not in letters(_page_text(doc, start, 300)):
            for index in range(start + 1, end + 1):
                if letters(_page_text(doc, index, 80)).startswith(key):
                    start = index
                    break
        fixed.append((title, start, end, original if start != original else None))
    fixed.sort(key=lambda part: part[1])
    out = []
    for i, (title, start, end, repaired) in enumerate(fixed):
        if i + 1 < len(fixed):
            following, following_original = fixed[i + 1][1], fixed[i + 1][3]
            # A chapter that ended at the next entry's outline page now ends at that entry's repaired page, so
            # the page the outline pointed at stays with this chapter instead of falling out of every part.
            if following_original is not None and end == following_original - 1:
                end = following - 1
            end = min(end, following - 1)
        out.append((title, start, end, repaired))
    return out


def add_uncovered(doc, chapters, items, min_text_pages=2):
    covered = {page for _, start, end, _ in chapters for page in range(start, end + 1)}
    gaps, current = [], []
    for page in range(doc.page_count):
        if page in covered:
            if current: gaps.append(current)
            current = []
        else: current.append(page)
    if current: gaps.append(current)
    out = list(chapters)
    for gap in gaps:
        if sum(len(_page_text(doc, page).strip()) >= 200 for page in gap) < min_text_pages:
            continue
        start, end = gap[0], gap[-1]
        named = [title for _, title, page in items if page == start]
        title = "Front matter" if start == 0 else (named[0] if named else f"Pages {start + 1}-{end + 1}")
        out.append((title, start, end, None))
    return sorted(out, key=lambda part: part[1])


def plan(doc, level=None, chunk=None, cover_all=False):
    count = doc.page_count
    items = outline(doc)
    if chunk:
        parts = [(f"pages {start + 1}-{min(start + chunk, count)}", start,
                  min(start + chunk, count) - 1, None) for start in range(0, count, chunk)]
    else:
        if not items:
            raise PdfError("BAD_INPUT", "This PDF has no outline", "pass --chunk N")
        def chapters_at(chosen):
            result = []
            for index, (entry_level, title, start) in enumerate(items):
                if entry_level != chosen: continue
                later = [page for other_level, _, page in items[index + 1:]
                         if other_level <= chosen and page > start]
                end = min(later) - 1 if later else count - 1
                result.append((title, start, max(start, end), start))
            return result
        levels = sorted({entry[0] for entry in items})
        if level is None:
            def median_length(chosen):
                lengths = sorted(end - start + 1 for _, start, end, _ in chapters_at(chosen))
                return lengths[len(lengths) // 2]
            level = min(levels, key=lambda chosen: abs(median_length(chosen) - TARGET_PAGES))
        parts = fix_starts(doc, chapters_at(level))
        parts = add_uncovered(doc, parts, items)
    notes = [f"outline entry '{title}' points at p{repaired + 1}, but the title is on p{start + 1}"
             for title, start, _, repaired in parts if repaired is not None]
    if cover_all and parts:
        parts = [list(part) for part in parts]
        for page in range(count):
            if any(start <= page <= end for _, start, end, _ in parts): continue
            following = next((part for part in parts if part[1] > page), None)
            if following: following[1] = page
            else: parts[-1][2] = page
        parts = [tuple(part) for part in parts]
    return {"parts": [{"title": title, "start": start, "end": end, "repaired_from": repaired}
                       for title, start, end, repaired in parts], "notes": notes}
