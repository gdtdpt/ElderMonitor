#!/usr/bin/env bash
# ElderMonitor 构建脚本（macOS）
#
# 用法：
#   bash tools/build.sh                # release 构建 + 测试 + 整理 dist/
#   bash tools/build.sh debug          # 快速 debug 构建
#   bash tools/build.sh release --no-test
#   bash tools/build.sh release 7734200   # 构建后跑冒烟（房间号）
#
# 为什么是 bash 而不是 pwsh：macOS 上不一定装了 PowerShell，
# 而 Windows 侧对应脚本是 tools/build.ps1（两者行为保持一致）。

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PROFILE="${1:-release}"
shift || true
SKIP_TEST=0
SMOKE=""

while [ $# -gt 0 ]; do
    case "$1" in
        --no-test) SKIP_TEST=1; shift ;;
        *) SMOKE="$1"; shift ;;
    esac
done

step() { printf '\033[36m[build] %s\033[0m\n' "$1"; }
fail() { printf '\033[31m[build] %s\033[0m\n' "$1"; exit 1; }

DYLIB="vendor/mpv/libmpv.2.dylib"
if [ ! -f "$DYLIB" ]; then
    echo "[build] 缺少 $DYLIB" >&2
    echo "        先跑一次：bash tools/fetch-mpv.sh" >&2
    exit 1
fi

if [ "$PROFILE" = "release" ]; then
    CARGO_ARGS=(build --release)
    OUT_DIR="target/release"
else
    CARGO_ARGS=(build)
    OUT_DIR="target/debug"
fi

step "cargo ${CARGO_ARGS[*]}"
cargo "${CARGO_ARGS[@]}" || fail "编译失败"

if [ "$SKIP_TEST" -eq 0 ]; then
    step "cargo test --workspace"
    cargo test --workspace --quiet || fail "测试失败"
fi

step "整理产物 → dist/"
mkdir -p dist
copied=0
for bin in em-smoke ElderMonitor; do
    if [ -f "$OUT_DIR/$bin" ]; then
        cp -f "$OUT_DIR/$bin" dist/
        copied=$((copied + 1))
    fi
done
[ "$copied" -gt 0 ] || fail "在 $OUT_DIR 里没找到任何可执行文件"

cp -f "$DYLIB" dist/
mkdir -p dist/config

step "产物："
ls -lh dist | tail -n +2

if [ -n "$SMOKE" ]; then
    step "冒烟：$SMOKE"
    ./dist/em-smoke "$SMOKE" || fail "冒烟未通过"
fi

step "完成"
