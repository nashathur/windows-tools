import os
import re
import shutil
import tempfile
import time
import uuid
from contextlib import contextmanager
from pathlib import Path

import psutil

from .errors import PdfError


def default_name(source, command, suffix=None, extension=None):
    source = Path(source)
    ext = extension or source.suffix or ".pdf"
    return source.with_name(source.stem + (suffix if suffix is not None else "_" + command) + ext)


def resolve_output(source, command, output=None, suffix=None, extension=None, multi=False):
    default = default_name(source, command, suffix, extension)
    if output is None:
        return default
    out = Path(output)
    if out.is_dir() or str(output).endswith(("/", "\\")):
        if multi:
            return out / (Path(source).stem + "_" + command)
        return out / default.name
    if multi:
        raise PdfError("BAD_ARG", "multi-file output requires a folder", "use -o with an existing or trailing-separator folder")
    return out


def staging_path(target, pid=None, token=None):
    target = Path(target)
    return target.with_name(f".{target.stem}.{pid or os.getpid()}-{token or uuid.uuid4().hex[:8]}.part{target.suffix}")


def create_staging(target, temp=None, pid=None, token=None):
    target = Path(target)
    if len(str(target)) > 240:
        raise PdfError("BAD_ARG", f"output path exceeds 240 characters: {target}", "use a shorter -o")
    stage = staging_path(target, pid, token)
    if len(str(stage)) > 240:
        if temp is None:
            temp = run_temp()
        stage = Path(temp) / f".{target.stem[:32]}.{pid or os.getpid()}-{token or uuid.uuid4().hex[:8]}.part{target.suffix}"
        if len(str(stage)) > 240:
            stage = Path(temp) / f".{pid or os.getpid()}-{token or uuid.uuid4().hex[:8]}.part{target.suffix}"
    return stage


def _running(pid):
    return psutil.pid_exists(pid)


def sweep_stale(folder, now=None):
    now = now or time.time()
    removed = []
    for path in Path(folder).glob(".*.*-*.part*"):
        match = re.match(r"^\..+\.(\d+)-[0-9a-fA-F]{8}\.part", path.name)
        if match and not _running(int(match.group(1))) and now - path.stat().st_mtime > 3600:
            if path.is_dir():
                shutil.rmtree(path)
            else:
                path.unlink()
            removed.append(path)
    return removed


def sweep_temp(now=None, parent=None):
    now = now or time.time()
    parent = Path(parent or Path(tempfile.gettempdir()) / "pdftools")
    removed = []
    if not parent.exists():
        return removed
    for folder in parent.iterdir():
        match = re.fullmatch(r"(\d+)-[0-9a-fA-F]{8}", folder.name)
        if folder.is_dir() and match and not _running(int(match.group(1))) and now - folder.stat().st_mtime > 3600:
            shutil.rmtree(folder)
            removed.append(folder)
    return removed


def run_temp():
    folder = Path(tempfile.gettempdir()) / "pdftools" / f"{os.getpid()}-{uuid.uuid4().hex[:8]}"
    folder.mkdir(parents=True)
    return folder


@contextmanager
def temp_folder():
    folder = run_temp()
    try:
        yield folder
    finally:
        shutil.rmtree(folder, ignore_errors=True)


def publish(staged, target, *, retry_seconds=2.0, sleep=time.sleep):
    staged, target = Path(staged), Path(target)
    deadline = time.monotonic() + retry_seconds
    candidate = target
    index = 2
    while True:
        copy = None
        try:
            if staged.parent.resolve() == candidate.parent.resolve():
                os.rename(staged, candidate)
            else:
                copy = candidate.with_name(f".{os.getpid()}-{uuid.uuid4().hex[:8]}.part{candidate.suffix}")
                shutil.copy2(staged, copy)
                os.rename(copy, candidate)
                staged.unlink()
            return candidate
        except FileExistsError:
            if copy is not None:
                copy.unlink(missing_ok=True)
            candidate = target.with_name(f"{target.stem}_{index}{target.suffix}")
            index += 1
        except PermissionError:
            if copy is not None:
                copy.unlink(missing_ok=True)
            if time.monotonic() >= deadline:
                raise PdfError("LOCKED", f"output is locked: {target}", "close the program holding the file, then rerun")
            sleep(min(0.05, max(0, deadline - time.monotonic())))


class Staging:
    def __init__(self, target, temp=None):
        self.target = Path(target)
        self.owned_temp = None
        self.path = create_staging(target, temp)
        if temp is None and self.path.parent != self.target.parent:
            self.owned_temp = self.path.parent
        self.published = None

    def publish(self):
        self.published = publish(self.path, self.target)
        return self.published

    def cleanup(self):
        if self.path.is_dir():
            shutil.rmtree(self.path, ignore_errors=True)
        else:
            self.path.unlink(missing_ok=True)

    def __enter__(self):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        return self

    def __exit__(self, exc_type, exc, tb):
        if exc_type or self.published is None:
            self.cleanup()
        if self.owned_temp is not None:
            shutil.rmtree(self.owned_temp, ignore_errors=True)


def disk_budget(inputs, dest, temp, multiplier=3, estimate=None):
    size = sum(Path(p).stat().st_size for p in inputs)
    need = estimate if estimate is not None else size * multiplier
    for drive in {Path(dest).anchor, Path(temp).anchor}:
        usage = shutil.disk_usage(drive or os.getcwd())
        if usage.free < need:
            raise PdfError("TOO_BIG", f"needs {need} bytes but only {usage.free} are free on {drive}", "free disk space or choose a drive with more room")
    return need
