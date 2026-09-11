#!/bin/sh
# 一键跑通前端可视渲染走查：编译 book-serve/ink-serve/note-serve/gateway（host debug 构建）→
# 隔离的临时 XDG 目录（绝不碰真实 ~/.config 等）→ 起四个服务 → 灌两本 fixture EPUB + 一份
# fixture 条目库（覆盖浏览/整理/回收站各状态，见 fixtures/ 两个生成脚本头注）→ 跑
# walk.mjs（Playwright 登录+中英文各切一遍 tab，全页截图）→ 无论成功失败都杀掉起的服务、
# 删临时目录。见同目录 README.md「已知局限」——这不是自动化断言测试，截图要人去看。
#
# 前置：`npm install`（装 playwright，见同目录 package.json）+ `npx playwright install
# chromium`（下载浏览器，一次性，不进 git）。
#
# 用法：./run.sh [截图输出目录]      默认 ./shots（相对本脚本所在目录）
set -eu
cd "$(dirname "$0")"
OUT="${1:-$(pwd)/shots}"
REPO_ROOT="$(cd ../../.. && pwd)"
PW="screenshot-walkthrough-pw-9527"

WORK="$(mktemp -d)"
export XDG_CONFIG_HOME="$WORK/config"
export XDG_DATA_HOME="$WORK/data"
export XDG_STATE_HOME="$WORK/state"
export XDG_RUNTIME_DIR="$WORK/runtime"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR"

PIDS=""
cleanup() {
    for p in $PIDS; do kill "$p" 2>/dev/null || true; done
    rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

echo "== 编译（host debug，快）=="
(cd "$REPO_ROOT/shelf" && cargo build --quiet -p book-serve)
(cd "$REPO_ROOT/notes" && cargo build --quiet -p ink-serve -p note-serve)
(cd "$REPO_ROOT/gateway" && cargo build --quiet)

echo "== 起服务（隔离临时 XDG 目录 $WORK）=="
"$REPO_ROOT/shelf/target/debug/book-serve" > "$WORK/book-serve.log" 2>&1 & PIDS="$PIDS $!"
"$REPO_ROOT/notes/target/debug/ink-serve" > "$WORK/ink-serve.log" 2>&1 & PIDS="$PIDS $!"
"$REPO_ROOT/notes/target/debug/note-serve" > "$WORK/note-serve.log" 2>&1 & PIDS="$PIDS $!"
sleep 1

echo "== 灌 fixture：两本占位 EPUB 走真实上传 API 入母版库 =="
python3 fixtures/make_fixture_epub.py "$WORK/book1.epub" "人骨拼图占位书名一" "作者甲" 4
python3 fixtures/make_fixture_epub.py "$WORK/book2.epub" "占位书名二·长标题测试用超长书名边界情况" "作者乙" 2
curl -sf -F "file=@$WORK/book1.epub" "http://127.0.0.1:8790/staging" >/dev/null
curl -sf -F "file=@$WORK/book2.epub" "http://127.0.0.1:8790/staging" >/dev/null

echo "== 灌 fixture：条目库直接落 ink-serve 状态目录（绕开 .rm 摄取管线，纯测试夹具）=="
mkdir -p "$XDG_STATE_HOME/notes/books"
python3 fixtures/make_fixture_book.py "$XDG_STATE_HOME/notes/books/screenshot-fixture-book-0001.json"

echo "== 起网关（先设一个非默认密码，跳过首登强制改密流程）=="
"$REPO_ROOT/gateway/target/debug/gateway" passwd "$PW"
"$REPO_ROOT/gateway/target/debug/gateway" serve --bind 127.0.0.1:8778 > "$WORK/gateway.log" 2>&1 & PIDS="$PIDS $!"
sleep 1

echo "== Playwright 走查 =="
WALK_PW="$PW" node walk.mjs "$OUT"

echo
echo "截图落在：$OUT（每张图靠人眼过一遍，这个脚本本身不判断「对不对」）"
