import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
import sys
from unittest.mock import patch

import pikepdf
import pymupdf
from pypdf import PdfReader

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT));sys.path.insert(0,str(ROOT.parent/"common"))
from pdftools import cli
from pdftools.registry import get
from pdftools.check import OperationResult
from pdftools.textmatch import find_matches
from pdftools.pipeline import write_pdf
from pdftools import coords
from tests.fixtures.p7 import build_fixtures


class Phase7CommandsTest(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name);self.fx=build_fixtures(self.root/"fx")

    def tearDown(self):self.tmp.cleanup()

    def run_cli(self,*args):
        out,err=io.StringIO(),io.StringIO()
        with patch("clierr.LOG_ROOT",self.root/"logs"),contextlib.redirect_stdout(out),contextlib.redirect_stderr(err):
            code=cli.main(["--json",*map(str,args)])
        records=[json.loads(row) for row in out.getvalue().splitlines() if row.strip()]
        return code,(records[0] if len(records)==1 else records),err.getvalue()

    def test_sanitize_removes_javascript_and_metadata(self):
        code,record,err=self.run_cli("sanitize",self.fx["active"],"-o",self.root/"safe.pdf")
        self.assertEqual(code,0,(record,err))
        with pikepdf.open(record["path"]) as pdf:
            self.assertEqual(str(pdf.Root["/OpenAction"].get("/S")),"/GoTo")
            self.assertFalse(any(str(item.get("/S",""))=="/JavaScript" for item in pdf.Root["/OpenAction"].get("/Next",[])))

    def test_sanitize_keep_meta_and_bad_keep(self):
        code,record,err=self.run_cli("sanitize",self.fx["active"],"--keep","meta","-o",self.root/"keep.pdf")
        self.assertEqual(code,0,(record,err))
        with pikepdf.open(record["path"]) as pdf:self.assertIn("/Title",pdf.docinfo)
        code,record,err=self.run_cli("sanitize",self.fx["active"],"--keep","unknown")
        self.assertEqual(code,2);self.assertEqual(record["code"],"BAD_ARG")

    def test_sanitize_attachment_region_coordinate_grid(self):
        for rotation in (0,90,180,270):
            for unit in (1,2):
                with self.subTest(rotation=rotation,unit=unit):
                    source=self.root/f"sanitize-grid-{rotation}-{unit}.pdf"
                    doc=pymupdf.open();page=doc.new_page(width=500,height=400)
                    page.insert_text((70,100),"Unaffected page content")
                    page.add_file_annot((250,250),b"attachment","fixture.txt")
                    doc.xref_set_key(page.xref,"CropBox","[20 30 420 330]")
                    if unit!=1:doc.xref_set_key(page.xref,"UserUnit",str(unit))
                    page.set_rotation(rotation);doc.save(source);doc.close()
                    output=self.root/f"sanitize-grid-{rotation}-{unit}-out.pdf"
                    code,record,err=self.run_cli("sanitize",source,"-o",output)
                    self.assertEqual(code,0,(rotation,unit,record,err))
                    with pymupdf.open(record["path"]) as result:
                        self.assertFalse(any(annot.type[0]==pymupdf.PDF_ANNOT_FILE_ATTACHMENT
                                             for annot in result[0].annots() or []))

    def test_redact_literal_preserves_other_occurrence(self):
        code,record,err=self.run_cli("redact",self.fx["base"],"--text","Ada Lovelace","--occurrence","2","-o",self.root/"redacted.pdf")
        self.assertEqual(code,0,(record,err))
        with pymupdf.open(record["path"]) as doc:
            self.assertIn("Ada Lovelace",doc[0].get_text());self.assertEqual(doc[0].get_text().count("Ada"),1)
        self.assertEqual(PdfReader(record["path"]).pages[0].extract_text().count("Ada"),1)

    def test_redact_coordinate_grid_for_text_rect_and_ocr(self):
        from pdftools.commands.p7_redact import _norm
        for rotation in (0,90,180,270):
            for unit in (1,2):
                with self.subTest(rotation=rotation,unit=unit):
                    source=self.root/f"grid-{rotation}-{unit}.pdf"
                    doc=pymupdf.open();page=doc.new_page(width=500,height=400)
                    white=pymupdf.Pixmap(pymupdf.csRGB,pymupdf.IRect(0,0,2,2),False)
                    white.clear_with(255)
                    page.insert_image(page.rect,stream=white.tobytes("png"))
                    page.insert_text((70,100),"SecretName",fontsize=20)
                    doc.xref_set_key(page.xref,"CropBox","[20 30 420 330]")
                    if unit!=1:doc.xref_set_key(page.xref,"UserUnit",str(unit))
                    page.set_rotation(rotation);doc.save(source);doc.close()
                    with pymupdf.open(source) as original:
                        page=original[0]
                        text_box=page.search_for("SecretName")[0]
                        displayed=coords.pymupdf_to_display(page,text_box)
                        mm=tuple(value for value in displayed)
                        full=page.get_pixmap(matrix=pymupdf.Matrix(2,2),alpha=False)
                        self.assertGreater(full.width,0)
                    for mode in ("text","rect","ocr"):
                        output=self.root/f"grid-{rotation}-{unit}-{mode}.pdf"
                        if mode=="text":
                            args=("redact",source,"--text","SecretName","-o",output)
                        elif mode=="rect":
                            pad=2*coords.MM_PER_POINT
                            expanded=(mm[0]-pad,mm[1]-pad,mm[2]+pad,mm[3]+pad)
                            args=("redact",source,"--rect",f"1,{expanded[0]},{expanded[1]},{expanded[2]},{expanded[3]}","-o",output)
                        else:
                            from tests.tooling import require_tool
                            require_tool(self,"tesseract","coordinate-grid OCR redaction")
                            args=("redact",source,"--ocr","--text","SecretName","--max-distance","3","-o",output)
                        code,record,err=self.run_cli(*args)
                        self.assertEqual(code,0,(rotation,unit,mode,record,err))
                        with pymupdf.open(record["path"]) as result:
                            self.assertNotIn("SecretName",_norm(result[0].get_text()))
                            inner=pymupdf.Rect(text_box.x0+1,text_box.y0+1,text_box.x1-1,text_box.y1-1)
                            displayed=coords.to_displayed_points(result[0],inner)
                            pix=result[0].get_pixmap(matrix=pymupdf.Matrix(2,2),alpha=False)
                            sx=pix.width/result[0].rect.width;sy=pix.height/result[0].rect.height
                            x0,x1=max(0,int(displayed.x0*sx)),min(pix.width,int(displayed.x1*sx))
                            y0,y1=max(0,int(displayed.y0*sy)),min(pix.height,int(displayed.y1*sy))
                            total=max(1,(x1-x0)*(y1-y0));samples=pix.samples;black=0
                            for y in range(y0,y1):
                                for x in range(x0,x1):
                                    at=(y*pix.width+x)*pix.n
                                    if max(samples[at:at+3])<8:black+=1
                            self.assertGreaterEqual(black/total,.98,(rotation,unit,mode))
                        self.assertNotIn("SecretName",_norm(PdfReader(record["path"]).pages[0].extract_text() or ""))
                    if rotation==0 and unit==1:
                        shifted=(mm[0]+20,mm[1],mm[2]+20,mm[3])
                        code,record,err=self.run_cli("redact",source,"--rect",
                            f"1,{shifted[0]},{shifted[1]},{shifted[2]},{shifted[3]}","-o",self.root/"shifted.pdf")
                        self.assertNotEqual(code,0,(record,err))

    def test_redact_pending_annotation_requires_explicit_apply(self):
        pending=self.root/"pending.pdf";doc=pymupdf.open(self.fx["base"]);doc[0].add_redact_annot(pymupdf.Rect(30,40,140,70));doc.save(pending);doc.close()
        code,record,err=self.run_cli("redact",pending,"--text","Ada")
        self.assertNotEqual(code,0);self.assertEqual(record["code"],"REDACTIONS_PENDING")

    def test_dry_run_does_not_publish(self):
        output=self.root/"dry.pdf"
        code,record,err=self.run_cli("sanitize",self.fx["active"],"--dry-run","-o",output)
        self.assertEqual(code,0,(record,err));self.assertFalse(output.exists())

    def test_scanned_ocr_fuzzy_names_are_redacted(self):
        from tests.tooling import require_tool
        try: require_tool(self,"tesseract","OCR redaction security test")
        except unittest.SkipTest as exc: self.fail(f"Tesseract is mandatory for this security test: {exc}")
        code,record,err=self.run_cli("redact",self.fx["scanned"],"--ocr","--text","Sutanto","--text","Pramudya","-o",self.root/"scan-redacted.pdf")
        self.assertEqual(code,0,(record,err))
        self.assertTrue(any(m["fuzzy"] and m["distance"] in (1,2) for m in record["matches"]),record)
        self.assertEqual(len(record["matches"]),2,record)
        self.assertTrue(all(m["source"]=="ocr" for m in record["matches"]),record)
        with pymupdf.open(record["path"]) as doc:
            pix=doc[0].get_pixmap(matrix=pymupdf.Matrix(100/72,100/72),alpha=False)
            self.assertLess(sum(1 for y in range(pix.height) for x in range(pix.width) if pix.samples[(y*pix.width+x)*pix.n]<16),pix.width*pix.height)
        with pymupdf.open(self.fx["scanned"]) as original,pymupdf.open(record["path"]) as redacted:
            before=original[0].get_pixmap(matrix=pymupdf.Matrix(100/72,100/72),alpha=False)
            after=redacted[0].get_pixmap(matrix=pymupdf.Matrix(100/72,100/72),alpha=False)
            for y in range(45,100):
                start=(y*before.width+350)*before.n;end=(y*before.width+580)*before.n
                self.assertEqual(before.samples[start:end],after.samples[start:end],"the same-length unrelated name outside the fuzzy bound changed")

    def test_ocr_fuzzy_windows_make_one_box_per_line(self):
        from pdftools.commands.p7_redact import _fuzzy_words
        words=[("Sutanto",(10,10,60,25)),("Pramudya",(10,35,75,50)),
               ("continued",(10,60,80,75))]
        matches=_fuzzy_words(words,"Sutanto Pramudya",2)
        self.assertEqual(len(matches),1)
        self.assertEqual(len(matches[0][4]),2)

    def test_sanitize_attack_leftover_javascript_fails_closed(self):
        context=cli.Context(None,True);context.secret_store=None
        def bad_writer(doc,target):
            with pikepdf.open(self.fx["active"]) as pdf:pdf.save(target)
            return OperationResult()
        with patch("clierr.LOG_ROOT",self.root/"logs"):
            record=write_pdf(self.fx["active"],get("sanitize"),type("Args",(),{"output":self.root/"unsafe.pdf","keep_failed":False,"dry_run":False,"ignore_restrictions":False,"break_signature":False,"file":self.fx["active"]})(),context,bad_writer,security=True)
        self.assertEqual(context.exit_code,3);self.assertEqual(record["verdict"],"failed");self.assertIsNone(record["path"])
        self.assertFalse((self.root/"unsafe.pdf").exists())

    def test_redact_attack_hidden_target_fails_closed(self):
        context=cli.Context(None,True);context.secret_store=None
        with pymupdf.open(self.fx["base"]) as source:
            match=find_matches(source,"Ada Lovelace",occurrence=2)["matches"][0]
            rect=match["quads"][0].rect
        op=OperationResult(measurements=[{"targets":[("Ada Lovelace",False)],"boxes":[{"page":1,"box":tuple(rect)}],"all_occurrences":False}])
        def bad_writer(doc,target):doc.save(target);return op
        args=type("Args",(),{"output":self.root/"unsafe-redacted.pdf","keep_failed":False,"dry_run":False,"ignore_restrictions":False,"break_signature":False,"file":self.fx["base"]})()
        with patch("clierr.LOG_ROOT",self.root/"logs"):
            record=write_pdf(self.fx["base"],get("redact"),args,context,bad_writer,security=True,operation=op)
        self.assertEqual(context.exit_code,3);self.assertEqual(record["verdict"],"failed");self.assertIsNone(record["path"])
        self.assertFalse((self.root/"unsafe-redacted.pdf").exists())

    def test_redact_each_glob_and_resolved_full_document_scope(self):
        for name in ("sample-a.pdf","sample-b.pdf"): (self.root/name).write_bytes(self.fx["base"].read_bytes())
        code,records,err=self.run_cli("redact",self.root/"sample-*.pdf","--text","Ada Lovelace","--each","-o",self.root/"each.pdf")
        self.assertEqual(code,0,(records,err));self.assertEqual(records[-1]["type"],"summary");self.assertEqual(records[-1]["files"],2)
        import pdftools.commands.p7_redact as redact_module
        captured=[]
        def capture(source,spec,args,context,writer,**kwargs):
            candidate=self.root/"captured.pdf"
            with pymupdf.open(source) as doc: captured.append(writer(doc,candidate).measurements[0])
            return {"type":"result","command":"redact","path":str(candidate),"kind":"pdf","pages":1,
                    "bytes":candidate.stat().st_size,"verdict":"ok","warnings":[]}
        with patch.object(redact_module,"write_pdf",capture):
            code,record,err=self.run_cli("redact",self.fx["base"],"--text","Ada Lovelace","--pages","1","-o",self.root/"capture-out.pdf")
        self.assertEqual(code,0,(record,err));self.assertTrue(captured[0]["all_occurrences"])

    def test_redact_ocr_uses_pipeline_temp_folder(self):
        import pdftools.commands.p7_redact as redact_module
        seen=[];real=redact_module._ocr_matches
        def capture(page,lang,context,temp):
            seen.append((Path(temp).resolve(),Path(context.temp).resolve()))
            return real(page,lang,context,temp)
        with patch.object(redact_module,"_ocr_matches",capture):
            code,record,err=self.run_cli("redact",self.fx["scanned"],"--ocr","--text","Sutanto","--max-distance","2","-o",self.root/"temp-ocr.pdf")
        self.assertEqual(code,0,(record,err));self.assertTrue(seen);self.assertTrue(all(a==b for a,b in seen),seen)


if __name__=="__main__":unittest.main()
