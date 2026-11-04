@echo off
rem Build a distributable zip on Windows without Git Bash or Python.
rem
rem   scripts\package.bat              build release and package
rem   scripts\package.bat --no-build   reuse existing binary, repackage only
rem   scripts\package.bat --clean      wipe dist\ before packaging
rem
rem Keep this file ASCII-only and CRLF: cmd.exe misparses UTF-8 text with LF
rem line endings, and a Chinese rem/echo line can get cut into a bogus command.

setlocal enabledelayedexpansion
cd /d "%~dp0.."
set "ROOT=%CD%"

if /i "%~1"=="-h" goto usage
if /i "%~1"=="--help" goto usage

set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo >nul 2>nul
if errorlevel 1 (
  echo cargo not found. Install Rust, or add %%USERPROFILE%%\.cargo\bin to PATH.
  goto fail
)

rem Version comes from scripts\version.txt - the single source of truth.
set "VERSION="
if exist "%ROOT%\scripts\version.txt" set /p VERSION=<"%ROOT%\scripts\version.txt"
if not defined VERSION set "VERSION=0.0.0"

set "NAME=TouchFish-!VERSION!-win64"
set "STAGE=%ROOT%\dist\!NAME!"
set "ZIP=%ROOT%\dist\!NAME!.zip"
set "BIN=%ROOT%\target\release\touchfish.exe"

echo.
echo === TouchFish !VERSION! win64 ===

if /i "%~1"=="--clean" (
  if exist "%ROOT%\dist" rmdir /s /q "%ROOT%\dist"
)

if /i "%~1"=="--no-build" goto checkbin
echo.
echo ^> cargo build --release
cargo build --release
if errorlevel 1 goto fail

:checkbin
if not exist "%BIN%" (
  echo Binary not found: %BIN%
  echo Run without --no-build to build it first.
  goto fail
)

if exist "%STAGE%" rmdir /s /q "%STAGE%"
mkdir "%STAGE%"

copy /y "%BIN%" "%STAGE%\touchfish.exe" >nul
if errorlevel 1 goto fail
copy /y "%ROOT%\README.md" "%STAGE%\README.md" >nul

if exist "%ROOT%\samples" (
  mkdir "%STAGE%\samples" 2>nul
  copy /y "%ROOT%\samples\*.epub" "%STAGE%\samples\" >nul 2>nul
  copy /y "%ROOT%\samples\*.txt" "%STAGE%\samples\" >nul 2>nul
)

set "COMMIT="
for /f "delims=" %%c in ('git rev-parse --short HEAD 2^>nul') do set "COMMIT=%%c"
if not defined COMMIT set "COMMIT=(no git info)"

> "%STAGE%\VERSION.txt" (
  echo TouchFish !VERSION!
  echo Platform  win64
  echo Built     %DATE% %TIME%
  echo Commit    !COMMIT!
)

if exist "%ZIP%" del /q "%ZIP%"
echo.
echo ^> compress dist\!NAME!.zip
powershell -NoProfile -ExecutionPolicy Bypass -Command "Add-Type -AssemblyName System.IO.Compression.FileSystem; if (Test-Path -LiteralPath '%ZIP%') { Remove-Item -LiteralPath '%ZIP%' -Force }; [System.IO.Compression.ZipFile]::CreateFromDirectory('%STAGE%', '%ZIP%', [System.IO.Compression.CompressionLevel]::Optimal, $true, [System.Text.Encoding]::UTF8)"
if errorlevel 1 goto fail
if not exist "%ZIP%" goto fail

for %%A in ("%ZIP%") do set "ZIPSIZE=%%~zA"
set "SHA="
for /f "delims=" %%h in ('powershell -NoProfile -Command "(Get-FileHash -LiteralPath '%ZIP%' -Algorithm SHA256).Hash"') do set "SHA=%%h"

echo.
echo   archive  dist\!NAME!.zip  ^(!ZIPSIZE! bytes^)
if defined SHA echo   SHA256   !SHA!
echo.
echo   Unzip and run touchfish.exe. For the browser view use the tray menu
echo   ("open web page"). Data lives in %%APPDATA%%\TouchFish.
if "%~1"=="" pause
exit /b 0

:fail
echo.
echo Package failed.
if "%~1"=="" pause
exit /b 1

:usage
echo Usage:
echo   scripts\package.bat              build release and package
echo   scripts\package.bat --no-build   reuse existing binary, repackage only
echo   scripts\package.bat --clean      wipe dist\ before packaging
exit /b 0
