"""Preflight and run short serial PDF command pipelines."""
import argparse
import glob
import json
import os
import shlex
from pathlib import Path

import pymupdf

from .. import io
from ..errors import BadArg, PdfError
from ..pipeline import run_each
from ..registry import command, discover
from .p5_compress import _password_args


ALLOWED={"organize","extract","remove","reorder","move","swap","reverse","duplicate","blank","rotate",
         "insert","rmblank","crop","gray","meta","bookmarks","labels","watermark","numbers","headfoot",
         "flatten","ocr","compress","protect"}


def add_arguments_batch(parser):
    parser.add_argument("file",type=Path,nargs="?");parser.add_argument("pipeline",nargs="?")
    parser.add_argument("--each",type=Path);parser.add_argument("-o","--output",type=Path)
    _password_args(parser)


@command("batch","5","pdf","pdf","_batch",True,"placements")
def batch(args,context):
    # With --each, argparse places the sole remaining positional in FILE; it is the pipeline shorthand.
    if args.each and args.file and not args.pipeline and getattr(args,"steps",None) is None:
        args.pipeline=Path(args.file).name;args.file=None
    raw=getattr(args,"steps",None)
    problems=[]
    if bool(args.file)==bool(args.each):problems.append("give exactly one of FILE and --each")
    try:steps=_normalize_steps(raw,args.pipeline,problems)
    except Exception as exc:steps=[];problems.append(str(exc))
    normalized=[]
    specs=discover()
    for index,step in enumerate(steps):
        if not isinstance(step,dict):problems.append(f"steps[{index}] must be an object");continue
        name=step.get("cmd")
        if not isinstance(name,str) or name not in specs or name not in ALLOWED:
            problems.append(f"steps[{index}] has disallowed or unknown command: {name}");continue
        values={k:v for k,v in step.items() if k!="cmd"}
        if name=="rotate":values.setdefault("pages","1-")
        forbidden=set(values)&{"file","input","output","each"}
        for key in sorted(forbidden):problems.append(f"steps[{index}] may not set {key}")
        parser=argparse.ArgumentParser(add_help=False,allow_abbrev=False)
        specs[name].add_arguments(parser)
        actions={a.dest:a for a in parser._actions if a.dest!="help"}
        unknown=set(values)-set(actions)
        for key in sorted(unknown):problems.append(f"steps[{index}] unknown key for {name}: {key}")
        required={a.dest for a in parser._actions if a.dest!="help" and (a.required or not a.option_strings)}
        missing=required-set(values)-{"file"}
        for key in sorted(missing):problems.append(f"steps[{index}] missing required key for {name}: {key}")
        for key,value in list(values.items()):
            action=actions.get(key)
            if action is None:continue
            try:
                if action.type is not None and not (action.type is json.loads and isinstance(value,(dict,list))):
                    value=[action.type(item) for item in value] if isinstance(value,list) else action.type(value)
                if action.choices is not None:
                    candidates=value if isinstance(value,list) else [value]
                    if any(item not in action.choices for item in candidates):
                        raise ValueError(f"{key} must be one of {tuple(action.choices)}")
                if action.type is Path and value is not None:
                    base=Path(getattr(args,"job_base",Path.cwd()))
                    resolve=lambda item: (base/Path(item)).resolve() if not Path(item).is_absolute() else Path(item)
                    value=[resolve(item) for item in value] if isinstance(value,list) else resolve(value)
                if isinstance(action,argparse._StoreTrueAction) and not isinstance(value,bool):
                    raise ValueError(f"{key} must be a boolean")
                values[key]=value
            except Exception as exc:problems.append(f"steps[{index}] invalid {key}: {exc}")
        for group in parser._mutually_exclusive_groups:
            selected=[action.dest for action in group._group_actions if values.get(action.dest) not in (None,False,[],"")]
            if len(selected)>1:problems.append(f"steps[{index}] options are mutually exclusive: {', '.join(selected)}")
            if group.required and not selected:problems.append(f"steps[{index}] requires one of: {', '.join(a.dest for a in group._group_actions)}")
        if name=="meta":
            setting_keys={"title","author","subject","keywords","creator","producer","created","modified","lang","custom"}
            if not (any(values.get(key) is not None for key in setting_keys) or values.get("clear") or values.get("from_file")):
                problems.append(f"steps[{index}] meta must set metadata, clear it, or use from_file")
            if values.get("to"):problems.append(f"steps[{index}] meta export mode is not allowed in batch")
        if name=="bookmarks":
            if values.get("list") or not (values.get("items") or values.get("clear")):
                problems.append(f"steps[{index}] bookmarks must set or clear; list mode is not allowed")
        if name=="protect" and not any(values.get(key) for key in ("password_file","password_env","owner_password_file","owner_password_env",
                                                                      "new_password_file","new_password_env","new_owner_password_file","new_owner_password_env")):
            problems.append(f"steps[{index}] protect needs a new password source")
        if any(values.get(key) for key in ("list","show")) or (name!="insert" and values.get("to")):
            problems.append(f"steps[{index}] listing or export mode is not allowed in batch")
        normalized.append((name,values,parser,actions))
    if normalized and any(name=="protect" for name,_,_,_ in normalized[:-1]):
        problems.append("protect must be the last step")
    if not normalized:problems.append("batch needs at least one step")
    if context.secret_store:
        for index,(name,values,parser,actions) in enumerate(normalized):
            if name!="protect":continue
            base=Path(getattr(args,"job_base",Path.cwd()))
            for source_key,env_key,owner in (("password_file","password_env",False),("owner_password_file","owner_password_env",True)):
                file_value=values.get(source_key);env_name=values.get(env_key)
                if file_value and env_name:problems.append(f"steps[{index}] choose one new password source for {source_key}");continue
                if file_value:
                    secret=Path(file_value).read_text(encoding="utf-8-sig")
                    from ..secrets import strip_one_newline
                    secret=context.secret_store.add(strip_one_newline(secret),owner=owner)
                    values["new_owner_password_file" if owner else "new_password_file"]=Path(file_value)
                elif env_name:
                    context.secret_store.env_names.append(env_name)
                    secret=context.secret_store.add(os.environ.get(env_name),owner=owner)
                    if secret is None:problems.append(f"steps[{index}] password environment variable is not set: {env_name}")
                    values["new_owner_password_env" if owner else "new_password_env"]=env_name
            # protect's own new-password keys are secrets too, hidden from every earlier step's children
            from ..secrets import strip_one_newline
            for key,owner in (("new_password_file",False),("new_owner_password_file",True)):
                if values.get(key):
                    context.secret_store.add(strip_one_newline(Path(values[key]).read_text(encoding="utf-8-sig")),owner=owner)
            for key,owner in (("new_password_env",False),("new_owner_password_env",True)):
                if values.get(key) and values[key] not in context.secret_store.env_names:
                    context.secret_store.env_names.append(values[key])
                    context.secret_store.add(os.environ.get(values[key]),owner=owner)
        context.secret_env=tuple(context.secret_store.env_names)
    if problems:raise BadArg(f"batch has {len(problems)} problem(s): {problems[0]}",problems)
    if getattr(args,"dry_run",False):
        return {"type":"report","command":"batch","input":str(args.file or args.each),"verdict":"report",
                "data":{"steps":[{"cmd":name,"values":values} for name,values,_,_ in normalized]}}
    if args.each:
        return run_each(_glob_files(args.each),lambda p:_run_file(p,args,context,normalized),"batch",context)
    base=Path(getattr(args,"job_base",Path.cwd()))
    return _run_file(Path(args.file).resolve() if Path(args.file).is_absolute() else (base/args.file).resolve(),args,context,normalized)


