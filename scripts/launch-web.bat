@echo off
chcp 65001 >nul
setlocal
cd /d "%~dp0"

rem 打包版：touchfish.exe 和本脚本在同一层，双击即起服务并打开浏览器
if exist "touchfish.exe" (
  echo 正在启动网页版，浏览器会自动打开；退出请用网页里的「设置 - 退出 TouchFish」。
  start "" "touchfish.exe" --server --open
  goto check
)

rem 源码目录：先看有没有编译好的 release，省掉一次 cargo build
cd /d "%~dp0.."
if exist "target\release\touchfish.exe" (
  echo 正在启动网页版（release），浏览器会自动打开。
  start "" "target\release\touchfish.exe" --server --open
  goto check
)

rem 都没有才回到 cargo：调试构建，控制台留着，Ctrl+C 即可退出
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo >nul 2>nul
if errorlevel 1 (
  echo 没找到 touchfish.exe，也没找到 cargo。
  echo 请把本脚本和 touchfish.exe 放在同一目录，或先安装 Rust。
  pause
  exit /b 1
)
echo 没有现成的 exe，改用 cargo 启动（调试模式）。按 Ctrl+C 退出。
cargo run -- --server --open
exit /b 0

:check
rem 等它起来：进程已经退出通常意味着端口被占或 exe 起不来，别让窗口一闪而过
timeout /t 2 >nul
tasklist /fi "imagename eq touchfish.exe" | find /i "touchfish.exe" >nul
if errorlevel 1 (
  echo 启动失败：端口可能被别的程序占用，或 touchfish.exe 无法运行。
  pause
)
exit /b 0
