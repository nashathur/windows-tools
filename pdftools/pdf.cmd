@echo off
set PYTHONUTF8=1
set PYTHONIOENCODING=utf-8
"C:\Users\thurs\miniconda3\Scripts\conda.exe" run -n dev --no-capture-output python C:\Tools\pdftools\pdf.py %*
