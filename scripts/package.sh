#!/usr/bin/env bash
# 打一个可以直接发给别人的压缩包。
#
#   scripts/package.sh              # 编译 release 并打包
#   scripts/package.sh --no-build   # 复用已有产物，只重新打包
#   scripts/package.sh --clean      # 打包前清掉 dist/
#
# 产物：dist/TouchFish-<版本>-win64.zip
# 里面只有一个 exe 加一份 README 与样书——资源已经用 include_dir! 编进二进制了，
# 所以解压到任何目录双击就能跑，不需要运行库、不需要安装。

set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

# Git Bash 里 cargo 常常不在默认 PATH
if ! command -v cargo >/dev/null 2>&1; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "找不到 cargo，请先安装 Rust 或把 ~/.cargo/bin 加进 PATH" >&2
  exit 1
fi
if ! command -v python >/dev/null 2>&1; then
  echo "找不到 python（打包用它的 zipfile 模块压包）" >&2
  exit 1
fi

DO_BUILD=1
CLEAN=0
for arg in "$@"; do
  case "$arg" in
    --no-build) DO_BUILD=0 ;;
    --clean) CLEAN=1 ;;
    -h|--help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "未知参数：$arg" >&2; exit 2 ;;
  esac
done

# 版本号唯一来源：scripts/version.txt（构建时会被内嵌进可执行文件，见 src/main.rs）
VERSION="$(tr -d '[:space:]' < "$ROOT/scripts/version.txt")"
[ -n "$VERSION" ] || VERSION="0.0.0"

# 目标平台：默认按宿主机判断，交叉编译时用 TF_TARGET 覆盖
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) HOST_OS=windows ;;
  Darwin) HOST_OS=macos ;;
  *) HOST_OS=linux ;;
esac
TARGET_OS="${TF_TARGET:-$HOST_OS}"
case "$TARGET_OS" in
  windows) PLAT="win64"; EXE="touchfish.exe" ;;
  macos)   PLAT="macos"; EXE="touchfish" ;;
  *)       PLAT="linux64"; EXE="touchfish" ;;
esac

NAME="TouchFish-${VERSION}-${PLAT}"
STAGE="$ROOT/dist/$NAME"
ZIP="$ROOT/dist/$NAME.zip"
BIN="$ROOT/target/release/$EXE"

echo "▶ TouchFish $VERSION · $PLAT"

if [ "$DO_BUILD" = "1" ]; then
  echo "▶ cargo build --release"
  cargo build --release
fi
if [ ! -f "$BIN" ]; then
  echo "没有找到 $BIN，去掉 --no-build 重新来一次" >&2
  exit 1
fi

if [ "$CLEAN" = "1" ]; then
  rm -rf "$ROOT/dist"
fi
rm -rf "$STAGE"
mkdir -p "$STAGE"

cp "$BIN" "$STAGE/$EXE"
cp "$ROOT/README.md" "$STAGE/README.md"

# 样书是可选的：能让人解压后立刻点开看看，不用自己找一本书
if [ -d "$ROOT/samples" ]; then
  mkdir -p "$STAGE/samples"
  cp "$ROOT"/samples/*.epub "$ROOT"/samples/*.txt "$STAGE/samples/" 2>/dev/null || true
fi

cat > "$STAGE/VERSION.txt" <<EOF
TouchFish $VERSION
构建平台  $PLAT
构建时间  $(date "+%Y-%m-%d %H:%M:%S %z")
提交      $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo "（无 git 信息）")
EOF

rm -f "$ZIP"
python - "$STAGE" "$ZIP" <<'PY'
import os, sys, zipfile
stage, out = sys.argv[1], sys.argv[2]
base = os.path.dirname(stage)
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for root, _, files in os.walk(stage):
        for f in sorted(files):
            full = os.path.join(root, f)
            z.write(full, os.path.relpath(full, base))
print("封包完成：%d 个文件" % len(zipfile.ZipFile(out).namelist()))
PY

SIZE_ZIP=$(python -c "import os,sys;print('%.2f' % (os.path.getsize(sys.argv[1])/1048576))" "$ZIP")
SIZE_BIN=$(python -c "import os,sys;print('%.2f' % (os.path.getsize(sys.argv[1])/1048576))" "$BIN")
SUM=$(python -c "import hashlib,sys;print(hashlib.sha256(open(sys.argv[1],'rb').read()).hexdigest())" "$ZIP")

echo
echo "  可执行文件  $SIZE_BIN MB"
echo "  压缩包      $SIZE_ZIP MB"
echo "  SHA256      $SUM"
echo "  输出        dist/$NAME.zip"
echo
echo "  解压后双击 $EXE 即可运行；书放在解压目录之外也没问题，"
echo "  数据（书库、进度、设置）都在 %APPDATA%\\TouchFish（Linux 为 ~/.local/share/touch-fish）。"