def _normalize_steps(raw,pipeline,problems):
    if raw is not None:
        if not isinstance(raw,list):return ["steps must be a list"]
        return raw
    if not pipeline:return []
    result=[]
    for index,chunk in enumerate(pipeline.split("|")):
        try:tokens=shlex.split(chunk.strip())
        except ValueError as exc:problems.append(f"pipeline step {index+1}: {exc}");continue
        if not tokens:problems.append(f"pipeline step {index+1} is empty");continue
        name=tokens[0];spec=discover().get(name)
        if not spec:result.append({"cmd":name});continue
        parser=_StepParser()
        spec.add_arguments(parser)
        command_tokens=tokens[1:]
        if name=="rotate" and "--pages" not in command_tokens:command_tokens=["--pages","1-",*command_tokens]
        try:parsed=vars(parser.parse_args(["__BATCH_INPUT__",*command_tokens]))
        except (SystemExit,ValueError):
            problems.append(f"pipeline step {index+1} has invalid flags for {name}");continue
        explicit={}
        for action in parser._actions:
            key=action.dest
            if key in {"help","file","output","each","password_file","password_env","owner_password_file","owner_password_env","ignore_restrictions","break_signature"}:continue
            value=parsed.get(key,action.default)
            if value!=action.default:explicit[key]=value
        result.append({"cmd":name,**explicit})
    return result


