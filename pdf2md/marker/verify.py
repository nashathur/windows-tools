"""Corrections checked against the PDF text layer, so the local GPU does the fixing instead of a reviewer.
Needs a born-digital PDF (pages without a text layer are skipped). Used two ways:
  - convert.py calls correct() on Marker's document before rendering (.md and .blocks.json stay in step);
  - reverify.py applies the same equation corrections to a chapter folder that was converted earlier.

For each display equation:
  1. equation number: Marker truncates "(4.19)" to "(4.1)" or drops it; the PDF prints it next to the equation.
  2. accents: \\bar for \\tilde, \\hat for a ring... when the PDF's accent glyphs say so and that is the only change.
  3. several readings: an equation whose symbols don't match the PDF is read again READINGS times (different crops
     of the PDF, greedy and sampled decoding: Marker's OCR varies, so each reading is a new chance). The reading
     closest to the PDF wins; it must keep the equation number(s) and pass KaTeX, ties keep the current LaTeX.
  4. duplicates: two overlapping equation blocks with the same number and near-identical LaTeX (Marker output one
     printed equation twice): the copy that matches the PDF worse is removed.
For paragraphs (convert.py only): inline accent swaps, and paragraphs that convert.py restored from the PDF text
(OCR had returned only math) are read again from the exact area of their text lines, to get LaTeX inline math back.
"""

import html as htmllib
import re
from collections import Counter

from rapidfuzz import fuzz
from surya.inference import get_default_manager
from surya.inference.parsers import clean_block_html
from surya.inference.prompts import PROMPT_TYPE_BLOCK
from surya.inference.schema import BatchInputItem

from marker.schema import BlockTypes

import qa

READINGS = 5  # readings per mismatching equation (see reading_plan)
MATH_HTML = re.compile(r"(<math\b[^>]*>)(.*?)(</math>)", re.S)
SINGLE_MATH = re.compile(r"\s*(?:<p\b[^>]*>)?\s*<math\b[^>]*>(?:(?!<math\b).)*?</math>\s*(?:</p>)?\s*", re.S)
TEXT_TYPES = (BlockTypes.Text, BlockTypes.TextInlineMath, BlockTypes.Caption, BlockTypes.Footnote, BlockTypes.ListItem)


# ----------------------------------------------------------------------------------------------------- helpers

def latex_of(block_html):
    return " ".join(htmllib.unescape(m.group(2)).strip() for m in MATH_HTML.finditer(block_html or ""))


def outside_math(block_html):
    """Text of a block outside its <math> elements (equation numbers in table cells, words)."""
    return htmllib.unescape(re.sub(r"<[^>]+>", " ", MATH_HTML.sub(" ", block_html or "")))


def math_html(latex):
    return f'<math display="block">{htmllib.escape(latex, quote=False)}</math>'


def score(pdf, pi, box, latex, extra_text=""):
    """(number of symbols that differ from the PDF, box used); None if the PDF has too little text there."""
    ours = sorted(qa.symbols_from_latex(latex) + qa.symbols(qa.EQ_NUMBER.sub(" ", extra_text)))
    labels = qa.eq_labels(latex + " " + extra_text)
    best = None
    for b in list(qa.box_variants(box)) + list(qa.label_anchored_boxes(pdf, pi, box, labels)):
        theirs = qa.symbols(qa.pdf_math_text(pdf.text_in_box(pi, b)))
        if len(theirs) < 3:
            continue
        missing, extra = qa.multiset_diff(theirs, ours, latex)
        if best is None or len(missing) + len(extra) < best[0]:
            best = (len(missing) + len(extra), b)
    return best


def is_subsequence(short, long):
    it = iter(long)
    return all(c in it for c in short)


def pdf_labels_near(pdf, pi, box):
    """Equation numbers printed on the lines of this box, up to the right edge of the page."""
    W, _ = pdf.size(pi)
    return qa.eq_labels(pdf.text_in_box(pi, (box[0], box[1], W, box[3])))


