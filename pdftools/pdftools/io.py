import ctypes
import errno
import os
import re
import shutil
import sys
import tempfile
import time
import uuid
from contextlib import contextmanager
from pathlib import Path
from threading import RLock

import psutil

from .errors import PdfError

_active_stages = set()
_active_temps = set()
_active_lock = RLock()


def cleanup_active():
    with _active_lock:
        stages, temps = list(_active_stages), list(_active_temps)
    temp_parent = Path(tempfile.gettempdir()) / "pdftools"
    if temp_parent.exists():
        temps.extend(temp_parent.glob(f"{os.getpid()}-*"))
    for stage in stages:
        try: stage.cleanup()
        except OSError: pass
    for path in temps:
        try:
            if Path(path).is_dir(): shutil.rmtree(path, ignore_errors=True)
            else: Path(path).unlink(missing_ok=True)
        except OSError:
            pass


def default_name(source, command, suffix=None, extension=None):
    source = Path(source)
    ext = extension or source.suffix or ".pdf"
    return source.with_name(source.stem + (suffix if suffix is not None else "_" + command) + ext)


def folder_marker(output, raw):
    """`-o` text ending in `/` or `\\` means a folder on every platform. argparse's `type=Path` drops the
    trailing separator, so re-apply it from the raw text, as the native separator (POSIX reads `\\` as a name)."""
    if output is None or not isinstance(raw, str) or not raw.endswith(("/", "\\")):
        return output
    return str(output).rstrip("/\\") + os.sep


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
            try:  # a leftover held open by another process is skipped; it must not fail this run
                if path.is_dir():
                    shutil.rmtree(path)
                else:
                    path.unlink()
            except OSError:
                continue
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
            try:  # a folder held open by an orphaned tool is skipped; it must not fail this run
                shutil.rmtree(folder)
            except OSError:
                continue
            removed.append(folder)
    return removed


def run_temp():
    folder = Path(tempfile.gettempdir()) / "pdftools" / f"{os.getpid()}-{uuid.uuid4().hex[:8]}"
    folder.mkdir(parents=True)
    with _active_lock: _active_temps.add(folder)
    from . import watchdog
    watchdog.register_path(folder)
    return folder


@contextmanager
def temp_folder():
    folder = run_temp()
    try:
        yield folder
    finally:
        shutil.rmtree(folder, ignore_errors=True)
        with _active_lock: _active_temps.discard(folder)
        from . import watchdog
        watchdog.unregister_path(folder)


RENAME_REPLACES = os.name != "nt"  # POSIX os.rename silently replaces an existing target
try:
    _renameat2 = ctypes.CDLL(None, use_errno=True).renameat2 if sys.platform.startswith("linux") else None
except (OSError, AttributeError):
    _renameat2 = None


def _rename_noreplace(src, dst):
    """Linux renameat2(RENAME_NOREPLACE): one atomic move; False when the system or filesystem lacks it."""
    if _renameat2 is None:
        return False
    if _renameat2(-100, os.fsencode(src), -100, os.fsencode(dst), 1) == 0:  # AT_FDCWD, RENAME_NOREPLACE
        return True
    code = ctypes.get_errno()
    if code in (errno.EINVAL, errno.ENOSYS, errno.ENOTSUP):
        return False
    raise OSError(code, os.strerror(code), str(src), None, str(dst))


def rename_exclusive(src, dst):
    """Move src to dst, raising FileExistsError if dst exists."""
    if not RENAME_REPLACES:
        os.rename(src, dst)
        return
    if _rename_noreplace(src, dst):
        return
    # fallback without an atomic no-replace move: a name taken by another process is never replaced,
    # but a source swapped between link and unlink, or a reservation deleted by another process, is not guarded
    folder = os.path.isdir(src)
    if folder:
        os.mkdir(dst)  # reserves the name; rename then replaces only this empty folder
    else:
        try:
            os.link(src, dst)
        except FileExistsError:
            raise
        except OSError:  # filesystem without hard links: reserve the name with an empty file
            os.close(os.open(dst, os.O_CREAT | os.O_EXCL | os.O_WRONLY))
        else:
            try:
                os.unlink(src)
            except OSError:
                os.unlink(dst)
                raise
            return
    try:
        os.replace(src, dst)  # replaces only the reservation made above
    except OSError:
        (os.rmdir if folder else os.unlink)(dst)
        raise


def publish(staged, target, *, retry_seconds=2.0, sleep=time.sleep):
    staged, target = Path(staged), Path(target)
    deadline = time.monotonic() + retry_seconds
    candidate = target
    index = 2
    while True:
        copy = None
        try:
            if staged.parent.resolve() == candidate.parent.resolve():
                rename_exclusive(staged, candidate)
            else:
                copy = candidate.with_name(f".{os.getpid()}-{uuid.uuid4().hex[:8]}.part{candidate.suffix}")
                if staged.is_dir():
                    shutil.copytree(staged, copy)
                else:
                    shutil.copy2(staged, copy)
                rename_exclusive(copy, candidate)
                if staged.is_dir():
                    shutil.rmtree(staged)
                else:
                    staged.unlink()
            return candidate
        except FileExistsError:
            if copy is not None:
                if copy.is_dir():
                    shutil.rmtree(copy, ignore_errors=True)
                else:
                    copy.unlink(missing_ok=True)
            candidate = target.with_name(f"{target.stem}_{index}{target.suffix}")
            index += 1
        except PermissionError:
            if copy is not None:
                if copy.is_dir():
                    shutil.rmtree(copy, ignore_errors=True)
                else:
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
        with _active_lock: _active_stages.add(self)
        from . import watchdog
        watchdog.register_path(self.path)
        if self.owned_temp is not None:
            watchdog.register_path(self.owned_temp)
        return self

    def __exit__(self, exc_type, exc, tb):
        if exc_type or self.published is None:
            self.cleanup()
        if self.owned_temp is not None:
            shutil.rmtree(self.owned_temp, ignore_errors=True)
            with _active_lock: _active_temps.discard(self.owned_temp)
        with _active_lock: _active_stages.discard(self)
        from . import watchdog
        watchdog.unregister_path(self.path)
        if self.owned_temp is not None:
            watchdog.unregister_path(self.owned_temp)


def disk_budget(inputs, dest, temp, multiplier=3, estimate=None):
    size = sum(Path(p).stat().st_size for p in inputs)
    need = estimate if estimate is not None else size * multiplier
    for drive in {Path(dest).anchor, Path(temp).anchor}:
        usage = shutil.disk_usage(drive or os.getcwd())
        if usage.free < need:
            raise PdfError("TOO_BIG", f"needs {need} bytes but only {usage.free} are free on {drive}", "free disk space or choose a drive with more room")
    return need
