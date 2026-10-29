# pdf2md

PDF → Markdown with math, without a GPU and without Claude tokens: text comes from the PDF itself, display equations are read by Codex (ChatGPT plan) and checked against the PDF text layer; what still fails after three readings is marked in the file. The tools' verdict is final (no review step). The Markdown is read only by Claude (Claude-project sources): no lost text and correct equations matter, cosmetics don't.

## Usage

All scripts run in the `dev` env: `conda run -n dev python C:\Tools\pdf2md\<script> …`

```powershell
inspect_pdf.py book.pdf                              # page count, text layer, outline
book.py book.pdf out --list                          # chapter plan from the outline
book.py book.pdf out --chapters 2,5-7                # convert, read equations, QA, 00_index.md
convert.py paper.pdf out --page-range 0-24 --name ch01   # one document or page range (same steps)
eqread.py out\ch02                                   # read equations again after a change (only what is unread)
fix.py out\ch02 fixes.txt --dry-run                  # manual corrections (rarely needed), logged to review_notes.md
export.py out out_project --title "Author, Title"    # lean project files, ~tokens per chapter
```

`--page-range` is 0-indexed (`"0-24"`, `"0,5-10"`); page numbers in reports and fixes files are 1-based PDF pages. `--no-read` (book.py, convert.py) stops before Codex; equations then stay as the PDF's own text, marked "equation not read yet".

## Files

| File | What it does |
|---|---|
| `inspect_pdf.py` | Page count, metadata, pages without a text layer, PDF outline with page numbers. |
| `book.py` | Chapters from the PDF outline (the level whose entries are ~25 pages, or `--level`; `--chunk N` without an outline). Fixes wrong outline links (title not on the linked page → the page that starts with it) and adds pages outside every chapter as their own entries (front matter = chapter 0). Converts the chosen chapters, then runs eqread.py on all of them (up to 3 Codex runs at once), then QA; writes `00_index.md`. Finished chapters are skipped (`--force` to redo). |
| `convert.py` | pymupdf4llm with the pymupdf-layout model (CPU): text, headings, tables, lists from the PDF text layer; Tesseract on pages without one. Running heads, page numbers and figure labels are dropped; a caption printed inside a figure's area is kept. Each `formula` box (boxes that overlap are merged; a "formula" that is really a line of prose stays text) is cropped out to the edges of the text column it sits in (two-column pages stay apart), so the equation number and a start the box missed are included. Writes `<name>.src.md` (one `<!--eq:ID-->` line per equation), `eqs.json` (box, crop, the PDF's text and bold letters for each equation, readings), `eqs/`, `<name>.pages.json`, and renders `<name>.md` and `<name>.blocks.json` (the shape qa.py and export.py read). Re-extraction keeps readings of equations whose box, PDF text and crop image are unchanged. |
| `eqread.py` | Equations → LaTeX. Pass 1: all unread crops of a chapter on numbered sheets (E1, E2, …, ~10 per image) in one Codex run (`quick` role at medium effort), each with its PDF text as a hint. Every reading is checked with qa.py's display-equation check (symbols, bold, accents, equation number vs the PDF text) and KaTeX. Pass 2 (`review` role, high effort): equations with any mismatch, low-confidence ones included, told the earlier reading and what differs. Pass 3 (`review`, xhigh): what still fails. Where the PDF text can't be compared (scans, glyphs like ‖ printed as `k`) passes 2 and 3 read blind and the last two readings must agree (same LaTeX up to spacing and equivalent notation). Best reading kept: one the PDF can check over one it can't, then fewest mismatches. Equations still failing get status `doubtful` and a "possible OCR error" note in the `.md`. Readings are saved after each pass (an interrupted run resumes); Codex runs for up to 3 chapters at once, all PDF access stays in one thread; chapters with nothing to read are not re-rendered. |
| `qa.py` | Flags problems (below), crops each flagged block, writes ready fixes for accent swaps to `qa_autofix.txt`. |
| `postprocess.py` | Markdown tidying used by convert.py's render: headings by numbering (`# Chapter N: Title`, `## N.M`, `### N.M.K`, sentence fragments demoted), single equation numbers as `\tag{N.M}` (KaTeX-checked), repeated paragraphs and margin notes dropped. |
| `fix.py` | Manual corrections from a fixes file (format below), not part of the normal run: whitespace-insensitive unique match, adds `<!-- reconstructed: verify against p.N -->`, logs to `review_notes.md`. convert.py and eqread.py re-render `<name>.md` when they change a chapter, which drops such edits; a recurring error belongs in the tools instead. |
| `export.py` | Final step: lean project files. Puts PDF text missing from the Markdown back next to its paragraph, marked `*(recovered from PDF p. N)*`; marks equations QA still flags; drops image links and HTML leftovers; adds a source line with the PDF page range; skips the Index (`--skip` for more); prints ~tokens per file. |
| `validate_math.js` | KaTeX validator used by qa.py, eqread.py and postprocess.py (Node.js; `katex` in `node_modules`). |
| `marker/` | The old GPU workflow (Marker 2.0 + Surya in env `pdfmath`: convert, verify, reverify, MinerU crosscheck, book), archived unused. |

