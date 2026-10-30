import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
import sys
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT));sys.path.insert(0,str(ROOT.parent/"common"))

import pikepdf
import pymupdf
from pypdf import PdfReader
from pdftools import cli
from pdftools import coords
from tests.fixtures.p4 import build_fixtures


class Phase4CommandsTest(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name)
        self.fx=build_fixtures(self.root/"fixtures")

    def tearDown(self):self.tmp.cleanup()

    def run_cli(self,*argv):
        out,err=io.StringIO(),io.StringIO()
        with patch("clierr.LOG_ROOT",self.root/"logs"),contextlib.redirect_stdout(out),contextlib.redirect_stderr(err):
            code=cli.main(["--json",*map(str,argv)])
        records=[json.loads(line) for line in out.getvalue().splitlines() if line.strip()]
        record=records if len(records)>1 else (records[0] if records else None)
        return code,record,err.getvalue()

    def success(self,*argv):
        code,record,err=self.run_cli(*argv)
        self.assertEqual(code,0,(record,err));self.assertEqual(record["type"],"result",record)
        self.assertTrue(Path(record["path"]).is_file(),record)
        return Path(record["path"]),record

    def error(self,argv,code):
        status,record,err=self.run_cli(*argv)
        self.assertIn(status,(1,2),record)
        self.assertEqual(record["code"],code,(record,err))
        self.assertEqual(sum(line.startswith("ERROR pdf ") for line in err.splitlines()),1,err)

    def test_meta_updates_info_xmp_and_preserves_custom_info(self):
        out,_=self.success("meta",self.fx["base"],"--title","New title","--created","2026-02-03","-o",self.root/"meta.pdf")
        with pikepdf.open(out) as pdf:
            self.assertEqual(str(pdf.docinfo["/Title"]),"New title")
            self.assertEqual(str(pdf.docinfo["/CustomFixture"]),"phase4")
            self.assertIn("+",str(pdf.docinfo["/CreationDate"]))
            with pdf.open_metadata(set_pikepdf_as_editor=False) as meta:self.assertEqual(meta["dc:title"],"New title")
        self.error(["meta",self.fx["base"],"--created","not-a-date"],"BAD_ARG")

    def test_bookmarks_and_labels_read_back_with_pypdf(self):
        out,_=self.success("bookmarks",self.fx["base"],"--set",json.dumps([{"level":1,"title":"New section","page":3}]),"-o",self.root/"bookmarks.pdf")
        self.assertEqual(PdfReader(out).outline[0].title,"New section")
        out,_=self.success("labels",self.fx["base"],"--set",json.dumps([{"from":1,"style":"roman","prefix":"p-","start":2}]),"-o",self.root/"labels.pdf")
        self.assertEqual(PdfReader(out).page_labels[0],"p-ii")
        self.error(["bookmarks",self.fx["base"],"--set","{}"],"BAD_ARG")

    def test_stamps_have_independently_extractable_text(self):
        for cmd,args,text in (
            ("watermark",["--text","DRAFT","--position","center"],"DRAFT"),
            ("numbers",["--format","{n} / {total}","--pages","1","--position","bottom-center"],"1 / 8"),
            ("headfoot",["--header","{file} {n}"],"phase4.pdf 1"),
        ):
            out,_=self.success(cmd,self.fx["base"],*args,"-o",self.root/f"{cmd}.pdf")
            self.assertIn(text,"\n".join(p.extract_text() or "" for p in PdfReader(out).pages))
        self.error(["watermark",self.fx["base"],"--text","x","--image",self.fx["base"]],"BAD_ARG")

    def test_watermark_bounds_duplicate_text_and_edge_image(self):
        source=self.root/"draft-source.pdf";doc=pymupdf.open();page=doc.new_page(width=300,height=400);page.insert_text((30,50),"DRAFT");doc.save(source);doc.close()
        text,_=self.success("watermark",source,"--text","DRAFT","--position","center","-o",self.root/"draft-watermarked.pdf")
        self.assertIn("DRAFT",PdfReader(text).pages[0].extract_text())
        from PIL import Image
        image=self.root/"mark.png";Image.new("RGBA",(12,8),(255,0,0,255)).save(image)
        stamped,_=self.success("watermark",source,"--image",image,"--position","top-left","-o",self.root/"edge-watermark.pdf")
        with pymupdf.open(stamped) as doc:
            rect=doc[0].get_image_rects(doc[0].get_images()[0][0])[0]
            self.assertGreaterEqual(rect.x0,0);self.assertGreaterEqual(rect.y0,0)
            self.assertLessEqual(rect.x1,doc[0].rect.x1);self.assertLessEqual(rect.y1,doc[0].rect.y1)

    def test_stamp_overflow_and_header_footer_regions(self):
        self.error(["headfoot",self.fx["base"],"--header","X"*500],"BAD_ARG")
        self.error(["numbers",self.fx["base"],"--format","X"*500,"--pages","1"],"BAD_ARG")
        from types import SimpleNamespace
        from pdftools.commands import p4_stamp
        captured={}
        def capture(path,spec,args,context,writer,**kwargs):
            doc=pymupdf.open(path);candidate=self.root/"headfoot-capture.pdf"
            captured["operation"]=writer(doc,candidate);doc.close()
            return {"type":"report"}
        with patch("pdftools.commands.p4_stamp.write_pdf",side_effect=capture):
            p4_stamp.headfoot(SimpleNamespace(file=self.fx["base"],header="TOP",footer="BOTTOM",margin=12,fontsize=9),SimpleNamespace())
        regions=captured["operation"].expected.regions
        self.assertEqual(sum(region["page"]==1 for region in regions),2)

    def test_annotate_literal_and_regex_per_line(self):
        out,_=self.success("annotate",self.fx["base"],"--regex","climate token[12]", "--pages","1-2","-o",self.root/"annot.pdf")
        reader=PdfReader(out)
        self.assertGreaterEqual(sum(len(p.get("/Annots",[])) for p in reader.pages),2)
        self.error(["annotate",self.fx["base"],"--highlight","no such phrase"],"BAD_ARG")

    def test_crop_cut_resize_nup_and_booklet_page_maps(self):
        crop,_=self.success("crop",self.fx["base"],"--margins","5","5","5","5","-o",self.root/"crop.pdf")
        self.assertEqual(len(PdfReader(crop).pages),8)
        cut,_=self.success("cut",self.fx["base"],"--grid","2x2","-o",self.root/"cut.pdf")
        self.assertEqual(len(PdfReader(cut).pages),32)
        resize,_=self.success("resize",self.fx["base"],"--size","210x297","--scale","fit","-o",self.root/"resize.pdf")
        self.assertEqual(len(PdfReader(resize).pages),8)
        nup,_=self.success("nup",self.fx["base"],"--grid","2","-o",self.root/"nup.pdf")
        self.assertEqual(len(PdfReader(nup).pages),4)
        with pymupdf.open(nup) as doc:self.assertGreater(doc[0].rect.width,doc[0].rect.height)
        booklet,_=self.success("booklet",self.fx["base"],"-o",self.root/"booklet.pdf")
        self.assertEqual(len(PdfReader(booklet).pages),4)
        def positioned_numbers(page):
            found=[]
            def visitor(text,cm,tm,font,size):
                import re
                m=re.search(r"PHASE FOUR PAGE (\d+)",text)
                if m:
                    x=cm[0]*tm[4]+cm[2]*tm[5]+cm[4]
                    y=cm[1]*tm[4]+cm[3]*tm[5]+cm[5]
                    found.append((int(m.group(1)),x,y))
            page.extract_text(visitor_text=visitor)
            grouped={}
            for number,x,y in found:grouped[number]=min(x,grouped.get(number,x))
            return sorted(grouped.items(),key=lambda row:row[1])
        nup_pages=PdfReader(nup).pages
        cells=positioned_numbers(nup_pages[0])
        self.assertEqual([n for n,_ in cells],[1,2])
        booklet_pages=PdfReader(booklet).pages
        front=positioned_numbers(booklet_pages[0]);back=positioned_numbers(booklet_pages[1])
        self.assertEqual([n for n,_ in front],[8,1])
        self.assertEqual([n for n,_ in back],[2,7])
        self.error(["cut",self.fx["base"],"--v","0"],"BAD_ARG")

    def test_flatten_and_gray(self):
        flattened,_=self.success("flatten",self.fx["base"],"--only","annots","-o",self.root/"flat.pdf")
        self.assertEqual(len(PdfReader(flattened).pages),8)
        raster,_=self.success("flatten",self.fx["base"],"--raster","72","-o",self.root/"raster.pdf")
        self.assertEqual(len(PdfReader(raster).pages),8)
        self.assertEqual(len(PdfReader(raster).pages[0].get("/Annots",[])),0)
        gray,_=self.success("gray",self.fx["color"],"-o",self.root/"gray.pdf")
        with pymupdf.open(gray) as doc:
            pix=doc[0].get_pixmap(matrix=pymupdf.Matrix(.5,.5),alpha=False)
            import numpy as np
            rgb=np.frombuffer(pix.samples,dtype=np.uint8).reshape(pix.height,pix.width,pix.n)[...,:3]
            self.assertLessEqual(int(np.max(rgb.max(axis=2).astype(np.int16)-rgb.min(axis=2).astype(np.int16))),2)
        self.error(["flatten",self.fx["base"],"--raster","0"],"BAD_ARG")

    def test_gray_ghostscript_fallback_runs_on_this_machine(self):
        import os
        from pdftools import tools
        # Resolve like the tools do: PATH first, then the dev env's tool folder (PATH lacks it outside `conda run`).
        gs_dir=Path(sys.prefix)/"Library"/"bin" if os.name=="nt" else Path(sys.prefix)/"bin"
        env_path=os.environ["PATH"]+os.pathsep+str(gs_dir)
        with patch.dict(os.environ,{"PATH":env_path}):gs=tools.find_tool("ghostscript")
        if not gs:
            reason="Ghostscript is absent from PATH and from "+str(gs_dir)
            if os.environ.get("PDFTOOLS_REQUIRE_TOOLS")=="1":self.fail("required tool ghostscript is missing: "+reason)
            print("SKIP ghostscript: "+reason,file=sys.stderr)
            self.skipTest(reason)
        with patch.dict(os.environ,{"PATH":env_path}),patch("pdftools.tools.run_tool",wraps=tools.run_tool) as ghostscript:
            out,record,err=self.run_cli("gray",self.fx["saturated"],"-o",self.root/"gray-saturated.pdf")
        self.assertEqual(ghostscript.call_count,1,"the Ghostscript fallback must run when chroma remains")
        self.assertIn("-dUseFastColor=true",ghostscript.call_args.args[0])
        self.assertTrue(Path(record["path"]).is_file(),(record,err))
        self.assertEqual(out,0,(record,err))
        self.assertEqual(record["verdict"],"ok",(record,err))
        with pymupdf.open(record["path"]) as doc:
            pix=doc[0].get_pixmap(matrix=pymupdf.Matrix(.5,.5),alpha=False)
            import numpy as np
            import cv2
            rgb=np.frombuffer(pix.samples,dtype=np.uint8).reshape(pix.height,pix.width,pix.n)[...,:3]
            self.assertLessEqual(int(np.max(rgb.max(axis=2).astype(np.int16)-rgb.min(axis=2).astype(np.int16))),2)
            output_gray=cv2.cvtColor(rgb,cv2.COLOR_RGB2GRAY)
        with pymupdf.open(self.fx["saturated"]) as original:
            pix=original[0].get_pixmap(matrix=pymupdf.Matrix(.5,.5),alpha=False)
            src=np.frombuffer(pix.samples,dtype=np.uint8).reshape(pix.height,pix.width,pix.n)[...,:3]
            input_gray=cv2.cvtColor(src,cv2.COLOR_RGB2GRAY)
        self.assertLessEqual(int(np.abs(input_gray.astype(np.int16)-output_gray.astype(np.int16)).max()),24)

    def test_one_error_line_for_every_registered_phase4_command(self):
        failures=(
            (["meta",self.fx["base"],"--created","not-a-date"],"BAD_ARG"),
            (["bookmarks",self.fx["base"],"--set","{}"],"BAD_ARG"),
            (["labels",self.fx["base"],"--set",json.dumps([{"from":1,"style":"bad"}])],"BAD_ARG"),
            (["watermark",self.fx["base"],"--text","x","--image",self.fx["base"]],"BAD_ARG"),
            (["numbers",self.fx["base"],"--format","{unknown}"],"BAD_ARG"),
            (["headfoot",self.fx["base"]],"BAD_ARG"),
            (["annotate",self.fx["base"]],"BAD_ARG"),
            (["flatten",self.fx["base"],"--raster","0"],"BAD_ARG"),
            (["crop",self.fx["base"]],"BAD_ARG"),
            (["cut",self.fx["base"],"--v","0"],"BAD_ARG"),
            (["resize",self.fx["base"],"--size","bad","--scale","fit"],"BAD_ARG"),
            (["nup",self.fx["base"],"--grid","2","--gap","10000"],"BAD_ARG"),
            (["booklet",self.fx["base"],"--sheet","bad"],"BAD_ARG"),
            (["gray",self.fx["base"],"--unknown"],"BAD_ARG"),
            (["rename",self.root/"missing.pdf"],"NOT_FOUND"),
            (["rmblank",self.fx["all_blank"]],"BAD_PAGE"),
        )
        for argv,code in failures:
            with self.subTest(command=argv[0]):self.error(argv,code)

    def test_protect_unlock_and_reject_wrong_password(self):
        user_file=self.root/"new-user.txt";user_file.write_text("new-user-secret\n",encoding="utf-8")
        owner_file=self.root/"new-owner.txt";owner_file.write_text("new-owner-secret\n",encoding="utf-8")
        protected,_=self.success("protect",self.fx["base"],"--new-password-file",user_file,
                                 "--new-owner-password-file",owner_file,"--no-print","--no-copy",
                                 "--no-modify","--no-assemble","-o",self.root/"protected.pdf")
        with pikepdf.open(protected,password="new-user-secret") as pdf:
            self.assertTrue(pdf.is_encrypted)
            self.assertFalse(pdf.allow.print_lowres)
            self.assertFalse(pdf.allow.extract)
            self.assertFalse(pdf.allow.modify_other)
            self.assertFalse(pdf.allow.modify_assembly)
        with self.assertRaises(pikepdf.PasswordError):
            pikepdf.open(protected,password="wrong-password")
        unlocked,_=self.success("unlock",protected,"--password-file",user_file,"-o",self.root/"unlocked.pdf")
        with pikepdf.open(unlocked) as pdf:self.assertFalse(pdf.is_encrypted)
        wrong_file=self.root/"wrong.txt";wrong_file.write_text("wrong-password",encoding="utf-8")
        self.error(["unlock",protected,"--password-file",wrong_file],"WRONG_PASSWORD")

    def test_repair_recoverable_and_truncated_inputs(self):
        from tests.fixtures.p2 import build_fixtures as build_p2_fixtures
        p2=build_p2_fixtures(self.root/"p2")
        repaired,record=self.success("repair",p2["recoverable_xref"],"-o",self.root/"repaired.pdf")
        self.assertGreater(record["recovered_pages"],0,record)
        self.assertIsInstance(record["recovery_warnings"],list)
        with pymupdf.open(repaired) as doc:self.assertGreater(doc.page_count,0)
        code,record,err=self.run_cli("repair",p2["truncated"],"-o",self.root/"truncated-fixed.pdf")
        self.assertEqual(code,1,(record,err))
        self.assertEqual(record["code"],"DAMAGED",(record,err))
        self.assertEqual(sum(line.startswith("ERROR pdf ") and " DAMAGED:" in line for line in err.splitlines()),1,err)

    def test_repair_uses_qpdf_when_pymupdf_save_fails(self):
        with patch.object(pymupdf.Document,"save",side_effect=OSError("simulated MuPDF save failure")):
            code,record,err=self.run_cli("repair",self.fx["base"],"-o",self.root/"repair-save-fallback.pdf")
        self.assertEqual(code,0,(record,err));self.assertTrue(Path(record["path"]).is_file())
        self.assertTrue(any("pikepdf/qpdf" in warning for warning in record["recovery_warnings"]))

    def test_gray_scratch_is_confined_to_run_temp_on_tool_failure(self):
        from pdftools import tools
        output=self.root/"gray-failure.pdf"
        with patch("pdftools.tools.run_tool",side_effect=RuntimeError("simulated tool failure")):
            code,record,err=self.run_cli("gray",self.fx["saturated"],"-o",output)
        self.assertNotEqual(code,0);self.assertFalse(output.exists())
        self.assertFalse(list(self.root.glob("*.orig.pdf")));self.assertFalse(list(self.root.glob("*.gs.pdf")))

    def test_layout_flatten_annots_uses_baked_baseline(self):
        out,_=self.success("cut",self.fx["base"],"--grid","2x2","--flatten-annots",
                           "-o",self.root/"cut-flat.pdf")
        self.assertEqual(len(PdfReader(out).pages),32)
        self.assertEqual(sum(len(page.get("/Annots",[])) for page in PdfReader(out).pages),0)

    def test_rmblank_rename_apply_and_partial_manifest_undo(self):
        out,_=self.success("rmblank",self.fx["blank"],"-o",self.root/"rmblank.pdf")
        self.assertEqual(len(PdfReader(out).pages),2)
        folder=self.root/"rename";folder.mkdir();source=folder/"CON.pdf";source.write_bytes(self.fx["base"].read_bytes())
        code,record,err=self.run_cli("rename",source,"--apply")
        self.assertEqual(code,0,(record,err));manifest=Path(record["data"]["manifest"])
        renamed=folder/record["data"]["renamed"][0]
        self.assertTrue((folder/renamed).exists())
        data=json.loads(manifest.read_text(encoding="utf-8"));row=data["files"][0]
        partial=folder/row["new"];temp=folder/row["temp"];partial.replace(temp)
        code,record,err=self.run_cli("rename",source,"--undo",manifest)
        self.assertEqual(code,0,(record,err));self.assertTrue(source.exists())
        self.error(["rmblank",self.fx["all_blank"],"--include-uncertain"],"BAD_PAGE")

    def test_rename_names_follow_windows_rules(self):
        from pdftools.commands.p4_files import _safe_name
        self.assertEqual(_safe_name("CON"),"CON_")
        self.assertNotIn(":",_safe_name("a:b"));self.assertFalse(_safe_name("trailing. ").endswith(("."," ")))

    def test_phase4_json_jobs_have_command_specific_schemas(self):
        secret=self.root/"job-password.txt";secret.write_text("job-secret",encoding="utf-8")
        cases=(
            ("numbers",{"format":"{n} of {total}"}),
            ("nup",{"grid":2}),
            ("protect",{"new_password_file":str(secret)}),
            ("bookmarks",{"items":[{"level":1,"title":"Job outline","page":2}]}),
            ("labels",{"items":[{"from":1,"style":"roman"}]}),
        )
        for command,values in cases:
            with self.subTest(command=command):
                output=self.root/f"job-{command}.pdf"
                job={"version":1,"command":command,"input":str(self.fx["base"]),"output":str(output),**values}
                job_path=self.root/f"job-{command}.json";job_path.write_text(json.dumps(job),encoding="utf-8")
                code,record,err=self.run_cli("--job",job_path)
                self.assertEqual(code,0,(command,record,err));self.assertEqual(record["type"],"result",record)
                self.assertTrue(Path(record["path"]).is_file(),record)

    def test_rename_dry_run_and_planned_collisions_are_safe(self):
        folder=self.root/"rename-preview";folder.mkdir()
        for name in ("one.pdf","two.pdf"):(folder/name).write_bytes(self.fx["base"].read_bytes())
        before={p.name:p.read_bytes() for p in folder.glob("*.pdf")}
        code,record,err=self.run_cli("rename",folder,"--title","Same title","--apply","--dry-run")
        self.assertEqual(code,0,(record,err));self.assertEqual({p.name:p.read_bytes() for p in folder.glob("*.pdf")},before)
        self.assertEqual([Path(row["new"]).name for row in record["data"]], ["Same title.pdf","Same title_2.pdf"])
        self.assertFalse(list(folder.glob("rename_manifest_*.json")))
        from pdftools.commands.p4_files import _safe_name
        self.assertEqual(_safe_name("CON.report"),"CON_.report")

    def test_rename_undo_dry_run_and_nonoverwriting_publication(self):
        folder=self.root/"rename-race";folder.mkdir();source=folder/"source.pdf";source.write_bytes(self.fx["base"].read_bytes())
        code,record,err=self.run_cli("rename",source,"--title","target","--apply")
        self.assertEqual(code,0,(record,err));manifest=Path(record["data"]["manifest"]);target=folder/"target.pdf"
        before={p.name:p.read_bytes() for p in folder.iterdir() if p.is_file()}
        code,preview,err=self.run_cli("rename",source,"--undo",manifest,"--dry-run")
        self.assertEqual(code,0,(preview,err));self.assertEqual({p.name:p.read_bytes() for p in folder.iterdir() if p.is_file()},before)
        target.unlink();race_source=folder/"race-source.pdf";race_source.write_bytes(self.fx["base"].read_bytes())
        race_target=folder/"race-target.pdf"
        from pdftools.commands import p4_files
        real_rename=p4_files.rename_exclusive
        def create_race(src,dst):
            if Path(dst)==race_target and not race_target.exists(): race_target.write_bytes(b"racing process file")
            return real_rename(src,dst)
        with patch("pdftools.commands.p4_files.rename_exclusive",side_effect=create_race):
            code,record,err=self.run_cli("rename",race_source,"--title","race-target","--apply")
        self.assertNotEqual(code,0);self.assertEqual(race_target.read_bytes(),b"racing process file")

    def test_rmblank_uncertain_class_and_measurements(self):
        path=self.root/"uncertain.pdf";doc=pymupdf.open();page=doc.new_page(width=100/coords.MM_PER_POINT,height=100/coords.MM_PER_POINT)
        for x,y in ((10,10),(20,10),(30,10),(40,10)):
            rect=pymupdf.Rect(x/coords.MM_PER_POINT,y/coords.MM_PER_POINT,(x+2)/coords.MM_PER_POINT,(y+2)/coords.MM_PER_POINT)
            page.draw_rect(rect,color=(0,0,0),fill=(0,0,0),width=0)
        doc.new_page(width=100/coords.MM_PER_POINT,height=100/coords.MM_PER_POINT);doc[1].insert_text((30,50),"KEEP")
        doc.save(path);doc.close()
        out,record=self.success("rmblank",path,"-o",self.root/"uncertain-kept.pdf")
        self.assertEqual(len(PdfReader(out).pages),2);self.assertEqual(record["uncertain"][0]["page"],1)
        self.assertGreater(record["uncertain"][0]["ink_percent"],.1)
        removed,record=self.success("rmblank",path,"--include-uncertain","-o",self.root/"uncertain-removed.pdf")
        self.assertEqual(len(PdfReader(removed).pages),1);self.assertEqual(record["removed"][0]["page"],1)

    def test_meta_each_translates_backend_failures_and_continues(self):
        folder=self.root/"metadata-each";folder.mkdir();(folder/"ok.pdf").write_bytes(self.fx["base"].read_bytes())
        (folder/"bad.pdf").write_bytes(self.fx["damaged"].read_bytes())
        code,records,err=self.run_cli("meta",folder,"--each")
        self.assertEqual(code,4,(records,err));self.assertEqual(records[-1]["type"],"summary")
        self.assertEqual(records[-1]["failed"],1)
        self.assertEqual(sum(row.get("type")=="error" for row in records),1)

    def test_flatten_default_ignores_popup_comments_and_flattens_widgets(self):
        flattened,_=self.success("flatten",self.fx["base"],"-o",self.root/"flat-default.pdf")
        remaining=[annot.get_object().get("/Subtype") for page in PdfReader(flattened).pages for annot in page.get("/Annots",[])]
        self.assertTrue(all(kind=="/Link" for kind in remaining),remaining)
        form,_=self.success("flatten",self.fx["forms"],"-o",self.root/"flat-forms.pdf")
        reader=PdfReader(form)
        self.assertEqual(len(reader.pages[0].get("/Annots",[])),0)
        self.assertIn("fixture value",reader.pages[0].extract_text())

    def test_gray_fallback_is_gated_and_secret_env_is_removed(self):
        import os
        source=self.root/"saturated-linked.pdf"
        with pymupdf.open(self.fx["saturated"]) as doc:
            doc[0].insert_link({"kind":pymupdf.LINK_URI,"from":pymupdf.Rect(10,10,20,20),"uri":"https://example.invalid"})
            doc.save(source)
        from pdftools import tools
        with patch.dict(os.environ,{"PDF_PASSWORD":"never-leak"}),patch("shutil.which",return_value="fakegs"),patch("pdftools.tools.run_tool",wraps=tools.run_tool) as gs:
            code,record,err=self.run_cli("gray",source,"--password-env","PDF_PASSWORD","-o",self.root/"gray-linked.pdf")
        self.assertEqual(gs.call_count,0,(record,err));self.assertIn("SKIPPED_GS",record["warnings"])
        def fake_gs(argv,**kwargs):
            import shutil
            shutil.copyfile(argv[-1],argv[-2])
            return __import__("subprocess").CompletedProcess(argv,0,"","")
        with patch.dict(os.environ,{"PDF_PASSWORD":"secret-value"}),patch("shutil.which",return_value="fakegs"),patch("pdftools.tools.run_tool",side_effect=fake_gs) as gs:
            code,record,err=self.run_cli("gray",self.fx["saturated"],"--password-env","PDF_PASSWORD","-o",self.root/"gray-secret.pdf")
        self.assertEqual(gs.call_count,1,(record,err))
        self.assertIn("PDF_PASSWORD",gs.call_args.kwargs["secret_env"])
        self.assertIn("secret-value",gs.call_args.kwargs["secret_values"])

    def test_meta_export_stages_collision_and_honors_dry_run(self):
        destination=self.root/"meta.json";destination.write_text("preserve",encoding="utf-8")
        code,record,err=self.run_cli("meta",self.fx["base"],"--to",destination,"--dry-run")
        self.assertEqual(code,0,(record,err));self.assertEqual(destination.read_text(encoding="utf-8"),"preserve")
        code,record,err=self.run_cli("meta",self.fx["base"],"--to",destination)
        self.assertEqual(code,0,(record,err));published=Path(record["data"])
        self.assertNotEqual(published,destination);self.assertEqual(destination.read_text(encoding="utf-8"),"preserve")
        exported=json.loads(published.read_text(encoding="utf-8"))
        self.assertEqual(exported["info"]["CustomFixture"],"phase4")
        self.error(["meta",self.fx["base"],"--to",self.fx["base"]],"BAD_ARG")

    def test_meta_import_custom_keys_clear_and_replace(self):
        patch_file=self.root/"meta-patch.json"
        patch_file.write_text(json.dumps({"info":{"Title":"Patch","CustomFixture":"roundtrip","CreationDate":"D:20260101000000+00'00'"}}),encoding="utf-8")
        out,_=self.success("meta",self.fx["base"],"--from",patch_file,"-o",self.root/"meta-patched.pdf")
        with pikepdf.open(out) as pdf:
            self.assertEqual(str(pdf.docinfo["/CustomFixture"]),"roundtrip")
            self.assertEqual(str(pdf.docinfo["/Title"]),"Patch")
        cleared,_=self.success("meta",out,"--clear","title,CustomFixture","-o",self.root/"meta-cleared.pdf")
        with pikepdf.open(cleared) as pdf:
            self.assertNotIn("/Title",pdf.docinfo);self.assertNotIn("/CustomFixture",pdf.docinfo)
        replacement=self.root/"meta-replace.json";replacement.write_text(json.dumps({"info":{"Title":"Only"}}),encoding="utf-8")
        replaced,_=self.success("meta",self.fx["base"],"--from",replacement,"--replace","-o",self.root/"meta-replaced.pdf")
        with pikepdf.open(replaced) as pdf:
            self.assertNotIn("/Lang",pdf.Root);self.assertNotIn("/CustomFixture",pdf.docinfo)
        xmp_source=self.root/"xmp-source.pdf";xmp_source.write_bytes(self.fx["base"].read_bytes())
        with pikepdf.open(xmp_source,allow_overwriting_input=True) as pdf:
            with pdf.open_metadata(set_pikepdf_as_editor=False) as metadata: metadata["xmp:Label"]="old-xmp-only"
            pdf.save(xmp_source)
        replaced_xmp,_=self.success("meta",xmp_source,"--from",replacement,"--replace","-o",self.root/"xmp-replaced.pdf")
        with pikepdf.open(replaced_xmp) as pdf:
            with pdf.open_metadata(set_pikepdf_as_editor=False) as metadata:self.assertNotIn("xmp:Label",metadata)
        dated,_=self.success("meta",self.fx["base"],"--created","2026-01-02","-o",self.root/"date-roundtrip-source.pdf")
        exported=self.root/"roundtrip.json"
        code,record,err=self.run_cli("meta",dated,"--to",exported)
        self.assertEqual(code,0,(record,err))
        roundtrip,_=self.success("meta",self.fx["blank"],"--from",exported,"-o",self.root/"roundtrip.pdf")
        with pikepdf.open(roundtrip) as pdf:
            self.assertEqual(str(pdf.docinfo["/CustomFixture"]),"phase4")
            with pikepdf.open(dated) as original:self.assertEqual(str(pdf.docinfo["/CreationDate"]),str(original.docinfo["/CreationDate"]))

    def test_metadata_scratch_is_cleaned_after_validation_failure(self):
        output=self.root/"meta-invalid.pdf"
        self.error(["meta",self.fx["base"],"--created","not-a-date","-o",output],"BAD_ARG")
        self.assertFalse(list(self.root.glob("*.edit.pdf")))

    def test_meta_bookmark_and_label_requests_are_verified_after_write(self):
        from pdftools.commands import p4_meta
        def fake_writer(source,spec,args,context,writer,**kwargs):
            output=self.root/f"wrong-{spec.name}.pdf"
            with pymupdf.open(source) as doc:doc.save(output)
            return {"type":"result","command":spec.name,"path":str(output),"verdict":"ok"}
        with patch("pdftools.commands.p4_meta.write_pdf",side_effect=fake_writer):
            self.error(["meta",self.fx["base"],"--title","wanted","-o",self.root/"meta-request.pdf"],"CHECK_FAILED")
        with patch("pdftools.commands.p4_meta.write_pdf",side_effect=fake_writer):
            self.error(["bookmarks",self.fx["base"],"--set",json.dumps([{"level":1,"title":"wanted","page":2}])],"CHECK_FAILED")
        with patch("pdftools.commands.p4_meta.write_pdf",side_effect=fake_writer):
            self.error(["labels",self.fx["base"],"--set",json.dumps([{"from":1,"style":"roman"}])],"CHECK_FAILED")

    def test_bookmarks_reject_hierarchy_jumps(self):
        self.error(["bookmarks",self.fx["base"],"--set",json.dumps([{"level":1,"title":"A","page":1},{"level":3,"title":"C","page":2}])],"BAD_ARG")
        self.error(["bookmarks",self.fx["base"],"--set",json.dumps([{"level":2,"title":"B","page":1}])],"BAD_ARG")

    def test_annotation_removal_pages_regex_pages_and_geometry_validation(self):
        source=self.root/"annot-pages.pdf"
        with pymupdf.open(self.fx["base"]) as doc:
            doc[1].add_text_annot((150,150),"keep-on-page-two");doc.save(source)
        code,record,err=self.run_cli("annotate",source,"--remove","all","--pages","1","-o",self.root/"annots-removed.pdf")
        self.assertEqual(code,0,(record,err))
        with pymupdf.open(record["path"]) as doc:
            self.assertEqual(len(list(doc[0].annots() or [])),0)
            self.assertEqual(len(list(doc[1].annots() or [])),1)
        code,record,err=self.run_cli("annotate",self.fx["base"],"--note","invalid","--at","0,10,10")
        self.assertEqual(code,2,(record,err));self.assertEqual(record["code"],"BAD_ARG")
        code,record,err=self.run_cli("annotate",self.fx["base"],"--regex","(")
        self.assertEqual(code,2,(record,err));self.assertEqual(record["code"],"BAD_ARG")
        self.error(["annotate",self.fx["base"],"--remove","1,bad"],"BAD_ARG")
        self.error(["annotate",self.fx["base"],"--textbox","text","--rect","1.5","10","10","30","30"],"BAD_ARG")
        self.error(["annotate",self.fx["base"],"--box","0,10,10,30,30"],"BAD_ARG")

    def test_literal_annotation_and_arrow_uses_expected_geometry(self):
        out,_=self.success("annotate",self.fx["base"],"--highlight","climate token1","--pages","1","-o",self.root/"literal-highlight.pdf")
        with pymupdf.open(out) as doc:
            self.assertEqual(len(list(doc[0].annots() or [])),2)  # fixture sticky note plus the new highlight
        arrow,_=self.success("annotate",self.fx["base"],"--arrow","1,10,10,30,30","-o",self.root/"arrow.pdf")
        with pymupdf.open(arrow) as doc:
            page=doc[0];annot=next(a for a in page.annots() if a.type[1]=="Line")
            self.assertEqual(annot.line_ends[1],pymupdf.PDF_ANNOT_LE_OPEN_ARROW)

    def test_literal_search_preserves_rotated_text_quads(self):
        source=self.root/"rotated-text.pdf";doc=pymupdf.open();page=doc.new_page(width=300,height=400)
        page.insert_text((100,100),"ANGLED",fontsize=20,morph=(pymupdf.Point(100,100),pymupdf.Matrix(30)))
        doc.save(source);doc.close()
        out,_=self.success("annotate",source,"--highlight","ANGLED","-o",self.root/"rotated-highlight.pdf")
        with pymupdf.open(out) as doc:
            page=doc[0];annot=next(iter(page.annots()));vertices=annot.vertices
            self.assertEqual(len(vertices),4)
            self.assertNotAlmostEqual(vertices[0][1],vertices[1][1],delta=.1)

    def test_layout_dry_run_plan_border_orientation_and_pad(self):
        code,record,err=self.run_cli("cut",self.fx["base"],"--grid","2x2","--dry-run")
        self.assertEqual(code,0,(record,err));self.assertEqual(record["pages"],32);self.assertEqual(len(record["page_map"]),32)
        self.assertTrue(all(len(row)==1 for row in record["page_map"]))
        code,record,err=self.run_cli("cut",self.fx["base"],"--grid","0x2","--dry-run")
        self.assertEqual(code,2,(record,err));self.assertEqual(record["code"],"BAD_ARG")
        nup,_=self.success("nup",self.fx["base"],"--grid","2","--sheet","a4","--border","-o",self.root/"nup-border.pdf")
        with pymupdf.open(nup) as doc:self.assertTrue(any(d["type"]=="s" for d in doc[0].get_drawings()))
        booklet,_=self.success("booklet",self.fx["base"],"--sheet","a3","-o",self.root/"booklet-a3.pdf")
        with pymupdf.open(booklet) as doc:self.assertGreater(doc[0].rect.width,doc[0].rect.height)
        small=self.root/"small.pdf";doc=pymupdf.open();page=doc.new_page(width=100/coords.MM_PER_POINT,height=100/coords.MM_PER_POINT);page.insert_text((10/coords.MM_PER_POINT,20/coords.MM_PER_POINT),"SCALE",fontsize=12);doc.save(small);doc.close()
        padded,_=self.success("resize",small,"--size","a4","--pad","-o",self.root/"small-pad.pdf")
        with pymupdf.open(small) as src,pymupdf.open(padded) as dst:
            self.assertAlmostEqual(dst[0].rect.width,595.28,delta=1)
            self.assertAlmostEqual(dst[0].rect.height,841.89,delta=1)
            self.assertAlmostEqual(dst[0].get_text("dict")["blocks"][0]["lines"][0]["spans"][0]["size"],12,delta=.1)
        one=self.root/"one-page.pdf";doc=pymupdf.open();doc.new_page(width=300,height=400);doc.save(one);doc.close()
        nup,_=self.success("nup",one,"--grid","2","--sheet","a4","-o",self.root/"one-nup.pdf")
        with pymupdf.open(nup) as doc:self.assertGreater(doc[0].rect.width,doc[0].rect.height)
        booklet,_=self.success("booklet",self.fx["base"],"-o",self.root/"booklet-default.pdf")
        with pymupdf.open(booklet) as doc:
            self.assertAlmostEqual(doc[0].rect.width,400,delta=1);self.assertAlmostEqual(doc[0].rect.height,300,delta=1)

    def test_layout_checker_uses_source_placements_and_blank_rectangles(self):
        from pdftools.check import OperationResult,Placement
        from pdftools.commands.p4_layout import _layout_policy
        scan=self.root/"scan.pdf";doc=pymupdf.open();page=doc.new_page(width=300,height=400)
        pix=pymupdf.Pixmap(pymupdf.csRGB,10,10,bytes([0,80,180]*100),False)
        page.insert_image(pymupdf.Rect(50,100,250,300),pixmap=pix);doc.save(scan);doc.close()
        wrong=self.root/"wrong-layout.pdf";doc=pymupdf.open();doc.new_page(width=300,height=400);doc.save(wrong);doc.close()
        with pymupdf.open(scan) as src,pymupdf.open(wrong) as output:
            operation=OperationResult(pages=[[Placement(None,0,dest_rect=(0,0,300,400))]])
            findings=_layout_policy(src,output,operation)
        self.assertIn("PLACEMENT_TILES_MISMATCH",{f.code for f in findings})
        one=self.root/"single-layout.pdf";doc=pymupdf.open();page=doc.new_page(width=300,height=400);page.insert_text((30,50),"ONE");doc.save(one);doc.close()
        out,_=self.success("nup",one,"--grid","2","-o",self.root/"single-nup.pdf")
        self.assertEqual(len(PdfReader(out).pages),1)

    def test_crop_handles_rotated_pages_and_auto_image_bounds(self):
        rotated=self.root/"rotated-crop.pdf";doc=pymupdf.open();page=doc.new_page(width=300,height=400);page.insert_text((30,50),"ROTATED");page.set_rotation(90);doc.save(rotated);doc.close()
        cropped,_=self.success("crop",rotated,"--margins","5","5","5","5","-o",self.root/"rotated-cropped.pdf")
        with pymupdf.open(cropped) as doc:self.assertGreater(doc[0].cropbox.width,0);self.assertGreater(doc[0].cropbox.height,0)
        from PIL import Image
        image=self.root/"image.png";Image.new("RGB",(40,40),(0,0,0)).save(image)
        image_pdf=self.root/"image-only.pdf";doc=pymupdf.open();page=doc.new_page(width=300,height=400);page.insert_image(pymupdf.Rect(60,100,200,240),filename=str(image));doc.save(image_pdf);doc.close()
        auto,_=self.success("crop",image_pdf,"--auto","--pad","0","-o",self.root/"image-auto-crop.pdf")
        with pymupdf.open(image_pdf) as src,pymupdf.open(auto) as dst:
            self.assertLess(dst[0].cropbox.width,src[0].cropbox.width)

    def test_crop_margin_coordinate_grid(self):
        for rotation in (0,90,180,270):
            for unit in (1,2):
                with self.subTest(rotation=rotation,unit=unit):
                    source=self.root/f"crop-{rotation}-{unit}.pdf"
                    doc=pymupdf.open();page=doc.new_page(width=500,height=400)
                    page.insert_text((70,150),"crop-grid")
                    doc.xref_set_key(page.xref,"CropBox","[20 30 420 330]")
                    if unit!=1:doc.xref_set_key(page.xref,"UserUnit",str(unit))
                    page.set_rotation(rotation);doc.save(source);doc.close()
                    output,_=self.success("crop",source,"--margins","5","5","5","5",
                                          "-o",self.root/f"crop-out-{rotation}-{unit}.pdf")
                    with pymupdf.open(source) as before,pymupdf.open(output) as after:
                        expected=(before[0].rect.width-10*coords.MM_PER_POINT**-1,
                                  before[0].rect.height-10*coords.MM_PER_POINT**-1)
                        self.assertAlmostEqual(after[0].rect.width,expected[0],places=1)
                        self.assertAlmostEqual(after[0].rect.height,expected[1],places=1)
                        self.assertEqual(after[0].get_text().strip(),"crop-grid")

    def test_cut_grid_uses_displayed_panels(self):
        for rotation in (0,90,180,270):
            for unit in (1,2):
                with self.subTest(rotation=rotation,unit=unit):
                    source=self.root/f"cut-{rotation}-{unit}.pdf"
                    doc=pymupdf.open();page=doc.new_page(width=500,height=400)
                    for label,point in (("A",(70,100)),("B",(250,100)),("C",(70,250)),("D",(250,250))):
                        page.insert_text(point,label,fontsize=20)
                    doc.xref_set_key(page.xref,"CropBox","[20 30 420 330]")
                    if unit!=1:doc.xref_set_key(page.xref,"UserUnit",str(unit))
                    page.set_rotation(rotation);doc.save(source);doc.close()
                    code,record,err=self.run_cli("cut",source,"--grid","2x2","-o",self.root/f"cut-out-{rotation}-{unit}.pdf")
                    if code!=0 and record.get("path"):
                        with pymupdf.open(record["path"]) as failed:
                            texts=[page.get_text().strip() for page in failed]
                        self.fail((record.get("findings"),texts,err))
                    self.assertEqual(code,0,(record,err));output=Path(record["path"])
                    with pymupdf.open(source) as before,pymupdf.open(output) as after:
                        self.assertEqual(len(after),4)
                        for page in after:
                            self.assertAlmostEqual(page.rect.width,before[0].rect.width/2,places=1)
                            self.assertAlmostEqual(page.rect.height,before[0].rect.height/2,places=1)

    def test_cut_rotated_and_ordinary_pages_have_panel_media_boxes(self):
        for rotation in (0, 90):
            with self.subTest(rotation=rotation):
                source=self.root/f"cut-panel-{rotation}.pdf"
                doc=pymupdf.open();page=doc.new_page(width=400,height=300)
                page.insert_text((100 if rotation==90 else 300,100),"right-panel",fontsize=12)
                page.set_rotation(rotation);doc.save(source);doc.close()
                output=self.root/f"cut-panel-out-{rotation}.pdf"
                code,record,err=self.run_cli("cut",source,"--grid","2x1","-o",output)
                self.assertEqual(code,0,(record,err))
                with pymupdf.open(output) as result:
                    self.assertEqual(len(result),2)
                    for panel in result:
                        self.assertAlmostEqual(panel.mediabox.width,200 if rotation==0 else 400,places=2)
                        self.assertAlmostEqual(panel.mediabox.height,300 if rotation==0 else 150,places=2)
                    words=result[1].get_text("words")
                    self.assertEqual([word[4] for word in words],["right-panel"])
                    self.assertAlmostEqual(words[0][0],100,delta=0.1)
                    self.assertAlmostEqual(words[0][1],87.1,delta=0.5)

    def test_annotate_box_coordinate_grid(self):
        for rotation in (0,90,180,270):
            for unit in (1,2):
                with self.subTest(rotation=rotation,unit=unit):
                    source=self.root/f"annot-{rotation}-{unit}.pdf"
                    doc=pymupdf.open();page=doc.new_page(width=500,height=400)
                    doc.xref_set_key(page.xref,"CropBox","[20 30 420 330]")
                    if unit!=1:doc.xref_set_key(page.xref,"UserUnit",str(unit))
                    page.set_rotation(rotation);doc.save(source);doc.close()
                    wanted=(30,40,60,55)
                    output,_=self.success("annotate",source,"--box",f"1,{','.join(map(str,wanted))}",
                                          "-o",self.root/f"annot-out-{rotation}-{unit}.pdf")
                    with pymupdf.open(output) as result:
                        page=result[0];annot=next(iter(page.annots()))
                        pix=page.get_pixmap(matrix=pymupdf.Matrix(1,1),alpha=False,annots=True)
                        import numpy as np
                        rgb=np.frombuffer(pix.samples,dtype=np.uint8).reshape(pix.height,pix.width,pix.n)
                        ink=np.where(np.any(rgb[:,:,:3]<245,axis=2))
                        self.assertTrue(len(ink[0]),"annotation should render visible ink")
                        actual=(ink[1].min(),ink[0].min(),ink[1].max(),ink[0].max())
                        expected=tuple(value*72/25.4 for value in wanted)
                        self.assertTrue(all(abs(value-target)<=3 for value,target in zip(actual,expected)),
                                        (actual,expected,rotation,unit))

    def test_flatten_raster_checks_against_annotation_inclusive_baseline(self):
        from pdftools.check import ExpectedChanges,OperationResult,Placement
        from pdftools.commands.p4_layout import _flatten_policy
        baseline=self.root/"raster-baseline.pdf";blank=self.root/"raster-blank.pdf"
        with pymupdf.open(self.fx["base"]) as source:
            out=pymupdf.open()
            for page in source:
                pix=page.get_pixmap(matrix=pymupdf.Matrix(1,1),alpha=False,annots=True)
                target=out.new_page(width=page.rect.width,height=page.rect.height);target.insert_image(target.rect,stream=pix.tobytes("png"))
            out.save(baseline);out.close()
        doc=pymupdf.open();doc.new_page(width=300,height=400);doc.save(blank);doc.close()
        operation=OperationResult(pages=[[Placement(None,0)]],expected=ExpectedChanges(text_removed=["__raster__"]))
        with pymupdf.open(baseline) as expected,pymupdf.open(blank) as actual:
            findings=_flatten_policy(expected,actual,operation)
        self.assertIn("TILES_CHANGED",{finding.code for finding in findings})

    def test_rmblank_reports_ink_and_keeps_navigation(self):
        nav=self.root/"blank-nav.pdf";doc=pymupdf.open();
        for text in ("FIRST","","LAST"):
            page=doc.new_page(width=300,height=400)
            if text:page.insert_text((30,50),text)
        doc.set_toc([[1,"Survivor",3]])
        doc.set_page_labels([{"startpage":0,"prefix":"p-","style":"D","firstpagenum":1}])
        doc[0].insert_link({"kind":pymupdf.LINK_GOTO,"from":pymupdf.Rect(20,20,100,40),"page":2,"to":pymupdf.Point(30,50)})
        doc.save(nav);doc.close()
        out,record=self.success("rmblank",nav,"-o",self.root/"blank-nav-rm.pdf")
        self.assertEqual(record["removed"][0]["page"],2);self.assertIsInstance(record["removed"][0]["ink_percent"],float)
        with pymupdf.open(out) as result:
            self.assertEqual(result.get_toc()[0][2],2)
            self.assertEqual(result[0].get_label(),"p-1")
            self.assertEqual(result[0].get_links()[0]["page"],1)

    def test_numbers_use_each_page_label_rule(self):
        source=self.root/"roman-labels.pdf"
        with pymupdf.open(self.fx["base"]) as doc:
            doc.set_page_labels([{"startpage":0,"style":"r","prefix":"p-","firstpagenum":1}]);doc.save(source)
        out,_=self.success("numbers",source,"--format","{label}","--pages","1-2","-o",self.root/"roman-numbered.pdf")
        text="\n".join(page.extract_text() or "" for page in PdfReader(out).pages)
        self.assertIn("p-i",text);self.assertIn("p-ii",text)

    def test_protect_unlock_policies_check_encryption_readback(self):
        from pdftools.commands.p4_files import _protect_policy,_unlock_policy
        from pdftools.check import OperationResult,Placement
        wrong=self.root/"wrong-protect.pdf";doc=pymupdf.open()
        for _ in range(8):doc.new_page(width=300,height=400)
        doc.save(wrong);doc.close()
        with pymupdf.open(self.fx["base"]) as source,pymupdf.open(wrong) as output:
            op=OperationResult(pages=[[Placement(None,i)] for i in range(source.page_count)])
            self.assertIn("TEXT_CHANGED",{f.code for f in _protect_policy(source,output,op)})
        with pymupdf.open(self.fx["base"]) as doc:
            op=OperationResult(pages=[[Placement(None,i)] for i in range(doc.page_count)])
            self.assertIn("ENCRYPTION_MISSING",{f.code for f in _protect_policy(doc,doc,op)})
        password=self.root/"policy-password.txt";password.write_text("policy-secret",encoding="utf-8")
        protected,_=self.success("protect",self.fx["base"],"--new-password-file",password,"-o",self.root/"policy-protected.pdf")
        with pymupdf.open(protected) as doc:
            doc.authenticate("policy-secret")
            op=OperationResult(pages=[[Placement(None,i)] for i in range(doc.page_count)])
            self.assertIn("ENCRYPTION_REMAINS",{f.code for f in _unlock_policy(doc,doc,op)})


if __name__=="__main__":unittest.main()
