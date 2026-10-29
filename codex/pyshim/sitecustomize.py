# Python start-up hook for Codex runs only: ask.ps1 puts this folder on PYTHONPATH and sets CODEX_CONSULT.
# Windows Python 3.12.4+/3.13 turns os.mkdir(path, 0o700) (tempfile.mkdtemp, TemporaryDirectory) into a
# protected owner-only ACL. Codex's unelevated sandbox token also needs its own sandbox SIDs in the ACL, so
# it is then denied inside the folder it just made. Any other mode inherits the workspace ACL, which has them.
import os
import sys

if sys.platform == "win32" and os.environ.get("CODEX_CONSULT"):
    _mkdir = os.mkdir

    def _mkdir_inherit(path, mode=0o777, *, dir_fd=None):
        return _mkdir(path, 0o777 if mode == 0o700 else mode, dir_fd=dir_fd)

    os.mkdir = _mkdir_inherit
