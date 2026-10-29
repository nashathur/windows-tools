import argparse
import json
import os
import re
import sys
import warnings
from pathlib import Path

import clierr

from . import io
from .errors import BadArg, library_warnings
from .registry import discover
from .secrets import SecretStore, strip_one_newline


class Parser(argparse.ArgumentParser):
    def __init__(self, *args, **kwargs):
        kwargs["allow_abbrev"] = False
        kwargs["add_help"] = kwargs.get("add_help", True)
        super().__init__(*args, **kwargs)

    def error(self, message):
        raise BadArg(message)


class Context:
    def __init__(self, password=None, json_output=False, secret_env=(), owner_password=None):
        self.password = password
        self.owner_password = owner_password
        self.json_output = json_output
        self.secret_env = tuple(secret_env)
        self.secret_store = None
        self.exit_code = 0


def build_parser(commands=None):
    commands = commands or discover()
    parser = Parser(prog="pdf")
    parser.add_argument("-v", action="store_true")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--stdout", action="store_true")
    sub = parser.add_subparsers(dest="command", required=True, parser_class=Parser)
    for name, spec in commands.items():
        child = sub.add_parser(name)
        child.add_argument("-v", action="store_true", default=argparse.SUPPRESS)
        child.add_argument("--json", action="store_true", default=argparse.SUPPRESS)
        child.add_argument("--dry-run", action="store_true", default=argparse.SUPPRESS)
        child.add_argument("--stdout", action="store_true", default=argparse.SUPPRESS)
        spec.add_arguments(child)
    return parser


def load_job(path, commands, *, cwd=None, secret_store=None):
    path = Path(path)
    base = Path(cwd or os.getcwd()) if str(path) == "-" else path.resolve().parent
    try:
        raw = sys.stdin.buffer.read() if str(path) == "-" else path.read_bytes()
        # PowerShell 5.1 piping through conda run can deliver more than one BOM
        job = json.loads(raw.decode("utf-8").lstrip("﻿"))
    except Exception as e:
        raise BadArg(f"invalid job JSON: {e}")
    problems = []
    if not isinstance(job, dict):
        raise BadArg("job must be a JSON object")
    for key in ("command", "cmd"):
        if key in job and not isinstance(job[key], str):
            problems.append(f"{key} must be a string")
    for key in ("input", "output", "password_file", "password_env", "owner_password_file", "owner_password_env"):
        if key in job and not isinstance(job[key], str) and not (key == "output" and job[key] is None):
            problems.append(f"{key} must be a string")
    steps = job.get("steps", [])
    if not isinstance(steps, list):
        problems.append("steps must be a list")
        steps = []
    else:
        for index, step in enumerate(steps):
            if not isinstance(step, dict):
                problems.append(f"steps[{index}] must be an object")
    if problems:
        raise BadArg(f"job has {len(problems)} problem(s): {problems[0]}", problems)
    if secret_store is not None:
        for key, owner in (("password_file", False), ("owner_password_file", True)):
            if job.get(key):
                secret_path = Path(job[key])
                if not secret_path.is_absolute():
                    secret_path = base / secret_path
                secret_store.add(strip_one_newline(secret_path.read_text(encoding="utf-8-sig")), owner=owner)
        for key, owner in (("password_env", False), ("owner_password_env", True)):
            if job.get(key):
                secret_store.env_names.append(job[key])
                secret_store.add(os.environ.get(job[key]), owner=owner)
    if job.get("version") != 1:
        problems.append("version must be 1")
    if len(steps) > 1:
        problems.append("multi-step jobs are not available in phase 1; run steps separately (batch arrives in phase 5)")
    name = job.get("command", job.get("cmd"))
    step_values = {}
    if name is None and steps:
        if not isinstance(steps[0], dict):
            problems.append("each step must be an object")
        else:
            name = steps[0].get("cmd")
            step_values = {k: v for k, v in steps[0].items() if k != "cmd"}
    if not isinstance(name, str) or name not in commands:
        problems.append(f"unknown command: {name}")
    allowed_global = {"version", "command", "cmd", "input", "steps", "output", "password_file", "password_env",
                      "owner_password_file", "owner_password_env"}
    command_dest = set()
    required_dest = set()
    if isinstance(name, str) and name in commands:
        parser = Parser()
        commands[name].add_arguments(parser)
        command_dest = {a.dest for a in parser._actions if a.dest != "help"}
        required_dest = {a.dest for a in parser._actions if a.dest != "help" and
                         (a.required or not a.option_strings)}
    unknown = set(job) - allowed_global - command_dest
    if unknown:
        problems.extend(f"unknown job key: {key}" for key in sorted(unknown))
    step_unknown = set(step_values) - command_dest
    if step_unknown:
        problems.extend(f"unknown job key: {key}" for key in sorted(step_unknown))
    if problems:
        raise BadArg(f"job has {len(problems)} problem(s): {problems[0]}", problems)
    values = {k: v for k, v in job.items() if k in command_dest}
    values.update(step_values)
    if "input" in job and "file" in command_dest:
        values["file"] = job["input"]
    for key in sorted(required_dest - set(values)):
        problems.append(f"missing required job key: {key}")
    if problems:
        raise BadArg(f"job has {len(problems)} problem(s): {problems[0]}", problems)
    if "file" in values:
        p = Path(values["file"])
        values["file"] = (base / p).resolve() if not p.is_absolute() else p
    for key in ("password_file", "owner_password_file"):
        if job.get(key):
            p = Path(job[key])
            values[key] = str((base / p).resolve() if not p.is_absolute() else p)
    # Every other Path-typed argument is job-relative too (check --against, insert's file, ...).
    for action in parser._actions:
        key = action.dest
        if action.type is not Path or key in {"file", "password_file", "owner_password_file"} or values.get(key) is None:
            continue
        resolve = lambda v: (base / Path(v)).resolve() if not Path(v).is_absolute() else Path(v)
        values[key] = [resolve(v) for v in values[key]] if isinstance(values[key], list) else resolve(values[key])
    values = {a.dest: a.default for a in parser._actions if a.dest != "help"} | values
    values.update(command=name, verbose=False, json=False, dry_run=False, stdout=False,
                  job_base=base)  # nested job paths (recipe files, merge inputs) resolve against this
    return argparse.Namespace(**values), job


