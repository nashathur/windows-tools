import json
import os
import sys
from pathlib import Path

from .errors import BadArg


def strip_one_newline(value):
    if value.endswith("\r\n"):
        return value[:-2]
    if value.endswith(("\r", "\n")):
        return value[:-1]
    return value


class SecretStore:
    def __init__(self):
        self.values = []
        self.password = None
        self.owner_password = None
        self.env_names = []

    def add(self, value, *, owner=False):
        if value is not None:
            value = self.add_value(value)
            if owner:
                self.owner_password = value
            else:
                self.password = value
            return value
        return None

    def add_value(self, value):
        """A secret that is not a password (a redaction target): redacted everywhere, never used to open a PDF."""
        import clierr
        value = str(value)
        self.values.append(value)
        clierr.register_secrets([value])
        return value

    def add_args(self, args, command, secret_args):
        """Register the values of `command`'s secret arguments from argv tokens or a job step dict."""
        dests = secret_args.get(command, ())
        if isinstance(args, dict):
            for dest in dests:
                value = args.get(dest)
                for item in value if isinstance(value, list) else [value]:
                    if isinstance(item, str) and item:
                        self.add_value(item)
            return
        flags = {"--" + dest.replace("_", "-") for dest in dests}
        for i, token in enumerate(args):
            name, eq, inline = token.partition("=")
            if name in flags and eq and inline:
                self.add_value(inline)
            elif token in flags and i + 1 < len(args) and args[i + 1]:
                self.add_value(args[i + 1])

    def extract(self, argv):
        clean, password = [], None
        sources = {"--password-file": ("file", False), "--password-env": ("env", False),
                   "--password": ("refused", False), "--owner-password-file": ("file", True),
                   "--owner-password-env": ("env", True), "--owner-password": ("refused", True)}
        i = 0
        while i < len(argv):
            arg, inline = argv[i], None
            if arg.split("=", 1)[0] in sources and "=" in arg:
                arg, inline = arg.split("=", 1)
            if arg in sources and (inline is not None or i + 1 < len(argv)):
                kind, owner = sources[arg]
                source = inline if inline is not None else argv[i + 1]
                if kind == "refused":
                    # stdin ("-") is refused too: PowerShell 5.1 + conda run don't deliver it reliably
                    if source != "-":
                        self.add(source, owner=owner)
                    flag = "--owner-password" if owner else "--password"
                    hint = "--owner-password-file or --owner-password-env" if owner else "--password-file or --password-env"
                    raise BadArg(f"{flag} VALUE and {flag} - are refused; use {hint}")
                if kind == "file":
                    value = strip_one_newline(Path(source).read_text(encoding="utf-8-sig"))
                else:
                    self.env_names.append(source)
                    value = os.environ.get(source)
                secret = self.add(value, owner=owner)
                if not owner:
                    password = secret
                i += 1 if inline is not None else 2
            else:
                clean.append(arg)
                i += 1
        return clean, password

    def redact(self, text):
        import clierr
        return clierr.redact(str(text), self.values)

    def redact_json(self, text):
        return self.redact(text)
