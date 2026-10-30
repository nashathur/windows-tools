"""Find the parts of a conversion that need review, so only those get looked at ("Marker" below means the
converter's output: the checks were written for Marker and now run on convert.py/eqread.py output).

Run in the dev env, on a folder made by convert.py:
    conda run -n dev python C:\\Tools\\pdf2md\\qa.py out_dir\\<name>

Checks:
  1. LaTeX: every inline and display equation is rendered with KaTeX; parse failures are flagged.
  2. Display equations vs the PDF text layer (born-digital PDFs): symbols (letters, digits, Greek and
     + − = < > ≤ ≥ ± × ≠; case-sensitive, order ignored) and bold (PDF bold fonts vs \\mathbf/\\boldsymbol).
     Catches cos<->sin, RO<->Ro, u<->v, wrong signs, lost bold on vectors.
  3. Inline math vs the PDF text layer: PDF tokens not found in Marker's prose are compared with the
     symbols in the paragraph's inline LaTeX; only small two-sided swaps are flagged.
  4. Duplicated equations: overlapping equation boxes with near-identical LaTeX on the same page.
  5. Numbering of equations, figures and tables in reading order: duplicates, out of order, from another
     chapter, gaps.
  6. Text blocks vs the PDF text layer: words Marker added (misreads) or lost.
  7. Equation blocks without LaTeX, garbled text.

Each flagged block is cropped into crops/. Equations that couldn't be compared with the PDF text are cropped
and listed under "Visual check". Writes a compact qa_report.md (one line per issue, likely errors first) and
qa_report.json (full details).
"""

import argparse
import ctypes
import glob
import html
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unicodedata
from pathlib import Path

from .. import coords, tools
from ..errors import PdfError
from .p8_io import atomic_write
from ..mathcheck import validate

def _pdfium():
    import pypdfium2 as pdfium
    return pdfium

def _pdfium_raw():
    import pypdfium2.raw as pdfium_c
    return pdfium_c

def _fuzz():
    from rapidfuzz import fuzz
    return fuzz


HERE = os.path.dirname(os.path.abspath(__file__))
MATH = re.compile(r"<math\b([^>]*)>(.*?)</math>", re.S)
TAG = re.compile(r"<[^>]+>")
GARBLED = re.compile(r"[\ufffd\ue000-\uf8ff]")
COMPARE_TYPES = {"Text", "SectionHeader", "ListItem", "Caption", "Footnote", "TableCell", "Table"}
SKIP_FOR_LOST_TEXT = {"Figure", "Picture", "Diagram", "PageHeader", "PageFooter"}  # text Marker rightly leaves out
CM_TEXT_FONTS = {"CMR", "CMTI", "CMBX", "CMSL", "CMSS", "CMTT", "CMCSC", "CMSSBX", "CMBXTI"}
EQ_LABEL = re.compile(r"\((\d+)\.(\d+)([a-z]?)\)")  # (2.52c)
CAPTION_LABEL = re.compile(r"\s*(Figure|Fig\.|Table)\s*(\d+)\s*[-–.]\s*(\d+)")  # Figure 2-14, Fig. 3.2, Table 1-1
EQ_NUMBER = re.compile(r"\(\s*\d+\s*\.?\s*\d*\s*[a-z]?\s*\)")  # (2.8a), also "(24b)" when the dot is lost
TEX_BRACE_PIECES = re.compile(r"z\s*\}\s*\|\s*\{|\|\s*\{\s*z\s*\}")  # \overbrace/\underbrace glyphs in old TeX fonts
BOLD_FONT = re.compile(r"bold|black|heavy|demi|cmbx|cmmib|cmbsy|cmssbx|bx\d", re.I)
BOLD_LATEX = re.compile(r"\\(?:mathbf|boldsymbol|bm|pmb|mathbfit|textbf)\s*(\{(?:[^{}]|\{[^{}]*\})*\}|\\[A-Za-z]+|\w)")
LATEX_TEXT_ARGS = re.compile(r"\\(?:text|mathrm|operatorname|textrm|mbox)\s*\{([^{}]*)\}")

GREEK = {}
for code in list(range(0x391, 0x3AA)) + list(range(0x3B1, 0x3CA)):
    name = unicodedata.name(chr(code), "")
    if name:
        word = name.split()[-1].lower()
        GREEK[word.capitalize() if code < 0x3B1 else word] = chr(code)
GREEK.update({"lambda": "λ", "Lambda": "Λ", "varphi": "φ", "phi": "φ", "vartheta": "θ", "varepsilon": "ε",
              "epsilon": "ε", "varrho": "ρ", "varsigma": "σ", "varpi": "π"})
VARIANTS = str.maketrans({"ϕ": "φ", "ϑ": "θ", "ϵ": "ε", "ϱ": "ρ", "ς": "σ", "ϖ": "π", "∆": "Δ"})
# Old TeX math-extension fonts expose big operators as letters in the PDF text layer (display, text size).
BIG_OPERATOR_LETTERS = {"int": "ZR", "iint": "ZR", "oint": "IH", "sum": "XP", "prod": "YQ"}
OPERATOR_NAMES = {"sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh", "tanh",
                  "coth", "exp", "log", "ln", "lg", "max", "min", "sup", "inf", "lim", "det", "dim", "ker", "arg",
                  "deg", "gcd", "Pr", "mod", "bmod", "pmod", "limsup", "liminf"}
OPERATORS = set("+−=<>≤≥±×≠")
# Accent glyphs in the PDF text layer and the LaTeX commands that print them (\bar and \tilde are often swapped).
ACCENT_CHARS = {"˜": "~", "~": "~", "¯": "¯", "ˉ": "¯", "ˆ": "^", "˙": "˙", "¨": "¨",
                "˚": "˚", "ˇ": "ˇ", "˘": "˘"}
LATEX_ACCENTS = {"tilde": "~", "widetilde": "~", "bar": "¯", "hat": "^", "widehat": "^", "dot": "˙", "ddot": "¨",
                 "mathring": "˚", "check": "ˇ", "breve": "˘"}
ACCENT_COMMAND = {"~": "tilde", "¯": "bar", "^": "hat", "˙": "dot", "¨": "ddot", "˚": "mathring", "ˇ": "check", "˘": "breve"}
OPERATOR_COMMANDS = {"leq": "≤", "le": "≤", "leqslant": "≤", "geq": "≥", "ge": "≥", "geqslant": "≥", "neq": "≠",
                     "ne": "≠", "pm": "±", "times": "×", "lt": "<", "gt": ">",
                     # TeX builds long arrows from a minus/equals sign plus an arrow head
                     "longrightarrow": "−", "longleftarrow": "−", "longleftrightarrow": "−", "longmapsto": "−",
                     "Longrightarrow": "=", "Longleftarrow": "=", "Longleftrightarrow": "="}
