## Read and report

Use `info FILE` for PDF triage or `info FOLDER` for one concise summary line per PDF. The folder line reports pages, bytes, title, encryption and whether text is present.

Use `pages FILE` to list each page's label, size, rotation, blank status and first text line. Use `text FILE` to extract text; add `--layout` to preserve positioned reading order. `count FILE` reports words, characters, paragraphs and lines by page; add `--total` for totals and pages without a text layer.

Use `json FILE` for page text blocks, lines, spans, font and size data, geometry in PyMuPDF points and displayed millimetres, and metadata. Use `html FILE` for per-page XHTML. Both commands accept `-o PATH` to write an export; otherwise output is bounded to the first 200 lines by the shared report protocol.

Use `size FILE` to inspect stored object sizes and per-page usage. Use `check FILE` for whole-file page measurements. Use `check FILE --against ORIG` to pair pages by text or image difference hash and compare them; `--map 1=1,2=3` supplies output-page to origin-page pairs when automatic matching is unsuitable.

Use `open FILE` to open the PDF in the default viewer. Read `report` as analysis to pass on; report `warn` findings to the user in words. Reports longer than 200 lines are saved and stdout gives the report path, line and word counts, and a preview; `--stdout` forces full printing.
