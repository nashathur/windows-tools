# pdf2md

Local PDF → Markdown conversion with math, plus automatic QA so only problem spots need a human (or Claude) to look at them.

## Usage

All scripts run in the `pdfmath` env: `conda run -n pdfmath python C:\Tools\pdf2md\<script> …`

```powershell
# Look at the PDF: page count, text layer, outline
inspect_pdf.py book.pdf

# Whole book (or some chapters): plan first, then run. Converts, runs QA, writes 00_index.md
book.py book.pdf out --list
book.py book.pdf out --chapters 2,5-7

# Single document or page range (qa.py also runs MinerU on flagged equations; --no-mineru to skip)
convert.py book.pdf out --page-range 54-85 --name ch02
qa.py out\ch02

# MinerU second opinion on chapters QA checked earlier (book.py/reverify.py/qa.py do this themselves)
crosscheck.py out --all

# Apply review corrections from a fixes file (format below), logging them to review_notes.md
fix.py out\ch02 fixes.txt --dry-run
fix.py out\ch02 fixes.txt
```

`--page-range` is 0-indexed (`"0-24"`, `"0,5-10"`). Page numbers in reports and fixes files are 1-based PDF pages.

Purpose: the Markdown is only used as Claude-project sources. Priorities: no lost text, correct equations, clear headings; not rendering or cosmetics. `PDF2MD_TEXT_FONT_MATH=1` makes convert.py OCR math set in the text font (publisher PDFs) as LaTeX; off by default (Claude reads `*T*Peak` fine; +30–40% time).

## Files