SNIPPET = 140
NEIGHBOUR = 14  # pt above/below a box searched for a neighbouring line that explains a one-sided difference
LOST_RUN = 5  # consecutive missing 3-word sequences (about 7 words) reported as PDF text missing from Marker
MAX_NUMBER_JUMP = 10  # a number further ahead than this is treated as a misread, not a gap


# ---------------------------------------------------------------------------------------------------- text helpers

def leaves(block):
    if block.get("children"):
        for child in block["children"]:
            yield from leaves(child)
    else:
        yield block


def page_index(block_id):
    return int(block_id.split("/")[2])


def plain_text(block_html):
    return html.unescape(TAG.sub(" ", MATH.sub(" ", block_html)))


def plain_text_with_math_words(block_html):
    """Like plain_text, but math is replaced by the words written in it (\\text{...}, \\mathrm{...})."""
    def words_in(m):
        latex = html.unescape(m.group(2))
        # Also each variable with its subscript as one word: PDFs that set math in the text font give "Tpeak" for
        # T_{\text{peak}}.
        s = re.sub(r"[_{}]", "", re.sub(r"\\[A-Za-z]+", " ", LATEX_TEXT_ARGS.sub(r"\1", latex)))
        return " " + " ".join(LATEX_TEXT_ARGS.findall(latex)) + " " + " ".join(re.findall(r"[A-Za-z]+", s)) + " "
    return html.unescape(TAG.sub(" ", MATH.sub(words_in, block_html)))


def normalize(text):
    text = unicodedata.normalize("NFKC", text).lower()
    text = re.sub(r"-\s*\n\s*", "", text)  # re-join words hyphenated across lines
    text = re.sub(r"[^\w]+", " ", text)
    return re.sub(r"\s+", " ", text).strip()


def short(text, n=SNIPPET):
    text = re.sub(r"\s+", " ", text).strip()
    return text if len(text) <= n else text[:n] + "…"


# ------------------------------------------------------------------------------------------------ symbol helpers

def symbols(text, operators=True):
    """Letters, digits, Greek (and relations/operators), case-sensitive, sorted: order is ignored because
    fractions and sub/superscripts reorder text."""
    text = unicodedata.normalize("NFKC", text).translate(VARIANTS).replace("-", "−")
    # accent glyphs such as "ˆ" (a modifier letter, so isalnum) are checked separately by accents()
    return sorted(c for c in text if (c.isalnum() and c not in ACCENT_CHARS) or (operators and c in OPERATORS))


def symbols_from_latex(latex, operators=True):
    latex = latex.replace("\\\\", " ")  # a line break, not a command: "\\y:" starts a row with y
    # environments (with an array's column spec, \begin{array}{r c l}) and \tag{} carry no symbols
    latex = re.sub(r"\\begin\s*\{(?:array|tabular)\}\s*\{[lcrp|@.\s]*\}|\\(?:begin|end)\s*\{[^{}]*\}|\\tag\*?\s*\{[^{}]*\}",
                   " ", latex)
    latex = EQ_NUMBER.sub(" ", latex)
    latex = LATEX_TEXT_ARGS.sub(lambda m: m.group(1), latex)
    latex = re.sub(r"\\([A-Za-z]+)", lambda m: GREEK.get(m.group(1)) or OPERATOR_COMMANDS.get(m.group(1)) or
                   (m.group(1) if m.group(1) in OPERATOR_NAMES else " "), latex)
    return symbols(latex, operators)


def pdf_math_text(text, display=True):
    # Old TeX fonts: \neq is a slash glyph exposed as "6" over "="; \varrho sits at the "%" position of cmmi
    # (only mapped in display equations: in prose "%" is a percent sign).
    text = text.replace("6=", "≠")
    if display:
        text = text.replace("%", "ρ")
    return EQ_NUMBER.sub(" ", TEX_BRACE_PIECES.sub(" ", text))


def bold_from_latex(latex):
    out = []
    for arg in BOLD_LATEX.findall(latex):
        out += [c for c in symbols_from_latex(arg.strip("{}"), operators=False) if c.isalpha()]
    return sorted(out)


def multiset_diff(expected, got, latex=""):
    """(in expected but not got, in got but not expected)."""
    exp, extra = list(expected), []
    for c in got:
        if c in exp:
            exp.remove(c)
        else:
            extra.append(c)
    for op, letters in BIG_OPERATOR_LETTERS.items():
        for _ in re.findall(rf"\\{op}(?![A-Za-z])", latex):
            for c in letters:
                if c in exp:
                    exp.remove(c)
                    break
    return exp, extra


def contained(chars, pool):
    pool = list(pool)
    for c in chars:
        if c not in pool:
            return False
        pool.remove(c)
    return True


def word_diff(ours, theirs, ours_page, theirs_page):
    """Words Marker has that the PDF doesn't (misreads), and real words the PDF has that Marker lost.
    Short/numeric PDF-only tokens are ignored (usually inline-math glyphs). Matching ignores spaces, because PDF
    text layers split hyphenated words ("refer ence"). A word only counts as misread if it's absent from the PDF's
    whole page, and as lost if it's absent from Marker's text for the page and its neighbours."""
    ours_w, theirs_w = ours.split(), theirs.split()
    ours_joined, theirs_joined = ours.replace(" ", ""), theirs.replace(" ", "")
    pool, added = list(theirs_w), []
    for w in ours_w:
        if w in pool:
            pool.remove(w)
        elif len(w) >= 2 and w not in theirs_joined and w not in theirs_page:
            added.append(w)
    pool_ours, dropped = list(ours_w), []
    for w in theirs_w:
        if w in pool_ours:
            pool_ours.remove(w)
        elif len(w) >= 3 and w.isalpha() and w not in ours_joined and w not in ours_page:
            dropped.append(w)
    return added, dropped


