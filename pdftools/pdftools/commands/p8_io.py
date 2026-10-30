"""Atomic writers for files created inside a pdf2md output folder."""
from __future__ import annotations

import os
import tempfile
from pathlib import Path


def atomic_write(path, data=None, writer=None):
    """Replace *path* only after a complete write; writer receives a same-suffix temp path."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temp_name = tempfile.mkstemp(prefix=f".{path.stem}.", suffix=path.suffix, dir=path.parent)
    os.close(fd)
    temp = Path(temp_name)
    try:
        if writer is not None:
            writer(temp)
        elif isinstance(data, bytes):
            temp.write_bytes(data)
        else:
            temp.write_text(str(data), encoding="utf-8", newline="\n")
        os.replace(temp, path)
    finally:
        temp.unlink(missing_ok=True)
