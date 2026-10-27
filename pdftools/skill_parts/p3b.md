## Organize and split PDFs

Use `organize FILE RECIPE` to choose, repeat, rotate, blank, and insert pages in one operation. Page numbers refer to the original PDF; use JSON `recipe` entries for other files and paths with spaces. A recipe `password_file` supplies that input's password. In job files, nested recipe and merge input paths are relative to the job file. `xK` repeats a whole range as a block, so `1-3x2` means pages 1, 2, 3, 1, 2, 3. The output map shows each output page's source. `extract`, `remove`, `reorder`, `move`, `swap`, `reverse`, `duplicate`, `blank`, `rotate`, and `insert` are shortcuts for common recipes.

Use `insert OTHER --pages PAGES --after N` or `--to start|end` to insert pages. Use `merge A B ...` to append PDFs (`--toc` adds a top-level bookmark per input occurrence), `interleave A B` to alternate pages, and `split FILE --every N`, `--ranges ...`, or `--bookmarks` to create a folder of parts. Bookmark splits include front matter before the first top-level bookmark in the first part. Unequal interleave inputs need `--allow-uneven`; `--reverse-back` reverses the second PDF first.

Read the result line and page map after each write. `ok` passed the invariant check; `warn` includes a reported preservation change; `unchecked` means an invariant failed and the output carries `_UNCHECKED`.