class Pdf:
    def __init__(self, path):
        import pymupdf
        self.path = path
        self.doc = _pdfium().PdfDocument(path)
        self.mupdf = pymupdf.open(path)
        self.text_pages, self.char_cache, self.joined, self.labels = {}, {}, {}, {}

    def close(self):
        self.doc.close()
        self.mupdf.close()

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        self.close()

    def pymupdf_box(self, pi, box):
        """Convert a PDFium bottom-left user-space box to PyMuPDF text coordinates."""
        import pymupdf
        return coords.from_user_space(self.mupdf[pi], pymupdf.Rect(box[0], box[3], box[2], box[1]))

    def pdfium_box(self, pi, box):
        """Convert a PyMuPDF displayed-point box to PDFium's bottom-left user space."""
        import pymupdf
        page = self.mupdf[pi]
        text_box = coords.display_to_pymupdf(page, coords.displayed_points_to_mm(box))
        user_box = coords.to_user_space(page, text_box)
        return user_box.x0, user_box.y1, user_box.x1, user_box.y0

    def displayed_box(self, pi, box):
        """Convert a PDFium bottom-left user-space box to displayed PyMuPDF points."""
        import pymupdf
        page = self.mupdf[pi]
        text_box = coords.from_user_space(page, pymupdf.Rect(box[0], box[3], box[2], box[1]))
        return coords.to_displayed_points(page, text_box)

    def size(self, pi):
        return self.doc[pi].get_size()

    def textpage(self, pi):
        if pi not in self.text_pages:
            self.text_pages[pi] = self.doc[pi].get_textpage()
        return self.text_pages[pi]

    def text_in_box(self, pi, box):
        left, top, right, bottom = box
        return self.textpage(pi).get_text_bounded(left=left, bottom=bottom, right=right, top=top)

    def page_text_joined(self, pi):
        if pi not in self.joined:
            W, H = self.size(pi)
            self.joined[pi] = normalize(self.text_in_box(pi, (0, H, W, 0))).replace(" ", "")
        return self.joined[pi]

    def has_text_layer(self, pi):
        return len(self.page_text_joined(pi)) > 20

    def chars(self, pi):
        """[(char, x_center, y_center, is_bold)] for the page, bold judged by font name (weights are unreliable)."""
        if pi not in self.char_cache:
            tp, out = self.textpage(pi), []
            buf, flags = ctypes.create_string_buffer(256), ctypes.c_int()
            for i in range(tp.count_chars()):
                ch = tp.get_text_range(i, 1)
                if not ch.strip():
                    continue
                left, bottom, right, top = tp.get_charbox(i)
                _pdfium_raw().FPDFText_GetFontInfo(tp.raw, i, buf, 256, ctypes.byref(flags))
                out.append((ch, (left + right) / 2, (bottom + top) / 2, bool(BOLD_FONT.search(buf.value.decode("latin-1")))))
            self.char_cache[pi] = out
        return self.char_cache[pi]

    def label_positions(self, pi):
        """{"(4.19)": y_centre} for equation numbers printed at the end of a line in the right half of the page."""
        if pi not in self.labels:
            tp, W = self.textpage(pi), self.size(pi)[0]
            out = {}
            text = tp.get_text_range()
            for m in re.finditer(r"\(\d+\.\d+[a-z]?\)(?=[ \t]*(?:\r?\n|$))", text):
                left, bottom, right, top = tp.get_charbox(m.start())
                # An equation number is set flush right with a wide gap before it; a reference at the end of a
                # prose line ("... from (2.24)") follows a normal word space.
                k = m.start() - 1
                while k >= 0 and text[k] in " \t":
                    k -= 1
                gap = left - tp.get_charbox(k)[2] if k >= 0 and text[k] not in "\r\n" else 99
                if left > 0.5 * W and gap > 12:
                    out.setdefault(m.group(), (bottom + top) / 2)
            self.labels[pi] = out
        return self.labels[pi]

    def font_family(self, pi, i):
        """Font family of character i: "Times-Italic" -> "Times"; Computer Modern text fonts (CMR, CMTI, CMBX, ...)
        are one family, math fonts (CMMI, CMSY, CMEX) another."""
        buf, flags = ctypes.create_string_buffer(256), ctypes.c_int()
        _pdfium_raw().FPDFText_GetFontInfo(self.textpage(pi).raw, i, buf, 256, ctypes.byref(flags))
        name = buf.value.decode("latin-1").split("+")[-1]
        family = re.sub(r"[-,].*$|\d+$", "", name).upper()
        return "CMTEXT" if family in CM_TEXT_FONTS else family

    def bold_in_box(self, pi, box):
        left, top, right, bottom = box
        text = "".join(c for c, x, y, b in self.chars(pi) if b and left <= x <= right and bottom <= y <= top)
        return [c for c in symbols(text, operators=False) if c.isalpha()]

    def crop(self, pi, box, dpi, path, prose=True):
        """Crop to the ink of the block, not to Marker's box: render with room above and below, split into
        horizontal ink bands, keep the bands that lie mostly inside the box, then add neighbouring bands that
        belong to it (lines cut by the box, underbrace labels) and drop slices of neighbouring lines. For
        equations (prose=False), lines of body text at the edges are dropped even when Marker's box covers them."""
        import pymupdf
        W, H = self.size(pi)
        unit = coords.physical_size(self.mupdf[pi], pymupdf.Rect(0, 0, 1, 1)).x
        left, top, right, bottom = pymupdf.Rect(self.displayed_box(pi, box)) / unit
        pad, room, s = 6 / unit, 18 / unit, dpi * unit / 72
        y_top, y_bottom = max(0, top - room), min(H, bottom + room)
        margins = (max(0, left - pad), H - y_bottom,
                   max(0, W - right - pad), y_top)
        img = self.doc[pi].render(scale=s, crop=margins).to_pil()
        import numpy as np
        ink = np.asarray(img.convert("L")) < 160
        rows = np.flatnonzero(ink.any(axis=1))
        if not len(rows):
            return atomic_write(path, writer=lambda temp: img.save(temp, format="PNG"))
        breaks = np.flatnonzero(np.diff(rows) > 1)
        bands = list(zip(np.r_[rows[0], rows[breaks + 1]], np.r_[rows[breaks], rows[-1]] + 1))
        box_lo, box_hi = (top - y_top) * s, (bottom - y_top) * s
        inside = lambda i: max(0, min(bands[i][1], box_hi) - max(bands[i][0], box_lo)) / (bands[i][1] - bands[i][0])
        text = [self.text_in_box(pi, self.pdfium_box(
            pi, tuple(value * unit for value in (left - pad, y_top + a / s,
                                                 right + pad, y_top + b / s)))) for a, b in bands]
        is_prose = lambda i: not prose and len(re.findall(r"[A-Za-z]{3,}", text[i])) >= 5
        # a short label (underbrace/overbrace text, a lone subscript line); on scanned pages, a band label-sized
        is_label = lambda i: (len(re.sub(r"\s", "", text[i])) <= 3 if text[i].strip()
                              else bands[i][1] - bands[i][0] <= 7 * s)
        keep = [i for i in range(len(bands)) if inside(i) >= 0.5 and not is_prose(i)]
        if not keep:
            return atomic_write(path, writer=lambda temp: img.save(temp, format="PNG"))
        lo, hi = keep[0], keep[-1]
        gap = lambda i, j: bands[j][0] - bands[i][1]
        belongs = lambda i, g: not is_prose(i) and (g <= 2 * s and inside(i) > 0 or g <= 5 * s and is_label(i))
        while lo > 0 and belongs(lo - 1, gap(lo - 1, lo)):
            lo -= 1
        while hi < len(bands) - 1 and belongs(hi + 1, gap(hi, hi + 1)):
            hi += 1
        y0, y1 = bands[lo][0], bands[hi][1]
        cols = np.flatnonzero(ink[y0:y1].any(axis=0))
        p = int(3 * s)
        crop = img.crop((max(0, cols[0] - p), max(0, y0 - p), min(img.width, cols[-1] + 1 + p),
                         min(img.height, y1 + p)))
        atomic_write(path, writer=lambda temp: crop.save(temp, format="PNG"))


