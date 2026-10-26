"""Convert a PDF with Marker in one pass, saving both Markdown (to read) and JSON (block positions, for qa.py).

Run in the pdfmath env:
    conda run -n pdfmath python C:\\Tools\\pdf2md\\convert.py book.pdf out_dir [--page-range 0-24] [--name ch01]

Writes to out_dir/<name>/:
    <name>.md, images, <name>_meta.json   (same as marker_single, plus equation tables -> display math)
    <name>.blocks.json                     (block types, HTML and bounding boxes, used by qa.py)
"""

import argparse
import json
import os
import re
import sys
import time
from pathlib import Path

os.environ.setdefault("GRPC_VERBOSITY", "ERROR")
os.environ.setdefault("GLOG_minloglevel", "2")

import psutil
import surya.inference.backends.spawn as surya_spawn

from marker.config.parser import ConfigParser
from marker.models import create_model_dict
from marker.output import save_output
from marker.processors.equation import EquationProcessor
from marker.schema import BlockTypes
from marker.renderers.json import JSONRenderer
from marker.renderers.markdown import MarkdownRenderer

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "common"))
import clierr

import verify
from postprocess import clean_markdown, fix_equation_tables


def stop_process_windows_safe(pid, name):
    """Replacement for surya's _stop_process. Surya checks "still alive?" with os.kill(pid, 0),
    which on Windows sends Ctrl+C to every process in the console (killing the calling shell)."""
    try:
        proc = psutil.Process(pid)
        proc.terminate()
        proc.wait(timeout=10)
    except psutil.NoSuchProcess:
        pass
    except psutil.TimeoutExpired:
        proc.kill()


surya_spawn._stop_process = stop_process_windows_safe


# Inline-math OCR of a paragraph sometimes returns only a (garbled) copy of the display equation above it, and
# the paragraph's text is lost (C-R&B: 27 paragraphs in the book, about 1 in 150 text blocks, right below an equation). When the OCR
# result has almost no words but the PDF text layer has a paragraph there, keep the PDF text (inline math then
# comes out as Unicode, not LaTeX) instead of losing the paragraph.
_original_equation_call = EquationProcessor.__call__
MIN_PROSE_WORDS = 8  # PDF words (3+ letters) needed before an OCR result without prose is rejected
repaired_blocks = []


def prose_word_count(text):
    return len(re.findall(r"[A-Za-z]{3,}", text))


def math_as_words(latex):
    """Each math variable with its subscript as one word ("T_{\\text{peak}}" -> "Tpeak"), the way a PDF that sets
    math in the text font spells it."""
    s = re.sub(r"\\(?:text|mathrm|operatorname)\s*\{([^{}]*)\}", r"\1", latex)
    return " ".join(re.findall(r"[A-Za-z]+", re.sub(r"[_{}]", "", re.sub(r"\\[A-Za-z]+", " ", s))))


def ocr_sequence(block_html):
    """Lowercase words (3+ letters) of an OCR result in reading order, math written as words."""
    text = re.sub(r"<math\b[^>]*>(.*?)</math>", lambda m: " " + math_as_words(m.group(1)) + " ", block_html, flags=re.S)
    return [w.lower() for w in re.findall(r"[A-Za-z]{3,}", re.sub(r"<[^>]+>", " ", text))]


def ocr_words(block_html):
    return set(ocr_sequence(block_html))


OPERATOR_WORDS = {"sin", "cos", "tan", "exp", "log", "max", "min", "sup", "inf", "lim", "sinh", "cosh", "tanh", "det"}


