# pdftools plan

Local PDF toolkit that Claude runs on request: the pdfjar.com tool set plus folder helpers, as one CLI and one skill. Only Claude runs it (no GUI, no right-click menu). The toolkit never sends file content off the PC (see Privacy). Status: phases 1, 2, 3a, 3b, 3c, 4 and 6 built. Plan reviews are finished; remaining issues are caught per phase in build, tests and the phase review.

## Deliverables

- `C:\Tools\pdftools\pdf.py`: the single entry point. Internals in a package `C:\Tools\pdftools\pdftools\`: `cli.py` (parsers, job loader, output protocol), `registry.py`, `io.py` (staging, publishing, naming, temp folders, disk budget), `pipeline.py` (shared writer open/check/stage/publish contract), `secrets.py`, `crypt.py` (encryption matrix), `coords.py`, `pages.py` (page-range parsing), `textmatch.py` (literal and regex matches → per-line quads, used by `annotate` and `redact`), `check.py` (provenance diff), `tools.py` (external tool runner: timeouts, process trees, stderr capture), `errors.py`, and `commands\` with one module per phase or group.
- `C:\Tools\pdftools\pdf.cmd`: launcher (see Launcher).
- `C:\Tools\pdftools\tests\`: tests, `make_fixtures.py`, fixtures (see Tests).
- `C:\Users\thurs\.claude\skills\pdftools\SKILL.md`: request → command map, written per the `writing-for-agents` skill; grows with each phase.

## Phases

Each phase is its own build, Claude diff read, Codex phase review and commit. A command not in a shipped phase is absent from the CLI and the skill.

| Phase | Contents | Done when |
|---|---|---|
| 1 | Contracts below in code: launcher, parsers, job loader, output protocol, exit codes, errors/logs, secrets, encryption matrix, naming, staging/publishing, temp/disk budget, coordinates, tool runner. One command, `info FILE`, drives them end to end | contract unit tests pass; launcher tests pass from PowerShell 5.1 |
| 2 | Self-registering command registry, provenance + check engine, `textmatch.py`, `pages.py` (if not already in phase 1), `tools\build_skill.py`, `thumb`, `selftest`, capability report (`selftest --caps`), `make_fixtures.py` with discovery plus the shared and calibration fixtures | check engine fixture tests pass; thresholds calibrated and written into Measurements |
| 3a | Read and report: `info FOLDER`, `pages`, `text`, `count`, `json`, `html`, `size`, `check`, `open` | phase tests pass; 500-page benchmark fixture: `info`, `pages`, `text` each under 30 s wall time (excluding conda start-up), peak RAM under 1.5 GB |
| 3b | `organize` and its wrappers, `merge`, `split`, `interleave` | phase tests pass, incl. feature preservation |
| 3c | `toimg` (PNG, JPEG), `zip`, `dupes`, `compare` | phase tests pass |
| 4 | `meta`, `protect`, `unlock`, `bookmarks`, `labels`, `watermark`, `numbers`, `headfoot`, `annotate`, `flatten`, `crop`, `cut`, `resize`, `nup`, `booklet`, `gray`, `rename`, `repair`, `rmblank` | phase tests pass |
| 5 | `compress`, `ocr`, `batch` (serial, PDF→PDF steps) | phase tests pass |
| 6 | `office2pdf`, `toword` (layout), `md2pdf`, `txt2pdf`, `img2pdf`, `toimg` other formats, `images`, `imgtext`, `imginfo`, `imgdiff` | phase tests pass; integration tests run (not skipped) for every tool installed on this PC |
| H | Hardening (see Hardening): full-tools test profile, validator attack tests, semantic feature checks, real-world corpus sweep | each item's tests pass; corpus sweep has no new or worsened failures against its baseline |
| 7 | `sanitize`, `redact` | adversarial fixtures pass; security tests are never skippable |

### Parallel builds

| Wave | Phases | Runs |
|---|---|---|
| 1 | 1 | alone |
| 2 | 2 | alone |
| 3 | 3a, 3b, 3c, 4, 6 | in parallel, 2–3 at a time (review is the bottleneck) |
| 4 | 5, H | in parallel, after 3b and 4 are merged |
| 5 | 7 | after H (7's security postconditions build on H's validator tests) |

So that parallel builds never edit the same file:
- Commands self-register: each module in `pdftools\commands\` declares its commands with a decorator (`@command(name, phase, input_kind, output_kind, suffix, rewrites_pdf, check_policy)` plus an argument-builder function); `registry.py` imports every module in `pdftools\commands\` (pkgutil) and `cli.py` builds parsers from the registry. Adding a command never edits `registry.py` or `cli.py`.
- Each phase owns: `pdftools\commands\p<phase>_*.py`, `tests\test_p<phase>_*.py`, `tests\fixtures\p<phase>.py` (fixture builders; `make_fixtures.py` discovers them), `skill_parts\p<phase>.md` (its SKILL.md section). `tools\build_skill.py` (phase 2) assembles `SKILL.md` from `skill_parts\` in phase order.
- Shared code (phases 1–2: everything outside `commands\`, `tests\test_p*`, `tests\fixtures\p*`, `skill_parts\`) is frozen for wave 3 and 4 builders: a builder that needs a change there stops and reports it; Claude makes the change on main and the builders rebase.
- Phase 2 also delivers `textmatch.py` and `pages.py`, so phase 7 does not depend on phase 4's code.
- Commands split across phases: 3a adds the folder branch of `info` in `commands\p1_info.py` (no other phase touches it). Phase 6 extends 3c's `toimg` module (other formats) and 3a's `check` module (`--source`), so phase 6 starts after 3a and 3c are committed; 3c keeps its `toimg` writers in a format → writer table so phase 6 only adds entries.
- A module with several commands defines `add_arguments_<name>(parser)` per command (the registry prefers it over `add_arguments`); never dispatch on `parser.prog`, which job parsing does not set.
- Pipeline hooks added for phase 4 (`pipeline.write_pdf`): `encrypt_output={"user_password", "owner_password", "permissions"}` replaces the encryption matrix (`protect`; owner defaults to user), `decrypt_output=True` publishes unencrypted (`unlock`), `opener(path, password)` replaces `open_input` for inputs and the check (`repair`), `OperationResult.baseline` names a PDF the check uses instead of the input (`--flatten-annots`), and `context.temp` is the run's temp folder for such scratch files.
- `--each` (folders/globs, any command that offers it): one result record per file plus a `summary` record; a failing file is reported and skipped; exit 4 if any failed (as `batch --each`).

Backlog (not a phase; each item needs its own spec first): `toword --mode math` (pdf2md + pandoc, GPU), a GPU lock shared with pdf2md, `batch --jobs N`, composed batch checks, `img2pdf --scan`, animated GIF/WebP export, HEIC lossless export (`quality=-1`, `chroma=444`, after a round-trip test), `open --page` viewer arguments, `imgtext --surya`, PDF version conversion, PDF/A output, hidden-text and hidden-layer removal in `sanitize`.

## Contracts (phase 1)

### Launcher

`C:\Tools\pdftools\pdf.cmd`:

```bat
@echo off
set PYTHONUTF8=1
set PYTHONIOENCODING=utf-8
"C:\Users\thurs\miniconda3\Scripts\conda.exe" run -n dev --no-capture-output python C:\Tools\pdftools\pdf.py %*
```

- stdout and stderr are UTF-8 (`pdf.py` also reconfigures them, so direct runs match). The skill sets `[Console]::OutputEncoding = [Text.Encoding]::UTF8` and, before piping a job to `--job -`, `$OutputEncoding = [Text.Encoding]::UTF8` (PowerShell 5.1 pipes to native programs as ASCII otherwise; the UTF-8 pipe adds BOMs, which the job loader strips). Prefer `--job FILE`.
- Tests use `unittest` (pytest is not in dev): `conda run -n dev python -m unittest discover -s pdftools\tests -q`; launcher: `pdftools\tests\test_launcher.ps1`.
- PyMuPDF messages: `PYMUPDF_MESSAGE=logging:name=pymupdf,level=30` set in `pdf.py` before importing pymupdf, captured by a logging handler.
- cmd.exe can mangle `%`, `^`, `&`, `!` in arguments, and PowerShell 5.1 strips quotes from native arguments (memory `ps51-native-args-strip-quotes`). Rule: on the command line only the command name, the main input path and simple flags; everything else goes in a JSON job.
- Launcher tests (run from PowerShell 5.1): exit codes 0–4 pass through; `--job -` reads stdin through the launcher; `--password-file` on an encrypted fixture; paths with spaces, non-ASCII, `'`, and (inside a job) `% ^ & !`.

