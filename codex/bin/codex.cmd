@echo off
rem `codex` on PATH: runs the newest codex.exe bundled with the Codex desktop app
rem (its folder under %LOCALAPPDATA%\OpenAI\Codex\bin is renamed on every app update).
setlocal
set "CX="
for /f "delims=" %%d in ('dir /b /ad /o-d "%LOCALAPPDATA%\OpenAI\Codex\bin" 2^>nul') do (
  if not defined CX if exist "%LOCALAPPDATA%\OpenAI\Codex\bin\%%d\codex.exe" set "CX=%LOCALAPPDATA%\OpenAI\Codex\bin\%%d\codex.exe"
)
if not defined CX (
  echo codex.exe not found under %%LOCALAPPDATA%%\OpenAI\Codex\bin - is the Codex app installed? 1>&2
  exit /b 1
)
endlocal & "%CX%" %*
