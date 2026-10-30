import contextlib
import hashlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import pikepdf
import pymupdf
from pypdf import PdfReader

ROOT=Path(__file__).resolve().parents[1]
import sys
sys.path.insert(0,str(ROOT));sys.path.insert(0,str(ROOT.parent/"common"))
from pdftools import cli
from tests.fixtures.p7 import build_fixtures


def _walk(obj, seen=None):
    seen=set() if seen is None else seen
    gen=getattr(obj,"objgen",(0,0))
    if gen!=(0,0):
        if gen in seen:return
        seen.add(gen)
    yield obj
    if isinstance(obj,(pikepdf.Dictionary,pikepdf.Stream)):
        for value in list(obj.values()):yield from _walk(value,seen)
    elif isinstance(obj,pikepdf.Array):
        for value in list(obj):yield from _walk(value,seen)


class Phase7SecurityTest(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name)
        self.fx=build_fixtures(self.root/"fx")

    def tearDown(self):self.tmp.cleanup()

    def run_cli(self,*args,env=None):
        out,err=io.StringIO(),io.StringIO()
        environ=os.environ.copy();environ.update(env or {})
        with patch.dict(os.environ,environ),patch("clierr.LOG_ROOT",self.root/"logs"),contextlib.redirect_stdout(out),contextlib.redirect_stderr(err):
            code=cli.main(["--json",*map(str,args)])
        records=[json.loads(row) for row in out.getvalue().splitlines() if row.strip()]
        return code,records[0] if len(records)==1 else records,err.getvalue()

    def test_sanitize_javascript_everywhere_and_af_attachments(self):
        nested=self.root/"nested-af.pdf"
        with pikepdf.open(self.fx["javascript_attachment"]) as source:
            spec=source.Root["/AF"][0];page=source.pages[0].obj
            page["/AF"]=pikepdf.Array([spec])
            note=pikepdf.Dictionary({"/Subtype":pikepdf.Name("/FileAttachment"),"/Rect":pikepdf.Array([10,10,20,20]),"/FS":spec,"/AF":pikepdf.Array([spec])})
            page["/Annots"]=pikepdf.Array([note])
            form=source.make_stream(b"",Type=pikepdf.Name("/XObject"),Subtype=pikepdf.Name("/Form"),
                                    BBox=pikepdf.Array([0,0,1,1]),AF=pikepdf.Array([spec]))
            resources=page.get("/Resources",pikepdf.Dictionary());resources["/XObject"]=pikepdf.Dictionary({"/Attached":form});page["/Resources"]=resources
            source.save(nested)
        candidates={"javascript":self.fx["javascript"],"javascript_attachment":nested}
        for key,input_path in candidates.items():
            target=self.root/(key+"-safe.pdf")
            code,record,err=self.run_cli("sanitize",input_path,"-o",target)
            self.assertEqual(code,0,(key,record,err))
            with pikepdf.open(target) as pdf:
                self.assertFalse(any(isinstance(obj,pikepdf.Dictionary) and str(obj.get("/S",""))=="/JavaScript" for obj in _walk(pdf.Root)))
                if key=="javascript_attachment":
                    self.assertFalse(any(isinstance(obj,pikepdf.Dictionary) and "/AF" in obj for obj in _walk(pdf.Root)))
                    self.assertFalse(pdf.Root.get("/Names",{}).get("/EmbeddedFiles"))
                    self.assertFalse(any(str(a.get("/Subtype",""))=="/FileAttachment" for p in pdf.pages for a in p.get("/Annots",[])))
                    self.assertFalse(any(isinstance(obj,pikepdf.Dictionary) and str(obj.get("/Type",""))=="/Filespec" for obj in _walk(pdf.Root)))
                    self.assertFalse(any(isinstance(obj,pikepdf.Dictionary) and "/EF" in obj for obj in _walk(pdf.Root)))

    def test_sanitize_external_keep_dest_form_and_media(self):
        source=self.root/"extras.pdf"
        with pikepdf.open(self.fx["widgets"]) as pdf:
            pdf.Root["/OpenAction"]=pikepdf.Array([pdf.pages[0].obj,"/Fit"])
            pdf.pages[0].obj["/Thumb"]=pdf.make_stream(b"thumb")
            pdf.docinfo["/Title"]="keep this"
            xmp=b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/></x:xmpmeta>'
            pdf.Root["/Metadata"]=pdf.make_stream(xmp,Type=pikepdf.Name("/Metadata"),Subtype=pikepdf.Name("/XML"))
            pdf.save(source)
        output=self.root/"extras-safe.pdf"
        stripped=self.root/"extras-stripped.pdf"
        code,record,err=self.run_cli("sanitize",source,"-o",stripped)
        self.assertEqual(code,0,(record,err))
        with pikepdf.open(stripped) as pdf:
            self.assertEqual(len(pdf.docinfo),0);self.assertFalse(pdf.Root.get("/Metadata"))
            self.assertFalse(any(p.obj.get("/Thumb") for p in pdf.pages))
        code,record,err=self.run_cli("sanitize",source,"--keep","meta,thumbnails","-o",output)
        self.assertEqual(code,0,(record,err))
        with pikepdf.open(output) as pdf:
            self.assertIn("/Title",pdf.docinfo);self.assertTrue(pdf.Root.get("/Metadata"))
            self.assertTrue(pdf.Root.get("/OpenAction"))
            self.assertTrue(any(p.obj.get("/Thumb") for p in pdf.pages))
        external=self.root/"external.pdf"
        with pikepdf.open(self.fx["links_bookmarks"]) as pdf:
            action=pikepdf.Dictionary({"/S":pikepdf.Name("/URI"),"/URI":pikepdf.String("https://example.invalid")})
            pdf.pages[0].obj["/Annots"][0]["/A"]=action
            extra={"/"+kind:pikepdf.Dictionary({"/S":pikepdf.Name("/"+kind)}) for kind in ("Launch","SubmitForm","ImportData","GoToR","GoToE")}
            pdf.Root["/AA"]=pikepdf.Dictionary(extra)
            pdf.save(external)
        code,_,err=self.run_cli("sanitize",external,"-o",self.root/"external-default.pdf")
        self.assertEqual(code,0,err)
        with pikepdf.open(self.root/"external-default.pdf") as pdf:
            self.assertTrue(any(isinstance(o,pikepdf.Dictionary) and str(o.get("/S",""))=="/URI" for o in _walk(pdf.Root)))
            self.assertTrue(any(isinstance(o,pikepdf.Dictionary) and str(o.get("/S",""))=="/Launch" for o in _walk(pdf.Root)))
        code,_,err=self.run_cli("sanitize",external,"--external","-o",self.root/"external-removed.pdf")
        self.assertEqual(code,0,err)
        with pikepdf.open(self.root/"external-removed.pdf") as pdf:
            self.assertFalse(any(isinstance(o,pikepdf.Dictionary) and str(o.get("/S","")) in {"/URI","/Launch","/SubmitForm","/ImportData","/GoToR","/GoToE"} for o in _walk(pdf.Root)))
        media=self.root/"media.pdf"
        with pikepdf.open(self.fx["base"]) as pdf:
            pdf.pages[0].obj["/Annots"]=pikepdf.Array([pikepdf.Dictionary({"/Subtype":pikepdf.Name("/Movie"),"/Movie":pikepdf.Dictionary()})])
            pdf.save(media)
        code,_,err=self.run_cli("sanitize",media,"--multimedia","-o",self.root/"media-safe.pdf")
        self.assertEqual(code,0,err)
        with pikepdf.open(self.root/"media-safe.pdf") as pdf:
            self.assertFalse(any(isinstance(o,pikepdf.Dictionary) and any(k in o for k in ("/Movie","/Sound","/RichMediaContent","/RichMediaSettings")) for o in _walk(pdf.Root)))
        with pymupdf.open(self.root/"extras-safe.pdf") as doc:
            self.assertEqual(next(doc[0].widgets()).field_value,"Ada")

    def test_sanitize_hidden_off_layers_encryption_signature_input_hash(self):
        path=self.fx["hidden_off"]
        before=hashlib.sha256(path.read_bytes()).hexdigest()
        code,record,err=self.run_cli("sanitize",path,"-o",self.root/"layers.pdf")
        self.assertEqual(code,0,(record,err))
        self.assertIn("layers_off",record["listed"])
        self.assertTrue(record["listed"]["hidden_text"])
        self.assertTrue(record["listed"]["layers_off"])
        self.assertIn("HIDDEN_CONTENT",record["warnings"])
        with pymupdf.open(record["path"]) as doc:self.assertIn("hidden phrase",doc[0].get_text())
        encrypted=self.fx["encrypted"]
        encrypted_hash=hashlib.sha256(encrypted.read_bytes()).hexdigest()
        for command,extra in (("sanitize",[]),("redact",["--text","Climate"])):
            output=self.root/(command+"-encrypted.pdf")
            code,record,err=self.run_cli(command,encrypted,*extra,"--password-env","P7_PASS","-o",output,env={"P7_PASS":"user-pass"})
            self.assertEqual(code,0,(command,record,err))
            with pikepdf.open(output,password="user-pass") as pdf:self.assertTrue(pdf.is_encrypted)
        for command,extra in (("sanitize",[]),("redact",["--text","Climate"])):
            code,record,err=self.run_cli(command,self.fx["signed"],*extra,"-o",self.root/(command+"-signed.pdf"))
            self.assertEqual(code,1,(record,err));self.assertEqual(record["code"],"SIGNED")
            code,record,err=self.run_cli(command,self.fx["signed"],*extra,"--break-signature","-o",self.root/(command+"-broken-signature.pdf"))
            self.assertEqual(code,0,(record,err));self.assertIn("SIGNATURE_INVALIDATED",record["warnings"])
        self.assertEqual(hashlib.sha256(encrypted.read_bytes()).hexdigest(),encrypted_hash)
        self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(),before)

    def test_ocr_searches_image_with_selectable_header_and_regex_lines(self):
        from tests.tooling import require_tool
        try: require_tool(self,"tesseract","OCR redaction security test")
        except unittest.SkipTest as exc: self.fail(f"Tesseract is mandatory for this security test: {exc}")
        code,record,err=self.run_cli("redact",self.fx["mixed_scan"],"--ocr","--text","Sutanto","--max-distance","2","-o",self.root/"mixed-redacted.pdf")
        self.assertEqual(code,0,(record,err));self.assertTrue(any(m["source"]=="ocr" for m in record["matches"]))
        code,record,err=self.run_cli("redact",self.fx["scanned"],"--ocr","--regex","Sut[a-z]+to","-o",self.root/"regex-scan.pdf")
        self.assertEqual(code,0,(record,err));self.assertEqual(len(record["matches"]),1)

    def test_glyphless_fuzzy_without_ocr_and_joint_occurrence_order(self):
        code,record,err=self.run_cli("redact",self.fx["ocr_layer"],"--text","Sutanto","--max-distance","2","-o",self.root/"glyphless.pdf")
        self.assertEqual(code,0,(record,err));self.assertTrue(any(row["fuzzy"] for row in record["matches"]))
        from tests.tooling import require_tool
        try: require_tool(self,"tesseract","OCR redaction security test")
        except unittest.SkipTest as exc: self.fail(f"Tesseract is mandatory for this security test: {exc}")
        code,record,err=self.run_cli("redact",self.fx["repeated_scan"],"--ocr","--text","Ada Lovelace","--occurrence","2","-o",self.root/"second-scan.pdf")
        self.assertEqual(code,0,(record,err));self.assertEqual(len(record["matches"]),1);self.assertEqual(record["matches"][0]["occurrence"],2)

    def test_rotated_crop_userunit_coordinate_redactions(self):
        source=self.fx["rotated_target"]
        code,record,err=self.run_cli("redact",source,"--text","Second page","--pages","2","-o",self.root/"rotated-lit.pdf")
        self.assertEqual(code,0,(record,err))
        from pdftools import coords
        with pymupdf.open(source) as doc:
            word=next(w for w in doc[1].get_text("words") if w[4]=="Second")
            mm=coords.pymupdf_to_display(doc[1],pymupdf.Rect(word[:4]))
        value=f"2,{mm.x0},{mm.y0},{mm.x1},{mm.y1}"
        code,record,err=self.run_cli("redact",source,"--rect",value,"-o",self.root/"rotated-rect.pdf")
        self.assertEqual(code,0,(record,err))
        from tests.tooling import require_tool
        try: require_tool(self,"tesseract","rotated OCR coordinate test")
        except unittest.SkipTest as exc: self.fail(f"Tesseract is mandatory for this security test: {exc}")
        code,record,err=self.run_cli("redact",source,"--pages","1","--ocr","--text","Sutanto","--max-distance","2","-o",self.root/"rotated-ocr.pdf")
        self.assertEqual(code,0,(record,err))
        with pymupdf.open(record["path"]) as doc:
            page=doc[0]
            pixels=page.get_pixmap(matrix=pymupdf.Matrix(100/72,100/72),alpha=False)
            unit=coords._user_unit(page)
            name=(pymupdf.Rect(self.fx["rotated_target_name_box"])*(1/unit))*page.rotation_matrix*unit
            x0=max(0,int(name.x0*pixels.width/page.rect.width));x1=min(pixels.width,int(name.x1*pixels.width/page.rect.width))
            y0=max(0,int(name.y0*pixels.height/page.rect.height));y1=min(pixels.height,int(name.y1*pixels.height/page.rect.height))
            samples=pixels.samples
            remaining=[]
            for y in range(y0,y1):
                for x in range(x0,x1):
                    at=(y*pixels.width+x)*pixels.n
                    if max(samples[at:at+3])>8: remaining.append((x,y))
            self.assertFalse(remaining,(f"drawn name pixels remain in displayed render: {remaining[:5]}",
                record.get("matches"),tuple(name),tuple(page.rect)))

    def test_xmp_decoded_entities_utf16_and_widget_appearances_removed(self):
        packets=(b'<x:xmpmeta xmlns:x="a"><rdf:RDF xmlns:rdf="b"><rdf:Description>Ada&#32;Lovelace</rdf:Description></rdf:RDF></x:xmpmeta>',
                 '<?xml version="1.0" encoding="UTF-16"?><x:xmpmeta xmlns:x="a"><v>Ada Lovelace</v></x:xmpmeta>'.encode("utf-16"))
        for index,payload in enumerate(packets):
            source=self.root/f"xmp-{index}.pdf"
            with pikepdf.open(self.fx["base"]) as pdf:
                pdf.Root["/Metadata"]=pdf.make_stream(payload,Type=pikepdf.Name("/Metadata"),Subtype=pikepdf.Name("/XML"));pdf.save(source)
            code,record,err=self.run_cli("redact",source,"--text","Ada Lovelace","-o",self.root/f"xmp-out-{index}.pdf")
            self.assertEqual(code,0,(record,err))
            from pdftools.commands.p7_redact import _xmp_text
            with pikepdf.open(record["path"]) as pdf:
                packet=pdf.Root.get("/Metadata")
                self.assertFalse(packet and "Ada Lovelace" in _xmp_text(packet.read_bytes()))
        malformed=self.root/"malformed-xmp.pdf"
        with pikepdf.open(self.fx["base"]) as pdf:
            pdf.Root["/Metadata"]=pdf.make_stream(b"<broken Ada Lovelace",Type=pikepdf.Name("/Metadata"),Subtype=pikepdf.Name("/XML"));pdf.save(malformed)
        code,record,err=self.run_cli("redact",malformed,"--text","absent target","-o",self.root/"malformed-xmp-out.pdf")
        self.assertEqual(code,0,(record,err))
        from pdftools.commands.p7_redact import _xmp_text
        with pikepdf.open(record["path"]) as pdf:
            packet=pdf.Root.get("/Metadata")
            self.assertFalse(packet and "broken Ada Lovelace" in _xmp_text(packet.read_bytes()))
        source=self.root/"widget-appearance.pdf"
        blank=self.root/"widget-blank.pdf";doc=pymupdf.open();doc.new_page(width=300,height=400);doc.save(blank);doc.close()
        with pikepdf.open(blank) as pdf:
            page=pdf.pages[0].obj
            parent=pdf.make_indirect(pikepdf.Dictionary({"/FT":pikepdf.Name("/Tx"),"/T":pikepdf.String("Parent"),"/V":pikepdf.String("Ada Lovelace")}))
            appearance=pdf.make_stream(b"BT (Ada Lovelace) Tj ET")
            child=pdf.make_indirect(pikepdf.Dictionary({"/Type":pikepdf.Name("/Annot"),"/Subtype":pikepdf.Name("/Widget"),"/Parent":parent,
                "/Rect":pikepdf.Array([10,10,100,30]),"/AP":pikepdf.Dictionary({"/N":appearance})}))
            parent["/Kids"]=pikepdf.Array([child]);page["/Annots"]=pikepdf.Array([child])
            pdf.Root["/AcroForm"]=pikepdf.Dictionary({"/Fields":pikepdf.Array([parent])});pdf.save(source)
        code,record,err=self.run_cli("redact",source,"--text","Ada Lovelace","-o",self.root/"widget-clean.pdf")
        self.assertEqual(code,0,(record,err))
        with pikepdf.open(record["path"]) as pdf:
            self.assertFalse(any(b"Ada Lovelace" in obj.read_bytes() for obj in _walk(pdf.Root) if isinstance(obj,pikepdf.Stream)))

    def test_sanitize_page_xmp_multimedia_sound_and_stream_metadata(self):
        source=self.root/"page-xmp.pdf"
        with pikepdf.open(self.fx["base"]) as pdf:
            xmp=pdf.make_stream(b'<x:xmpmeta xmlns:x="a"/>',Type=pikepdf.Name("/Metadata"),Subtype=pikepdf.Name("/XML"))
            pdf.pages[0].obj["/Metadata"]=xmp
            form=pdf.make_stream(b"",Type=pikepdf.Name("/XObject"),Subtype=pikepdf.Name("/Form"),BBox=pikepdf.Array([0,0,1,1]),Metadata=xmp)
            pdf.pages[0].obj["/Resources"]=pikepdf.Dictionary({"/XObject":pikepdf.Dictionary({"/F":form})})
            image=pdf.make_stream(b"x",Type=pikepdf.Name("/XObject"),Subtype=pikepdf.Name("/Image"),Width=1,Height=1,ColorSpace=pikepdf.Name("/DeviceGray"),BitsPerComponent=8,Metadata=xmp)
            pdf.pages[0].obj["/Resources"]["/XObject"]["/I"]=image
            pdf.Root["/OpenAction"]=pikepdf.Dictionary({"/S":pikepdf.Name("/Sound"),"/Sound":pdf.make_stream(b"sound")})
            pdf.save(source)
        code,record,err=self.run_cli("sanitize",source,"--multimedia","-o",self.root/"page-xmp-clean.pdf")
        self.assertEqual(code,0,(record,err))
        with pikepdf.open(record["path"]) as pdf:
            self.assertFalse(any(isinstance(obj,pikepdf.Stream) and str(obj.get("/Type",""))=="/Metadata" for obj in _walk(pdf.Root)))
            self.assertFalse(any(isinstance(obj,pikepdf.Dictionary) and str(obj.get("/S",""))=="/Sound" for obj in _walk(pdf.Root)))

    def test_encryption_matrix_restrictions_and_ocr_child_secrets(self):
        encrypted=self.fx["encrypted"]
        for command,extra in (("sanitize",[]),("redact",["--text","Climate"])):
            out=self.root/f"{command}-aes.pdf"
            code,record,err=self.run_cli(command,encrypted,*extra,"--password-env","P7_PASS","--owner-password-env","P7_OWNER","-o",out,
                env={"P7_PASS":"user-pass","P7_OWNER":"owner-pass"})
            self.assertEqual(code,0,(record,err))
            with pikepdf.open(out,password="owner-pass") as pdf:
                with self.subTest(command=command,encryption="AES-256"):
                    self.assertEqual(pdf.encryption.R,6);self.assertTrue(pdf.is_encrypted)
                with self.subTest(command=command,permissions=True):
                    with pikepdf.open(encrypted,password="user-pass") as original:
                        self.assertEqual(pdf.encryption.P,original.encryption.P)
            with self.subTest(command=command,user_password="user-pass"):
                with pikepdf.open(out,password="user-pass") as pdf:self.assertTrue(pdf.is_encrypted)
        for command,extra in (("sanitize",[]),("redact",["--text","Climate"])):
            out=self.root/f"{command}-reset.pdf"
            code,record,err=self.run_cli(command,encrypted,*extra,"--password-env","P7_PASS","-o",out,env={"P7_PASS":"user-pass"})
            self.assertEqual(code,0,(record,err));self.assertIn("OWNER_RESET",record["warnings"])
        restricted=self.fx["restricted"]
        for command,extra in (("sanitize",[]),("redact",["--text","Climate"])):
            code,record,err=self.run_cli(command,restricted,*extra,"-o",self.root/f"{command}-refused.pdf")
            self.assertNotEqual(code,0);self.assertEqual(record["code"],"ENCRYPTED")
            code,record,err=self.run_cli(command,restricted,*extra,"--ignore-restrictions","-o",self.root/f"{command}-ignored.pdf")
            self.assertEqual(code,0,(record,err));self.assertIn("RESTRICTIONS_IGNORED",record["warnings"])

        scan=self.root/"encrypted-scan.pdf"
        with pymupdf.open(self.fx["scanned"]) as doc: doc.save(scan,encryption=pymupdf.PDF_ENCRYPT_AES_256,user_pw="user-pass",owner_pw="owner-pass")
        import pdftools.commands.p7_redact as redact_module
        observed=[];real=redact_module.tools.run_tool
        def capture(argv,**kwargs):
            child_env=dict(os.environ if kwargs.get("env") is None else kwargs["env"])
            for name in kwargs.get("secret_env",()): child_env.pop(name,None)
            observed.append((list(argv),child_env))
            return real(argv,**kwargs)
        from tests.tooling import require_tool
        try: require_tool(self,"tesseract","OCR secret environment test")
        except unittest.SkipTest as exc: self.fail(f"Tesseract is mandatory for this security test: {exc}")
        with patch.object(redact_module.tools,"run_tool",capture):
            code,record,err=self.run_cli("redact",scan,"--password-env","P7_PASS","--ocr","--text","no-match","-o",self.root/"encrypted-scan-out.pdf",env={"P7_PASS":"user-pass"})
        self.assertEqual(code,0,(record,err));self.assertTrue(observed)
        self.assertTrue(all("P7_PASS" not in env and "user-pass" not in str(argv) and "user-pass" not in str(env) for argv,env in observed))
        code,record,err=self.run_cli("redact",encrypted,"--password-env","P7_PASS","--text","user-pass","--regex","(",env={"P7_PASS":"user-pass"})
        joined=err+json.dumps(record)
        self.assertNotIn("user-pass",joined)

    def test_redact_cross_engine_regex_pages_rect_and_pending(self):
        code,record,err=self.run_cli("redact",self.fx["base"],"--text","Ada Lovelace","--occurrence","2","-o",self.root/"second.pdf")
        self.assertEqual(code,0,(record,err))
        with pymupdf.open(record["path"]) as doc:
            self.assertEqual(doc[0].get_text().count("Ada"),1)
            self.assertNotIn("Lovelace met Ada Lovelace",doc[0].get_text())
            self.assertNotIn("Ada Lovelace",doc[0].get_text("rawdict").__str__())
        self.assertEqual(PdfReader(record["path"]).pages[0].extract_text().count("Ada"),1)
        code,record,err=self.run_cli("redact",self.fx["base"],"--regex","Ada\\s+Lovelace","-o",self.root/"regex.pdf")
        self.assertEqual(code,0,(record,err));self.assertEqual(len(record["matches"]),2)
        code,record,err=self.run_cli("redact",self.fx["plain"],"--text","Second page","--pages","2","-o",self.root/"pages.pdf")
        self.assertEqual(code,0,(record,err))
        with pymupdf.open(record["path"]) as doc:self.assertIn("Climate report",doc[0].get_text());self.assertNotIn("Second page",doc[1].get_text())
        code,record,err=self.run_cli("redact",self.fx["pending_redaction"],"--text","Climate")
        self.assertEqual(record["code"],"REDACTIONS_PENDING")
        code,record,err=self.run_cli("redact",self.fx["pending_redaction"],"--text","Climate","--apply-existing","-o",self.root/"pending-applied.pdf")
        self.assertEqual(code,0,(record,err))

    def test_redact_repeated_fixture_and_regex_line_quads(self):
        code,record,err=self.run_cli("redact",self.fx["repeated"],"--text","repeated word","--occurrence","2","-o",self.root/"repeat.pdf")
        self.assertEqual(code,0,(record,err))
        with pymupdf.open(record["path"]) as doc:self.assertEqual(doc[0].get_text().count("repeated word"),1)
        lines=self.root/"line-break.pdf";doc=pymupdf.open();page=doc.new_page(width=300,height=150)
        page.insert_text((30,50),"Cross");page.insert_text((30,75),"Line");doc.save(lines);doc.close()
        code,record,err=self.run_cli("redact",lines,"--regex","Cross\\s+Line","-o",self.root/"line-break-redacted.pdf")
        self.assertEqual(code,0,(record,err));self.assertEqual(len(record["matches"]),1)
        self.assertIn("2 areas redacted",record["note"])

    def test_redact_clears_metadata_outline_annotations_forms_structure_names(self):
        path=self.root/"structured.pdf";doc=pymupdf.open();page=doc.new_page(width=300,height=300)
        page.insert_text((30,60),"Erase Me");page.add_text_annot((40,100),"Erase Me")
        widget=pymupdf.Widget();widget.field_name="target";widget.field_type=pymupdf.PDF_WIDGET_TYPE_TEXT
        widget.rect=pymupdf.Rect(30,130,160,160);widget.field_value="Erase Me";page.add_widget(widget)
        doc.set_toc([[1,"Erase Me",1]]);doc.set_metadata({"title":"Erase Me"});doc.save(path);doc.close()
        enriched=self.root/"structured-enriched.pdf"
        with pikepdf.open(path) as pdf:
            page_obj=pdf.pages[0].obj
            annot=pikepdf.Dictionary({"/Subtype":pikepdf.Name("/Text"),"/Contents":pikepdf.String("Erase Me"),"/RC":pikepdf.String("Erase Me rich")})
            page_obj["/Annots"].append(pdf.make_indirect(annot))
            widget_obj=next(a for a in page_obj["/Annots"] if str(a.get("/Subtype",""))=="/Widget")
            widget_obj["/DV"]=pikepdf.String("Erase Me");widget_obj["/V"]=pikepdf.String("Erase Me")
            if "/AP" in widget_obj:del widget_obj["/AP"]
            pdf.Root["/StructTreeRoot"]=pikepdf.Dictionary({"/Alt":pikepdf.String("Erase Me"),"/ActualText":pikepdf.String("Erase Me"),"/T":pikepdf.String("Erase Me")})
            pdf.Root["/Names"]=pikepdf.Dictionary({"/Dests":pikepdf.Dictionary({"/Names":pikepdf.Array([pikepdf.String("Erase Me destination"),pikepdf.Array([pdf.pages[0].obj,pikepdf.Name("/Fit")])])})})
            pdf.Root["/PageLabels"]=pikepdf.Dictionary({"/Nums":pikepdf.Array([0,pikepdf.Dictionary({"/P":pikepdf.String("Erase Me label")})])})
            pdf.attachments["Erase Me attachment.txt"]=b"secret"
            xmp=b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description>Erase Me</rdf:Description></rdf:RDF></x:xmpmeta>'
            pdf.Root["/Metadata"]=pdf.make_stream(xmp,Type=pikepdf.Name("/Metadata"),Subtype=pikepdf.Name("/XML"))
            pdf.save(enriched)
        code,record,err=self.run_cli("redact",enriched,"--text","Erase Me","-o",self.root/"structured-redacted.pdf")
        self.assertEqual(code,0,(record,err));self.assertTrue(record["cleared"])
        with pymupdf.open(record["path"]) as result:
            self.assertNotIn("Erase Me","\n".join(p.get_text() for p in result))
            self.assertNotIn("Erase Me",str(result[0].get_text("rawdict")))
        self.assertNotIn("Erase Me","\n".join(p.extract_text() or "" for p in PdfReader(record["path"]).pages))
        with pikepdf.open(record["path"]) as pdf:
            self.assertFalse(any(isinstance(o,pikepdf.String) and "Erase Me" in str(o) for o in _walk(pdf.Root)))
            self.assertFalse(pdf.docinfo.get("/Title"))

    def test_redact_graphics_touched_and_scanned_without_ocr_warning(self):
        vector=self.root/"vector-line.pdf";doc=pymupdf.open();page=doc.new_page(width=300,height=200)
        page.draw_line((20,100),(280,100),color=(0,0,0),width=4);doc.save(vector);doc.close()
        code,record,err=self.run_cli("redact",vector,"--rect","1,31.75,33.5,35.3,37","--graphics","touched","-o",self.root/"touched.pdf")
        self.assertEqual(code,0,(record,err))
        with pymupdf.open(record["path"]) as result:
            pix=result[0].get_pixmap(matrix=pymupdf.Matrix(1,1),alpha=False)
            def pixel(x,y):return pix.samples[(y*pix.width+x)*pix.n]
            self.assertLess(pixel(95,100),8)
            self.assertGreater(pixel(50,100),240)
            self.assertGreater(pixel(150,100),240)
        code,record,err=self.run_cli("redact",self.fx["scanned_image"],"--text","no-match","-o",self.root/"scan-no-ocr.pdf")
        self.assertEqual(code,0,(record,err));self.assertIn("UNVERIFIED_REPRESENTATION",record["warnings"])
        self.assertIn("UNVERIFIED_REPRESENTATION",err)
        paths=self.root/"path-text.pdf";doc=pymupdf.open();page=doc.new_page(width=300,height=300)
        for row in range(8):
            for col in range(8):page.draw_rect(pymupdf.Rect(30+col*12,40+row*12,35+col*12,45+row*12),fill=(0,0,0))
        doc.save(paths);doc.close()
        code,record,err=self.run_cli("redact",paths,"--text","vector words","-o",self.root/"paths-redacted.pdf")
        self.assertEqual(code,0,(record,err));self.assertIn("UNVERIFIED_REPRESENTATION",record["warnings"])

    def test_targets_do_not_leak_to_stderr_or_logs(self):
        secret="Ada Lovelace"
        input_hash=hashlib.sha256(self.fx["base"].read_bytes()).hexdigest()
        code,record,err=self.run_cli("redact",self.fx["base"],"--text",secret,"--occurrence","2","-o",self.root/"secret.pdf")
        self.assertEqual(code,0,(record,err))
        self.assertEqual(hashlib.sha256(self.fx["base"].read_bytes()).hexdigest(),input_hash)
        self.assertNotIn(secret,err)
        if record.get("log"):self.assertNotIn(secret,Path(record["log"]).read_text())

    def test_redact_rect_dry_run_and_bad_argument_line(self):
        output=self.root/"redact-dry.pdf"
        code,record,err=self.run_cli("redact",self.fx["base"],"--text","Ada","--dry-run","-o",output)
        self.assertEqual(code,0,(record,err));self.assertFalse(output.exists())
        code,record,err=self.run_cli("redact",self.fx["base"],"--regex","(")
        self.assertEqual(code,2);self.assertEqual(record["code"],"BAD_ARG");self.assertEqual(err.count("ERROR "),1)
        code,record,err=self.run_cli("redact",self.fx["base"],"--rect","1,10,10,20,20","-o",self.root/"rect.pdf")
        self.assertEqual(code,0,(record,err));self.assertEqual(record["matches"][0]["source"],"rect")


if __name__=="__main__":unittest.main()