class _StepParser(argparse.ArgumentParser):
    def error(self,message):raise ValueError(message)


def _run_file(source,args,context,steps):
    if not source.is_file():raise PdfError("NOT_FOUND",f"no PDF file: {source}","check the input path")
    spec=discover()["batch"];final=io.resolve_output(source,"batch",getattr(args,"output",None),"_batch",".pdf")
    final.parent.mkdir(parents=True,exist_ok=True)
    records=[];warnings=[];changed=False;current=source;last_published=False
    with io.temp_folder() as temp:
        for index,(name,values,parser,actions) in enumerate(steps,1):
            last=index==len(steps)
            cmd_spec=discover()[name]
            out=final if last else Path(temp)/f"step-{index}{cmd_spec.suffix or '_'+name}.pdf"
            step_values={key:value for key,value in values.items() if not key.startswith("_batch_new_")}
            namespace={a.dest:a.default for a in parser._actions if a.dest!="help"}
            namespace.update(step_values);namespace.update(file=current,output=out,each=None,
                password_file=getattr(args,"password_file",None),password_env=getattr(args,"password_env",None),
                owner_password_file=getattr(args,"owner_password_file",None),owner_password_env=getattr(args,"owner_password_env",None),
                ignore_restrictions=getattr(args,"ignore_restrictions",False),break_signature=getattr(args,"break_signature",False),
                verbose=getattr(args,"verbose",False),dry_run=False)
            namespace["job_base"]=getattr(args,"job_base",Path.cwd())
            # Context passwords unlock the current pipeline input. Protect's new
            # passwords are carried separately in its --new-password arguments.
            result=cmd_spec.run(argparse.Namespace(**namespace),context)
            if isinstance(result,list):raise PdfError("BAD_ARG","--each is not allowed inside batch","remove --each from the step")
            verdict=result.get("verdict","ok");warnings.extend(result.get("warnings",[]))
            records.append({"cmd":name,"verdict":verdict,"warnings":list(result.get("warnings",[]))})
            if verdict=="unchecked":
                context.exit_code=3
                produced=Path(result["path"])
                unchecked=final.with_name(final.stem+"_UNCHECKED"+final.suffix)
                try:inside_temp=produced.resolve().is_relative_to(Path(temp).resolve())
                except AttributeError:inside_temp=Path(temp).resolve() in produced.resolve().parents
                published=io.publish(produced,unchecked) if inside_temp else produced
                return _record(source,published,result.get("pages"),records,warnings,"unchecked",index,name)
            if verdict in {"failed"} or not result.get("path"):
                raise PdfError("CHECK_FAILED",f"batch step {index} ({name}) failed","review the step output and rerun that command")
            produced=Path(result["path"])
            if produced.resolve()!=current.resolve():changed=True
            last_published=last and produced.parent.resolve()==final.parent.resolve() and \
                (produced.name==final.name or produced.stem.startswith(final.stem+"_"))
            current=produced
        if not changed:
            return _record(source,source,None,records,warnings,"warn",None,None,add="NO_GAIN")
        if last_published:
            published=current
        elif current.resolve()!=final.resolve():
            published=io.publish(current,final)
        else:published=current
        return _record(source,published,None,records,warnings,"warn" if warnings else "ok")


def _record(source,path,pages,steps,warnings,verdict,index=None,name=None,add=None):
    if add:warnings.append(add)
    if pages is None:
        try:
            with pymupdf.open(path) as doc:pages=doc.page_count
        except Exception:pages=None
    data={"type":"result","command":"batch","path":str(path),"kind":"pdf","pages":pages,
          "bytes":Path(path).stat().st_size,"verdict":verdict,"warnings":list(dict.fromkeys(warnings)),"steps":steps}
    if index:data["failed_step"]={"index":index,"cmd":name}
    return data


def _glob_files(value):
    p=Path(value)
    if p.is_dir():return sorted(p.glob("*.pdf"))
    matches=glob.glob(str(value));return [Path(x) for x in matches] if matches else ([p] if p.is_file() else [])