### Invocation and output protocol

- Every parser is a subclass of `argparse.ArgumentParser` with `allow_abbrev=False` and `error()` raising `BadArg`; usage text is never printed.
- stdout carries results only. Default: one human line per result. `--json`: JSON Lines, each record with `type`:
  - `result`: `{type, command, path, kind: pdf|image|docx|html|txt|json|zip|folder, pages, bytes, verdict, warnings: [codes]}`
  - `report`: read-only commands: `{type, command, input, verdict, data | path}`
  - `error`: `{type, command, code, message, fix, log, problems: [...]}`
  - `summary`: batch totals
- `verdict` is one of `ok`, `warn`, `report` (no built-in diff), `unchecked` (published as `_UNCHECKED`), `failed`.
- stderr carries `ERROR`/`WARN` lines, progress, estimates and timing (elapsed only when over 5 s).
- Before importing pymupdf, `pdf.py` routes PyMuPDF's messages away from stdout (`pymupdf.set_messages(pylogging=True)`, or `PYMUPDF_MESSAGE` set to stderr; pick the one that works in 1.28.2) into the log. After each document operation, drain `pymupdf.TOOLS.mupdf_warnings()` and pikepdf `Pdf.get_warnings()` into the log. Other library logging and Python warnings are silenced; `-v` sends them to stderr. Test: a damaged fixture with `--json` gives stdout that parses fully as JSON Lines.
- Bounded output: a report longer than 200 lines goes to `C:\Tools\pdftools\reports\<time>-<pid>-<command>-<stem>.txt` (keep last 50, git-ignored); stdout gets the path, line and word count and the first 20 lines. `--stdout` forces full printing. `text`, `json` and `html` write a file only with `-o`.
- `--dry-run` on every writing command: prints planned outputs (names marked provisional, since collisions are resolved at publish time), page map and warnings; creates no output or staging file. Logs and reports are still written.
- Long jobs print an estimate to stderr first (pages × measured s/page from a 2-page sample). Skill rule: estimate over 90 s → Claude runs it with `run_in_background`.

### Exit codes

| Situation | Exit | Verdict / output |
|---|---|---|
| Success | 0 | `ok`, `warn` or `report` |
| Error | 1 | nothing published |
| Bad usage, bad job, bad recipe | 2 | nothing published; `problems` lists every problem, the ERROR line shows the count and the first |
| Writing command, built-in check failed | 3 | published as `<name>_UNCHECKED.<ext>`, verdict `unchecked` |
| `sanitize`/`redact` check failed | 3 | nothing published, verdict `failed`; measurements in the log; `--keep-failed` publishes `<name>_FAILED_UNSAFE.pdf` |
| `check`, `compare` find differences | 0 | verdict `warn` |
| `compress` finds no gain | 0 | `path` = the input, warning `NO_GAIN`; a following batch step gets the input |
| Batch with any failed file | 4 | each file's record carries its own code |

### JSON jobs

The canonical agent interface; shorthand is a convenience for simple cases.

```json
{"version": 1,
 "input": "D:\\GFD NS-353B\\report.pdf",
 "password_file": "C:\\Tools\\_tmp\\pw.txt",
 "steps": [{"cmd": "organize", "recipe": [{"pages": "1-3"}, {"blank": "a4"}, {"pages": "5-", "rotate": 90}]}],
 "output": null}
```

- `pdf.cmd <command> --job FILE` or `--job -` (stdin). UTF-8, BOM allowed. `version` must be 1.
- Keys are the command's argparse `dest` names; unknown keys are `BAD_ARG`. With `--job`, only global flags (`-v`, `--json`, `--dry-run`, `--stdout`) may be given on the command line.
- Paths: relative paths resolve against the job file's folder (`--job -`: the current folder); on the plain command line, against the current folder.
- Before phase 5, a job with more than one step is `BAD_ARG ... | fix: run steps separately (batch arrives in phase 5)`.
- Shorthand grammar, organizer: items separated by `,`; item = `N | N-M | N- | -M | odd | even | rev` optionally followed by `@90|@180|@270` and/or `xK`, or `blank` / `blank=a4|letter|WxH`. No other files, colons or quotes in shorthand. Batch shorthand: steps separated by ` | `, each step only a command name and flags without spaces in values. Anything else needs a job.
- Nothing is ever run through a shell.

### Naming and where outputs go

