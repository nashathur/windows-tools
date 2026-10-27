## Export and folder helpers

Run `pdf.cmd toimg FILE --format png|jpeg` to render selected PDF pages. Set one size option with `--dpi`, `--width`, `--height`, or `--scale`; use `--pages`, `--clip` in displayed millimetres, `--gray`, or `--bw`. `--alpha` keeps transparency for individual PNG pages and may be combined with `--gray`; it cannot be combined with `--bw`, `--stitch`, or `--grid`. `--stitch` combines pages and `--grid` labels a contact sheet. Multi-page exports go in a published folder; `--zip` also creates a ZIP copy.

Run `pdf.cmd zip FILE_OR_FOLDER` to create a deflated archive. Run `pdf.cmd dupes FOLDER [--similar]` to report exact SHA-256 duplicates and optional matching-text candidates; provide `--password-file` or `--password-env` to inspect encrypted PDFs. `compare A B [-o PATH] [--break-signature] [--ignore-restrictions]` reports tile differences and annotates changed regions in a copy of B, beside B by default.

Read the verdict and warnings first. `ok` means the operation completed; `warn` needs review; `report` is informational.
