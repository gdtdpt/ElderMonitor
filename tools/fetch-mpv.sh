#!/usr/bin/env bash
# fetch-mpv.sh — 在 macOS 上准备 libmpv 到 vendor/mpv
#
# 用法：
#   tools/fetch-mpv.sh            # brew 安装/确认 mpv，并把 dylib 与头文件复制到 vendor/mpv
#   tools/fetch-mpv.sh /tmp/mpv   # 指定落盘目录
#
# 产物：
#   vendor/mpv/libmpv.2.dylib
#   vendor/mpv/include/mpv/*.h
#   vendor/mpv/VERSION.txt

set -euo pipefail

DEFAULT_DEST="$(cd "$(dirname "$0")/.." && pwd)/vendor/mpv"
DEST="${1:-$DEFAULT_DEST}"

if ! command -v brew >/dev/null 2>&1; then
  echo "[mpv] 需要 Homebrew：https://brew.sh" >&2
  exit 1
fi

if brew list --versions mpv >/dev/null 2>&1; then
  echo "[mpv] 已安装：$(brew list --versions mpv)"
else
  echo "[mpv] 安装 mpv（含 libmpv）..."
  brew install mpv
fi

PREFIX="$(brew --prefix mpv)"
echo "[mpv] brew prefix: $PREFIX"

mkdir -p "$DEST/include"
rm -rf "$DEST/include/mpv"
cp -R "$PREFIX/include/mpv" "$DEST/include/"

DYLIB=""
for cand in "$PREFIX/lib/libmpv.2.dylib" "$PREFIX/lib/libmpv.dylib"; do
  if [ -f "$cand" ]; then DYLIB="$cand"; break; fi
done

if [ -z "$DYLIB" ]; then
  echo "[mpv] 找不到 libmpv dylib，请检查 $PREFIX/lib" >&2
  exit 1
fi

cp -f "$DYLIB" "$DEST/libmpv.2.dylib"

# 修正内部 install name，保证打包后能按 @loader_path 找到
install_name_tool -id "@loader_path/libmpv.2.dylib" "$DEST/libmpv.2.dylib" 2>/dev/null || true

printf '%s\n' "$(brew list --versions mpv)" > "$DEST/VERSION.txt"

echo "[mpv] 完成：$DEST/libmpv.2.dylib"
echo "[mpv] 头文件：$DEST/include/mpv/client.h"
