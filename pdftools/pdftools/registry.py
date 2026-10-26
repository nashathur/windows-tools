import importlib
import pkgutil
from dataclasses import dataclass


@dataclass(frozen=True)
class Command:
    name: str
    phase: int
    input_kind: str
    output_kind: str
    suffix: str
    rewrites_pdf: bool
    check_policy: str
    add_arguments: object
    run: object


_COMMANDS = {}


def command(name, phase, input_kind, output_kind, suffix, rewrites_pdf, check_policy):
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