def box_variants(box):
    """Marker's box vs the printed equation: exact, taller (denominators and superscripts stick out), and trimmed
    at the top/bottom (a neighbouring equation's line creeps in)."""
    left, top, right, bottom = box
    for top_pad, bottom_pad in ((0, 0), (3, 3), (-3, 0), (0, -3), (-3, -3)):
        yield left, top + top_pad, right, bottom - bottom_pad


def neighbour_symbols(pdf, pi, box, operators=True):
    left, top, right, bottom = box
    above = pdf.text_in_box(pi, (left, top + NEIGHBOUR, right, top - 3))
    below = pdf.text_in_box(pi, (left, bottom + 3, right, bottom - NEIGHBOUR))
    return symbols(pdf_math_text(above + " " + below), operators)


# ---------------------------------------------------------------------------------------------------------- checks

def accents(text):
    """Accent glyphs as the PDF text layer has them (taken before NFKC, which turns them into combining marks)."""
    return sorted(ACCENT_CHARS[c] for c in text if c in ACCENT_CHARS)


def accents_from_latex(latex):
    return sorted(LATEX_ACCENTS[m] for m in re.findall(r"\\([A-Za-z]+)", latex) if m in LATEX_ACCENTS)


def accent_swap(latex, pdf_only, latex_only):
    """LaTeX with one accent replaced by another, when that is the whole difference (e.g. every \\bar should be
    \\tilde). None if the difference is anything else."""
    if len(set(pdf_only)) != 1 or len(set(latex_only)) != 1 or len(pdf_only) != len(latex_only):
        return None
    wrong = [cmd for cmd, kind in LATEX_ACCENTS.items() if kind == latex_only[0]]
    pattern = re.compile(r"\\(?:" + "|".join(wrong) + r")(?![A-Za-z])")
    if len(pattern.findall(latex)) != len(latex_only):
        return None
    right = ACCENT_COMMAND[pdf_only[0]]
    return pattern.sub(lambda m: "\\" + right, latex, count=len(latex_only))


def eq_labels(text):
    text = re.sub(r"\\tag\*?\{(\d+\.\d+[a-z]?)\}", r"(\1)", text)
    return {f"({c}.{n}{l})" for c, n, l in EQ_LABEL.findall(text)}


def label_anchored_boxes(pdf, pi, box, labels):
    """Marker's box moved so it is centred on the equation's number in the PDF: when Marker's boxes overlap or are
    shifted, the number still says where the printed equation is."""
    left, top, right, bottom = box
    h = top - bottom
    for label in labels:
        y = pdf.label_positions(pi).get(label)
        if y is not None:
            yield from box_variants((left, y + h / 2, right, y - h / 2))


def check_equation(pdf, pi, box, latex, block_html, args, stats):
    """Symbols, accents, equation number and bold of a display equation vs the PDF. Returns a list of partial issues."""
    ours = sorted(symbols_from_latex(latex) + symbols(EQ_NUMBER.sub(" ", plain_text(block_html))))
    our_labels = eq_labels(latex + " " + plain_text(block_html))
    variants = list(box_variants(box)) + list(label_anchored_boxes(pdf, pi, box, our_labels))
    best = min(((*multiset_diff(symbols(pdf_math_text(pdf.text_in_box(pi, b))), ours, latex), b)
                for b in variants), key=lambda d: len(d[0]) + len(d[1]))
    missing, extra, best_box = best
    n = len(missing) + len(extra)
    n_pdf = len(symbols(pdf_math_text(pdf.text_in_box(pi, best_box))))
    if n_pdf < 3:
        stats["equations_unverifiable"] += 1
        return [{"visual": True, "reason": "no usable PDF text", "latex": latex}]
    stats["equations_compared"] += 1
    found = []
    diff = f"PDF[{''.join(missing)}] LaTeX[{''.join(extra)}]"
    if missing and extra and n <= args.max_symbol_diff:
        found.append({"confidence": "high", "reason": f"symbols {diff}", "latex": latex})
    elif 0 < n <= 3:
        explained = contained(missing, neighbour_symbols(pdf, pi, box)) if missing else \
            contained(extra, symbols(pdf_math_text(pdf.text_in_box(pi, (box[0], box[1] + NEIGHBOUR, box[2], box[3] - NEIGHBOUR)))))
        found.append({"confidence": "low" if explained else "high",
                      "reason": f"symbols {diff}" + (" (probably a neighbouring line)" if explained else ""),
                      "latex": latex})
    elif n:
        stats["equations_unverifiable"] += 1
        return [{"visual": True, "reason": "box doesn't line up with PDF text", "latex": latex}]

    # Accents (\tilde vs \bar, \hat vs \mathring): not letters, so the symbol check can't see them.
    raw = pdf.text_in_box(pi, best_box)
    acc_missing, acc_extra = multiset_diff(accents(raw), accents_from_latex(latex))
    if acc_missing or acc_extra:
        issue = {"confidence": "high" if acc_missing and acc_extra else "low",
                 "reason": f"accents differ: PDF[{' '.join(acc_missing)}] LaTeX[{' '.join(acc_extra)}]", "latex": latex}
        fixed = accent_swap(latex, acc_missing, acc_extra)
        if fixed:
            issue["fix"] = [latex, fixed]
        found.append(issue)

    # Equation number: Marker sometimes truncates it ("(4.19)" -> "(4.1)") or drops it when merging equations.
    W, _ = pdf.size(pi)
    pdf_labels = eq_labels(pdf.text_in_box(pi, (best_box[0], best_box[1], W, best_box[3])))
    if pdf_labels - our_labels:
        found.append({"confidence": "high" if our_labels - pdf_labels else "low",
                      "reason": f"equation number PDF{sorted(pdf_labels)} Marker{sorted(our_labels)}", "latex": latex})

    # Bold: compare in the same box variants, keep the best.
    ours_bold = bold_from_latex(latex)
    lost, spurious = min((multiset_diff(pdf.bold_in_box(pi, b), ours_bold) for b in variants),
                         key=lambda d: len(d[0]) + len(d[1]))
    if lost or spurious:
        found.append({"confidence": "high" if len(lost) + len(spurious) <= 6 else "low",
                      "reason": f"bold differs: bold in PDF only[{''.join(lost)}] bold in LaTeX only[{''.join(spurious)}]",
                      "latex": latex})
    return found