| Output kind | Default |
|---|---|
| Edit of one PDF | `<stem><suffix>.pdf` beside the input; suffix from the registry: `_<command>` by default (`_compressed`, `_rotated`, `_organized`, `_numbered`, `_watermarked`, ...), `extract` uses `_p<ranges>` (`_p3-5`) |
| PDF from several inputs (`merge`, `interleave`) | `<first stem>_<command>.pdf` beside the first input |
| Export to another type (`office2pdf`, `md2pdf`, `txt2pdf`, `toword`, `img2pdf` with one input) | `<stem>.<ext>` beside the input |
| `img2pdf` with several inputs | `<folder name or first stem>_images.pdf` |
| `toimg` of one page | `<stem>_p<NNN>.<ext>` beside the PDF |
| Several files | folder `<stem>_<command>\` (`toimg`: `<stem>_<ext>\`) holding `<stem>_<part>.<ext>`; parts: pages `p001` (zero-padded to max(3, digits of the page count)), split `--every` `part01`, split `--ranges` `p1-3`, split `--bookmarks` the bookmark title made Windows-safe and cut to 60 characters |
| `compare A B` | `<A stem>_vs_<B stem>.pdf` beside B |
| `text`, `json`, `html` | stdout (bounded) unless `-o` |

- `-o PATH`: an existing folder, or a path ending in `/` or `\` (on every platform and for every command: `cli.main` reads the raw `-o` text, from argv or the job, and re-applies the marker as the native separator after argparse), means "default name in that folder"; otherwise the exact file path. Multi-file outputs require a folder. An exact path that exists gets `_2` with `WARN NAME_TAKEN`.
- Collisions: `_2`, `_3`, ... resolved at publish time; never overwrite.
- Never modify an input file. The one exception is `rename --apply` (names only, undoable).

### Staging, publishing, temp space

- Staging path in the destination folder: `.<name>.<pid>-<8 hex>.part<ext>`; writers get the format explicitly (`pixmap.save(path, output="png")`, Pillow `format=`). Multi-file outputs stage a folder `.<folder>.<pid>-<8 hex>.part\`.
- Order: write staged → close all handles → check → publish. Publish with `os.rename` (fails on Windows if the target exists): on `FileExistsError` try the next suffix; on `PermissionError` (Defender, indexer, OneDrive, Explorer thumbnailer) retry with backoff for 2 s, then `LOCKED`.
- On error, delete only this run's staging paths (tracked in memory). At start-up, delete `.*.<pid>-*.part*` entries in the destination folder whose pid is not running and that are older than 1 hour.
- Path length: if any staging, output or tool path exceeds 240 characters, stage in the run's temp folder instead and publish by copy + rename; if the final path itself exceeds 240, `BAD_ARG ... | fix: use a shorter -o`.
- Per-run temp folder `%TEMP%\pdftools\<pid>-<8 hex>\`, deleted in `finally` (and swept like staging when stale). Child processes get `TEMP`/`TMP` pointing into it.
- Disk budget, checked on both the destination drive and the temp drive before starting: edits 3 × input; merge 3 × sum of inputs; `toimg` pages × width × height × 4 bytes; `ocr`/`compress` scanned path 20 × input on the temp drive. Short → `TOO_BIG`.

### Secrets

- Sources: `--password-file FILE`, `--password-env NAME` (owner: `--owner-password-file`, `--owner-password-env`). In jobs: `password_file` / `password_env` / `owner_password_file` / `owner_password_env`. `--password VALUE` is refused (command lines are visible to other processes), and so is `--password -`: stdin from PowerShell 5.1 through `conda run` does not reach the program reliably. Equals forms (`--password=X`) are handled the same way. One trailing CR/LF is stripped from file values.
- Extraction happens first: secret flags are read from argv, and secret files/env read, before job parsing or validation, so every error path can redact.
- The log writer replaces each secret in its raw, JSON-escaped and `repr` forms with `***` in argv, job text, exception messages, tracebacks and child output, before writing. Console lines never contain secrets.
- Secrets never go on a child process command line, and password environment variables are removed from children's environment. External tools that cannot take a secret another way get a decrypted copy (see Encryption).
- Test: a password with quotes, backslashes, Unicode and an embedded newline, used across `protect`, `unlock`, an encrypted `organize`, a failing run and a Ghostscript compress, appears nowhere in stdout, stderr, logs, reports or child argv.

### Encryption and signatures

| Case | Behaviour |
|---|---|
| Read-only commands | open with the password; restrictions-only PDFs (empty user password, owner password set) open without one |
| Writing a PDF from an encrypted input | re-encrypted AES-256 with the same permissions and the given user password; owner password: the given one (`--owner-password-file/-env`), else a random 32-character one with `WARN OWNER_RESET` (restrictions kept, owner password not known) |
| Restrictions-only input with "no modify" or "no assemble" | writing commands refuse (`ENCRYPTED ... | fix: give --owner-password-file, or --ignore-restrictions`); `--ignore-restrictions` proceeds with `WARN RESTRICTIONS_IGNORED` and keeps the restrictions on the output |
| Several inputs, any encrypted (any order) | output encrypted with the settings of the first encrypted input; `WARN MIXED_ENCRYPTION` when inputs differ |
| Export to a non-PDF format (images, text, json, html, docx, zip of those) | allowed; `WARN DECRYPTED_EXPORT` |
| External tools (Ghostscript, ocrmypdf refuse encrypted input) | decrypt into the run's temp folder, run the tool, delete the copy in `finally`, re-encrypt the result per the rows above |
| `--decrypt-output` / `unlock` | output without encryption, on purpose |

- The checker opens outputs with the same credentials.
- Signatures: before a command that writes a new PDF derived from a signed PDF (registry flag "rewrites PDF"), detect signature fields with a value; refuse with `ERROR SIGNED ... | fix: add --break-signature if a modified copy is fine`; with the flag, `WARN SIGNATURE_INVALIDATED` (ocrmypdf also gets `--invalidate-digital-signatures`). Read-only commands, exports to other formats, `zip`, `rename` and `open` are not gated; exports note that the signature is not carried.

### Errors and logs

Follows the global one-line error rule in `C:\Users\thurs\.claude\CLAUDE.md` and is built on the shared helper `C:\Tools\common\clierr.py` (read its header for usage; extend the helper rather than duplicating it, e.g. for secret redaction and the extra log sections below). Tool name in the line: `pdf <command>`.

`ERROR pdf <command> <CODE>: <what happened> | fix: <what to do> | log: <path>` on stderr, one line.

- Codes: the shared ones (`NOT_FOUND`, `LOCKED`, `BAD_ARG`, `BAD_INPUT`, `TOOL_FAILED`, `TIMEOUT`, `INTERRUPTED`, `INTERNAL`) plus `BAD_PAGE`, `ENCRYPTED`, `WRONG_PASSWORD`, `SIGNED`, `REDACTIONS_PENDING`, `DAMAGED` (fix: `repair`), `UNSUPPORTED`, `TOO_BIG`, `CHECK_FAILED`.
- `WARN pdf <command> <CODE>: ... | fix: ...` for non-fatal issues.
- `conda run` appends its own `ERROR conda.cli.main_run ... failed` line after a non-zero exit; it adds nothing and the skill tells Claude to ignore it.
- Log in `C:\Tools\logs\pdftools\` (the helper's location; last 50 kept), named `<time>-<pid>-<command>.log`, sections `== command`, `== problems`, `== error`, `== traceback`, `== library warnings`, `== tool stderr (last 30 lines)`; all redacted.
- External tools: the ERROR line quotes the most relevant stderr line (first with error/fatal/failed/exception).
- `fix:` names the exact flag, command or file to try next; written for Claude.

### External tools

- Run through `tools.py`: argv list (never a shell), cwd in the run's temp folder, timeout, stdout/stderr captured.
- Timeouts: LibreOffice 120 s per file; Ghostscript max(120 s, 30 s/page); ocrmypdf max(120 s, 60 s/page); pdf2docx max(120 s, 20 s/page) (run in a child Python process so it can be stopped); browser/puppeteer 120 s.
- On timeout or Ctrl-C, kill only the launched process tree (psutil), then clean its temp files.
- Tool paths: `tools.find_tool` resolves the three tools with platform-specific names, first found wins: `ghostscript` = `gswin64c`, `gswin32c`, `gs` on PATH; `soffice` = `C:\Program Files\LibreOffice\program\soffice.com` (Windows), then `soffice` on PATH; `browser` = `msedge` on PATH, `C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe` (Windows), then `google-chrome`, `chromium`, `chromium-browser` on PATH. Every other tool (`tesseract`, `ocrmypdf`, `jbig2`, `pngquant`, `node`, ...) is `shutil.which` inside the dev env (Windows: `envs\dev\Library\bin\`; Linux: `envs/dev/bin/`). `selftest --caps` reports each with its version, the three under those keys.

### Coordinates

Three spaces, converted only in `coords.py`:
1. PDF user space: points, origin bottom-left of the media box, y up, scaled by `/UserUnit`.
2. PyMuPDF page space: points, unrotated, y down (what `get_text`, `search_for` and annotations use); `page.transformation_matrix` maps 1 → 2.
3. Displayed space: millimetres, origin top-left of the page as shown (after `/Rotate`, within the crop box); from 2 via `page.rotation_matrix` and the crop-box position (`page.cropbox_position`), then × 25.4/72 × UserUnit.

- User-facing coordinates (annotate, crop, clip, redact, check boxes) are displayed mm. `json` and `pages` report `bbox_pymupdf` (space 2) and `bbox_mm` (space 3).
- Fixture: `/Rotate 90`, crop box with a non-zero origin, `/UserUnit 2`; tests place an annotation, crop and clip and compare positions.

### Resource limits

- One page at a time; release each pixmap (`pix = None`). Text checks use `get_text("words")`.
- Caps: render 100 MP per page (`--force`), `--stitch` 200 MP, `--grid` 100 MP, animated output 300 frames, decoded input image 200 MP.
- Multi-page TIFF is written frame by frame (`TiffImagePlugin.AppendingTiffWriter`).
- ocrmypdf `--jobs` and Ghostscript threads capped to the CPU count.

### Privacy

- The toolkit never calls a model or web service. If Claude wants a model to describe an image, that is outside the toolkit.
- md2pdf: see its section (static HTML, scripts blocked, network blocked).
- `file:` URLs with a host part (`file://server/share`, UNC paths) are refused everywhere a URL or path comes from document content; they would make Windows contact that server with the user's credentials.
- LibreOffice runs with macros disabled and link updates off (see office2pdf).

## Checking (phase 2)

### Provenance

Every writing command returns an operation result:
- per output page, a list of placements: `{source_doc, source_page | "blank" | "generated", transform (matrix from source space 2 to output space 2), dest_rect}`; one placement for copies, several for n-up and booklet;
- expected changes: regions (in output space) where new content may appear or content may be removed, text that may be added or removed;
- features that must survive (bookmarks remapped, labels, links, widgets, annotations) or are declared dropped.

The checker compares each placement with its source region after the transform; never by position.

Writing commands run through `pipeline.write_pdf` / `write_non_pdf` (input opening per the encryption matrix, signature and restrictions gates, naming, staging, check, publish or `_UNCHECKED`, `--dry-run`, result record with `findings`); long jobs call `pipeline.announce_estimate`. Commands declare `check_policy` as a built-in policy name (`registry.BUILTIN_POLICIES`, one per row of the invariant table) or their own callable.

Operation result fields (`check.py`):
- `Placement.source_doc` is a path key into the opened inputs; `source_page` is zero-based; `transform` is PyMuPDF `(a, b, c, d, e, f)` from source space 2 to output space 2; `dest_rect`, when present, is in output space 2 and scopes source words and pixels by inverse-mapping its bounds.
- `Placement.rotation`, when set, is the expected output `/Rotate` value in degrees. Placement policies check it and report `ROTATION_CHANGED` on mismatch.
- `OperationResult.ssim_floor` is the explicit minimum grayscale SSIM for each mapped source image region during lossy compression. The source image bbox from `get_image_info` is transformed into output space before scoring.
- `OperationResult.annotation_delta` is the signed output-minus-source annotation count for `annot_add`/`annot_remove`.
- `OperationResult.cropboxes` contains each expected output crop box expressed in unrotated source space 2 points; crop text comparison is scoped to that box and a mismatch reports `CROPBOX_CHANGED`.
- `ExpectedChanges.regions` are displayed millimetre rectangles on output pages. A changed tile is allowed when it overlaps one; changed-tile boxes use the same displayed-mm space.
- `Features.must_survive` stores exact expected output counts. `Features.dropped` stores counts intentionally removed from the source; successful removals report `WARN FEATURES_DROPPED` with source, removed, expected and actual counts.