| File | What it does |
|---|---|
| `inspect_pdf.py` | Page count, metadata, pages without a text layer, PDF outline with page numbers. |
| `book.py` | Chapters from the PDF outline (the level whose entries are ~25 pages, or `--level`); converts them with the models loaded once, runs QA, writes `00_index.md` with QA counts. Skips chapters already done (resume), `--force` to redo, `--chunk N` for PDFs without an outline. Fixes wrong outline links (title not on the linked page → moves to the page that starts with it; e.g. C-R&B "Index" pointing at References) and adds pages outside every chapter as their own entries (front matter = chapter 0, so chapter numbers match the book). |
| `convert.py` | Runs Marker once and saves Markdown and block JSON (types, HTML, positions), then runs `postprocess.py`. Works around a Surya bug on Windows that sent Ctrl+C to the whole console when Marker finished. **Keeps paragraphs that Marker's inline-math OCR loses**: on equation-dense pages the OCR of the paragraph below an equation often returns only a garbled copy of that equation; when the OCR text misses most of the PDF words, the PDF text layer is kept instead (inline math then comes out as Unicode/`<sup>`, not LaTeX). The count is printed ("N paragraphs kept from the PDF text"). |
| `postprocess.py` | Turns equations that Marker rendered as Markdown tables into `$$…$$` display math (real tables left alone, KaTeX-checked). Then tidies the Markdown: headings by numbering (`# Chapter N: Title`, `## N.M`, `### N.M.K`, sentence fragments like "… yields" demoted to text), equation numbers as `\tag{N.M}` (KaTeX-checked), Marker's `<span id="page-…">` anchors and `#page-…` links removed, single spaces around inline math. Can also be run on an existing `.md`. |
| `verify.py` | **Corrections checked against the PDF text layer, done by the GPU instead of a reviewer** (called by `convert.py` before rendering). Per display equation: truncated/missing equation number from the PDF; accent swaps; an equation whose symbols don't match the PDF is read again 5 times (different crops, greedy and sampled decoding: Marker's OCR varies, so each reading is a new chance) and the reading closest to the PDF is kept if it keeps the number(s) and passes KaTeX; duplicate copies of one equation removed. Paragraphs: inline accent swaps; paragraphs restored from the PDF text are read again without the equation above them. Set `PDF2MD_SAVE_UNVERIFIED=1` to also save `<name>.unverified.blocks.json` (before/after from the same OCR run). |
| `reverify.py` | Applies verify.py's equation corrections to chapters converted earlier (`reverify.py out\ch02` or `reverify.py out --all`): updates `.blocks.json` and `.md` in place, re-runs QA; ~35 s per chapter. Better than converting again, which would re-roll every equation. |
| `export.py` | **Final step: lean source files for a Claude project** (the only use of the Markdown; not reviewed by a person). Puts PDF text that is missing from the Markdown back from the PDF text layer, next to its paragraph and marked `*(recovered from PDF p. N)*` (C-R&B: 143 passages, ~2,400 words; text already present in another form is skipped); adds `*(possible OCR error in this equation; see PDF p. N)*` after equations QA still flags; drops image links, HTML leftovers, ORCID links, review comments; adds a source line with the PDF page range; skips the Index (`--skip` for more); prints ~tokens, recovered words and marked equations per file. The chapter .md files are not changed. |
| `crosscheck.py` | **Second opinion from MinerU** (separate `mineru` env, run as a subprocess; ~5 s/page plus ~30 s startup) on display equations QA flags (symbols, accents, bold, KaTeX, visual check). MinerU reads only the flagged pages, and its equation at the same place is compared with Marker's and with the PDF text: same reading → flag moved to lower confidence; MinerU matches the PDF where Marker doesn't → its LaTeX with Marker's equation number goes to `qa_mineru_fixes.txt` (fix.py format); otherwise MinerU's reading is shown next to the flag. Readings are cached in `mineru.json`, so re-running QA doesn't run MinerU again. MinerU and Marker don't fit on the 8 GB GPU together (~3 GB + ~2 GB on top of ~3.5 GB used by Windows and apps), and MinerU on the CPU is ~5× slower, so book.py and reverify.py run it once after their last chapter, after freeing Marker's GPU memory (`--no-mineru` to skip). Measured on C-R&B ch. 2: 10 of the 31 flagged equations got a fix (all matching the manual review), 8 demoted. |
| `qa.py` | Flags problems (below), crops each flagged block from the PDF, and writes ready fixes for accent swaps to `qa_autofix.txt`. Crops follow the ink, not Marker's box: slices of neighbouring lines and, for equations, body-text lines inside Marker's box are left out; lines and underbrace labels the box cuts off are put back. |
| `fix.py` | Applies corrections from a short fixes file: whitespace-insensitive match that must be unique, adds `<!-- reconstructed: verify against p.N -->`, logs to `review_notes.md`. Accepts equations as QA reports them (`\quad (2.31)`) when the Markdown has `\tag{2.31}`. |
| `validate_math.js` | KaTeX validator used by `qa.py` and `postprocess.py` (Node.js; `katex` in `node_modules`). |

## What qa.py checks

- **LaTeX** that KaTeX can't render (inline and display).
- **Display equations vs the PDF**: symbols (letters, digits, Greek, `+ − = < > ≤ ≥ ± × ≠`; case-sensitive, order ignored) and **bold** (PDF bold fonts such as CMBX/…-Bold vs `\mathbf`/`\boldsymbol`). Catches `cos`↔`sin`, `RO`↔`Ro`, `ũ`↔`ṽ`, wrong signs, lost or spurious bold.
- **Inline math vs the PDF**: small symbol swaps only (catches some misreads, very few false alarms).
- **Duplicated equations**: overlapping boxes with near-identical LaTeX.
- **Numbering** of equations, figures, tables: duplicates, out of order, other chapter, gaps.
- **Accents** in display and inline math vs the PDF's accent glyphs (˜ ¯ ˆ ˙ ¨ ˚): Marker often writes `\bar` for `\tilde` and `\hat`/`\dot` for `\mathring`. Clean swaps in display equations go to `qa_autofix.txt` (fix.py format); look at one or two crops, then `fix.py <chapter> <chapter>\qa_autofix.txt`.
- **Equation numbers** vs the number printed next to the equation in the PDF: truncated (`(4.19)` → `(4.1)`) or dropped when Marker merges two equations. A numbering gap is "likely" when the PDF prints that number at a line end. When Marker's box is off, the equation is compared in a box centred on its printed number (fewer "visual check" items).
- **PDF text missing from Marker**: runs of ~7+ words of the PDF's body text (body font only, figures and headers ignored) found nowhere in Marker's text for the page and its neighbours. Catches lost paragraphs, lost reference/contents entries.
- **Display equation left as plain text**: a text block ending in an equation number that is printed flush right in the PDF.
- **Text** words added (misreads) or lost vs the PDF text layer; letters in `<sup>/<sub>`; equation blocks without LaTeX; garbled characters.