def check_inline_math(pdf, pi, box, block_html, maths, marker_words):
    """Inline math in a paragraph vs the PDF: PDF tokens that aren't Marker prose words are the PDF's math."""
    latex = " ".join(html.unescape(inner) for _, inner in maths)
    ours = symbols_from_latex(latex, operators=False)
    if len(ours) < 2:
        return None
    raw = pdf_math_text(pdf.text_in_box(pi, box), display=False)
    tokens = re.findall(r"[^\W_]+", unicodedata.normalize("NFKC", raw).translate(VARIANTS))
    prose = set(normalize(plain_text(block_html)).split())
    # A PDF token is prose if it's a word of this paragraph, or a real word (3+ letters) found near this page.
    pdf_math = [c for t in tokens
                if t.lower() not in prose and not (len(t) >= 3 and t.isalpha() and t.lower() in marker_words)
                for c in t]
    missing, extra = multiset_diff(sorted(pdf_math), ours, latex)
    if missing and extra and len(missing) + len(extra) <= 4:
        return {"confidence": "low", "reason": f"inline math PDF[{''.join(missing)}] LaTeX[{''.join(extra)}]",
                "latex": short(latex)}
    acc_missing, acc_extra = multiset_diff(accents(pdf.text_in_box(pi, box)), accents_from_latex(latex))
    if acc_missing and acc_extra:
        return {"confidence": "high", "latex": short(latex),
                "reason": f"inline accents differ: PDF[{' '.join(acc_missing)}] LaTeX[{' '.join(acc_extra)}]"}
    return None


PROSE_WORD = re.compile(r"[A-Za-z]+(?:-[ \t]*\r?\n[ \t]*[A-Za-z]+)?")  # a word, re-joined if hyphenated at a line end


def prose_words(text):
    """Words of 3+ letters (lowercase, hyphenation at line ends removed), skipping operator names (sin, exp...)."""
    out = []
    for m in PROSE_WORD.finditer(text):
        w = re.sub(r"-\s*", "", m.group()).lower()
        if len(w) >= 3 and w not in OPERATOR_NAMES:
            out.append((w, m.start(), m.end()))
    return out


def lost_prose(pdf, pi, marker_text, skip_boxes, min_run=LOST_RUN):
    """PDF prose that is nowhere in Marker's text for this page and its neighbours, as QA issues (see lost_runs)."""
    tp = pdf.textpage(pi)
    toks, found = lost_runs(pdf, pi, marker_text, skip_boxes, min_run)
    W, H = pdf.size(pi)
    issues = []
    for n, (a, b) in enumerate(found):
        boxes = [tp.get_charbox(i) for t in (toks[a], toks[b]) for i in (t[1], t[2] - 1)]
        top, bottom = max(bx[3] for bx in boxes) + 2, min(bx[1] for bx in boxes) - 2
        text = " ".join(t[0] for t in toks[a:b + 1])
        issues.append({"page": pi + 1, "pdf_index": pi, "block": f"/page/{pi}/LostText/{n}", "type": "Text",
                       "box": (40, top, W - 40, bottom), "confidence": "high",
                       "reason": f"PDF text missing from Marker ({b - a + 1} words): {short(text, 90)}"})
    return issues


def lost_runs(pdf, pi, marker_text, skip_boxes, min_run=LOST_RUN):
    """Return missing body-font runs, using shared ordered alignment after the caller supplies skipped boxes."""
    tp = pdf.textpage(pi)

    def skipped(i):
        left, bottom, right, top = tp.get_charbox(i)
        x, y = (left + right) / 2, (bottom + top) / 2
        return any(l <= x <= r and b <= y <= t for l, t, r, b in skip_boxes)

    # Only words set in the page's body-text font family: inline math (CMMI/CMSY, subscripts glued to symbols like
    # "Latmosphere") is not prose and isn't in Marker's prose either.
    toks = [t for t in prose_words(tp.get_text_range()) if not skipped(t[1])]
    families = [pdf.font_family(pi, t[1]) for t in toks]
    if families:
        body = max(set(families), key=families.count)
        toks = [t for t, f in zip(toks, families) if f == body]
    from ..wordalign import missing_runs
    return toks, missing_runs(toks, marker_text, min_words=max(7, min_run + 2))


def duplicate_equations(page_eqs):
    """Equations on one page whose boxes overlap vertically and whose LaTeX is near-identical: Marker output the
    same printed equation twice (often one copy wrong)."""
    found = []
    for j, (ref_b, latex_b) in enumerate(page_eqs):
        for ref_a, latex_a in page_eqs[max(0, j - 3):j]:
            a_top, a_bottom = ref_a["box"][1], ref_a["box"][3]
            b_top, b_bottom = ref_b["box"][1], ref_b["box"][3]
            overlap = min(a_top, b_top) - max(a_bottom, b_bottom)
            height = min(a_top - a_bottom, b_top - b_bottom)
            similar = _fuzz().partial_ratio(EQ_NUMBER.sub("", latex_a), EQ_NUMBER.sub("", latex_b))
            if (height > 0 and overlap / height >= 0.25 and similar >= 80) or _fuzz().ratio(latex_a, latex_b) >= 97:
                found.append({**ref_b, "confidence": "high", "latex": short(latex_b),
                              "reason": f"possibly duplicated equation (overlaps {ref_a['block'].split('/', 3)[3]}, "
                                        f"{similar:.0f}% similar)"})
                break
    return found