### Tiers

- Invariant (per command, table below). Violation → exit 3 (`_UNCHECKED`, or nothing for security commands).
- New warning: present in the output, absent in the source (cut-off text, missing glyphs, blank page, low-dpi image) → `WARN`.
- Pre-existing: present in the source too → counted, listed with `-v`.

### Measurements

- Text: `get_text("words")`, normalised (whitespace, ligatures, end-of-line hyphenation), compared as an ordered sequence per placement.
- Visual: render source region and output region at 100 dpi RGB, 10 mm tiles; a tile changed = more than 1 % of its pixels differ by more than 8/255 in any channel. Changed tiles reported as mm boxes.
- Image quality for lossy steps: per image region, SSIM on grayscale ≥ floor (0.90 `med`, 0.85 `high`, 0.78 `max`; the operation passes its floor in `ssim_floor`); effective dpi of images that carry text (scans) ≥ 150 at `med`/`high`, `max` warns `LOW_DPI` below 150.
- Colour: max chroma (max−min of R,G,B) per page.
- Whole-page stats for `check`: blank, mostly black, ink coverage, smallest font size, `U+FFFD`/`.notdef`, spans outside the crop box, image effective dpi, image count.
- Calibrated thresholds (constants at the top of `check.py`; `test_calibration_margins` asserts the margins on fixtures from `tests\fixtures\p2.py`, built with pymupdf directly):

| Threshold | Value | Passing case (measured) | Failing case (measured) |
|---|---|---|---|
| Changed tile | > 1 % of pixels differ by > 8/255 in any channel | tiles outside a recompressed photo: 0 % | table with one digit changed: 2.56 %, delta 255 |
| Image-region SSIM `med`/`high`/`max` | 0.90 / 0.85 / 0.78 | textured photo at JPEG q80/q65/q50: 0.993 / 0.988 / 0.982 | small image replaced by grey: 0.739; photo removed: 0.637 |
| Gray: max chroma | ≤ 2/255 | grayscale page: 0 | colour photo: 167 |
| Gray: luminance delta | ≤ 24/255 | photo converted to gray: 5 | photo removed: 254 |

SSIM is scored per image region because whole-page SSIM hides a destroyed image (photo removed: 0.910 whole-page). Changed tiles inside declared image regions (up to 37 % at q50) are allowed only for lossy policies.

### Invariants per command

| Command / mode | Invariant |
|---|---|
| organize and wrappers, merge, split, interleave, insert | page count as planned; each page's words and tiles match its source placement; rotation as planned; features per Feature preservation |
| rotate | words and (rotated) tiles unchanged; `/Rotate` as planned |
| watermark, numbers, headfoot | changes only in tiles overlapping the stamp boxes on the chosen pages; all source words present |
| annotate (add) | content tiles and words unchanged; annotation count + planned number |
| annotate `--remove` | content unchanged; annotation count − planned number |
| flatten | annotation/widget count 0 for the flattened kinds; tiles outside their rects unchanged; each flattened field value present as text |
| flatten `--raster` | tiles only (words not checked); `WARN TEXT_RASTERIZED` |
| crop | words inside the new crop box unchanged; crop box as planned |
| cut, resize, nup, booklet | each placement matches its source after its transform |
| rmblank | removed pages are exactly the reported ones; the remaining pages match their sources |
| meta, bookmarks, labels, protect, unlock | page words and tiles unchanged; edited structure reads back as set |
| gray | words unchanged; max chroma ≤ 2/255 on every page; tile luminance within tolerance |
| compress `lossless` | words unchanged; no tile changed |
| compress `med/high/max` | words unchanged; tiles outside image regions unchanged; image regions meet the SSIM floor |
| ocr | source words still present; pages without text either gain text or are reported `OCR_EMPTY` |
| sanitize, redact | see phase 7 |
| repair | no baseline; report only |

Commands without a row report `verdict: report`.

### Looking

The skill rule: the built-in check is the verification, so Claude does not re-verify by hand. On `ok`, report and stop (no thumbnails, no re-extraction, no extra `check`); on `warn`, report the warnings. `thumb` is for an `unchecked`/`failed` verdict or when the user asks to see a page: a crop of the flagged box, or a whole page at low resolution when the question is about the whole page. Robustness comes from the tests and the built-in check, not from Claude looking.

`thumb` (phase 2): downscaled (`--max-side 800`) or cropped (`--crop` displayed mm on a PDF page, px on an image) copy written to the report folder as `<time>-<pid>-thumb-<stem>.png` for Claude to view; a crop is rendered so its longer side is `--max-side` (at most 600 dpi). The report folder keeps the newest 50 files of any type.

## Commands

Libraries in dev: pymupdf 1.28.2 (`import pymupdf`), pikepdf 10.16, pypdf, img2pdf 0.6.3, Pillow 10.4 + pillow-heif + pillow-avif-plugin, ocrmypdf 17.13, fpdf2, numpy, opencv-headless, psutil.

### Feature preservation (phase 3b)

| Feature | organize (one input) | organize / insert with other files, merge, interleave | split |
|---|---|---|---|
| Method | `Document.select(list)` for the page order (verify repeated pages are supported; if not, use `insert_pdf(final=0)` from the source), then `new_page` for blanks and rotation set per page | `insert_pdf(..., links=True, annots=True, widgets=True, final=0)` per source, `join_duplicates=False` | `select` on a copy per part |
| Bookmarks | entries to kept pages remapped; entries to dropped pages removed | each input's bookmarks kept under its own top-level entry with `--toc`, else appended at top level | entries to pages in the part, remapped |
| Page labels | kept when page order is unchanged, else dropped with `WARN LABELS_DROPPED` | dropped, `WARN LABELS_DROPPED` | dropped, warned |
| Internal links | remapped; links to dropped pages removed (counted) | links within one source kept; cross-call links counted in `WARN LINKS_DROPPED` | as organize |
| Form fields | duplicated pages get renamed fields (`name#2`) so values are not shared | name collisions between inputs renamed the same way | kept |
| Structure tree, document JavaScript, attachments | kept by `select` | not carried; `WARN FEATURES_DROPPED` lists them | as organize |

The checker verifies bookmark, link and widget counts against the plan.

### Phase 1 and 3a: read and report

| Command | Behaviour |
|---|---|
| `info FILE` | triage: pages, size, page sizes, PDF version, encryption (user/owner/restrictions), signed, text layer or scanned per page, fonts embedded, forms, attachments, JavaScript, bookmarks, largest space user, `next:` hints ("scanned → ocr before text"; "signed → PDF rewrites need --break-signature") |
| `info FOLDER` | one line per PDF: pages, size, title, encrypted, text layer |
| `pages` | one line per page: number, label, size in mm + name (A4, Letter), rotation, blank?, first heading or line |
| `text` | `--layout`; stdout bounded, or `-o` |
| `count` | words (split on Unicode whitespace), characters with/without spaces, paragraphs (non-empty `get_text("blocks")` text blocks), lines; per page and total (`--total`); pages with no text layer flagged |
| `json` | pages, blocks, lines, spans (font, size, `bbox_pymupdf`, `bbox_mm`), metadata |
| `html` | `get_text("xhtml")` per page into one file (`-o`) or bounded stdout |
| `size` | per stored object (xref, counted once): images (pixel size, filter, bytes), fonts (embedded, subset, bytes), attachments, metadata, content streams; per-page usage listed separately |
| `check FILE` | whole-file report (Measurements) |
| `check FILE --against ORIG` | for outputs of other tools: pages matched by text similarity; pages without text by a 64-bit difference hash of a 32×32 render; ambiguous and unmatched pages reported; `--map 1=1,2=3` (job: `map`) sets pairs by hand; page-size differences reported as findings, never hidden by resizing; then tile diff |
| `open` | opens the file in the default viewer (`os.startfile`) for the user |

### Phase 3b: page organizer

`organize` lists the output pages in order; one pass, one check.

