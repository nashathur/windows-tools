@echo off
rem Activates the dev env here instead of `conda run`, which took ~3.7 s of every call.
rem Mirrors etc\conda\activate.d and env_vars.d; tests\test_launcher.ps1 fails when conda run sets something this doesn't.
setlocal
set "CONDA_PREFIX=C:\Users\thurs\miniconda3\envs\dev"
set "PATH=%CONDA_PREFIX%;%CONDA_PREFIX%\Library\mingw-w64\bin;%CONDA_PREFIX%\Library\usr\bin;%CONDA_PREFIX%\Library\bin;%CONDA_PREFIX%\Scripts;%CONDA_PREFIX%\bin;%PATH%"
if not defined SSL_CERT_FILE set "SSL_CERT_FILE=%CONDA_PREFIX%\Library\ssl\cacert.pem"
if not defined SSL_CERT_DIR set "SSL_CERT_DIR=%CONDA_PREFIX%\Library\ssl\certs"
if defined XML_CATALOG_FILES set "XML_CATALOG_FILES=%XML_CATALOG_FILES% "
set "XML_CATALOG_FILES=%XML_CATALOG_FILES%file:///C:/Users/thurs/miniconda3/envs/dev/etc/xml/catalog"
set "TESSDATA_PREFIX=C:/Users/thurs/miniconda3/envs/dev/share/tessdata/"
set PYTHONUTF8=1
set PYTHONIOENCODING=utf-8
"%CONDA_PREFIX%\python.exe" C:\Tools\pdftools\pdf.py %*
exit /b %ERRORLEVEL%