def emit(record, json_output, force_stdout=False):
    if record is None:
        return
    if isinstance(record, list):  # --each: one record per file plus a summary
        for item in record:
            emit(item, json_output, force_stdout)
        return
    if record.get("type") == "error":
        if json_output:
            print(json.dumps(record, ensure_ascii=False, separators=(",", ":")))
        else:
            print(f"{record.get('input')}: {record.get('code')}: {record.get('message')} | fix: {record.get('fix')}")
        return
    if record.get("type") == "report" and not force_stdout:
        data = record.get("data")
        lines = data.splitlines() if isinstance(data, str) else json.dumps(data, ensure_ascii=False, indent=2).splitlines()
        if len(lines) > 200:
            import datetime
            import os
            root = Path(__file__).resolve().parents[1] / "reports"
            root.mkdir(parents=True, exist_ok=True)
            path = report_path(root, record.get("command", "report"), Path(record.get("input", "report")).stem, ".txt")
            path.write_text("\n".join(lines) + "\n", encoding="utf-8")
            prune_reports(root)
            words = sum(len(line.split()) for line in lines)
            if json_output:
                record.pop("data", None)
                record.update(path=str(path), lines=len(lines), words=words, preview="\n".join(lines[:20]))
                print(json.dumps(record, ensure_ascii=False, separators=(",", ":")))
            else:
                print(f"{path} ({len(lines)} lines, {words} words)\n" + "\n".join(lines[:20]))
            return
    if json_output:
        print(json.dumps(record, ensure_ascii=False, separators=(",", ":")))
    elif record.get("type") == "report":
        data = record.get("data")
        if record.get("command") == "info" and isinstance(data, dict):
            print(f"{record['input']}: {data['pages']} pages, {data['bytes']} bytes; verdict {record['verdict']}")
            return
        if isinstance(data, str):
            lines = data.splitlines()
        else:
            lines = json.dumps(data, ensure_ascii=False, indent=2).splitlines()
        print("\n".join(lines))
    elif record.get("type") == "result":
        path = record.get("path") or "(nothing published)"
        if record.get("pages") is None:
            print(f"{path}: {record.get('bytes', 0)} bytes; verdict {record.get('verdict')}")
        else:
            print(f"{path}: {record.get('pages')} pages, {record.get('bytes', 0)} bytes; verdict {record.get('verdict')}")
    elif record.get("type") == "summary":
        print(json.dumps(record, ensure_ascii=False))


def report_path(root, command, stem, suffix):
    import datetime
    import os
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    return Path(root) / f"{stamp}-{os.getpid()}-{command}-{stem}{suffix}"


def prune_reports(root, keep=50):
    root = Path(root)
    files = sorted((p for p in root.iterdir() if p.is_file()), key=lambda p: p.stat().st_mtime, reverse=True)
    for old in files[keep:]:
        old.unlink(missing_ok=True)