def fix_label(latex, pdf, pi, box, used=frozenset()):
    """Replace a truncated equation number ("(4.1)" for "(4.19)": Marker's characters appear in order in the PDF's)
    by the one printed in the PDF, or add a missing one that no other equation in the chapter has. A box that
    reaches a neighbouring equation's number must not relabel this one."""
    printed, ours = pdf_labels_near(pdf, pi, box), qa.eq_labels(latex)
    if len(printed) != 1 or printed == ours or len(ours) > 1:
        return latex
    right = next(iter(printed))
    if right in used:
        return latex
    if ours:
        wrong = next(iter(ours))
        return latex.replace(wrong, right) if is_subsequence(wrong, right) else latex
    return latex.rstrip().rstrip(".,") + rf" \quad {right}"


def fix_accents(latex, pdf, pi, box):
    missing, extra = qa.multiset_diff(qa.accents(pdf.text_in_box(pi, box)), qa.accents_from_latex(latex))
    if missing and extra:
        return qa.accent_swap(latex, missing, extra) or latex
    return latex


def katex_ok(latexes):
    failed = qa.validate_math([{"id": str(k), "latex": l, "display": True} for k, l in enumerate(latexes)])
    return [str(k) not in failed for k in range(len(latexes))]


def crop_image(pdf, pi, box, pad=4, scale=4):
    W, H = pdf.size(pi)
    left, top, right, bottom = box
    margins = (max(0, left - pad), max(0, bottom - pad), max(0, W - right - pad), max(0, H - top - pad))
    return pdf.doc[pi].render(scale=scale, crop=margins).to_pil()


def read_many(recognition_model, items):
    """OCR (image, temperature) items with Surya's block prompt; temperature 0 = greedy. Returns html strings."""
    if not items:
        return []
    manager = recognition_model.manager or get_default_manager()
    batch = [BatchInputItem(image=img, prompt_type=PROMPT_TYPE_BLOCK, max_tokens=2048,
                            temperature=t or None, top_p=0.95 if t else None, metadata={"k": k})
             for k, (img, t) in enumerate(items)]
    out = {o.metadata["k"]: o for o in manager.generate(batch)}
    return ["" if k not in out or out[k].error else clean_block_html(out[k].raw or "") for k in range(len(items))]


def reading_plan(pdf, pi, box, best_box):
    """READINGS crops/decodings for one equation: different framing and resolution, then sampled decoding."""
    plan = [(crop_image(pdf, pi, best_box), 0.0),
            (crop_image(pdf, pi, best_box, pad=12, scale=3), 0.0),
            (crop_image(pdf, pi, box, pad=6, scale=5), 0.0),
            (crop_image(pdf, pi, best_box), 0.5),
            (crop_image(pdf, pi, best_box), 0.8)]
    return plan[:READINGS]


# -------------------------------------------------------------------------------------------- equation core