`qa_report.md` is compact (one line per issue: page, reason, LaTeX, crop) in three sections: **Likely errors**, **Lower confidence** (e.g. a difference explained by a neighbouring line), **Visual check** (equations that couldn't be compared). Full details are in `qa_report.json`.

## Fixes file format

```
p56 (2.3b) cos -> sin
- y = -X \cos \Omega t
+ y = -X \sin \Omega t

p57 (2.8a) second line duplicated by Marker
-- \underbrace{(X \cos \Omega t + Y \sin \Omega t)}_V

ok p65 (2.20b) flag came from the line above
```

`- old` / `+ new` replaces, `-- old` deletes the whole line, `ok` only logs. Each "old" must occur exactly once (spacing ignored); a block with any non-matching edit is skipped and reported.

## Measured on Chapter 2 of Cushman-Roisin & Beckers (2006 LaTeX/dvips PDF)

- Marker got about 1 in 10 display equations wrong, plus lost bold on vectors (Ω, u).
- QA flagged all known errors; about 80% of "Likely errors" are real.
- Inline-math check: 2 of 4 planted symbol swaps caught, no false alarms on the real chapter.

## Compared with Mathpix on the whole of Cushman-Roisin & Beckers (Oct 2026)

Equations matched by number (~1,440), PDF text layer as referee (`C:\gfdwork\compare.py`):
- Plain Marker had wrong symbols in 176 equations (~12%), Mathpix in 90 (~6%). QA flagged 159 of Marker's 176 (90%). After verify/reverify: 74 (~5%), fewer than Mathpix.
- Systematic Marker errors QA was blind to before: accent swaps (`\bar`↔`\tilde`, `\hat`↔ring), truncated equation numbers (`(4.19)`→`(4.1)`, one `(8.464)` that used to produce 418 "missing" lines), paragraphs replaced by garbled equations (28 restored by convert.py; text flagged missing went from ~1,800 to ~770 words, a third of it in contents/references/index). All are now checked or repaired.
- Marker's OCR is not deterministic (llama.cpp batching): the same chapter gives a different set of wrong equations on each run, so compare versions on the same OCR run (`PDF2MD_SAVE_UNVERIFIED`) or over several runs. verify.py turns this into an advantage by reading doubtful equations several times.
- reverify.py (two passes) on the whole book, same output before → after: wrong display equations 176 → 74 (Mathpix: 90 where the local output is closer to the PDF), agreeing equations 1,152 → 1,271, QA likely errors 798 → 635 and visual checks 205 → 169. postprocess.py also unwraps pages Marker had turned into fake tables (margin notes): 21 places in 9 chapters.
- Still not checked: structure inside an equation (e.g. `\sqrt{\frac{Δρ}{ρ_0}} gH` vs `\sqrt{\frac{Δρ}{ρ_0} gH}`), which Marker gets wrong now and then.
- Marker's prose is as good as Mathpix's (word-level misreads are rare on both sides; Mathpix had more). Mathpix has cleaner tables of contents, references and index (Marker drops some entries and turns the two-column index into table rows).

## Limits

- Text, symbol, operator and bold checks need a text layer. Scanned pages only get the LaTeX and numbering checks; they're listed at the top of the report.
- Where a symbol sits (numerator vs denominator, sub- vs superscript) is not checked.
- Handles old-TeX PDF quirks: split hyphenated words, `∆` vs `Δ`, `\underbrace` glyphs (`z }| {`), `∫` → `Z`, `\neq` as `6=`, `\varrho` as `%`, long arrows as `−`/`=`. Other PDF producers may need similar tweaks.
- Options: `qa.py --threshold 70 --max-dropped 3 --max-symbol-diff 8 --dpi 150`.