def _raw_output(argv):
    """The `-o` text as typed (argparse keeps the last one), before `type=Path` drops a trailing separator."""
    raw = None
    for index, token in enumerate(map(str, argv)):
        if token in ("-o", "--output") and index + 1 < len(argv):
            raw = str(argv[index + 1])
        elif token.startswith("--output="):
            raw = token.split("=", 1)[1]
    return raw


def main(argv=None):
    argsv = list(sys.argv[1:] if argv is None else argv)
    warnings.filterwarnings("default" if "-v" in argsv else "ignore")
    library_warnings.clear()
    original_args = list(argsv)
    store = SecretStore()
    secrets = store.values
    commands = {}
    try:
        commands = discover()
        argsv, password = store.extract(argsv)
        job_path = None
        global_flags = {"-v": False, "--json": False, "--dry-run": False, "--stdout": False}
        remaining = []
        i = 0
        while i < len(argsv):
            if argsv[i] == "--job" and i + 1 < len(argsv):
                job_path = argsv[i + 1]
                i += 2
            elif argsv[i] in global_flags:
                global_flags[argsv[i]] = True
                i += 1
            else:
                remaining.append(argsv[i]); i += 1
        if job_path is not None:
            if remaining:
                raise BadArg("with --job only global flags are allowed on the command line")
            if any(x.split("=", 1)[0] in ("--password-file", "--password-env", "--password",
                                           "--owner-password-file", "--owner-password-env", "--owner-password")
                   for x in original_args):
                raise BadArg("password sources must be in the job when --job is used")
            args, job = load_job(job_path, commands, secret_store=store)
            password = password or store.password
            args.json = global_flags["--json"]
            args.verbose = global_flags["-v"]
            args.dry_run = global_flags["--dry-run"]
            args.stdout = global_flags["--stdout"]
            raw_output = next((step["output"] for step in job.get("steps", [])[:1] if "output" in step), job.get("output"))
        else:
            args = build_parser(commands).parse_args(argsv)
            raw_output = _raw_output(argsv)
            if getattr(args, "file", None):
                args.file = Path(args.file).resolve()
            if password is None and getattr(args, "password_file", None):
                password = store.add(strip_one_newline(Path(args.password_file).read_text(encoding="utf-8-sig")))
            if password is None and getattr(args, "password_env", None):
                password = store.add(os.environ.get(args.password_env))
            if store.owner_password is None and getattr(args, "owner_password_file", None):
                store.add(strip_one_newline(Path(args.owner_password_file).read_text(encoding="utf-8-sig")), owner=True)
            if store.owner_password is None and getattr(args, "owner_password_env", None):
                store.env_names.append(args.owner_password_env)
                store.add(os.environ.get(args.owner_password_env), owner=True)
        if hasattr(args, "output"):
            args.output = io.folder_marker(args.output, raw_output)
        context = Context(password, args.json, store.env_names, store.owner_password)
        context.secret_store = store
        record = commands[args.command].run(args, context)
        if library_warnings:
            clierr.write_log("pdftools", "INFO", "PDF library warnings", "review the warning section",
                             secrets=secrets, sections={"library warnings": "\n".join(library_warnings)},
                             log_name=args.command)
        emit(record, args.json, getattr(args, "stdout", False))
        return context.exit_code
    except BaseException as e:
        if isinstance(e, SystemExit):
            raise
        command_name = getattr(locals().get("args"), "command", None)
        if not isinstance(command_name, str):
            command_name = next((arg for arg in argsv if arg in commands), "unknown")
        log_name = re.sub(r"[^A-Za-z0-9_.-]+", "_", command_name).strip(" .") or "unknown"
        if "--json" in argsv:
            code, classified_what, classified_fix = ((e.code, e.what, e.fix) if isinstance(e, clierr.Fail)
                                                     else clierr._classify(e))
            what = store.redact(classified_what)
            fix = store.redact(classified_fix)
            log = clierr.write_log("pdftools", code, what, fix, e, secrets=secrets,
                                   sections={"problems": "\n".join(getattr(e, "problems", ())),
                                             "library warnings": "\n".join(library_warnings)}, log_name=log_name)
            print(json.dumps({"type": "error", "command": command_name, "code": code,
                              "message": what, "fix": fix, "log": log,
                              "problems": [store.redact(p) for p in getattr(e, "problems", ())]}, ensure_ascii=False))
            print(f"ERROR pdf {command_name} {code}: {clierr._one_line(what)} | fix: {clierr._one_line(fix)} | log: {log}", file=sys.stderr)
            return 2 if code == "BAD_ARG" else 1
        return clierr.fail("pdftools", e, secrets=secrets, log_name=log_name,
                           sections={"problems": "\n".join(getattr(e, "problems", ())),
                                     "library warnings": "\n".join(library_warnings)},
                           display_tool=f"pdf {command_name}")
