import contextlib
import io
import json
import re
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT));sys.path.insert(0,str(ROOT.parent/"common"))
import numpy as np
import pymupdf
from PIL import Image, ImageDraw, ImageFont
from pdftools import cli
from pdftools.commands.p5_ocr import _measure_page
from tests.fixtures import sans_font
from tests.fixtures.p5 import build_fixtures


class OCRPrepTest(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name);self.fx=build_fixtures(self.root/"fixtures")
    def tearDown(self):self.tmp.cleanup()
    def run_cli(self,*args):
        out,err=io.StringIO(),io.StringIO()
        with patch("clierr.LOG_ROOT",self.root/"logs"),contextlib.redirect_stdout(out),contextlib.redirect_stderr(err):
            code=cli.main(["--json",*map(str,args)])
        records=[json.loads(x) for x in out.getvalue().splitlines() if x.strip()]
        return code,(records[0] if len(records)==1 else records),err.getvalue()
    def require_ocr(self):
        from tests.tooling import require_tool
        require_tool(self,"ocrmypdf","OCR preprocessing integration exercises OCRmyPDF")
        require_tool(self,"tesseract","OCR preprocessing integration exercises Tesseract")
    def test_measures_skew_uneven_background_and_small_blobs(self):
        image=Image.new("L",(600,400),255);draw=ImageDraw.Draw(image)
        font=ImageFont.truetype(sans_font(),38)
        for y in (80,170,260):draw.text((35,y),"known skew measurement",font=font,fill=0)
        image=image.rotate(-4,expand=False,fillcolor=255)
        path=self.root/"synthetic.png";image.save(path)
        doc=pymupdf.open();page=doc.new_page(width=432,height=288);page.insert_image(page.rect,filename=str(path))
        measurement=_measure_page(page,1);doc.close()
        self.assertAlmostEqual(measurement["skew"],4,delta=0.5, msg=measurement)
        gradient=np.tile(np.linspace(120,255,600,dtype=np.uint8),(400,1))
        path=self.root/"gradient.png";Image.fromarray(gradient).save(path)
        doc=pymupdf.open();page=doc.new_page(width=432,height=288);page.insert_image(page.rect,filename=str(path))
        uneven=_measure_page(page,1);doc.close()
        self.assertGreater(uneven["spread"],40,uneven);self.assertTrue(uneven["uneven"])
        dots=Image.new("L",(720,720),255)
        for x,y in ((20,20),(40,40),(60,60)):dots.putpixel((x,y),0)
        path=self.root/"dots.png";dots.save(path)
        doc=pymupdf.open();page=doc.new_page(width=518.4,height=518.4);page.insert_image(page.rect,filename=str(path))
        blobs=_measure_page(page,1);doc.close()
        self.assertGreaterEqual(blobs["blobs"],3,blobs)
    def test_numbers_scan_preserves_punctuation(self):
        self.require_ocr()
        code,record,err=self.run_cli("ocr",self.fx["numbers"],"--lang","eng","-o",self.root/"numbers-out.pdf")
        self.assertEqual(code,0,(record,err))
        with pymupdf.open(record["path"]) as doc:text=doc[0].get_text()
        found=re.findall(r"[-−–]?\d[\d,]*\.\d+",text)
        normal=lambda values:[value.replace("−","-").replace("–","-") for value in values]
        self.assertEqual(normal(found),["-2.3","1,250.00","-0.75","3.14159","-1,000.5"],text)
    def test_tilted_scan_is_deskewed_and_recognized(self):
        self.require_ocr()
        code,record,err=self.run_cli("ocr",self.fx["tilted"],"--lang","eng","-o",self.root/"tilted-out.pdf")
        self.assertEqual(code,0,(record,err));self.assertTrue(record["preprocess"]["deskew"],record)
        self.assertEqual(len(record["deskewed"]),1,record)
        self.assertAlmostEqual(record["deskewed"][0]["angle"],4,delta=1, msg=record)
        with pymupdf.open(record["path"]) as doc:text=doc[0].get_text().lower()
        self.assertIn("tilted",text);self.assertIn("readable",text)
    def test_low_resolution_warn_and_default_straight_scan_options(self):
        self.require_ocr()
        code,low,err=self.run_cli("ocr",self.fx["lowdpi"],"--lang","eng","-o",self.root/"low-out.pdf")
        self.assertEqual(code,0,(low,err));self.assertIn("OCR_LOW_QUALITY",low["warnings"])
        self.assertNotEqual(low["verdict"],"ok");self.assertEqual(low["preprocess"]["oversample"],300)
        self.assertEqual(low["page_measurements"][0]["dpi"],100)
        code,even,err=self.run_cli("ocr",self.fx["even"],"--lang","eng","-o",self.root/"even-out.pdf")
        self.assertEqual(code,0,(even,err));self.assertFalse(even["preprocess"]["deskew"])
        self.assertIsNone(even["preprocess"]["oversample"]);self.assertIsNone(even["preprocess"]["thresholding"])
    def test_clean_requires_unpaper_and_adds_only_clean(self):
        from pdftools import tools
        with patch("pdftools.commands.p5_ocr.shutil.which",return_value=None), \
             patch("pdftools.commands.p5_ocr.run_tool") as runner:
            code,record,err=self.run_cli("ocr",self.fx["scan"],"--clean","-o",self.root/"no-clean.pdf")
        self.assertEqual(code,1,(record,err));self.assertEqual(record["code"],"NOT_FOUND");runner.assert_not_called()
        def fake_run(argv,**kwargs):
            target=Path(argv[-1]);shutil.copyfile(argv[-2],target)
            if "--sidecar" in argv:Path(argv[argv.index("--sidecar")+1]).write_text("",encoding="utf-8")
        with patch("pdftools.commands.p5_ocr.shutil.which",return_value="/fake/unpaper"), \
             patch("pdftools.commands.p5_ocr.run_tool",side_effect=fake_run) as runner:
            code,record,err=self.run_cli("ocr",self.fx["scan"],"--clean","-o",self.root/"clean.pdf")
        self.assertEqual(code,0,(record,err))
        argv=runner.call_args.args[0]
        self.assertIn("--clean",argv);self.assertNotIn("--clean-final",argv)
        self.assertTrue(record["preprocess"]["clean"])


if __name__=="__main__":unittest.main()