def numbering_issues(entries, kind):
    """Equation/figure/table numbers in reading order: duplicates, out of order, other chapter, gaps."""
    if not entries:
        return []
    chapters = [key[0] for key, _, _ in entries]
    chapter = max(set(chapters), key=chapters.count)
    found, seen, prev, prev_label = [], {}, None, None
    for key, label, ref in entries:
        if key[0] != chapter:
            found.append({**ref, "confidence": "high", "reason": f"{kind} {label} from another chapter"})
            continue
        k = key[1:]
        if k in seen:
            found.append({**ref, "confidence": "high",
                          "reason": f"duplicate {kind} {label} (also p.{seen[k]['page']}): duplicated content or misread number"})
            continue
        if prev is not None and k < prev:
            found.append({**ref, "confidence": "high", "reason": f"{kind} {label} out of order (after {prev_label})"})
            continue
        if prev is not None and k[0] > prev[0] + MAX_NUMBER_JUMP:
            # "(8.464)" for "(8.46)": a misread, not hundreds of missing equations.
            found.append({**ref, "confidence": "high", "reason": f"{kind} {label} jumps far ahead of {prev_label}: misread number?"})
            continue
        seen[k] = ref
        prev, prev_label = k, label

    def fmt(num, letter):
        return f"({chapter}.{num}{letter})" if kind == "equation" else f"{kind.capitalize()} {chapter}-{num}"

    def missing(key, next_ref):
        found.append({**next_ref, "confidence": "low", "nocrop": True,
                      "reason": f"{kind} {fmt(*key)} missing (expected before this block)"})

    nums = sorted({k[0] for k in seen})
    for n in range(nums[0], nums[-1] + 1):
        letters = sorted(k[1] for k in seen if k[0] == n)
        if not letters:
            missing((n, ""), next(seen[k] for k in sorted(seen) if k[0] > n))
        elif letters != [""]:
            for code in range(ord("a"), ord(max(letters)) + 1):
                if chr(code) not in letters:
                    missing((n, chr(code)), next(seen[k] for k in sorted(seen) if k > (n, chr(code))))
    return found


def _markdown_table_shape(value):
    rows = [line.strip() for line in plain_text(value).splitlines() if "|" in line]
    rows = [line for line in rows if not re.fullmatch(r"\|?[\s:|\-]+\|?", line)]
    if not rows:
        return None
    columns = max(len(line.strip("|").split("|")) for line in rows)
    return len(rows), columns


