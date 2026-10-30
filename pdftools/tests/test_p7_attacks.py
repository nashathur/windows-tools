import contextlib
import io
import json
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import pikepdf
import pymupdf

ROOT=Path(__file__).resolve().parents[1]
import sys
sys.path.insert(0,str(ROOT));sys.path.insert(0,str(ROOT.parent/"common"))
from pdftools import cli
from pdftools.check import OperationResult
from pdftools.pipeline import write_pdf
from pdftools.registry import get
from tests.fixtures.p7 import build_fixtures


class Phase7AttacksTest(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name);self.fx=build_fixtures(self.root/"fx")
    def tearDown(self):self.tmp.cleanup()

    def _args(self,source,output,keep=False):
        return type("Args",(),{"file":source,"output":output,"keep_failed":keep,"dry_run":False,
            "ignore_restrictions":False,"break_signature":False,"verbose":False})()

    def _run(self,command,source,writer,operation=None,keep=False):
        output=self.root/(command+"-candidate.pdf");context=cli.Context(None,True);context.secret_store=None
        with patch("clierr.LOG_ROOT",self.root/"logs"):
            record=write_pdf(source,get(command),self._args(source,output,keep),context,writer,
                             security=True,operation=operation)
        return context.exit_code,record,output

    def test_sanitize_policy_rejects_next_af_word_and_external_tile_attacks(self):
        cases=[("javascript","js"),("javascript_attachment","af"),("base","word"),("base","tile")]
        for key,kind in cases:
            with self.subTest(kind=kind):
                source=self.fx[key]
                def writer(doc,target,_kind=kind):
                    if _kind in ("js","af"):
                        shutil.copyfile(source,target)
                    elif _kind=="word":
                        page=doc[0];page.add_redact_annot(pymupdf.Rect(30,40,130,70),fill=(1,1,1));page.apply_redactions();doc.save(target)
                    else:
                        doc[0].draw_rect(pymupdf.Rect(200,200,235,235),fill=(0,0,0),overlay=True);doc.save(target)
                code,record,out=self._run("sanitize",source,writer)
                self.assertEqual(code,3,record);self.assertEqual(record["verdict"],"failed")
                self.assertIsNone(record["path"]);self.assertFalse(out.exists())
        code,record,out=self._run("sanitize",self.fx["javascript"],
            lambda doc,target:shutil.copyfile(self.fx["javascript"],target),keep=True)
        self.assertEqual(code,3);self.assertTrue(Path(record["path"]).name.endswith("_FAILED_UNSAFE.pdf"))
        self.assertTrue(Path(record["path"]).exists())

    def test_redact_policy_rejects_overlay_metadata_occurrence_and_image_attacks(self):
        source=self.fx["base"]
        with pymupdf.open(source) as doc:
            words=doc[0].get_text("words");chosen=pymupdf.Rect(words[3][:4])
        op=OperationResult(measurements=[{"boxes":[{"page":1,"box":tuple(chosen)}],"all_occurrences":True}])
        op._targets=[("Ada Lovelace",False)]
        for kind in ("overlay","metadata","image"):
            with self.subTest(kind=kind):
                local=self.fx["chart"] if kind=="image" else source
                box=chosen if kind!="image" else pymupdf.Rect(25,25,70,70)
                attack=OperationResult(measurements=[{"boxes":[{"page":1,"box":tuple(box)}],"all_occurrences":True}])
                attack._targets=[("Ada Lovelace",False)] if kind!="image" else [("thin",False)]
                def writer(doc,target,_kind=kind):
                    if _kind=="overlay":doc[0].draw_rect(box,fill=(0,0,0),overlay=True);doc.save(target)
                    elif _kind=="metadata":
                        doc.save(target)
                        with pikepdf.open(target) as pdf:pdf.docinfo["/Title"]="Ada Lovelace";pdf.save(target.with_suffix(".stage.pdf"))
                        target.with_suffix(".stage.pdf").replace(target)
                    else:
                        doc[0].draw_rect(box,fill=(0,0,0),overlay=True);doc.save(target)
                code,record,out=self._run("redact",local,writer,attack)
                self.assertEqual(code,3,record);self.assertEqual(record["verdict"],"failed")
                self.assertIsNone(record["path"]);self.assertFalse(out.exists())
                if record.get("log"):
                    self.assertNotIn("Ada Lovelace",Path(record["log"]).read_text())
        # Removing a non-chosen occurrence changes words outside the chosen box.
        with pymupdf.open(source) as doc:
            match=doc[0].get_text("words");first=pymupdf.Rect(match[0][:4]);second=pymupdf.Rect(match[3][:4])
        chosen_op=OperationResult(measurements=[{"boxes":[{"page":1,"box":tuple(second)}],"all_occurrences":False}])
        chosen_op._targets=[("Ada Lovelace",False)]
        def remove_both(doc,target):
            doc[0].add_redact_annot(first,fill=(0,0,0));doc[0].add_redact_annot(second,fill=(0,0,0));doc[0].apply_redactions();doc.save(target)
        code,record,out=self._run("redact",source,remove_both,chosen_op)
        self.assertEqual(code,3,record);self.assertIsNone(record["path"]);self.assertFalse(out.exists())

    def test_pypdf_only_extractor_attack_fails(self):
        source=self.fx["base"]
        with pymupdf.open(source) as doc:box=pymupdf.Rect(doc[0].get_text("words")[0][:4])
        op=OperationResult(measurements=[{"boxes":[{"page":1,"box":tuple(box)}],"all_occurrences":False}])
        op._targets=[("Ada Lovelace",False)]
        def writer(doc,target):doc.save(target)
        original=pymupdf.Page.get_text
        def no_text(page,*args,**kwargs):return "" if page.number==0 else original(page,*args,**kwargs)
        with patch.object(pymupdf.Page,"get_text",no_text):
            code,record,out=self._run("redact",source,writer,op)
        self.assertEqual(code,3,record);self.assertIn("PYPDF_TEXT_REMAINS",[f["code"] for f in record["findings"]])
        self.assertIsNone(record["path"]);self.assertFalse(out.exists())

    def test_rect_overlay_text_and_translated_cm_attack_fail(self):
        source=self.fx["base"]
        with pymupdf.open(source) as doc: box=pymupdf.Rect(doc[0].get_text("words")[0][:4])
        op=OperationResult(measurements=[{"boxes":[{"page":1,"box":tuple(box)}],"all_occurrences":False}])
        op._targets=[]
        def overlay(doc,target):doc[0].draw_rect(box,fill=(0,0,0),overlay=True);doc.save(target)
        code,record,out=self._run("redact",source,overlay,op)
        self.assertEqual(code,3,record);self.assertEqual(record["verdict"],"failed");self.assertFalse(out.exists())

        shifted=self.root/"translated.pdf"
        with pikepdf.open(source) as pdf:
            stream=pdf.make_stream(b"BT /helv 12 Tf 1 0 0 1 0 0 cm 1 0 0 1 35 350 cm (SECRET) Tj ET")
            page=pdf.pages[0].obj;page["/Contents"]=pikepdf.Array([page["/Contents"],stream]);pdf.save(shifted)
        shifted_box=pymupdf.Rect(35,35,100,60)
        shifted_op=OperationResult(measurements=[{"boxes":[{"page":1,"box":tuple(shifted_box)}],"all_occurrences":False}])
        shifted_op._targets=[]
        code,record,out=self._run("redact",shifted,lambda doc,target:doc.save(target),shifted_op)
        self.assertEqual(code,3,record);self.assertIn("PYPDF_TEXT_REMAINS",[row["code"] for row in record["findings"]])

    def test_large_box_around_small_scanned_name_must_be_filled(self):
        # A box mostly over blank paper is nearly uniform: only the fill colour (render) and an exactly
        # rewritten image area count as redacted.
        from PIL import Image, ImageDraw
        image=Image.new("RGB",(600,800),"white");ImageDraw.Draw(image).rectangle((280,400,320,410),fill="black")
        buffer=io.BytesIO();image.save(buffer,format="PNG")
        scan=self.root/"small-name-scan.pdf"
        with pymupdf.open() as doc:
            page=doc.new_page(width=600,height=800);page.insert_image(page.rect,stream=buffer.getvalue());doc.save(scan)
        box=pymupdf.Rect(100,300,500,500)
        op=OperationResult(measurements=[{"boxes":[{"page":1,"box":tuple(box)}],"all_occurrences":False}]);op._targets=[]
        code,record,out=self._run("redact",scan,lambda doc,target:doc.save(target),op)
        self.assertEqual(code,3,record);self.assertIn("IMAGE_REMAINS",[row["code"] for row in record["findings"]])
        op=OperationResult(measurements=[{"boxes":[{"page":1,"box":tuple(box)}],"all_occurrences":False}]);op._targets=[]
        def painted(doc,target):doc[0].draw_rect(box,fill=(0,0,0),overlay=True);doc.save(target)
        code,record,out=self._run("redact",scan,painted,op)
        self.assertEqual(code,3,record);self.assertIn("IMAGE_SOURCE_REMAINS",[row["code"] for row in record["findings"]])
        self.assertFalse(out.exists())

    def test_sanitize_stream_dictionary_javascript_and_af_fail(self):
        for key in ("js","af"):
            with self.subTest(kind=key):
                source=self.root/(key+"-form.pdf")
                with pikepdf.open(self.fx["base"]) as pdf:
                    form=pdf.make_stream(b"",Type=pikepdf.Name("/XObject"),Subtype=pikepdf.Name("/Form"),
                        BBox=pikepdf.Array([0,0,10,10]))
                    if key=="js":form["/OpenAction"]=pikepdf.Dictionary({"/S":pikepdf.Name("/JavaScript"),"/JS":pikepdf.String("secret")})
                    else:form["/AF"]=pikepdf.Array([pdf.make_indirect(pikepdf.Dictionary({"/Type":pikepdf.Name("/Filespec"),"/F":pikepdf.String("only-form")}))])
                    pdf.pages[0].obj["/Resources"]=pikepdf.Dictionary({"/XObject":pikepdf.Dictionary({"/F":form})})
                    pdf.save(source)
                code,record,out=self._run("sanitize",source,lambda doc,target:shutil.copyfile(source,target))
                self.assertEqual(code,3,record);self.assertEqual(record["verdict"],"failed");self.assertFalse(out.exists())

    def test_sanitize_semantic_widget_change_fails(self):
        source=self.fx["widgets"]
        def writer(doc,target):
            doc.save(target)
            with pikepdf.open(target) as pdf:
                widget=next(a for a in pdf.pages[0].obj["/Annots"] if str(a.get("/Subtype",""))=="/Widget")
                widget["/V"]=pikepdf.String("changed value");pdf.save(target.with_suffix(".stage.pdf"))
            target.with_suffix(".stage.pdf").replace(target)
        code,record,out=self._run("sanitize",source,writer)
        self.assertEqual(code,3,record);self.assertIn("FEATURE_CHANGED",[f["code"] for f in record["findings"]])

    def test_unreadable_xmp_fails_independent_readback_closed(self):
        source=self.root/"bad-xmp.pdf"
        from pypdf import PdfWriter
        from pypdf.generic import DecodedStreamObject,NameObject
        writer=PdfWriter(clone_from=self.fx["base"])
        xmp=b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description>target</rdf:Description></rdf:RDF></x:xmpmeta>'
        packet=DecodedStreamObject();packet.set_data(xmp)
        packet[NameObject("/Type")]=NameObject("/Metadata");packet[NameObject("/Subtype")]=NameObject("/XML")
        writer._root_object[NameObject("/Metadata")]=writer._add_object(packet)
        with source.open("wb") as stream: writer.write(stream)
        data=source.read_bytes();self.assertIn(b"</rdf:Description>",data)
        source.write_bytes(data.replace(b"</rdf:Description>",b"</rdf:DescriptioX>"))
        op=OperationResult(measurements=[{"boxes":[],"all_occurrences":True}]);op._targets=[("target",False)]
        code,record,out=self._run("redact",source,lambda doc,target:shutil.copyfile(source,target),op)
        self.assertEqual(code,3,record);self.assertEqual(record["verdict"],"failed")
        self.assertIn("PYPDF_STRUCTURE_TARGET_REMAINS",[row["code"] for row in record["findings"]])

    def test_misplaced_redaction_box_on_rotated_crop_fails(self):
        source=self.fx["rotated_target"]
        box=pymupdf.Rect(5000,5000,5100,5100)
        op=OperationResult(measurements=[{"boxes":[{"page":1,"box":tuple(box)}],"all_occurrences":False}])
        op._targets=[]
        code,record,out=self._run("redact",source,lambda doc,target:doc.save(target),op)
        self.assertEqual(code,3,record);self.assertEqual(record["verdict"],"failed");self.assertFalse(out.exists())

    def test_representation_warnings_preserve_failed_verdict_and_each_exit(self):
        import pdftools.commands.p7_redact as redact_module
        import pdftools.commands.p7_sanitize as sanitize_module
        for command,module,source,extra in (("redact",redact_module,self.fx["scanned"],["--text","missing"]),
                ("sanitize",sanitize_module,self.fx["hidden_off"],[])):
            for verdict in ("failed","unchecked"):
              for keep in ([],["--keep-failed"]):
                with self.subTest(command=command,verdict=verdict,keep=bool(keep)):
                    def fake_write(source_path,spec,args,context,writer,**kwargs):
                        candidate=self.root/(command+"-candidate.pdf")
                        with pymupdf.open(source_path) as doc: writer(doc,candidate)
                        return {"type":"result","command":command,"path":None,"kind":"pdf","pages":1,
                                "bytes":0,"verdict":verdict,"warnings":[]}
                    out,err=io.StringIO(),io.StringIO()
                    with patch.object(module,"write_pdf",fake_write),patch("clierr.LOG_ROOT",self.root/"logs"),contextlib.redirect_stdout(out),contextlib.redirect_stderr(err):
                        code=cli.main(["--json",command,str(source),*extra,*keep])
                    row=json.loads(out.getvalue().splitlines()[0])
                    self.assertEqual(row["verdict"],verdict);self.assertNotIn("UNVERIFIED_REPRESENTATION",row["warnings"])
                    self.assertNotIn("HIDDEN_CONTENT",row["warnings"])
            for i in (1,2):
                (self.root/f"{command}-glob-{i}.pdf").write_bytes(source.read_bytes())
            def failed_write(source_path,spec,args,context,writer,**kwargs):
                candidate=self.root/(command+"-each-candidate.pdf")
                with pymupdf.open(source_path) as doc: writer(doc,candidate)
                return {"type":"result","command":command,"path":None,"kind":"pdf","pages":1,
                        "bytes":0,"verdict":"failed","warnings":[]}
            out,err=io.StringIO(),io.StringIO()
            with patch.object(module,"write_pdf",failed_write),patch("clierr.LOG_ROOT",self.root/"logs"),contextlib.redirect_stdout(out),contextlib.redirect_stderr(err):
                code=cli.main(["--json",command,str(self.root/f"{command}-glob-*.pdf"),*extra,"--each"])
            rows=[json.loads(line) for line in out.getvalue().splitlines() if line.strip()]
            self.assertEqual(code,4,(rows,err.getvalue()));self.assertEqual(rows[-1]["failed"],2)

    def test_shared_image_is_repointed_page_locally(self):
        source=self.fx["chart"]
        output=self.root/"chart-redacted.pdf"
        code,record,err=self._cli_redact(source,"--rect","1,25,25,50,50","-o",output)
        self.assertEqual(code,0,(record,err))
        with pymupdf.open(source) as before,pymupdf.open(record["path"]) as after:
            a=before[1].get_pixmap(matrix=pymupdf.Matrix(1,1),alpha=False).samples
            b=after[1].get_pixmap(matrix=pymupdf.Matrix(1,1),alpha=False).samples
            self.assertEqual(a,b)
            source_xref=before[1].get_image_info(xrefs=True)[0]["xref"]
            other_xref=after[1].get_image_info(xrefs=True)[0]["xref"]
            self.assertEqual(pymupdf.Pixmap(before,source_xref).samples,pymupdf.Pixmap(after,other_xref).samples)
            self.assertNotEqual(after[0].get_image_info(xrefs=True)[0]["xref"],other_xref)

    def _cli_redact(self,source,*args):
        import json
        out,err=io.StringIO(),io.StringIO()
        with patch("clierr.LOG_ROOT",self.root/"logs"),contextlib.redirect_stdout(out),contextlib.redirect_stderr(err):
            code=cli.main(["--json","redact",str(source),*map(str,args)])
        rows=[json.loads(row) for row in out.getvalue().splitlines() if row.strip()]
        return code,rows[0] if len(rows)==1 else rows,err.getvalue()


if __name__=="__main__":unittest.main()