JSON (canonical): `{"cmd": "organize", "recipe": [{"pages": "1-3"}, {"blank": "a4"}, {"pages": "5"}, {"pages": "4"}, {"pages": "6-8", "rotate": 90}, {"file": "appendix.pdf", "pages": "2-4"}, {"pages": "10-"}]}`
Shorthand: `organize report.pdf "1-3,blank=a4,5,4,6-8@90,10-"`.

- Page items: `N`, `N-M`, `N-` (to end), `-M` (from 1), `M-N` with M>N (reversed), `odd`, `even`, `rev` (all, reversed). Numbers are original page numbers.
- `rotate` / `@`: added to the current rotation (relative), mod 360; `rotate_abs` sets it.
- `times` / `xK`: the item appears K times in total; a range repeats as a block (`1-3x2` → 1,2,3,1,2,3).
- Other files' passwords: `pipeline.write_pdf(..., passwords={path: password})`; nested job paths (recipe `file`, merge inputs) resolve against `args.job_base` (set by the job loader; absent → current folder).
- `blank`: size of the previous output page; as the first item, size of input page 1; `blank=a4|letter|WxH` (mm).
- Unlisted pages are dropped; `keep_rest` / `--keep-rest` appends never-listed pages in original order.
- Other files: JSON only (`file`, `password_file`).
- Whole recipe validated first; all problems reported (exit 2).
- Prints a compact map (`out 1-3 ← 1-3, out 4 ← blank, ...`).

| Wrapper | Meaning |
|---|---|
| `extract --pages 3-5` | those pages |
| `remove --pages 3,7` | all except those; removing every page is `BAD_PAGE` |
| `reorder --order 3,1,2` | exactly that order; every page once |
| `move --pages 5-6 --after 9` / `--to start|end` | the block keeps its order; numbers are original; a target inside the block is `BAD_PAGE` |
| `swap --pages 2,7` | two pages exchanged |
| `reverse [--pages a-b]` | that range reversed in place |
| `duplicate --pages 3 --times 2` | page 3 appears 2 times in total, copies right after it |
| `blank --at 1,5 [--size a4]` | blank before the given original pages; `--at end` |
| `rotate --pages 2-4 --by 90` / `--set 0` | relative or absolute |
| `insert other.pdf --pages 2-4 --after 6` | pages of another PDF inserted (job form for paths with spaces); `--after N` or `--to start|end` is required |
| `merge` | inputs in order; `--toc` one top-level bookmark per input |
| `split` | `--every N` (last part may be shorter), `--ranges` (one file per range), `--bookmarks` (one file per top-level bookmark; pages before the first bookmark join the first part) |
| `interleave A B` | A1,B1,A2,B2,...; `--reverse-back` reverses B first; unequal counts are `BAD_ARG` unless `--allow-uneven` (leftover pages appended) |

Workflow for Claude: `pages` → build the recipe → `organize` → read the map and verdict.

### Phase 3c: export and folder helpers