def equation_call_keep_prose(self, document):
    before = {}
    for page in document.pages:
        for block in page.contained_blocks(document, self.inline_math_block_types):
            if not block.html and block.structure:
                pdf_text = block.raw_text(document)
                before[block.id] = (list(block.structure), pdf_text)
                # The OCR crop is the block's box, which can end above the paragraph's last text line (the line
                # was then cut off). Make the box cover all of the block's lines.
                lines = [document.get_block(i) for i in block.structure]
                lines = [l.polygon for l in lines if l is not None and l.polygon is not None]
                if lines:
                    block.polygon = block.polygon.merge(lines)
                # Marker's output budget is the layout model's token estimate + 100: long paragraphs got cut off
                # once their math is written as LaTeX. Budget at least half a token per character + 200.
                block.layout_token_count = max(block.layout_token_count or 0, len(pdf_text) // 2 + 200)
    _original_equation_call(self, document)
    for block_id, (structure, pdf_text) in before.items():
        block = document.get_block(block_id)
        if not block.html or block.structure:
            continue
        ocr_text = re.sub(r"<math\b.*?</math>|<[^>]+>", " ", block.html, flags=re.S)
        pdf_list = [w.lower() for w in re.findall(r"[A-Za-z]{3,}", pdf_text)]
        ocr_set = ocr_words(block.html)
        coverage = sum(w in ocr_set for w in pdf_list) / max(1, len(pdf_list))
        # Stops early: of the paragraph's last three real words (4+ letters, no operator names; a paragraph may end in
        # math that the OCR writes as LaTeX), fewer than two are in the last third (at least 12 words) of the OCR text.
        tail = [w for w in pdf_list if len(w) >= 4 and w not in OPERATOR_WORDS][-3:]
        seq = ocr_sequence(block.html)
        end = set(seq[-max(12, len(seq) // 3):])
        truncated = len(tail) == 3 and sum(w in end for w in tail) < 2
        # Reject when the OCR text has (almost) no words, misses half of the paragraph's words, or stops early.
        if len(pdf_list) >= MIN_PROSE_WORDS and (prose_word_count(ocr_text) < 0.3 * len(pdf_list) or coverage < 0.5
                                                 or truncated):
            if os.environ.get("PDF2MD_DEBUG"):
                print(f"  kept PDF text {block_id}: words {prose_word_count(ocr_text)}/{len(pdf_list)}, "
                      f"coverage {coverage:.2f}, truncated {truncated} | PDF: {pdf_text[:80]!r} | OCR: {block.html[:120]!r}")
            block.html, block.structure = None, structure
            repaired_blocks.append(block_id)


EquationProcessor.__call__ = equation_call_keep_prose


# Marker only OCRs a paragraph for inline math when it sees TeX math fonts or math Unicode. Publisher PDFs
# (Springer, Elsevier...) set math in the text font, so "T_Peak" stayed as "*T*Peak". Also treat as inline math:
# an italic span of 1-2 letters (a variable) or a subscript span with letters. Superscript digits (citations) don't
# count. The paragraph is then OCR'd; equation_call_keep_prose keeps the PDF text if the OCR loses words.
_original_has_inline_math = EquationProcessor._block_has_inline_math
VARIABLE = re.compile(r"[A-Za-zͰ-Ͽ]{1,2}")


def has_inline_math_incl_text_font(self, block, document):
    if _original_has_inline_math(self, block, document):
        return True
    for span in block.contained_blocks(document, (BlockTypes.Span,)):
        text = (span.text or "").strip()
        if span.italic and VARIABLE.fullmatch(text):
            return True
        if span.has_subscript and re.search(r"[A-Za-z]", text):
            return True
    return False


# Off by default: the files are sources for a Claude project, where "*T*Peak" reads as well as LaTeX, and the extra
# OCR costs ~30-40% more time on such papers. Set PDF2MD_TEXT_FONT_MATH=1 to get LaTeX inline math there.
if os.environ.get("PDF2MD_TEXT_FONT_MATH"):
    EquationProcessor._block_has_inline_math = has_inline_math_incl_text_font


def block_to_dict(block):
    """Plain-dict copy of a JSONBlockOutput without its images (already saved as files by save_output).
    pydantic's model_dump() fails on blocks with images ("unhashable type: 'dict'")."""
    return {
        "id": block.id,
        "block_type": block.block_type,
        "html": block.html,
        "polygon": block.polygon,
        "bbox": block.bbox,
        "section_hierarchy": block.section_hierarchy,
        "children": [block_to_dict(c) for c in block.children] if block.children else None,
    }


def convert(pdf, out_dir, page_range=None, name=None, models=None, verify_pdf=True):
    """Convert one PDF (or page range) into out_dir/<name>/. Pass `models` (from create_model_dict()) to reuse
    loaded models across calls, as book.py does. Returns the output folder."""
    if not os.path.isfile(pdf):
        raise clierr.Fail("NOT_FOUND", f"{pdf} not found", "check the PDF path")
    name = name or os.path.splitext(os.path.basename(pdf))[0]
    out_folder = os.path.join(out_dir, name)
    os.makedirs(out_folder, exist_ok=True)

    start = time.time()
    models = models if models is not None else create_model_dict()
    parser = ConfigParser({"output_format": "markdown", "output_dir": out_dir, "page_range": page_range})
    converter = parser.get_converter_cls()(
        config=parser.generate_config_dict(),
        artifact_dict=models,
        processor_list=parser.get_processors(),
        renderer=parser.get_renderer(),
    )

    # Build the document once, correct it against the PDF text layer (verify.py), then render it twice.
    repaired_blocks.clear()
    document = converter.build_document(pdf)
    if os.environ.get("PDF2MD_SAVE_UNVERIFIED"):  # for measuring verify.py: same OCR run, before corrections
        before = converter.resolve_dependencies(JSONRenderer)(document)
        with open(os.path.join(out_folder, f"{name}.unverified.blocks.json"), "w", encoding="utf-8") as f:
            json.dump({"source_pdf": os.path.abspath(pdf), "pages": [block_to_dict(p) for p in before.children]}, f)
    fixes = verify.correct(document, pdf, models["recognition_model"], EquationProcessor(models["recognition_model"]),
                           list(repaired_blocks)) if verify_pdf else {}
    markdown = converter.resolve_dependencies(MarkdownRenderer)(document)
    blocks = converter.resolve_dependencies(JSONRenderer)(document)

    save_output(markdown, out_folder, name)

    # Equations Marker rendered as Markdown tables -> display math; headings, \tag numbers, anchors, spacing
    # tidied (see postprocess.py).
    md_path = os.path.join(out_folder, f"{name}.md")
    with open(md_path, encoding="utf-8") as f:
        md_text, n_tables, n_kept = fix_equation_tables(f.read())
    md_text = clean_markdown(md_text)
    with open(md_path, "w", encoding="utf-8") as f:
        f.write(md_text)
    blocks_json = json.dumps({"source_pdf": os.path.abspath(pdf), "pages": [block_to_dict(p) for p in blocks.children]})
    with open(os.path.join(out_folder, f"{name}.blocks.json"), "w", encoding="utf-8") as f:
        f.write(blocks_json)

    print(f"Saved {name}.md and {name}.blocks.json to {out_folder} ({len(document.pages)} pages, {time.time() - start:.0f} s); "
          f"{n_tables} equation tables converted to display math" + (f", {n_kept} kept (failed KaTeX)" if n_kept else "") +
          (f"; {len(repaired_blocks)} paragraphs kept from the PDF text (OCR returned only math)" if repaired_blocks else ""))
    if fixes:
        print("  verified against the PDF: " + ", ".join(f"{k} {v}" for k, v in fixes.items()))
    return out_folder


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("pdf")
    ap.add_argument("out_dir")
    ap.add_argument("--page-range", help='0-indexed pages, e.g. "0-24" or "0,5-10"')
    ap.add_argument("--name", help="output base name (default: PDF file name)")
    args = ap.parse_args()
    convert(args.pdf, args.out_dir, args.page_range, args.name)


if __name__ == "__main__":
    clierr.run("convert", main)