## What qa.py checks

- LaTeX that KaTeX can't render.
- Display equations vs the PDF: symbols (letters, digits, Greek, `+ − = < > ≤ ≥ ± × ≠`; case-sensitive, order ignored), bold (PDF bold fonts vs `\mathbf`/`\boldsymbol`), accents (˜ ¯ ˆ ˙ ¨ ˚ glyphs vs `\tilde`, `\bar`, …), equation numbers printed beside the equation. A one-sided difference of up to 3 symbols explained by a neighbouring line is "lower confidence".
- Duplicated equations, numbering of equations, figures and tables (duplicates, order, gaps).
- PDF text missing from the Markdown: runs of ~7+ body-text words found nowhere in the page and its neighbours.
- Text words added or lost vs the PDF text layer; garbled characters (glyphs without Unicode in the PDF).

`qa_report.md` is one line per issue (page, reason, LaTeX, crop) in Likely errors, Lower confidence, Visual check. Full details in `qa_report.json`. Pages without a text layer only get the LaTeX and numbering checks; they are listed at the top.

## Fixes file format

```
p56 (2.3b) cos -> sin
- y = -X \cos \Omega t
+ y = -X \sin \Omega t

p57 (2.8a) second line duplicated
-- \underbrace{(X \cos \Omega t + Y \sin \Omega t)}_V

ok p65 (2.20b) flag came from the line above
```

`- old` / `+ new` replaces, `-- old` deletes the line, `ok` only logs. Each "old" must occur exactly once (spacing ignored); a block with a non-matching edit is skipped and reported.

## Measured (Cushman-Roisin & Beckers, 2006 LaTeX/dvips PDF; Mathpix as second reader, PDF text as referee via C:\gfdwork\compare.py)

Oct 2026, chapters 2 and 3 (PDF pp. 55–106, 113 display equation boxes, 105 numbers), final code in one fresh run, against the old Marker output after verify/reverify:

| | Marker (GPU) | This workflow |
|---|---|---|
| Numbered equations missing | ch. 3: 1 | 0 |
| Equation boxes with wrong symbols (Mathpix closer to the PDF) | ch. 2: 5 of 75, ch. 3: 1 of 37 | 0 of 59, 0 of 35 (the 2 remaining differences are Mathpix's errors) |
| Equations marked "possible OCR error" | — | ch. 2: 2 of 70, ch. 3: 1 of 43 (glyph quirks such as ∇ printed as "5"; the readings were right) |
| PDF words lost | ch. 2: 16, ch. 3: 4 | 0, 0 |
| QA likely errors left | ch. 2: 23 | 3 and 3, none in equations (a number out of order in the book itself, glyphs without Unicode) |
| Time | ~5 s/page on the GPU (ch. 2: ~2.5 min) | both chapters 128 s: ~10 s extraction each, Codex passes for both chapters side by side |
| Claude tokens | reading qa_report.md and crops: ~10k+ per chapter | 0 |

Earlier development runs of the same chapters gave one real error (an extra `+` before an `\underbrace` in (2.4)), since handled by the prompt rule on printed signs. Codex readings vary between runs; the PDF check and re-reads absorb that.
Scanned pages (three pages rendered to images): Tesseract text, all 12 equations read; 9 confirmed by two agreeing blind readings, 3 marked (85 s).

## Limits

- The equation check compares symbols, bold, accents and numbers, not where a symbol sits (numerator vs denominator, sub- vs superscript, extent of a `\sqrt`). Two readings agreeing is the only guard there.
- Inline math stays as the PDF's Unicode text (`d²X/dt²` as `d<sup>2</sup>X/dt<sup>2</sup>`, Ω, ũ); bold on vectors inside sentences is lost. Claude reads it fine.
- Scanned pages: Tesseract text (occasional run-together words), equations confirmed only by two agreeing readings.
- Old-TeX PDF quirks handled by the check: `∆` vs `Δ`, `\underbrace` glyphs (`z }| {`), `∫` → `Z`, `\neq` as `6=`, `\varrho` as `%`. Not handled: `‖` printed as `k` (such equations fall back to two agreeing readings).