def _source_table_shape(pdf, pi, box, scanned):
    import pymupdf
    if not scanned:
        try:
            with pymupdf.open(pdf.path) as doc:
                page = doc[pi]
                clip = pdf.pymupdf_box(pi, box)
                tables = page.find_tables(clip=clip).tables
                if tables:
                    table = max(tables, key=lambda item: item.row_count * item.col_count)
                    return table.row_count, table.col_count
                # pymupdf.layout can make find_tables miss ruled grids in its active layout context. Recover only
                # clear, full-width ruling runs from the same table box; ordinary text strokes are too short.
                horizontal, vertical = [], []
                for drawing in page.get_drawings():
                    for item in drawing.get("items", []):
                        if item[0] == "l":
                            points = item[1:3]
                            rects = [(points[0].x, points[0].y, points[1].x, points[1].y)]
                        elif item[0] == "re":
                            rect = item[1]
                            rects = [(rect.x0, rect.y0, rect.x1, rect.y0), (rect.x0, rect.y1, rect.x1, rect.y1),
                                     (rect.x0, rect.y0, rect.x0, rect.y1), (rect.x1, rect.y0, rect.x1, rect.y1)]
                        else:
                            continue
                        for ax, ay, bx, by in rects:
                            if (max(ax, bx) < clip.x0 or min(ax, bx) > clip.x1
                                    or max(ay, by) < clip.y0 or min(ay, by) > clip.y1):
                                continue
                            if abs(ay - by) <= 1 and abs(bx - ax) >= clip.width * 0.65:
                                horizontal.append(round((ay + by) / 2))
                            elif abs(ax - bx) <= 1 and abs(by - ay) >= clip.height * 0.65:
                                vertical.append(round((ax + bx) / 2))
                rows, columns = len(set(horizontal)), len(set(vertical))
                if rows >= 2 and columns >= 2:
                    return rows - 1, columns - 1
        except Exception:
            return None
        return None
    import cv2
    import numpy as np
    page = pdf.mupdf[pi]
    displayed_box = pdf.displayed_box(pi, box)
    displayed_box = pymupdf.Rect(displayed_box.x0 - 1, displayed_box.y0 - 1,
                                 displayed_box.x1 + 1, displayed_box.y1 + 1)
    pix = page.get_pixmap(matrix=pymupdf.Matrix(100 / 72, 100 / 72), clip=displayed_box, alpha=False)
    crop = np.frombuffer(pix.samples, dtype=np.uint8).reshape(pix.height, pix.width, pix.n)[:, :, :3]
    crop = cv2.cvtColor(crop, cv2.COLOR_RGB2GRAY)
    if crop.size == 0:
        return None
    _, binary = cv2.threshold(crop, 0, 255, cv2.THRESH_BINARY_INV | cv2.THRESH_OTSU)
    horizontal = cv2.morphologyEx(binary, cv2.MORPH_OPEN,
                                  cv2.getStructuringElement(cv2.MORPH_RECT, (max(8, crop.shape[1] // 12), 1)))
    vertical = cv2.morphologyEx(binary, cv2.MORPH_OPEN,
                                cv2.getStructuringElement(cv2.MORPH_RECT, (1, max(8, crop.shape[0] // 12))))
    row_stats = cv2.connectedComponentsWithStats(horizontal)[2][1:]
    col_stats = cv2.connectedComponentsWithStats(vertical)[2][1:]
    row_lines = sum(stats[cv2.CC_STAT_WIDTH] >= crop.shape[1] * 0.65 for stats in row_stats)
    col_lines = sum(stats[cv2.CC_STAT_HEIGHT] >= crop.shape[0] * 0.65 for stats in col_stats)
    if row_lines < 2 or col_lines < 2:
        return None
    return row_lines - 1, col_lines - 1


# ------------------------------------------------------------------------------------------------------------ main

def run(folder, threshold=70, max_dropped=3, max_symbol_diff=8, dpi=150, source_pdf=None):
    blocks_path = glob.glob(os.path.join(folder, "*.blocks.json"))[0]
    data = json.loads(Path(blocks_path).read_text(encoding="utf-8"))
    pdf = Pdf(source_pdf or data["source_pdf"])
    try:
        return _run(folder, threshold, max_dropped, max_symbol_diff, dpi, pdf, data)
    finally:
        pdf.close()


def _run(folder, threshold, max_dropped, max_symbol_diff, dpi, pdf, data):
    args = argparse.Namespace(threshold=threshold, max_dropped=max_dropped, max_symbol_diff=max_symbol_diff, dpi=dpi)
    blocks_path = glob.glob(os.path.join(folder, "*.blocks.json"))[0]
    name = os.path.basename(blocks_path)[: -len(".blocks.json")]
    ocr_pages = set(data.get("ocr_pages", []))
    crops_dir = os.path.join(folder, "crops")
    shutil.rmtree(crops_dir, ignore_errors=True)

    issues, math_items, math_owner, eq_numbers, captions = [], [], {}, [], {}
    stats = {"pages": len(data["pages"]), "blocks": 0, "inline_math": 0, "display_math": 0, "text_compared": 0,
             "equations_compared": 0, "equations_unverifiable": 0, "inline_compared": 0}
    no_text_layer = []

    # Marker's text per page, plus previous and next page (paragraphs and boxes continue across pages).
    marker_page = {page_index(p["id"]): normalize(" ".join(plain_text(b["html"]) for b in leaves(p)))
                   for p in data["pages"]}
    marker_near = {pi: marker_page.get(pi - 1, "") + " " + marker_page[pi] + " " + marker_page.get(pi + 1, "")
                   for pi in marker_page}
    # Same, keeping words written inside math (\text{time of one revolution}), for lost_prose.
    marker_page_words = {page_index(p["id"]): " ".join(plain_text_with_math_words(b["html"]) for b in leaves(p))
                         for p in data["pages"]}
    marker_near_words = {pi: " ".join(marker_page_words.get(k, "") for k in (pi - 1, pi, pi + 1)) for pi in marker_page}

    for page in data["pages"]:
        pi = page_index(page["id"])
        W, H = pdf.size(pi)
        text_layer = pdf.has_text_layer(pi) and pi + 1 not in ocr_pages
        if not text_layer:
            no_text_layer.append(pi + 1)
        near_words = set(marker_near[pi].split())
        near_joined = marker_near[pi].replace(" ", "")
        page_eqs, skip_boxes = [], []

        for block in leaves(page):
            stats["blocks"] += 1
            btype, block_html = block["block_type"], block["html"]
            box = pdf.pdfium_box(pi, block["bbox"])
            ref = {"page": pi + 1, "pdf_index": pi, "block": block["id"], "type": btype, "box": box}
            if btype in SKIP_FOR_LOST_TEXT:
                skip_boxes.append(box)

            if btype == "Table":
                markdown_shape = _markdown_table_shape(block_html)
                source_shape = _source_table_shape(pdf, pi, box, pi + 1 in ocr_pages)
                if source_shape is None:
                    issues.append({**ref, "confidence": "low", "reason": "table structure not checkable",
                                   "text": plain_text(block_html)[:SNIPPET]})
                elif markdown_shape and markdown_shape != source_shape:
                    issues.append({**ref, "confidence": "high",
                                   "reason": f"table structure: Markdown {markdown_shape[0]}×{markdown_shape[1]}, "
                                             f"PDF {source_shape[0]}×{source_shape[1]}",
                                   "text": plain_text(block_html)[:SNIPPET]})

            maths = MATH.findall(block_html)
            for k, (attrs, inner) in enumerate(maths):
                display = "block" in attrs
                stats["display_math" if display else "inline_math"] += 1
                mid = f"{block['id']}#{k}"
                math_items.append({"id": mid, "latex": html.unescape(inner).strip(), "display": display})
                math_owner[mid] = ref

            if btype == "Equation":
                latex = " ".join(html.unescape(inner).strip() for _, inner in maths)
                if not maths:
                    issues.append({**ref, "confidence": "high", "reason": "equation block without LaTeX",
                                   "text": short(plain_text(block_html))})
                else:
                    page_eqs.append((ref, latex))
                    if text_layer:
                        for found in check_equation(pdf, pi, box, latex, block_html, args, stats):
                            issues.append({**ref, **found, "latex": short(found["latex"])})
                for c, num, letter in EQ_LABEL.findall(html.unescape(TAG.sub(" ", block_html))):
                    eq_numbers.append(((int(c), int(num), letter), f"({c}.{num}{letter})", ref))

            if btype == "Caption":
                m = CAPTION_LABEL.match(plain_text(block_html))
                if m:
                    captions.setdefault(m.group(1).lower().rstrip("."), []).append(
                        ((int(m.group(2)), int(m.group(3)), ""), f"{m.group(1)} {m.group(2)}-{m.group(3)}", ref))

            # A display equation Marker didn't OCR: a text block ending in an equation number, with no display math.
            if btype in COMPARE_TYPES and btype not in ("Table", "TableCell") and not any("block" in a for a, _ in maths):
                m = re.search(r"\((\d+)\.(\d+)([a-z]?)\)\s*$", plain_text(block_html).strip())
                y = pdf.label_positions(pi).get(m.group(0).strip()) if m and text_layer else None
                if m and (y is None and not text_layer or y is not None and box[3] - 3 <= y <= box[1] + 3):
                    issues.append({**ref, "confidence": "high", "reason": "display equation left as plain text (not OCR'd)",
                                   "text": short(plain_text(block_html))})
                    eq_numbers.append(((int(m.group(1)), int(m.group(2)), m.group(3)), f"({m.group(1)}.{m.group(2)}{m.group(3)})", ref))

            if GARBLED.search(block_html):
                issues.append({**ref, "confidence": "high", "reason": "garbled characters", "text": short(plain_text(block_html))})

            if text_layer and btype in COMPARE_TYPES:
                ours = normalize(plain_text(block_html))
                if len(ours) >= 25:
                    stats["text_compared"] += 1
                    theirs = normalize(pdf.text_in_box(pi, box))
                    added, dropped = word_diff(ours, theirs, near_joined, pdf.page_text_joined(pi))
                    score = _fuzz().ratio(ours, theirs)
                    if added or len(dropped) >= args.max_dropped or score < args.threshold:
                        issues.append({**ref, "confidence": "high" if added else "low",
                                       "reason": f"text Marker[{' '.join(added)}] PDF[{' '.join(dropped)}] (similarity {score:.0f})"})
                if maths and not any("block" in a for a, _ in maths):
                    stats["inline_compared"] += 1
                    found = check_inline_math(pdf, pi, box, block_html, maths, near_words)
                    if found:
                        issues.append({**ref, **found})

        issues += duplicate_equations(page_eqs)
        if text_layer:
            issues += lost_prose(pdf, pi, marker_near_words[pi], skip_boxes)

    for mid, error in validate(math_items).items():
        latex = next(m["latex"] for m in math_items if m["id"] == mid)
        issues.append({**math_owner[mid], "confidence": "high", "reason": f"KaTeX: {short(error, 80)}", "latex": short(latex)})

    for issue in numbering_issues(eq_numbers, "equation"):
        # A gap is a real loss when the PDF prints that number at the end of a line on this page or the one before.
        m = re.match(r"equation (\(\d+\.\d+[a-z]?\)) missing", issue["reason"])
        if m and text_layer:
            on = [p + 1 for p in (issue["pdf_index"] - 1, issue["pdf_index"]) if p >= 0 and m.group(1) in pdf.label_positions(p)]
            if on:
                issue.update(confidence="high", reason=f"equation {m.group(1)} is in the PDF (p.{on[-1]}) but not in Marker")
        issues.append(issue)
    for kind, entries in captions.items():
        issues += numbering_issues(entries, kind)

    # Use the current Markdown as the numbering source of truth: users may edit it after extraction, so the
    # generated blocks file alone would miss a deleted display equation during mdqa.
    eqs_path = os.path.join(folder, "eqs.json")
    md_path = os.path.join(folder, f"{name}.md")
    if os.path.isfile(eqs_path) and os.path.isfile(md_path):
        markdown_text = Path(md_path).read_text(encoding="utf-8")
        expected = {}
        for equation in json.loads(Path(eqs_path).read_text(encoding="utf-8")):
            for label in equation.get("labels", []):
                expected[label] = equation
        present = eq_labels(markdown_text)
        for label, equation in expected.items():
            if label not in present:
                issues.append({"page": equation["page"], "pdf_index": equation["page"] - 1,
                               "block": equation.get("block", ""), "type": "Equation",
                               "box": tuple(equation.get("box", (0, 0, 0, 0))),
                               "confidence": "high", "reason": f"equation {label} is in the PDF but not in Markdown",
                               "latex": equation.get("latex") or ""})
        for page_number, page in enumerate(data["pages"], 1):
            for block in leaves(page):
                if block["block_type"] != "Table" or not block.get("table_garbled"):
                    continue
                x0, y0, x1, y1 = block["bbox"]
                actual_page = int(block.get("table_garbled_page", page_number))
                pi = page_index(page["id"])
                box = pdf.pdfium_box(pi, (x0, y0, x1, y1))
                issues.append({"page": actual_page, "pdf_index": pi,
                               "block": block["id"], "type": "Table", "box": box,
                               "confidence": "high", "reason": "table may be garbled; see PDF"})

    rank = {"high": 0, "low": 1}
    issues.sort(key=lambda i: (2 if i.get("visual") else rank[i["confidence"]], i["pdf_index"], -i["box"][1]))
    if issues:
        os.makedirs(crops_dir)
    cropped = {}
    for issue in issues:
        key = issue["block"]
        if issue.get("nocrop"):
            continue
        if key not in cropped:
            fname = f"p{issue['page']:03d}_{key.split('/')[3]}_{key.split('/')[4]}.png"
            pdf.crop(issue["pdf_index"], issue["box"], dpi, os.path.join(crops_dir, fname),
                     prose=issue["type"] != "Equation")
            cropped[key] = f"crops/{fname}"
        issue["crop"] = cropped[key]

    counts = write_reports(folder, name, stats, no_text_layer, issues)
    print(f"{name}: {stats['pages']} pages, {stats['display_math']} display + {stats['inline_math']} inline equations -> "
          f"{counts['high']} likely errors, {counts['low']} lower confidence, {counts['visual']} to check visually. "
          f"See {os.path.join(folder, 'qa_report.md')}")
    return counts


def write_reports(folder, name, stats, no_text_layer, issues):
    for issue in issues:
        issue.pop("pdf_index", None)
        issue["box"] = [round(v, 1) for v in issue["box"]]
    groups = {"high": [i for i in issues if not i.get("visual") and i["confidence"] == "high"],
              "low": [i for i in issues if not i.get("visual") and i["confidence"] == "low"],
              "visual": [i for i in issues if i.get("visual")]}
    counts = {k: len(v) for k, v in groups.items()}
    atomic_write(os.path.join(folder, "qa_report.json"), json.dumps(
        {"stats": stats, "counts": counts, "pages_without_text_layer": no_text_layer, "issues": issues},
        indent=1, ensure_ascii=False))

    lines = [f"# QA {name}: {stats['pages']} pages · {stats['display_math']} display, {stats['inline_math']} inline equations · "
             f"likely {counts['high']} · lower {counts['low']} · visual {counts['visual']}"]
    if no_text_layer:
        lines.append(f"Scanned pages (only LaTeX and numbering checked): {', '.join(map(str, no_text_layer))}")
    titles = {"high": "Likely errors", "low": "Lower confidence (check if time allows)",
              "visual": "Visual check (equation couldn't be compared: compare crop with LaTeX)"}
    n = 0
    for key in ("high", "low", "visual"):
        if not groups[key]:
            continue
        lines += ["", f"## {titles[key]}"]
        for issue in groups[key]:
            n += 1
            parts = [f"{n}. p{issue['page']} {issue['type']}", issue["reason"]]
            if issue.get("latex"):
                parts.append(f"`{issue['latex']}`")
            if issue.get("text"):
                parts.append(issue["text"])
            parts.append(issue.get("crop", "(no crop)"))
            lines.append(" — ".join(parts))
    if not issues:
        lines.append("No issues found.")

    # Accent swaps in display equations are mechanical: write them as a ready fixes file for fix.py.
    fixes = [i for i in issues if i.get("fix")]
    autofix_path = os.path.join(folder, "qa_autofix.txt")
    if fixes:
        atomic_write(autofix_path, "".join(
            f"p{i['page']} {' '.join(sorted(eq_labels(i['fix'][0])))} {i['reason']}\n- {i['fix'][0]}\n+ {i['fix'][1]}\n\n"
            for i in fixes))
        lines.insert(1, f"Accent swaps in {len(fixes)} display equations are written to qa_autofix.txt "
                        f"(apply with fix.py after a look at one or two crops).")
    elif os.path.exists(autofix_path):
        os.remove(autofix_path)

    atomic_write(os.path.join(folder, "qa_report.md"), "\n".join(lines) + "\n")
    return counts


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("folder", help="output folder from convert.py (contains <name>.blocks.json)")
    ap.add_argument("--threshold", type=float, default=70, help="minimum overall text similarity (0-100)")
    ap.add_argument("--max-dropped", type=int, default=3, help="flag a block when this many PDF words are missing")
    ap.add_argument("--max-symbol-diff", type=int, default=8,
                    help="largest two-sided equation symbol difference treated as a misread (bigger = box mismatch)")
    ap.add_argument("--dpi", type=int, default=150, help="resolution of crop images")
    a = ap.parse_args()
    if not os.path.isdir(a.folder):
        raise PdfError("NOT_FOUND", f"chapter folder not found: {a.folder}", "check the converted chapter folder path")
    if not glob.glob(os.path.join(a.folder, "*.blocks.json")):
        raise PdfError("NOT_FOUND", f"block JSON not found in {a.folder}",
                          "use a convert.py output folder containing .blocks.json")
    run(a.folder, a.threshold, a.max_dropped, a.max_symbol_diff, a.dpi)


# Command wrapper registered in p8_md.py.
