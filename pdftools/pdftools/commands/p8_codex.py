"""Run equation or table prompts with the repository's Codex wrapper."""
from __future__ import annotations

import os
import re
import shutil
import sys
from pathlib import Path

from ..errors import PdfError
from ..tools import run_tool

WINDOWS = os.name == "nt"

def _role_models(script):
    text = Path(script).read_text(encoding="utf-8")
    block = re.search(r"\$roles\s*=\s*@\{(.*?)\n\s*\}", text, re.S)
    if not block:
        raise PdfError("BAD_INPUT", "could not read Codex roles from ask.ps1", "restore the $roles block")
    return {role: model for role, model in re.findall(
        r"(?m)^\s*(\w+)\s*=\s*@\{\s*Model\s*=\s*'([^']+)'", block.group(1))}


def ask(prompt_path, folder, role, effort, images, context=None):
    prompt_path, folder = Path(prompt_path).resolve(), Path(folder).resolve()
    repo = Path(__file__).resolve().parents[3]
    script = repo / "codex" / "ask.ps1"
    if WINDOWS:
        powershell = shutil.which("powershell")
        if not script.is_file() or not powershell:
            raise PdfError("CODEX_UNAVAILABLE", "Codex ask.ps1 or PowerShell is unavailable", "install Codex or use --no-read")
        argv = [powershell, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(script),
                "-PromptFile", str(prompt_path), "-Dir", str(folder), "-Role", role, "-Effort", effort]
        try:
            run_tool(argv, timeout=3600, cwd=folder,
                     secret_env=getattr(context, "secret_env", ()),
                     secret_values=(context.secret_store.values if context and getattr(context, "secret_store", None) else ()))
        except Exception as exc:
            raise PdfError(getattr(exc, "code", "TOOL_FAILED"), str(exc), "review the Codex log")
    else:
        codex = shutil.which("codex")
        if not codex or not script.is_file():
            raise PdfError("CODEX_UNAVAILABLE", "Codex CLI or codex/ask.ps1 is unavailable", "install Codex or use --no-read")
        models = _role_models(script)
        model = models.get(role)
        if not model:
            raise PdfError("BAD_INPUT", f"Codex role {role!r} is not defined in ask.ps1", "add the role to $roles")
        answer = str(prompt_path) + ".answer.md"
        argv = linux_argv(codex, model, effort, folder, answer, images)
        try:
            prompt_text = prompt_path.read_text(encoding="utf-8")
            run_tool(argv, timeout=3600, cwd=folder, env=os.environ.copy(), input=prompt_text,
                     secret_env=getattr(context, "secret_env", ()),
                     secret_values=(context.secret_store.values if context and getattr(context, "secret_store", None) else ()))
        except Exception as exc:
            raise PdfError(getattr(exc, "code", "TOOL_FAILED"), str(exc), "review the Codex log")
    answer_path = Path(str(prompt_path) + ".answer.md")
    if not answer_path.is_file():
        raise PdfError("TOOL_FAILED", "Codex did not write its answer file", "retry the reading")
    return answer_path.read_text(encoding="utf-8")


def linux_argv(codex, model, effort, folder, answer, images):
    argv = [codex, "exec", "-m", model, "-c", f"model_reasoning_effort={effort}", "-s", "read-only",
            "-C", str(folder), "--skip-git-repo-check", "-o", str(answer)]
    for image in images:
        argv.extend(["-i", str(image)])
    argv.append("-")
    return argv