def correct_equations(pdf, eqs, recognition_model, equation_processor):
    """eqs: list of {"id", "pi", "box" (PDF points), "html" (str or None), "text" (block text when html is None)}.
    Returns ({id: new html, or None to remove the block}, stats)."""
    stats = Counter()
    html = {e["id"]: e["html"] for e in eqs}

    def text_of(e):
        return outside_math(html[e["id"]]) if html[e["id"]] else e["text"]

    def single(e):
        return bool(html[e["id"]]) and SINGLE_MATH.fullmatch(html[e["id"]]) is not None

    # 1-2. numbers and accents (in place: single-math blocks fully, table/mixed blocks only the number)
    used = set()
    for e in eqs:
        used |= qa.eq_labels(latex_of(html[e["id"]]) + " " + text_of(e))
    for e in eqs:
        h, pi, box = html[e["id"]], e["pi"], e["box"]
        if not h:
            continue
        if not single(e):
            plain = latex_of(h) + " " + outside_math(h)
            old, new = qa.eq_labels(plain), qa.eq_labels(fix_label(plain, pdf, pi, box, used))
            if len(old) == 1 and len(new) == 1 and old != new and h.count(next(iter(old))) == 1:
                html[e["id"]] = h.replace(next(iter(old)), next(iter(new)))
                used |= new
                stats["numbers"] += 1
            continue
        latex = latex_of(h)
        new = fix_accents(fix_label(latex, pdf, pi, box, used), pdf, pi, box)
        if new != latex and katex_ok([new])[0]:
            used |= qa.eq_labels(new)
            stats["numbers"] += qa.eq_labels(new) != qa.eq_labels(latex)
            stats["accents"] += qa.accents_from_latex(new) != qa.accents_from_latex(latex)
            html[e["id"]] = math_html(new)

    # 3. several readings of equations that don't match the PDF
    todo = []
    for e in eqs:
        latex, text = latex_of(html[e["id"]]), text_of(e)
        s = score(pdf, e["pi"], e["box"], latex, text)
        if s and s[0] > 0:
            todo.append((e, latex, text, s))
    stats["reread"] = len(todo)
    items, owner = [], []
    for n, (e, _, _, s) in enumerate(todo):
        for item in reading_plan(pdf, e["pi"], e["box"], s[1]):
            items.append(item)
            owner.append(n)
    readings = read_many(recognition_model, items)
    options = {n: [] for n in range(len(todo))}  # n -> [(latex, score)]
    for n, h in zip(owner, readings):
        e, latex, text, s = todo[n]
        new = latex_of(equation_processor.fix_latex(h)) if h else ""
        if not new:
            continue
        # Expected numbers: the old ones, with truncated ones ("(2.1)") read as the PDF prints them ("(2.12)").
        printed = pdf_labels_near(pdf, e["pi"], s[1])
        expected = {next((p for p in printed if is_subsequence(l, p)), l) for l in qa.eq_labels(latex + " " + text)}
        if len(expected) == 1 and not qa.eq_labels(new):
            new = new.rstrip().rstrip(".,") + r" \quad " + next(iter(expected))
        new = fix_accents(new, pdf, e["pi"], s[1])
        if expected - qa.eq_labels(new):
            continue  # this reading lost an equation number
        s_new = score(pdf, e["pi"], s[1], new)
        if s_new and s_new[0] < s[0]:
            options[n].append((new, s_new[0]))
    flat = [(n, latex) for n, opts in options.items() for latex, _ in opts]
    valid = {(n, latex) for (n, latex), ok in zip(flat, katex_ok([l for _, l in flat])) if ok}
    for n, opts in options.items():
        opts = [(l, sc) for l, sc in opts if (n, l) in valid]
        if not opts:
            continue
        best = min(sc for _, sc in opts)
        # Among equally good readings, the one read most often (whitespace ignored).
        tied = Counter(re.sub(r"\s+", "", l) for l, sc in opts if sc == best)
        winner = next(l for l, sc in opts if sc == best and re.sub(r"\s+", "", l) == tied.most_common(1)[0][0])
        html[todo[n][0]["id"]] = math_html(winner)
        stats["reread_improved"] += 1
        stats["reread_perfect"] += best == 0

    # 4. duplicates: same page, overlapping, same number (or none), near-identical
    removed = set()
    by_page = {}
    for e in eqs:
        by_page.setdefault(e["pi"], []).append(e)
    for pi, items_ in by_page.items():
        for j, b in enumerate(items_):
            for a in items_[max(0, j - 3):j]:
                if a["id"] in removed or b["id"] in removed:
                    continue
                box_a, box_b = a["box"], b["box"]
                overlap = min(box_a[1], box_b[1]) - max(box_a[3], box_b[3])
                height = min(box_a[1] - box_a[3], box_b[1] - box_b[3])
                la, lb = latex_of(html[a["id"]]), latex_of(html[b["id"]])
                if not la or not lb:
                    continue
                same_number = qa.eq_labels(la) == qa.eq_labels(lb) or not qa.eq_labels(la) or not qa.eq_labels(lb)
                if height > 0 and overlap / height >= 0.5 and same_number and \
                        fuzz.ratio(qa.EQ_NUMBER.sub("", la), qa.EQ_NUMBER.sub("", lb)) >= 80:
                    sa, sb = score(pdf, pi, box_a, la), score(pdf, pi, box_b, lb)
                    removed.add(b["id"] if (sb[0] if sb else 99) >= (sa[0] if sa else 99) else a["id"])
                    stats["duplicates"] += 1

    changes = {e["id"]: html[e["id"]] for e in eqs if html[e["id"]] != e["html"]}
    changes.update({i: None for i in removed})
    return changes, stats


