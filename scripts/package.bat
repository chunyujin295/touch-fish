@echo off
chcp 65001 >nul
setlocal enabledelayedexpansion
cd /d "%~dp0.."
set "ROOT=%CD%"

rem 打一个可以直接发给别人的压缩包（Windows 一键版，不需要 Git Bash / Python）
rem
rem   scripts\package.bat              编译 release 并打包
rem   scripts\package.bat --no-build   复用已有产物，只重新打包
rem   scripts\package.bat --clean      打包前清掉 dist\

if /i "%~1"=="-h" goto usage
if /i "%~1"=="--help" goto usage

set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo >nul 2>nul
if errorlevel 1 (
  echo 找不到 cargo，请先安装 Rust 或把 %%USERPROFILE%%\.cargo\bin 加进 PATH。
  goto fail
)

set "VERSION="
for /f "tokens=2 delims== " %%v in ('findstr /b /c:"version" Cargo.toml') do set "VERSION=%%~v"
if not defined VERSION set "VERSION=0.0.0"

set "NAME=TouchFish-!VERSION!-win64"
set "STAGE=%ROOT%\dist\!NAME!"
set "ZIP=%ROOT%\dist\!NAME!.zip"
set "BIN=%ROOT%\target\release\touchfish.exe"

echo.
echo === TouchFish !VERSION! · win64 ===

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
  echo 没有找到 %BIN%，去掉 --no-build 重新跑一次。
  goto fail
)

if exist "%STAGE%" rmdir /s /q "%STAGE%"
mkdir "%STAGE%"

copy /y "%BIN%" "%STAGE%\touchfish.exe" >nul
if errorlevel 1 goto fail
copy /y "%ROOT%\README.md" "%STAGE%\README.md" >nul
if exist "%ROOT%\scripts\launch-web.bat" (
  copy /y "%ROOT%\scripts\launch-web.bat" "%STAGE%\启动网页版.bat" >nul
)

if exist "%ROOT%\samples" (
  mkdir "%STAGE%\samples" 2>nul
  copy /y "%ROOT%\samples\*.epub" "%STAGE%\samples\" >nul 2>nul
  copy /y "%ROOT%\samples\*.txt" "%STAGE%\samples\" >nul 2>nul
)

set "COMMIT="
for /f "delims=" %%c in ('git rev-parse --short HEAD 2^>nul') do set "COMMIT=%%c"
if not defined COMMIT set "COMMIT=(无 git 信息)"

> "%STAGE%\VERSION.txt" (
  echo TouchFish !VERSION!
  echo 构建平台  win64
  echo 构建时间  %DATE% %TIME%
  echo 提交      !COMMIT!
)

if exist "%ZIP%" del /q "%ZIP%"
echo.
echo ^> 压缩 dist\!NAME!.zip
powershell -NoProfile -ExecutionPolicy Bypass -Command "Add-Type -AssemblyName System.IO.Compression.FileSystem; if (Test-Path -LiteralPath '%ZIP%') { Remove-Item -LiteralPath '%ZIP%' -Force }; [System.IO.Compression.ZipFile]::CreateFromDirectory('%STAGE%', '%ZIP%', [System.IO.Compression.CompressionLevel]::Optimal, $true, [System.Text.Encoding]::UTF8)"
if errorlevel 1 goto fail
if not exist "%ZIP%" goto fail

for %%A in ("%ZIP%") do set "ZIPSIZE=%%~zA"
set "SHA="
for /f "delims=" %%h in ('powershell -NoProfile -Command "(Get-FileHash -LiteralPath '%ZIP%' -Algorithm SHA256).Hash"') do set "SHA=%%h"

echo.
echo   压缩包   dist\!NAME!.zip  ^(!ZIPSIZE! 字节^)
if defined SHA echo   SHA256   !SHA!
echo.
echo   解压后双击 touchfish.exe 即可运行；想用浏览器版就双击「启动网页版.bat」。
echo   数据（书库、进度、设置）在 %%APPDATA%%\TouchFish，删掉即彻底卸载。
if "%~1"=="" pause
exit /b 0

:fail
echo.
echo 打包失败。
if "%~1"=="" pause
exit /b 1

:usage
echo 用法：
echo   scripts\package.bat              编译 release 并打包
echo   scripts\package.bat --no-build   复用已有产物，只重新打包
echo   scripts\package.bat --clean      打包前清掉 dist\
exit /b 0
