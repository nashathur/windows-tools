"""Word alignment shared by source checks and PDF-to-Markdown QA."""

import difflib
import re


def norm_word(word):
    return re.sub(r"[^\w'-]", "", word, flags=re.UNICODE).casefold()


PROSE_WORD = re.compile(r"[A-Za-z]+(?:-[ \t]*\r?\n[ \t]*[A-Za-z]+)?")
OPERATOR_NAMES = {"sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh", "tanh",
                  "coth", "exp", "log", "ln", "lg", "max", "min", "sup", "inf", "lim", "det", "dim", "ker", "arg",
                  "deg", "gcd", "Pr", "mod", "bmod", "pmod", "limsup", "liminf"}


def prose_words(text):
    """Return QA prose tokens with offsets, joining words hyphenated at line ends."""
    out = []
    for match in PROSE_WORD.finditer(text):
        word = re.sub(r"-\s*", "", match.group()).lower()
        if len(word) >= 3 and word not in OPERATOR_NAMES:
            out.append((word, match.start(), match.end()))
    return out


def align(source_words, target_words):
    """Align words in order, recovering long tokens split across target words."""
    matcher = difflib.SequenceMatcher(None, source_words, target_words, autojunk=False)
    matched, consumed = set(), set()
    blocks = matcher.get_matching_blocks()
    for a, b, size in blocks:
        matched.update(range(a, a + size))
        consumed.update(range(b, b + size))
    aligned = [(a, b, size) for a, b, size in blocks if size]
    for position, token in enumerate(source_words):
        if position in matched or len(token) < 12:
            continue
        previous = [(a + size, b + size) for a, b, size in aligned if a + size <= position]
        following = [(a, b) for a, b, size in aligned if a > position]
        source_floor, pdf_floor = max(previous, default=(0, 0), key=lambda pair: pair[0])
        source_ceiling, pdf_ceiling = min(following, default=(len(source_words), len(target_words)), key=lambda pair: pair[0])
        if not source_floor <= position < source_ceiling:
            continue
        for start in range(pdf_floor, pdf_ceiling):
            if start in consumed:
                continue
            combined, end = "", start
            while end < pdf_ceiling and end not in consumed and len(combined) < len(token):
                combined += target_words[end]
                end += 1
            if combined == token:
                matched.add(position)
                consumed.update(range(start, end))
                break
    return matched, consumed


def missing_runs(pdf_tokens, md_text, min_words=7):
    """Find substantial PDF prose runs absent from Markdown, tolerating reordered columns."""
    plain = [word for word, _, _ in prose_words(md_text)]
    glued_text = re.sub(r"\b([A-Za-z]) +(?=[A-Za-z])", r"\1", md_text)
    glued = [word for word, _, _ in prose_words(glued_text)]
    pdf_words = [norm_word(token[0]) for token in pdf_tokens]
    matched_plain, _ = align(pdf_words, plain)
    matched_glued, _ = align(pdf_words, glued)
    present = matched_plain | matched_glued
    # A short unmatched gap bracketed by matches is treated as present (layout reordering noise).
    for hits in (matched_plain, matched_glued):
        ordered = sorted(hits)
        for left, right in zip(ordered, ordered[1:]):
            if right - left <= 3:
                present.update(range(left + 1, right))

    positions = []
    for seq in (plain, glued):
        pos = {}
        for i, word in enumerate(seq):
            pos.setdefault(word, []).append(i)
        positions.append(pos)
    joined = "".join(plain) + "|" + "".join(glued)

    def triple_present(tri):
        if "".join(tri) in joined:
            return True
        for pos in positions:
            for i in pos.get(tri[0], ()):
                js = [j for j in pos.get(tri[1], ()) if i < j <= i + 3]
                if any(i < j < k <= j + 3 for j in js for k in pos.get(tri[2], ())):
                    return True
        return False

    absent = [i not in present for i in range(len(pdf_tokens))]
    runs, start = [], None
    for i, is_absent in enumerate(absent + [False]):
        if is_absent and start is None:
            start = i
        elif not is_absent and start is not None:
            end = i - 1
            if end - start + 1 >= min_words:
                triples = [pdf_words[j:j + 3] for j in range(start, end - 1)]
                if not all(triple_present(tri) for tri in triples):
                    runs.append((start, end))
            start = None
    return runs