# ------------------------------------------------------------------------------------ convert.py front end

def pdf_box(block, page, pdf, pi):
    """Block polygon (Marker page coordinates, origin top-left) -> (left, top, right, bottom) in PDF points."""
    W, H = pdf.size(pi)
    pw, ph = page.polygon.width, page.polygon.height
    x0, y0, x1, y1 = block.polygon.bbox
    return (x0 * W / pw, H - y0 * H / ph, x1 * W / pw, H - y1 * H / ph)


def correct(document, pdf_path, recognition_model, equation_processor, restored_ids=()):
    pdf = qa.Pdf(pdf_path)
    eqs, blocks = [], {}
    for page in document.pages:
        pi = page.page_id
        if not pdf.has_text_layer(pi):
            continue
        for block in page.contained_blocks(document, (BlockTypes.Equation,)):
            if not block.removed:
                key = str(block.id)
                blocks[key] = block
                eqs.append({"id": key, "pi": pi, "box": pdf_box(block, page, pdf, pi), "html": block.html,
                            "text": "" if block.html else block.raw_text(document)})
    changes, stats = correct_equations(pdf, eqs, recognition_model, equation_processor)
    for key, new in changes.items():
        if new is None:
            blocks[key].removed = True
        else:
            blocks[key].html, blocks[key].structure = new, []

    # Paragraphs: inline accent swaps
    for page in document.pages:
        pi = page.page_id
        if not pdf.has_text_layer(pi):
            continue
        for block in page.contained_blocks(document, TEXT_TYPES):
            if not block.html or "<math" not in block.html:
                continue
            box = pdf_box(block, page, pdf, pi)
            inline = latex_of(block.html)
            missing, extra = qa.multiset_diff(qa.accents(pdf.text_in_box(pi, box)), qa.accents_from_latex(inline))
            if not (missing and extra) or qa.accent_swap(inline, missing, extra) is None:
                continue
            block.html = MATH_HTML.sub(lambda m: m.group(1) + htmllib.escape(
                qa.accent_swap(htmllib.unescape(m.group(2)), missing, extra) or htmllib.unescape(m.group(2)), quote=False)
                + m.group(3) if qa.accents_from_latex(htmllib.unescape(m.group(2))) else m.group(0), block.html)
            stats["inline_accents"] += 1

    # Restored paragraphs: read again from the exact area of their PDF text lines (no equation in the crop)
    jobs = []
    for block_id in restored_ids:
        block = document.get_block(block_id)
        page = document.get_page(block.page_id)
        lines = [l for l in (document.get_block(i) for i in block.structure or []) if l is not None]
        if not lines:
            continue
        x0 = min(l.polygon.bbox[0] for l in lines); y0 = min(l.polygon.bbox[1] for l in lines)
        x1 = max(l.polygon.bbox[2] for l in lines); y1 = max(l.polygon.bbox[3] for l in lines)
        W, H = pdf.size(block.page_id)
        pw, ph = page.polygon.width, page.polygon.height
        jobs.append((block, block.raw_text(document), (x0 * W / pw, H - y0 * H / ph, x1 * W / pw, H - y1 * H / ph)))
    htmls = read_many(recognition_model, [(crop_image(pdf, b.page_id, box, pad=2), 0.0) for b, _, box in jobs])
    for (block, pdf_text, box), h in zip(jobs, htmls):
        h = re.sub(r"</?p\b[^>]*>", "", h or "").strip()
        words = [w.lower() for w in re.findall(r"[A-Za-z]{3,}", pdf_text)]
        got = {w.lower() for w in re.findall(r"[A-Za-z]{3,}", re.sub(r"<math\b.*?</math>|<[^>]+>", " ", h, flags=re.S))}
        if words and sum(w in got for w in words) / len(words) >= 0.9 and "<math" in h:
            block.html, block.structure = h, []
            stats["paragraphs_reread"] += 1
    return dict(stats)
