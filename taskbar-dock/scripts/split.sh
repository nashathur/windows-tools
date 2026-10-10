#!/bin/sh
# Move lines <from>..<to> of main.rs into src/<module>.rs (refactoring big main.rs files).
#   split.sh <main.rs> <module> <from> <to> "<doc line>"
# The module gets `use super::*;`; leading "// ----" section headers and blank lines of the moved
# chunk are dropped; top-level items become pub(crate). The moved lines and one blank line after
# them are removed from main.rs. Run sections bottom-up so earlier line numbers stay valid, then
# add `mod <module>; use <module>::*;` to main.rs.
set -e
f=$1; m=$2; a=$3; b=$4; doc=$5
dir=$(dirname "$f")
{ printf '//! %s\n\nuse super::*;\n\n'"" "$doc"; sed -n "${a},${b}p" "$f" | awk 'body || !/^(\/\/ ----|\/\/$|$)/ { body=1; print }'; } > "$dir/$m.rs"
sed -i "${a},$((b+1))d" "$f"
sed -i -E 's/^((unsafe )?(extern "system" )?fn|struct|enum|const|static|type) /pub(crate) \1 /' "$dir/$m.rs"