| Command | Behaviour |
|---|---|
| `toimg` | PNG and JPEG (`pixmap.save(path, output=..., jpg_quality=)`); `--format` picks the format (default PNG); `--stitch` / `--grid` publish `<stem>_stitch.<ext>` / `<stem>_grid.<ext>`; other formats in phase 6; options under PDF → images |
| `zip` | files or a folder → `<name>.zip` (deflate); `--zip` on multi-file outputs zips the published folder and keeps it |
| `dupes` | identical: same SHA-256 (authoritative); `--similar`: same normalised non-empty text and page count, reported as candidates |
| `compare A B` | `check --against` report (reuses 3a's page matching in `commands\p3a_core.py` and `check.tile_differences`) plus `<A>_vs_<B>.pdf`: a copy of B with changed tiles outlined by rectangle annotations, for the user |

### Phase 4: edit and stamp

| Command | Behaviour |
|---|---|
| `meta` | pikepdf. Show: Info keys (standard and custom), XMP fields, `/Lang`. Set `--title --author --subject --keywords --creator --producer --created --modified --lang --custom KEY=VALUE`; `--clear [keys]`; `--to meta.json`; `--from meta.json` patches (`--replace` replaces). Order: write Info, then `with pdf.open_metadata(set_pikepdf_as_editor=False) as m: m.load_from_docinfo(pdf.docinfo, ...)` with the option that deletes XMP fields missing from Info (check the parameter name in 10.16). Custom keys stay in Info only (no XMP mapping; the warning pikepdf gives is expected). Dates: `YYYY-MM-DD` or ISO 8601; a given offset is kept; no offset means the PC's local zone for that date (`datetime.astimezone()`, DST-aware; the PC is currently W. Europe Standard Time); written `D:YYYYMMDDHHmmSS+HH'mm'`. `--each` for folders/globs |
| `protect` | AES-256; user and/or owner password from the secret sources; `--no-print`, `--no-copy`, `--no-modify`, `--no-assemble` |
| `unlock` | needs the password (user or owner); output without encryption |
| `bookmarks` | `--list`; `--set` from a JSON job list `[{"level": 1, "title": "...", "page": 3}]`; `--clear` |
| `labels` | job list `[{"from": 1, "style": "roman|Roman|arabic|alpha|Alpha|none", "prefix": "", "start": 1}]`, mapped to pymupdf `set_page_labels` entries `{startpage: from-1, style: r|R|D|a|A|"", prefix, firstpagenum: start}` |
| `watermark` | text: `page.insert_text(point, text, fontsize, fontname, color, fill_opacity=..., morph=(center, pymupdf.Matrix(angle)), overlay=not under)`, any angle; image: Pillow rotates (expand) and applies opacity to the alpha channel, PNG bytes passed to `page.insert_image(rect, stream=..., overlay=not under)`; `--under` = `overlay=False`; position 9 anchors or centre, `--pages` |
| `numbers` | 9 anchors, format `{n}`, `{n} / {total}`, `{label}`, `--start`, `--pages`, `--margin` mm, `--fontsize` |
| `headfoot` | header/footer text with `{n}`, `{total}`, `{date}`, `{file}`; margins in mm |
| `annotate` | `--highlight/--underline/--strike TEXT`: literal via `search_for(quads=True)`; `--regex` matches against the page text built from `get_text("rawdict")` characters, and each match becomes one quad per line it spans (never one box across lines); `--pages`, `--color`; `--note TEXT --at page,x,y`; `--textbox TEXT --rect page,x0,y0,x1,y1`; `--box`, `--arrow` (displayed mm); `--list`; `--remove all|ids`; reports matches per page |
| `flatten` | `--only forms|annots`; before baking, `widget.update()` for every widget without an appearance stream (or when `/NeedAppearances` is true); then `bake(annots=, widgets=)`; `--raster DPI` re-renders pages as images |
| `crop` | margins in mm, or `--auto` to the content box plus `--pad 3mm`; `--pages`; sets the crop box only. The result line states: crop is not redaction, cropped content stays in the file |
| `cut` | `--v`: vertical cut line → left then right half (`--rtl` right first); `--h`: horizontal line → top then bottom; `--grid 2x2` row-major; via `show_pdf_page(clip=)` |
| `resize` | all pages to A4/Letter/`WxH` mm; `--scale fit` (scaled, centred) or `--pad` (content kept, margins added; if the content is larger than the target, falls back to fit with `WARN PAD_TOO_SMALL`) |
| `nup` | grids: 2 = 2×1, 4 = 2×2, 8 = 4×2, 16 = 4×4, 32 = 8×4 (columns × rows); sheet = source page size, landscape for 2, 8, 32, portrait for 4, 16; `--sheet a3|a4|letter`; order row-major, `--order col`; `--gap` mm, `--border` |
| `booklet` | saddle-stitch, duplex: pad with blanks to a multiple of 4; landscape sheets, 2-up; default sheet = source page size (pages scaled to half; A4 → A5 booklet), `--sheet a3` keeps 100 %; sheet k (from 0) front = [N−2k, 2k+1], back = [2k+2, N−2k−1]; prints for flip on the short edge, `--flip long` rotates backs 180°; `--binding right` swaps left/right |
| `gray` | `doc.recolor(components=1)` first; if the check finds chroma left, Ghostscript fallback (`-sColorConversionStrategy=Gray -dProcessColorModel=/DeviceGray`, full argv per Compression) when not gated |
| `rmblank` | candidate: no words, no annotations/widgets/links, ink < 0.5 % at 50 dpi, and no dark connected component larger than 3 mm (stamps, marks); ink 0.1–0.5 % or any component → `UNCERTAIN`, kept unless `--include-uncertain`; prints ink % per removed or uncertain page; removing every page is `BAD_PAGE` |
| `rename` | from Info title or first heading; dry run by default. `--apply`: write and fsync `rename_manifest_<time>.json` (size + SHA-256 + old + temp + new name per file) before the first move; phase 1 renames each to its temp name, phase 2 to the final name, updating the manifest after every move; `rename --undo MANIFEST` restores from any partial state, checks identity by size + hash, and refuses where the old name is now taken. Names: Windows-illegal characters replaced, reserved names (`CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`) suffixed, trailing dots/spaces trimmed, ≤ 100 characters, case-insensitive collisions `_2`; case-only renames go through the temp name |
| `repair` | open with pymupdf (rebuilds the xref) and save `garbage=3, deflate=True`; if that fails, open with pikepdf (qpdf recovery) and save. Reports pages recovered and the recovery warnings (`get_warnings()` / `mupdf_warnings()`); a missing page tree → `ERROR DAMAGED ... unrecoverable`. Encryption per the matrix |

`cut`, `resize`, `nup` and `booklet` draw page content only (`show_pdf_page`): links, annotations and form fields are dropped and cropped-away content is still imported. Before running, count them; if any, `WARN FEATURES_DROPPED` with counts; `--flatten-annots` bakes annotations first (the baked version becomes the source for the check).

### Phase 5: compress, OCR, batch

#### Compression

Start from `size`: big images → downsample; fonts not subset → subset (unembedded fonts cannot be subset; reported as a limitation); scanned pages → scanned candidate; already small → `NO_GAIN`. Every candidate runs on the original (decrypted copy for external tools). Lossless (Flate/PNG-type), bitonal and indexed images are never recompressed or downsampled unless `--include-graphics`; the result line says how many images were left alone and why.

| Level | pymupdf candidate | Ghostscript candidate (only with `--include-graphics`) | Scanned candidate |
|---|---|---|---|
| `lossless` | `subset_fonts()`; `ez_save(garbage=4, deflate=True, use_objstms=True)`; thumbnails removed | — | `-O1` |
| `med` | lossless + `rewrite_images(dpi_threshold=225, dpi_target=150, quality=80, lossy=True, lossless=False, bitonal=False, color=True, gray=True)` | — | `-O1` |
| `high` | `rewrite_images(dpi_threshold=165, dpi_target=110, quality=65, ...same flags)` | resolution 110 | `-O2` |
| `max` | `rewrite_images(dpi_threshold=110, dpi_target=72, quality=50, ...same flags)` | resolution 72 | `-O3` |

- `rewrite_images` resets both dpi values to 0 when `dpi_threshold` is falsy and requires threshold > target, so both are always passed. With `--include-graphics`, `lossless=True, bitonal=True` are added.
- Ghostscript argv (no presets, so nothing else changes): `<ghostscript> -dSAFER -dBATCH -dNOPAUSE -dQUIET -sDEVICE=pdfwrite -dCompatibilityLevel=1.7 -dAutoRotatePages=/None -dDetectDuplicateImages=true -dCompressFonts=true -dSubsetFonts=true -dColorConversionStrategy=/LeaveColorUnchanged -dDownsampleColorImages=true -dDownsampleGrayImages=true -dDownsampleMonoImages=false -dColorImageResolution=<r> -dGrayImageResolution=<r> -dColorImageDownsampleThreshold=1.5 -dGrayImageDownsampleThreshold=1.5 -dAutoFilterColorImages=true -dAutoFilterGrayImages=true -o <staged> <input>`.
- Scanned candidate (pages that are a single image with no text or OCR text only): `ocrmypdf --ocr-engine none --skip-text --output-type pdf -O<n>`; jbig2enc lossless; pngquant runs at `-O2`/`-O3`, so those two are used only with `--include-graphics` (otherwise `-O1`).
- Ghostscript and the scanned candidate are skipped for PDFs with form fields, links, annotations or signatures (`WARN SKIPPED_GS` with the reason).
- Selection: across all attempted candidates, keep the smallest whose check passes. If none is smaller than the input → `NO_GAIN` (exit 0, path = input). `--target SIZE`: try levels in order, stop at the first passing candidate ≤ SIZE; otherwise keep the smallest passing one with `WARN TARGET_MISSED` and the gap.
- Result line: before → after, %, winning method, what changed (`42 images 300→150 dpi, 3 fonts subset, 7 graphics left lossless`).

#### OCR

`ocr`: `ocrmypdf --skip-text --output-type pdf -l eng+ind --sidecar <temp>` (`--lang`), `--pdfa` opt-in. Pages that already have text are skipped. From the sidecar: pages OCRed, pages that still have no text (`WARN OCR_EMPTY` with the list). Timeout per External tools; pages skipped by a timeout are reported the same way.

#### Batch

- One file, several steps: job with `steps`, or shorthand `batch report.pdf "rotate --by 90 | numbers | compress --level med"`.
- Many files: `batch --each GLOB`; one record per file and a `summary`; a failing file is reported and skipped; exit 4 if any failed.
- Allowed steps (PDF → PDF): organize and wrappers, rotate, rmblank, crop, gray, meta, bookmarks, labels, watermark, numbers, headfoot, flatten, ocr, compress, protect (last step only). Anything else is refused in preflight.
- Serial, one process; intermediates in the run's temp folder. The check runs after every step against that step's input (composed final checks are backlog).
- The whole job is validated before any file is touched.

### Phase 6: conversions

#### Office → PDF (`office2pdf`)

- `<soffice> --headless --norestore --nolockcheck -env:UserInstallation=<Path(profile).as_uri()> --convert-to pdf --outdir <staging dir> <file>`; a fresh profile per run in the run's temp folder, and one staging folder per input (LibreOffice names its output after the input stem).
- Before launch, write `user\registrymodifications.xcu` in the fresh profile setting macro security to the highest level and link updates to never (exact keys checked in phase 6 against LibreOffice 26.8). Tripwire tests (.docx cannot hold macros): an `.odt` whose document-open event runs a Basic macro that writes a marker file, and a document with an external link to a local file; the conversion succeeds, no marker file appears, and the linked content is not refreshed.
- Success = exit 0 and a new PDF that pymupdf opens with ≥ 1 page; else `TOOL_FAILED`. Ignore the harmless "Could not find platform independent libraries" line.

#### PDF → Word (`toword`, layout mode)

- In a child Python process (killable): `cv = Converter(pdf, password=...)`; `cv.convert(docx, ignore_page_error=False)`; `cv.close()` in `finally`. A page error is `TOOL_FAILED` naming the page.
- Validation: the .docx opens with python-docx; its word count ≥ 80 % of the PDF's, else `WARN LOW_TEXT`.
- Equations come out as text or images; math mode is backlog.

#### Markdown → PDF (`md2pdf`)

Port of the generic parts of `D:\GFD NS-353B\practice\render_pdf.js` to `C:\Tools\pdftools\md2pdf.js`, using Puppeteer-native calls (the GFD script's Edge CLI and Playwright fallback are not copied). Leave the GFD files alone.

- Render in Node: marked + KaTeX produce static HTML (no script in the page). Math rules: `$$…$$` display; inline `$…$` needs a non-space after the opening and before the closing `$`, and the closing `$` not followed by a digit; `\$` is a literal dollar; code spans and blocks are never parsed for math.
- The HTML gets `<base href="<Markdown folder as file URI>">` and `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src file:; style-src 'unsafe-inline' file:; font-src file:; script-src 'none'">`. Raw HTML from the Markdown stays but cannot run scripts or reach the network.
- puppeteer-core (pinned version, `package-lock.json` kept) drives the installed browser (Edge on Windows, Chrome or Chromium on Linux). Request interception allows only `file:` URLs without a host whose canonical path is the generated HTML file, inside the Markdown folder, or inside `node_modules\katex`; everything else is aborted (`WARN REMOTE_RESOURCE`).
- Sequence on one page: `emulateMediaType('print')` → viewport at A4 printable width → `page.evaluate` waits for `document.fonts.ready` and images (60 s bound) → measures and shrinks wide display equations (flush left, then scale) → `page.pdf({format: 'A4', preferCSSPageSize: true, printBackground: true})` → status read from the same page. Confirm in phase 6 that `page.evaluate` works under `script-src 'none'`; if not, use `page.setBypassCSP(true)` with interception as the only network barrier.
- CSS `md2pdf.css`: A4, 20 mm margins, `break-inside: avoid` on display math, figures, table rows; headings `break-after: avoid`.
- Outcome: KaTeX errors > 0 or failed local images → exit 3 (`_UNCHECKED`); equations shrunk below 70 % → `WARN`.
- `selftest --caps` records the browser version next to the pinned puppeteer-core version.

`check --source FILE.md` (run automatically by md2pdf):
- `md2pdf.js --tokens <json>` emits the ordered blocks from the marked lexer: `{type: heading|paragraph|list_item|table_cell|code, section, text}` (math removed, code kept literally).
- Python aligns the block word sequence with the PDF word sequence in order (occurrence-aware). A block with ≥ 50 % of its words missing → invariant failure: inside md2pdf that publishes `_UNCHECKED` (exit 3); a standalone `check --source` reports it as a finding with verdict `warn` and exit 0 (Exit codes: `check` finding differences). A run of 3+ consecutive missing words → `WARN` with the section.
- Raw TeX in the PDF text (`\frac`, `\partial`, `^{`, `_{`) outside code-derived spans → invariant failure.
- Geometry warnings only: text under 6 pt; a heading in the bottom 15 % with nothing after it; a block cut at the bottom margin and continued (tables across pages and blocks taller than a page allowed). Image count: warning only.

#### Text → PDF (`txt2pdf`)

pymupdf Story, A4; input UTF-8 (BOM allowed); `&`, `<`, `>` escaped before building the Story HTML; `--mono` for a monospace font.

#### Images → PDF (`img2pdf`)

Setup: `pillow_heif.register_heif_opener()`; `import pillow_avif`.

| Input | Route | Result |
|---|---|---|
| JPEG (incl. CMYK), JPEG 2000 | img2pdf | compressed bytes embedded unchanged |
| PNG, TIFF, GIF (single frame) that img2pdf accepts (incl. 16-bit non-interlaced PNG and 8-bit alpha, kept as a soft mask) | img2pdf | pixels unchanged |
| PNG/TIFF img2pdf refuses (interlaced PNG, alpha deeper than 8 bit, palette with ICC, unusual TIFF compression); catch `AlphaChannelError`, `ExifOrientationError` and its other refusals | Pillow → PNG stream → `page.insert_image` | lossless; 16-bit → 8-bit only where unavoidable, `WARN BIT_DEPTH_REDUCED` |
| HEIC/HEIF, AVIF, WebP, BMP, ICO, TGA, PSD, EPS (Pillow via Ghostscript) | Pillow → PNG stream (lossless) by default; `--lossy [Q]` → JPEG, default Q 92 | lossless unless `--lossy` |
| SVG | pymupdf `open(svg).convert_to_pdf()` | vector |
| RAW (DNG, CR2, NEF), JPEG XL | `UNSUPPORTED ... | fix: export to JPEG` | — |

- Alpha is kept as a soft mask on every route; `--flatten-alpha` puts it on white.
- Colour: an embedded ICC profile is kept with the pixels it describes. CMYK on the Pillow route is converted to sRGB with `ImageCms` using the embedded profile (without one: Pillow's basic conversion, `WARN CMYK_APPROX`), and the sRGB profile is attached. HDR/high-bit-depth reduced to 8-bit is warned.
- Orientation: EXIF 1/3/6/8 on the img2pdf route via img2pdf's rotation; mirrored 2/4/5/7 via Pillow `ImageOps.exif_transpose`.
- Order: as given; folders/globs natural-sorted (`img2` before `img10`); `--sort name|date|exif`; no EXIF date → file time with `WARN NO_EXIF_DATE`.
- Page: `--page auto` (default): image with dpi metadata ≥ 150 whose page would fit within A3 → page = image size; otherwise A4 `contain`, orientation per image. Explicit: `--page fit|a4|letter`, `--fit contain|cover|stretch`, `--orient auto|portrait|landscape`, `--margin 10mm`.
- `--each` one PDF per image; `--max-dpi N` downsamples on the Pillow route only; `--ocr` runs `ocr`; `--toc` one bookmark per image; animated inputs use the first frame.

#### PDF → images (`toimg`)

| Format | Writer | Options |
|---|---|---|
| PNG, JPEG | `pixmap.save(path, output=...)` | `--quality` (JPEG) |
| WebP | `pixmap.pil_save(path, format="WEBP")` | `--quality`, `--lossless` |
| TIFF | `pixmap.pil_save`; `--single` via `AppendingTiffWriter` | `--tiff-compression lzw|deflate|group4` (group4 needs `--bw`) |
| BMP, GIF | `pixmap.pil_save` | — |
| AVIF | Pillow + `pillow_avif` | `--quality`; `--lossless` refused (no lossless mode in the plugin) |
| HEIC | Pillow + pillow-heif | `--quality`; `--lossless` refused (backlog) |
| SVG | `page.get_svg_image(text_as_path=True)` | `--svg-text` keeps text as text (`WARN` that it needs the fonts); `--clip`, `--gray`, `--bw`, `--dpi` refused for SVG |

- Size: `--dpi 150` default, or `--width`/`--height` px (aspect kept), or `--scale`; mutually exclusive.
- `--pages`; `--clip x0,y0,x1,y1` displayed mm.
- `--gray`; `--bw` (`--threshold`); `--alpha` for PNG/WebP/TIFF/AVIF only.
- `--stitch v|h` (one image of the chosen pages); `--grid CxR` (contact sheet, page numbers labelled). `--single`, `--stitch` and `--grid` are mutually exclusive.

#### Embedded images (`images`)

XObject images via `extract_image(xref)`, written once per xref; inline images via `get_text("dict")` image blocks. Formats that viewers rarely open (`jb2`, `jbig2`, `jpx`, CCITT fax, raw) are converted to PNG through `pymupdf.Pixmap(doc, xref)`, noted in the result. Soft masks merged into PNG when present. `--min-px` skips tiny images.

#### Image helpers

| Command | Behaviour |
|---|---|
| `imgtext` | tesseract (resolved per External tools) `-l eng+ind` with TSV output; prints text (bounded) and mean confidence of words with conf ≥ 0 |
| `imginfo` | size, dpi, mode, bit depth, ICC present, gray/colour, blank or not, EXIF date/camera/orientation |
| `imgdiff` | tile method of the checker; different sizes are reported as a finding; comparison after resizing only with `--resize` |

### Phase 7: sanitize and redact

Failure policy for both: if the check fails, nothing is published (exit 3, verdict `failed`), measurements go to the log; `--keep-failed` publishes `<name>_FAILED_UNSAFE.pdf` for diagnosis.

#### `sanitize`

| Removed (default) | How |
|---|---|
| JavaScript everywhere (document name tree, open action, page/annotation/field/outline actions, `/Next` chains) | `pikepdf.sanitize.remove_javascript` |
| Attached and embedded files, incl. `/AF` associated files and file-attachment annotations | `pikepdf.sanitize.remove_attachments`, plus a walk that removes `/AF` arrays and their file specs |
| Thumbnails | `pikepdf.sanitize.remove_thumbnails` |
| Info metadata and XMP | pikepdf: Info cleared, `/Metadata` removed |
| Unused objects | full non-incremental save with garbage collection |

- Opt-in: `--external` (`remove_external_access`: Launch, URI, SubmitForm, ImportData actions), `--multimedia` (`remove_multimedia`).
- Listed only (counts and pages, no removal): hidden text (detected like pymupdf `scrub()`'s hidden-span test: `GlyphLessFont` as OCR layer, render mode 3, alpha 0) and optional-content layers that are OFF by default. Removal is backlog.
- Kept: form values, links (unless `--external`), annotations other than file attachments, initial-view open actions.
- `--keep meta,attachments,thumbnails` keeps listed kinds. Each removal is counted in the result.
- Check: page words and tiles unchanged, except tiles of removed file-attachment annotation icons (declared); a pikepdf structural re-scan finds 0 of each removed kind.

#### `redact`

- Targets: literal text, regex (per-line quads as in `annotate`), or rectangles (displayed mm). `--pages`; `--occurrence N` (job: list) limits which matches; default all matches on the chosen pages.
- A page that already has redaction annotations → `ERROR REDACTIONS_PENDING ... | fix: --apply-existing`, since applying redactions applies every redaction annotation on the page.
- `add_redact_annot(quad, fill=(0, 0, 0), cross_out=False)`; `apply_redactions(images=PDF_REDACT_IMAGE_PIXELS, graphics=PDF_REDACT_LINE_ART_REMOVE_IF_COVERED, text=PDF_REDACT_TEXT_REMOVE)`; `--graphics touched` opts into `REMOVE_IF_TOUCHED`. Images shared between pages are copied before pixel redaction.
- Then: thumbnails removed; annotations whose contents contain the target removed; metadata values containing the target cleared; full non-incremental save, `garbage=4`.
- Scanned pages: `--ocr` OCRs the page in memory first; boxes come from the OCR words; pixels are filled.
- Check, scoped:
  - each chosen box: no text from either engine (pymupdf `get_text` clipped to the box, pypdf extraction with positions) and image pixels filled;
  - text outside the chosen boxes unchanged (non-chosen occurrences stay);
  - when every occurrence in the document was chosen: no normalised match in text re-extracted by both engines, nor in decoded strings of Info/XMP, outline titles, annotation `/Contents`/`/RC`, form `/V`/`/DV`, structure tree `/Alt`/`/ActualText`/`/T`, named destinations, page labels, attachment names;
  - representations the checker cannot read (text drawn as paths, text in images on pages without `--ocr`) are reported `WARN UNVERIFIED_REPRESENTATION`.
- The result line states that `crop` is not redaction and `redact` is.

## Hardening (phase H)

The verdict replaces Claude looking (see Looking), so `ok` must be demonstrably trustworthy. Items, in order:

1. Full-tools profile: env `PDFTOOLS_REQUIRE_TOOLS=1` turns every integration-test skip for a missing external tool into a failure and records each tool's version and the executed test count. The plain run keeps skips (`SKIP <tool>: <reason>`).
2. Validator attack tests: hand `check.compare` / `pipeline.write_pdf` deliberately bad candidates (truncated file, blanked page, deleted minus sign or subscript, same page count with wrong pages, link to the wrong page, changed field value, extractor failure, hang) and assert the verdict is never `ok`, the exit code, and the published name. Includes the rules: an unreadable source page makes the result `unchecked`, never an empty baseline that matches; a pre-existing input defect never excuses a failed postcondition. Fix in the same item: text normalisation must not delete a spaced ASCII minus (`x - y`).
3. Semantic feature checks: `Features` gains per-item records next to counts: bookmark title, level and target page; link target (page or URI) and rectangle; form field name, type, value and widget page. Operations declare the expected records (remapped through the page map); the checker compares them. An order-sensitive text score (per page, symbols kept) starts as a `WARN` until calibrated.
4. Time limits: in-process command work and the check run under a wall-clock limit (child process or watchdog), so a pathological PDF ends in `TIMEOUT`, not a hang.
5. Corpus sweep: `tests\corpus.py` runs a pinned corpus, one child process per (file, command), with a timeout and process-tree kill; records outcome category per file hash and command in a baseline JSON; fails only on new or worsened categories (matched by hash and category, not filename). Corpus = both: a public set (pdf.js test PDFs, pinned by commit and digest) and the user's own documents (BMKG reports, papers, scans; listed in a local, git-ignored manifest; files never leave the PC and are never copied into the repo). Commands swept: `info`, `text`, `organize rev`, `rotate`, `numbers`, `compress --level lossless`, plus later phases' commands as they ship.

pikepdf wraps qpdf, so the two count as one opinion; independent cross-checks use pypdf or poppler where available.

## Tests

- `tests\make_fixtures.py` builds every fixture deterministically with pymupdf, pikepdf, Pillow and hand-written files (no toolkit commands): rotated page with offset crop box and `/UserUnit`; optional-content layers; annotations of each kind; form fields incl. one without appearance streams; links and bookmarks; AES-256 encrypted with a known test password; restrictions-only PDF; a signed-looking PDF (a `/Sig` field with a `/V` dictionary; detection needs no cryptography); equation-heavy page; scanned page; chart with thin coloured lines, an indexed image and a shared image across pages; repeated words; pending redaction annotation; JavaScript in link, outline and field actions; `/AF` attachment; a recoverable broken xref and an unrecoverable truncation; a deterministic 500-page text PDF (seeded text, A4, Helvetica) for the benchmark; images in every input format (PSD via `pixmap.save(output="psd")`); calibration fixtures (Measurements).
- Layout: `tests\test_p<phase>_*.py` (unittest, discovered by `unittest discover -s pdftools\tests`); fixtures from `tests\fixtures\p<phase>.py`, built into a temp folder or `tests\fixtures\out\` (`make_fixtures.py [folder] [--bench]`; the 500-page fixture only with `--bench`).
- Unit cases (no external tools, whole suite under 60 s): contracts, parsers, job loader, naming, staging/publishing (two processes racing for one name; locked target retry), secrets redaction, encryption matrix, coordinates, recipe parsing, check measurements.
- Independent assertions: outputs are inspected with a different engine where possible (pikepdf/pypdf for pages, rotation, boxes, metadata, encryption, bookmarks; Pillow for images), not only through `check`.
- Integration cases (LibreOffice, browser/puppeteer, Ghostscript, ocrmypdf, tesseract): on this PC they must run; a skip is allowed only when the tool is absent and prints `SKIP <tool>: <reason>`.
- Security tests (secrets, encryption, signatures, sanitize, redact) are never skippable.
- Each phase adds its cases; standing cases: collision `_2`, inputs unchanged by hash, one ERROR line per code, quiet output, JSON Lines parse with a damaged fixture, bounded output, `--dry-run` creates no output, path with spaces and non-ASCII.
- `selftest`: imports, one render, one write per shipped format, presence and version of each external tool; under 45 s; prints only failures.

## Installed (2026-10-26, smoke-tested)

- dev (conda): pandoc 3.12.1 (for the backlog math mode), jbig2enc 0.32, pillow-avif-plugin 1.6.0; already there: ghostscript 10.08, qpdf, pngquant, tesseract (with `ind`), ocrmypdf 17.13, img2pdf, pillow-heif, fpdf2. dev (pip): pdf2docx 0.5.13 (with opencv-python-headless, python-docx). All in `C:\Users\thurs\conda-envs\dev.yml`.
- `C:\Tools\pdftools\node_modules`: katex 0.19.1, marked 12.0.2, puppeteer-core 25.16.0 (exact pin, `package-lock.json` kept; `node_modules` git-ignored, so worktree builds run with `NODE_PATH=C:\Tools\pdftools\node_modules`).
- Git-ignore in `C:\Tools\pdftools\.gitignore`: `reports/`, `tests/fixtures/out/` (logs live in `C:\Tools\logs\`, handled by the shared helper).

## Build process

- Each wave runs in its own Claude Code session, started when the previous wave is committed; that session coordinates the wave's phases and ends when they are committed.
- Within a wave, start every phase at once, split by file ownership: the parts of a phase that touch only its own files build in parallel from the start; only the parts that extend another phase's files (e.g. phase 6's `toimg` formats and `check --source`) wait until that phase is committed.
- Each phase review is done twice and blind: Claude's own review and a Codex review (role `review`). Claude then compares them (findings both caught, only one caught, contradictions), verifies each finding against the code, and reports the comparison with the findings. When Codex is unavailable, its review waits; a Claude review never stands in for it.
- Builds, test runs and Codex reviews of committed code run in the WSL workshop (global CLAUDE.md, "Two machines"), on the workshop's clone of `windows-tools` at the pushed Windows HEAD. A build is `ask.ps1 -Role build -Worktree -Workshop` from this plan; shared-code changes a builder asks for are committed on main and pushed before the builder is relaunched.
- Per phase: Claude reads the workshop worktree's diff, then `ask.ps1 -Accept <path>` copies its changes into `C:\Tools` uncommitted, and the full suite runs once on Windows (env dev) before the commit. The Codex review (role `review`) runs in the workshop on committed code, `-Workshop -Review commit:<sha>` or `-Review custom` naming the commits; before the phase is committed and pushed it runs on Windows as `-Review custom` naming the phase's files. Findings go in the CLAUDE.md Reviews format; commit after the user's OK.
- After the push, every phase's tests also pass in the workshop clone (`~/src/windows-tools`, `conda run -n dev python -m unittest discover -s pdftools/tests -q`).
- A builder that finds the plan wrong or silent on something stops and reports it instead of guessing; Claude settles it and updates this plan.
- `SKILL.md` updated at the end of each phase: that phase's commands, the rules (the verdict is the verification: no hand re-checks after `ok`, JSON jobs for anything beyond simple arguments, `thumb` only for `unchecked`/`failed` or on request, background for estimates over 90 s), common recipes; per-command options from `pdf.cmd <cmd> --help`. Form filling → `anthropic-skills:pdf`; PDF → Markdown → the `pdf2md` skill.
