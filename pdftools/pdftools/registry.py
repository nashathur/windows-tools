import importlib
import pkgutil
from dataclasses import dataclass

INPUT_KINDS = frozenset({"pdf", "image", "folder", "text", "markdown", "docx", "any"})
OUTPUT_KINDS = frozenset({"pdf", "image", "docx", "html", "txt", "json", "zip", "folder", "report"})
BUILTIN_POLICIES = frozenset({
    "report", "placements", "rotate", "stamp", "annot_add", "annot_remove", "flatten",
    "flatten_raster", "crop", "rmblank", "structure", "gray", "compress_lossless",
    "compress_lossy", "ocr",
})


@dataclass(frozen=True)
class Command:
    name: str
    phase: int
    input_kind: str
    output_kind: str
    suffix: str
    rewrites_pdf: bool
    check_policy: object
    add_arguments: object
    run: object


_COMMANDS = {}


def command(name, phase, input_kind, output_kind, suffix, rewrites_pdf, check_policy):
    phase = str(phase)
    if not name or not isinstance(name, str):
        raise ValueError("command name must be a non-empty string")
    if phase not in {"1", "2", "3a", "3b", "3c", "4", "5", "6", "7"}:
        raise ValueError(f"unknown command phase: {phase}")
    if input_kind not in INPUT_KINDS:
        raise ValueError(f"unknown input kind: {input_kind}")
    if output_kind not in OUTPUT_KINDS:
        raise ValueError(f"unknown output kind: {output_kind}")
    if not isinstance(suffix, str):
        raise ValueError("suffix must be a string")
    if not isinstance(rewrites_pdf, bool):
        raise ValueError("rewrites_pdf must be a bool")
    if not (callable(check_policy) or check_policy in BUILTIN_POLICIES):
        raise ValueError(f"unknown check policy: {check_policy}")
    def register(func):
        if name in _COMMANDS:
            raise ValueError(f"duplicate command: {name}")
        import sys
        add_arguments = getattr(sys.modules[func.__module__], "add_arguments")
        _COMMANDS[name] = Command(name, phase, input_kind, output_kind, suffix,
                                  rewrites_pdf, check_policy, add_arguments, func)
        return func
    return register


def discover():
    from . import commands
    for info in pkgutil.iter_modules(commands.__path__, commands.__name__ + "."):
        importlib.import_module(info.name)
    return dict(_COMMANDS)


def get(name):
    discover()
    return _COMMANDS.get(name)


def commands_by_phase(phase=None):
    discover()
    if phase is None:
        return dict(_COMMANDS)
    phase = str(phase)
    return {name: spec for name, spec in _COMMANDS.items() if spec.phase == phase}


def output_formats():
    """Writable formats represented by registered commands."""
    return sorted({"pdf", *{spec.output_kind for spec in discover().values() if spec.output_kind != "report"}})
