import contextlib
import hashlib
import io
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT));sys.path.insert(0,str(ROOT.parent/"common"))
import pymupdf
import pikepdf
from pdftools import cli
from tests.fixtures.p5 import build_fixtures


class Phase5CommandsTest(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name);self.fx=build_fixtures(self.root/"fixtures")
    def tearDown(self):self.tmp.cleanup()
    def run_cli(self,*args):
        out,err=io.StringIO(),io.StringIO()
        with patch("clierr.LOG_ROOT",self.root/"logs"),contextlib.redirect_stdout(out),contextlib.redirect_stderr(err):
            code=cli.main(["--json",*map(str,args)])
        records=[json.loads(x) for x in out.getvalue().splitlines() if x.strip()]
        return code,(records[0] if len(records)==1 else records),err.getvalue()
    def test_compress_candidate_and_no_gain(self):
        code,record,err=self.run_cli("compress",self.fx["photo"],"--level","med","-o",self.root/"photo-out.pdf")
        self.assertEqual(code,0,(record,err));self.assertEqual(record["verdict"],"ok",(record,err))
        self.assertLess(record["after"],record["before"]);self.assertIn("Known phase five words",pymupdf.open(record["path"])[0].get_text())
        code,small,err=self.run_cli("compress",self.fx["small"])
        self.assertEqual(code,0,(small,err));self.assertEqual(small["path"],str(self.fx["small"]));self.assertIn("NO_GAIN",small["warnings"])
    def test_compress_lossless_graphics_and_target_validation(self):
        code,record,err=self.run_cli("compress",self.fx["lossless"],"--level","lossless","-o",self.root/"lossless-out.pdf")
        self.assertEqual(code,0,(record,err));self.assertIn("left alone",record["changes"])
        code,record,err=self.run_cli("compress",self.fx["photo"],"--target","1KB","-o",self.root/"target.pdf")
        self.assertEqual(code,0,(record,err));self.assertIn("TARGET_MISSED",record["warnings"])
        code,record,err=self.run_cli("compress",self.fx["small"],"--target","1MB","--level","high")
        self.assertEqual(code,2);self.assertEqual(record["code"],"BAD_ARG")
        self.assertFalse(self.fx["small"].with_name("small_compressed.pdf").exists())
        code,target,err=self.run_cli("compress",self.fx["photo"],"--target","2MB","-o",self.root/"target-reached.pdf")
        self.assertEqual(code,0,(target,err));self.assertLessEqual(target["after"],2*1024*1024)
    def test_scanned_and_graphics_compression_candidates_run(self):
        from tests.tooling import require_tool
        require_tool(self,"ocrmypdf","scanned compression uses the OCRmyPDF optimizer")
        require_tool(self,"ghostscript","graphics compression exercises the Ghostscript candidate")
        from pdftools import tools
        with patch("pdftools.commands.p5_compress.run_tool",wraps=tools.run_tool) as runner:
            code,record,err=self.run_cli("compress",self.fx["scan"],"--level","lossless","-o",self.root/"scan-compressed.pdf")
        self.assertEqual(code,0,(record,err))
        self.assertTrue(any("ocrmypdf" in str(call.args[0][0]) and "--ocr-engine" in call.args[0] for call in runner.call_args_list))
        with patch("pdftools.commands.p5_compress.run_tool",wraps=tools.run_tool) as runner:
            code,record,err=self.run_cli("compress",self.fx["photo"],"--include-graphics","--level","high","-o",self.root/"photo-high.pdf")
        self.assertEqual(code,0,(record,err))
        self.assertTrue(any("-sDEVICE=pdfwrite" in call.args[0] for call in runner.call_args_list))
    def test_ocr_and_batch_forms(self):
        from tests.tooling import require_tool
        require_tool(self,"ocrmypdf","OCR integration exercises OCRmyPDF")
        require_tool(self,"tesseract","OCR integration exercises the Tesseract engine")
        code,ocr,err=self.run_cli("ocr",self.fx["scan"],"--lang","eng+ind","--pdfa","-o",self.root/"ocr.pdf")
        self.assertEqual(code,0,(ocr,err));self.assertGreaterEqual(ocr["ocr_pages"],1)
        with pymupdf.open(ocr["path"]) as doc:
            text=doc[0].get_text().lower();self.assertIn("hello",text);self.assertIn("selamat",text)
        existing=self.root/"existing.pdf";doc=pymupdf.open();page=doc.new_page();page.insert_text((30,50),"Already searchable");doc.save(existing);doc.close()
        code,skipped,err=self.run_cli("ocr",existing,"-o",self.root/"existing-ocr.pdf")
        self.assertEqual(code,0,(skipped,err));self.assertIn("0 pages OCRed, 1 had text",skipped["note"])
        blank=self.root/"blank-scan.pdf";from PIL import Image
        import numpy as np
        image=self.root/"blank.png";Image.fromarray(np.full((160,240,3),255,dtype=np.uint8)).save(image)
        doc=pymupdf.open();page=doc.new_page(width=240,height=160);page.insert_image(page.rect,filename=str(image));doc.save(blank);doc.close()
        code,empty,err=self.run_cli("ocr",blank,"-o",self.root/"blank-ocr.pdf")
        self.assertEqual(code,0,(empty,err));self.assertIn("OCR_EMPTY",empty["warnings"]);self.assertEqual(err.count("WARN pdf ocr OCR_EMPTY"),1)
        self.assertEqual(empty["verdict"],"warn")
    def test_encrypted_ocr_preserves_password_without_leaking_it(self):
        from tests.tooling import require_tool
        require_tool(self,"ocrmypdf","encrypted OCR integration exercises OCRmyPDF")
        require_tool(self,"tesseract","encrypted OCR integration exercises Tesseract")
        encrypted=self.root/"encrypted-scan.pdf"
        with pymupdf.open(self.fx["scan"]) as doc:
            doc.save(encrypted,encryption=pymupdf.PDF_ENCRYPT_AES_256,user_pw="phase5-secret",owner_pw="owner-secret")
        encrypted_hash=hashlib.sha256(encrypted.read_bytes()).hexdigest()
        password_file=self.root/"password.txt";password_file.write_text("phase5-secret\n",encoding="utf-8")
        from pdftools import tools
        with patch("pdftools.commands.p5_ocr.run_tool",wraps=tools.run_tool) as runner:
            code,record,err=self.run_cli("ocr",encrypted,"--password-file",password_file,"-o",self.root/"encrypted-out.pdf")
        self.assertEqual(code,0,(record,err));self.assertNotIn("phase5-secret",json.dumps(record)+err)
        self.assertEqual(hashlib.sha256(encrypted.read_bytes()).hexdigest(),encrypted_hash)
        self.assertTrue(all("phase5-secret" not in str(arg) for call in runner.call_args_list for arg in call.args[0]))
        with pikepdf.open(record["path"],password="phase5-secret") as output:self.assertTrue(output.is_encrypted)
        with patch("pdftools.commands.p5_compress.run_tool",wraps=tools.run_tool) as runner:
            code,compressed,err=self.run_cli("compress",encrypted,"--password-file",password_file,"--level","lossless","-o",self.root/"encrypted-compressed.pdf")
        self.assertEqual(code,0,(compressed,err));self.assertNotIn("phase5-secret",json.dumps(compressed)+err)
        self.assertEqual(hashlib.sha256(encrypted.read_bytes()).hexdigest(),encrypted_hash)
        self.assertTrue(all("phase5-secret" not in str(arg) for call in runner.call_args_list for arg in call.args[0]))
        with pikepdf.open(compressed["path"],password="phase5-secret") as output:self.assertTrue(output.is_encrypted)
        code,short,err=self.run_cli("batch",self.fx["small"],"rotate --by 90", "-o",self.root/"short.pdf")
        self.assertEqual(code,0,(short,err))
        from pdftools.commands.p5_batch import batch
        import argparse
        args=argparse.Namespace(file=self.fx["small"],pipeline=None,steps=[{"cmd":"rotate","by":90}],each=None,
            output=self.root/"job.pdf",password_file=None,password_env=None,owner_password_file=None,owner_password_env=None,
            ignore_restrictions=False,break_signature=False,dry_run=False,verbose=False)
        long=batch(args,cli.Context())
        self.assertEqual(long["verdict"],"ok");self.assertEqual(pymupdf.open(short["path"])[0].rotation,pymupdf.open(long["path"])[0].rotation)
    def test_batch_preflight_dry_run_and_each(self):
        before=set(self.root.rglob("*.pdf"))
        code,record,err=self.run_cli("batch",self.fx["small"],"rotate --by 90 | bogus --x 1")
        self.assertEqual(code,2,(record,err));self.assertGreaterEqual(len(record["problems"]),1);self.assertEqual(before,set(self.root.rglob("*.pdf")))
        code,plan,err=self.run_cli("--dry-run","batch",self.fx["small"],"rotate --by 90")
        self.assertEqual(code,0,(plan,err));self.assertEqual(before,set(self.root.rglob("*.pdf")))
        folder=self.root/"many";folder.mkdir();(folder/"bad.pdf").write_bytes(b"bad");(folder/"good.pdf").write_bytes(self.fx["small"].read_bytes())
        code,rows,err=self.run_cli("batch","--each",folder,"rotate --by 90")
        self.assertEqual(code,4,(rows,err));self.assertEqual(rows[-1]["type"],"summary");self.assertEqual(rows[-1]["failed"],1)
        from pdftools.commands.p5_batch import batch
        args=type("Args",(),dict(file=self.fx["small"],pipeline=None,steps=[{"cmd":"rotate","by":90,"bogus":1},
            {"cmd":"unsupported","extra":1}],each=None,output=self.root/"bad-preflight.pdf",password_file=None,password_env=None,
            owner_password_file=None,owner_password_env=None,ignore_restrictions=False,break_signature=False,dry_run=False,verbose=False))()
        with self.assertRaises(Exception) as caught:batch(args,cli.Context())
        self.assertGreaterEqual(len(caught.exception.problems),2)

    def test_protect_must_be_last_step(self):
        job=self.root/"protect-order.json"
        job.write_text(json.dumps({"version":1,"command":"batch","input":str(self.fx["small"]),"steps":[
            {"cmd":"protect","password_env":"P5_NEW_PROTECT_PASSWORD"},{"cmd":"rotate","by":90}]}),encoding="utf-8")
        with patch.dict(os.environ,{"P5_NEW_PROTECT_PASSWORD":"new password"}):
            code,record,err=self.run_cli("batch","--job",job)
        self.assertEqual(code,2,(record,err));self.assertTrue(any("protect must be the last step" in p for p in record["problems"]))
    def test_form_skips_external_compression_tool(self):
        from tests.fixtures.p4 import build_fixtures as p4_fixtures
        forms=p4_fixtures(self.root/"p4")["forms"]
        with patch("pdftools.commands.p5_compress.run_tool") as tool:
            code,record,err=self.run_cli("compress",forms,"--include-graphics","--level","high","-o",self.root/"forms-out.pdf")
        self.assertEqual(tool.call_count,0);self.assertEqual(code,0,(record,err));self.assertIn("SKIPPED_GS",record["warnings"])

    def test_file_or_each_argument_forms(self):
        folder=self.root/"inputs";folder.mkdir()
        for command in ("compress","ocr"):
            code,record,err=self.run_cli(command,"--each",folder)
            self.assertNotEqual(code,2,(command,record,err))
            code,record,err=self.run_cli(command)
            self.assertEqual(code,2,(command,record,err));self.assertEqual(record["code"],"BAD_ARG")
            job=self.root/f"{command}-each.json"
            job.write_text(json.dumps({"version":1,"command":command,"each":str(folder)}),encoding="utf-8")
            code,record,err=self.run_cli("--json",command,"--job",job)
            self.assertNotEqual(code,2,(command,record,err))

    def test_compress_no_gain_uses_decrypted_size_and_warning_verdict_is_preserved(self):
        # The encrypted input is larger than its decrypted candidate baseline; a no-gain record still
        # reports the actual published input size, and warning merges never rewrite unchecked verdicts.
        encrypted=self.root/"encrypted-small.pdf"
        with pymupdf.open(self.fx["small"]) as doc:
            doc.save(encrypted,encryption=pymupdf.PDF_ENCRYPT_AES_256,user_pw="input-secret",owner_pw="owner-secret")
        password=self.root/"input-pw.txt";password.write_text("input-secret\n",encoding="utf-8")
        code,record,err=self.run_cli("compress",encrypted,"--password-file",password,"--level","lossless")
        self.assertEqual(code,0,(record,err));self.assertEqual(record["before"],encrypted.stat().st_size)
        self.assertEqual(record["after"],encrypted.stat().st_size);self.assertEqual(record["bytes"],encrypted.stat().st_size)
        from pdftools.commands.p5_compress import _merge_warnings
        merged=_merge_warnings({"verdict":"unchecked","warnings":["SKIPPED_GS"]},["SKIPPED_GS","TARGET_MISSED"])
        self.assertEqual(merged["warnings"],["SKIPPED_GS","TARGET_MISSED"]);self.assertEqual(merged["verdict"],"unchecked")

    def test_ocr_dry_run_never_opens_destination(self):
        from tests.tooling import require_tool
        require_tool(self,"ocrmypdf","dry-run still requires the OCR command setup")
        for existing in (False,True):
            target=self.root/("existing-dry.pdf" if existing else "new-dry.pdf")
            if existing:target.write_bytes(self.fx["small"].read_bytes())
            code,record,err=self.run_cli("--dry-run","ocr",self.fx["scan"],"-o",target)
            self.assertEqual(code,0,(record,err));self.assertTrue(record["provisional"])
            self.assertEqual(target.exists(),existing)

    def test_ocr_sidecar_ranges_and_timeout_empty_reporting(self):
        from pdftools.commands.p5_ocr import _sidecar_pages
        # ocrmypdf 17.13's real sidecar: one entry per run of skipped pages, then one entry per OCRed page
        page_text,skipped=_sidecar_pages("[OCR skipped on page(s) 1-2]\fSCANNED WORDS HERE\n")
        self.assertEqual((page_text,skipped),({2:"SCANNED WORDS HERE"},{0,1}))
        page_text,skipped=_sidecar_pages("page one\f[OCR skipped on page(s) 2-3]\fpage four\f[OCR skipped on page(s) 5]\f")
        self.assertEqual((page_text,skipped),({0:"page one",3:"page four"},{1,2,4}))

    def test_scanned_dpi_floor_follows_a_coarse_source(self):
        from pdftools.commands.p5_compress import _scanned_dpi_ok,_scanned_min_dpi
        doc=pymupdf.open();page=doc.new_page(width=144,height=144)  # 2 x 2 inches
        page.insert_image(page.rect,pixmap=pymupdf.Pixmap(pymupdf.csRGB,pymupdf.IRect(0,0,200,200),False))
        self.assertAlmostEqual(_scanned_min_dpi(doc),100,delta=0.5)
        self.assertFalse(_scanned_dpi_ok(doc,150-1))  # a 100 dpi candidate fails the plain floor
        self.assertTrue(_scanned_dpi_ok(doc,min(150,100)-1))  # but passes against a 100 dpi source

    def test_ocr_timeout_is_reported_as_timeout(self):
        import clierr
        with patch("pdftools.commands.p5_ocr._tool",return_value="ocrmypdf.exe"), \
             patch("pdftools.commands.p5_ocr.run_tool",side_effect=clierr.Fail("TIMEOUT","ocrmypdf timed out","shrink input")):
            code,record,err=self.run_cli("ocr",self.fx["scan"],"-o",self.root/"timeout.pdf")
        self.assertEqual(code,1,(record,err));self.assertEqual(record["code"],"TIMEOUT")

    def test_batch_shorthand_and_json_job_return_same_pages(self):
        shorthand=self.root/"shorthand.pdf"
        code,short,err=self.run_cli("batch",self.fx["small"],"rotate --by 90","-o",shorthand)
        self.assertEqual(code,0,(short,err))
        job=self.root/"same-pages.json"
        job.write_text(json.dumps({"version":1,"command":"batch","input":str(self.fx["small"]),
            "output":str(self.root/"job.pdf"),"steps":[{"cmd":"rotate","by":90}]}),encoding="utf-8")
        code,record,err=self.run_cli("batch","--job",job)
        self.assertEqual(code,0,(record,err));self.assertEqual(record["pages"],short["pages"])
        self.assertEqual(pymupdf.open(record["path"])[0].rotation,pymupdf.open(short["path"])[0].rotation)

    def test_batch_preflight_types_report_modes_and_job_relative_insert(self):
        jobdir=self.root/"another folder";jobdir.mkdir()
        (jobdir/"base.pdf").write_bytes(self.fx["small"].read_bytes())
        (jobdir/"insert.pdf").write_bytes(self.fx["scan"].read_bytes())
        job=jobdir/"job.json"
        job.write_text(json.dumps({"version":1,"command":"batch","input":"base.pdf","steps":[
            {"cmd":"rotate","by":90},{"cmd":"insert","other":"insert.pdf","pages":"1","after":0}]}),encoding="utf-8")
        code,record,err=self.run_cli("--json","batch","--job",job)
        self.assertEqual(code,0,(record,err));self.assertEqual(record["pages"],2)
        bad=jobdir/"bad.json";bad.write_text(json.dumps({"version":1,"command":"batch","input":"base.pdf","steps":[
            {"cmd":"rotate","by":90},{"cmd":"compress","level":"invalid"}]}),encoding="utf-8")
        before=set(self.root.rglob("*.pdf"));code,record,err=self.run_cli("--json","batch","--job",bad)
        self.assertEqual(code,2,(record,err));self.assertTrue(any("invalid level" in p for p in record["problems"]))
        self.assertEqual(before,set(self.root.rglob("*.pdf")))
        for step in ({"cmd":"meta"},{"cmd":"meta","to":"out.json"},{"cmd":"bookmarks","list":True}):
            bad.write_text(json.dumps({"version":1,"command":"batch","input":"base.pdf","steps":[step]}),encoding="utf-8")
            code,record,err=self.run_cli("--json","batch","--job",bad)
            self.assertEqual(code,2,(step,record,err))

    def test_batch_unchecked_publication_and_no_gain_mid_pipeline(self):
        from types import SimpleNamespace
        from pdftools.commands import p5_batch
        source=self.fx["small"];outdir=self.root/"not-created"
        fake=SimpleNamespace(name="rotate",suffix="_rotated",run=None)
        def run(args,context):
            produced=args.output.with_name(args.output.stem+"_UNCHECKED"+args.output.suffix)
            produced.parent.mkdir(parents=True,exist_ok=True);produced.write_bytes(source.read_bytes())
            return {"path":str(produced),"pages":1,"bytes":produced.stat().st_size,"verdict":"unchecked","warnings":[]}
        fake.run=run; batch_spec=SimpleNamespace(name="batch")
        args=type("Args",(),dict(file=source,pipeline=None,steps=None,each=None,output=outdir/"final.pdf",dry_run=False,
            password_file=None,password_env=None,owner_password_file=None,owner_password_env=None,ignore_restrictions=False,
            break_signature=False,verbose=False,job_base=self.root))()
        context=cli.Context();parser=__import__("argparse").ArgumentParser(add_help=False);parser.add_argument("--by",type=int)
        step=("rotate",{"by":90},parser,{a.dest:a for a in parser._actions})
        with patch("pdftools.commands.p5_batch.discover",return_value={"batch":batch_spec,"rotate":fake}):
            early=p5_batch._run_file(source,args,context,[step])
        self.assertTrue(outdir.exists());self.assertTrue(early["path"].endswith("_UNCHECKED.pdf"));self.assertEqual(context.exit_code,3)
        self.assertFalse((outdir/"final_UNCHECKED_2.pdf").exists())
        args.output=outdir/"last.pdf";context=cli.Context()
        with patch("pdftools.commands.p5_batch.discover",return_value={"batch":batch_spec,"rotate":fake}):
            last=p5_batch._run_file(source,args,context,[step])
        self.assertTrue(last["path"].endswith("_UNCHECKED.pdf"));self.assertFalse((outdir/"last_UNCHECKED_2.pdf").exists())
        code,record,err=self.run_cli("batch",source,"compress --level lossless | rotate --by 90","-o",self.root/"no-gain-mid.pdf")
        self.assertEqual(code,0,(record,err));self.assertEqual(pymupdf.open(record["path"])[0].rotation,90)

    def test_batch_unchecked_exit_code_for_early_and_last_steps(self):
        from types import SimpleNamespace
        from pdftools.commands import p5_batch
        source=self.fx["small"]
        class FakeStep:
            def __init__(self,name,suffix):self.name=name;self.suffix=suffix
            def add_arguments(self,parser):
                if self.name=="rotate":
                    parser.add_argument("--by",type=int,default=90);parser.add_argument("--pages",default="1-")
            def run(self,args,context):
                produced=args.output.with_name(args.output.stem+"_UNCHECKED"+args.output.suffix)
                produced.parent.mkdir(parents=True,exist_ok=True);produced.write_bytes(source.read_bytes())
                return {"path":str(produced),"pages":1,"bytes":produced.stat().st_size,"verdict":"unchecked","warnings":[]}
        specs={"batch":SimpleNamespace(name="batch"),"rotate":FakeStep("rotate","_rotated"),
               "gray":FakeStep("gray","_gray")}
        job=self.root/"unchecked.json"
        for number,steps in enumerate(([{"cmd":"rotate","by":90},{"cmd":"gray"}],
                                       [{"cmd":"rotate","by":90}])):
            job.write_text(json.dumps({"version":1,"command":"batch","input":str(source),
                "output":str(self.root/f"unchecked-{number}.pdf"),"steps":steps}),encoding="utf-8")
            with patch("pdftools.commands.p5_batch.discover",return_value=specs):
                code,record,err=self.run_cli("batch","--job",job)
            self.assertEqual(code,3,(record,err));self.assertEqual(record["verdict"],"unchecked")
            self.assertTrue(record["path"].endswith("_UNCHECKED.pdf"))

    def test_signature_gate_and_ocr_dry_run_estimate_contract(self):
        signed=self.fx["small"]
        with patch("pdftools.pipeline._signed",return_value=True):
            for command in ("compress","ocr"):
                code,record,err=self.run_cli(command,signed)
                self.assertEqual(code,1,(command,record,err));self.assertEqual(record["code"],"SIGNED")

    def test_ocr_disk_budget_is_checked_before_tool_launch(self):
        size=self.fx["scan"].stat().st_size
        from collections import namedtuple
        Usage=namedtuple("Usage","total used free")
        with patch("pdftools.commands.p5_ocr.io.shutil.disk_usage",return_value=Usage(100*size,85*size,15*size)), \
             patch("pdftools.commands.p5_ocr.run_tool") as runner:
            code,record,err=self.run_cli("ocr",self.fx["scan"],"-o",self.root/"too-big.pdf")
        self.assertEqual(code,1,(record,err));self.assertEqual(record["code"],"TOO_BIG");runner.assert_not_called()

    def test_compress_estimate_is_announced_for_large_input(self):
        source=self.root/"12-pages.pdf";doc=pymupdf.open()
        for number in range(12):doc.new_page().insert_text((40,50),f"page {number+1} estimate")
        doc.save(source);doc.close()
        from pdftools.pipeline import announce_estimate
        with patch("pdftools.commands.p5_compress.announce_estimate",side_effect=lambda pages,spp:announce_estimate(pages,2)) as estimate:
            code,record,err=self.run_cli("compress",source,"--level","lossless","-o",self.root/"estimate.pdf")
        self.assertEqual(code,0,(record,err));estimate.assert_called_once();self.assertIn("estimate: ~24 s for 12 pages",err)

    def test_secret_redaction_with_ghostscript_and_failed_encrypted_run(self):
        from tests.tooling import require_tool
        require_tool(self,"ghostscript","high graphics compression exercises Ghostscript")
        secret="q'\"\\\u2603\nline"
        encrypted=self.root/"secret.pdf"
        with pymupdf.open(self.fx["photo"]) as doc:
            doc.save(encrypted,encryption=pymupdf.PDF_ENCRYPT_AES_256,user_pw=secret,owner_pw="owner secret")
        secret_file=self.root/"complex-password.txt";secret_file.write_text(secret+"\n",encoding="utf-8")
        from pdftools import tools
        with patch("pdftools.commands.p5_compress.run_tool",wraps=tools.run_tool) as runner:
            code,record,err=self.run_cli("compress",encrypted,"--password-file",secret_file,"--include-graphics","--level","high","-o",self.root/"secret-compressed.pdf")
        self.assertEqual(code,0,(record,err))
        bad=self.root/"bad-password.txt";bad.write_text("incorrect\n",encoding="utf-8")
        fail_code,failed,fail_err=self.run_cli("compress",encrypted,"--password-file",bad,"--include-graphics","--level","high")
        self.assertEqual(fail_code,1,(failed,fail_err))
        data=record.__repr__()+err+failed.__repr__()+fail_err
        for folder in (self.root/"logs",ROOT/"reports"):
            if folder.exists():
                data+="".join(path.read_text(encoding="utf-8",errors="replace") for path in folder.rglob("*") if path.is_file())
        for call in runner.call_args_list:
            data+=repr(call.args)+repr(call.kwargs)
        self.assertNotIn(secret,data)
        self.assertTrue(any("-sDEVICE=pdfwrite" in str(call.args[0]) for call in runner.call_args_list))

    def test_scan_high_compression_preserves_150_dpi(self):
        from tests.tooling import require_tool
        require_tool(self,"ocrmypdf","scanned compression uses OCRmyPDF")
        code,record,err=self.run_cli("compress",self.fx["scan"],"--level","high","-o",self.root/"scan-high.pdf")
        self.assertEqual(code,0,(record,err))
        with pymupdf.open(record["path"]) as doc:
            self.assertTrue(__import__("pdftools.commands.p5_compress",fromlist=["_scanned_dpi_ok"])._scanned_dpi_ok(doc))

    def test_batch_protect_password_is_step_local_and_encrypted_input_round_trips(self):
        from tests.tooling import require_tool
        require_tool(self,"ocrmypdf","batch OCR integration exercises OCRmyPDF")
        require_tool(self,"tesseract","batch OCR integration exercises Tesseract")
        jobdir=self.root/"protect job";jobdir.mkdir()
        encrypted=jobdir/"input.pdf"
        with pymupdf.open(self.fx["scan"]) as doc:
            doc.save(encrypted,encryption=pymupdf.PDF_ENCRYPT_AES_256,user_pw="input batch password",owner_pw="owner input")
        (jobdir/"input-password.txt").write_text("input batch password\n",encoding="utf-8")
        job=jobdir/"job.json"
        import subprocess
        real_popen=subprocess.Popen
        for key in ("password_env","new_password_env"):  # the batch form and protect's own key
            with self.subTest(key=key):
                job.write_text(json.dumps({"version":1,"command":"batch","input":"input.pdf","password_file":"input-password.txt",
                    "steps":[{"cmd":"ocr","lang":"eng+ind"},{"cmd":"protect",key:"P5_NEW_PROTECT_PASSWORD"}]}),encoding="utf-8")
                child_envs=[]
                def capture(*args,**kwargs):
                    child_envs.append(dict(kwargs.get("env",{})))
                    return real_popen(*args,**kwargs)
                before=hashlib.sha256(encrypted.read_bytes()).hexdigest()
                with patch.dict(os.environ,{"P5_NEW_PROTECT_PASSWORD":"new secret password"}), \
                     patch("pdftools.tools.subprocess.Popen",side_effect=capture):
                    code,record,err=self.run_cli("--json","batch","--job",job)
                self.assertEqual(code,0,(record,err));self.assertEqual(hashlib.sha256(encrypted.read_bytes()).hexdigest(),before)
                self.assertTrue(child_envs);self.assertTrue(all("P5_NEW_PROTECT_PASSWORD" not in env for env in child_envs))
                with pikepdf.open(record["path"],password="new secret password") as output:self.assertTrue(output.is_encrypted)
                with self.assertRaises(pikepdf.PasswordError):pikepdf.open(record["path"],password="input batch password")


if __name__=="__main__":unittest.main()
