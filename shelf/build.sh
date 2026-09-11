#!/bin/sh
# 交叉编译书架全部服务的 aarch64 **全静态** 二进制（reMarkable Paper Pro Move）。
# 前置同 reading/device-rs/build.sh：rustup target add aarch64-unknown-linux-musl + aarch64 交叉 gcc。
# 链接器与 CC/AR 在 .cargo/config.toml。
set -e
cd "$(dirname "$0")"

TARGET=aarch64-unknown-linux-musl
BINS="book-serve koreader-serve"
# 网关（../gateway）+ 笔记线（../notes）+ enhance 的 wallpaper-serve/font-serve 都是独立
# 顶层 Cargo 项目，随书架一起编/装（目录不存在则跳过）；网关是 shelf/notes/enhance 三条线
# 共用的唯一前端，2026-09-11 从 shelf 内部 workspace 正名搬出去，见 ../gateway/README.md；
# wallpaper-serve/font-serve 同批从 shelf 内部 workspace 挪进 ../enhance/（概念上更贴近
# 系统增强），见 ../enhance/README.md。
GATEWAY_BINS="gateway"
ENHANCE_BINS="wallpaper-serve font-serve battop"
NOTES_BINS="ink-serve transcribe-serve mind-serve note-serve"

echo "== host 构建 + 测试 =="
cargo build --release --workspace
cargo test --workspace --quiet

echo "== 交叉编译 $TARGET（全静态）=="
cargo build --release --workspace --target "$TARGET"
if [ -f ../gateway/Cargo.toml ]; then
    echo "== 网关 gateway/：host 测试 + 交叉编译 =="
    (cd ../gateway && cargo test --quiet && cargo build --release --target "$TARGET")
fi
for b in $ENHANCE_BINS; do
    if [ -f "../enhance/$b/Cargo.toml" ]; then
        echo "== enhance/$b/：host 测试 + 交叉编译 =="
        (cd "../enhance/$b" && cargo test --quiet && cargo build --release --target "$TARGET")
    fi
done
if [ -f ../notes/Cargo.toml ]; then
    echo "== 笔记线 notes/：host 测试 + 交叉编译 =="
    (cd ../notes && cargo test --workspace --quiet && cargo build --release --workspace --target "$TARGET")
fi

echo
echo "aarch64 全静态产物："
for b in $BINS; do
    f="target/$TARGET/release/$b"
    [ -f "$f" ] && echo "  $f  $(wc -c <"$f")B  $(file "$f" | grep -o 'statically linked' || echo dynamic)"
done
for b in $GATEWAY_BINS; do
    f="../gateway/target/$TARGET/release/$b"
    [ -f "$f" ] && echo "  $f  $(wc -c <"$f")B  $(file "$f" | grep -o 'statically linked' || echo dynamic)"
done
for b in $ENHANCE_BINS; do
    f="../enhance/$b/target/$TARGET/release/$b"
    [ -f "$f" ] && echo "  $f  $(wc -c <"$f")B  $(file "$f" | grep -o 'statically linked' || echo dynamic)"
done
for b in $NOTES_BINS; do
    f="../notes/target/$TARGET/release/$b"
    [ -f "$f" ] && echo "  $f  $(wc -c <"$f")B  $(file "$f" | grep -o 'statically linked' || echo dynamic)"
done
