#!/bin/sh
# host 侧一键部署书架+网关+笔记线+两个 enhance 领域服务到设备。2026-09-11 从 shelf/deploy.sh
# 搬到这里——它编排的是跨 shelf/gateway/enhance/notes 四个目录的一整套安装，本质上是"全项目安装
# 编排"的一部分，该跟 packaging/ 放一起（shelf/build.sh、shelf/install.sh、shelf/uninstall.sh 没有跟着搬）。
#   组载荷（bin/ systemd/ lo-alias/ xovi/ install.sh uninstall.sh manifest.sh devlib.sh）→ tar → ssh 送到
#   设备暂存目录 → 校验完整后换位 → 设备端 install.sh。
# 用法：./deploy.sh [host] [install.sh 的参数…]      host 默认 10.11.99.1（第一个参数以 - 开头则视为 install.sh 参数、host 用默认）
#   例：./deploy.sh 10.11.99.1 --only font,wallpaper     ./deploy.sh 10.11.99.1 --password '新密码'
#   环境 SHELF_NO_BUILD=1 跳过交叉编译（直接用 target/ 里现成产物）
#
# 2026-09-20 改动（脚本审计 M9/M10）：
#  · 密码不再拼进远端命令行：--password 的值经 ssh 标准输入写进设备上 0600 的临时文件，install.sh 用
#    --password-file 读后即删——含空格/引号/分号的密码不会被远端 shell 解释，host 与设备的 ps 里也看不到；
#  · 其余参数逐个 shquote 后再传，不再 `$*` 裸拼；
#  · 载荷先在本地打成 tar 文件（打包失败当场退出），推到 shelf-pkg.new，校验有 install.sh 后才换掉 shelf-pkg——
#    传输中断不会把设备上现成的 shelf-pkg 清空又留下残缺载荷；
#  · HTTPS 探测只测无认证应 401，不再拿默认密码 shelf 试登录（会在网关上制造失败登录记录）。
# 2026-09-22 审计：
#  · 第一个参数是 --only 之类选项时不再被当成 host；-h/--help；动手（含耗时的交叉编译）前先确认 ssh 通；
#  · 要装的服务清单取自 ../shelf/manifest.sh（与设备端 install.sh 同一份），二进制缺失在**推送前**就报错并指路
#    `sh shelf/build.sh`（旧版静默跳过，传完 20MB 才被设备端拒绝）；
#  · 安装无论成败都清掉设备上可能残留的 shelf-pkg/.pw 密码临时文件；HTTPS 探测 curl 失败不再输出 "000000"。
set -eu
cd "$(dirname "$0")"
# shellcheck disable=SC1091
. ./lib.sh
# shellcheck disable=SC1091
. ../shelf/manifest.sh   # SHELF_ALL / shelf_svc_of / shelf_select：与设备端 install.sh 同一份服务清单与选择规则
TARGET=aarch64-unknown-linux-musl

usage() {
    cat <<'USAGE_EOF'
用法：./deploy.sh [host] [install.sh 的参数…]      host 默认 10.11.99.1
  例：./deploy.sh 10.11.99.1 --only font,wallpaper     ./deploy.sh 10.11.99.1 --password '新密码'
  install.sh 参数：--only a,b · --no-systemd · --password PW（经 stdin 传，不上命令行）
  环境：SHELF_NO_BUILD=1 跳过交叉编译（用 target/ 里现成产物）
USAGE_EOF
}
case "${1:-}" in -h|--help) usage; exit 0 ;; esac
case "${1:-}" in ""|-*) HOST="10.11.99.1" ;; *) HOST="$1"; shift ;; esac

# 服务令牌 → 交叉编译产物路径（这套映射与 shelf/build.sh 的产出目录一一对应；新增服务要在这里加一行）
bin_src() {
    b="$(shelf_svc_of "$1")"
    case "$1" in
        gateway) echo "../gateway/target/$TARGET/release/$b" ;;
        book|koreader) echo "../shelf/target/$TARGET/release/$b" ;;
        wallpaper|font) echo "../enhance/$b/target/$TARGET/release/$b" ;;
        ink|transcribe|mind|note) echo "../notes/target/$TARGET/release/$b" ;;
        *) echo "!! deploy.sh 不知道服务 $1 的产物位置（manifest.sh 新增了服务？更新本脚本的 bin_src）" >&2; return 1 ;;
    esac
}

# 拆出 --password（值走 stdin）与 --only（本机也要用它算载荷），其余参数原样保留（逐个 shquote）
PASSWORD=""; HAVE_PW=0; REMOTE_ARGS=""; ONLY=""; _prev=""
for a in "$@"; do
    if [ "$_prev" = "--password" ]; then PASSWORD="$a"; HAVE_PW=1; _prev=""; continue; fi
    if [ "$_prev" = "--only" ]; then ONLY="$a"; REMOTE_ARGS="$REMOTE_ARGS --only $(shquote "$a")"; _prev=""; continue; fi
    case "$a" in
        --password) _prev="--password" ;;
        --password=*) PASSWORD="${a#--password=}"; HAVE_PW=1 ;;
        --only) _prev="--only" ;;
        --only=*) ONLY="${a#--only=}"; REMOTE_ARGS="$REMOTE_ARGS $(shquote "$a")" ;;
        *) REMOTE_ARGS="$REMOTE_ARGS $(shquote "$a")" ;;
    esac
done
[ -z "$_prev" ] || { echo "!! $_prev 缺参数"; exit 2; }

# 要装的服务：与设备端 install.sh 同一个函数（manifest.sh 的 shelf_select）——--only 给的 + 网关（总会装）；缺省全装
SEL="$(shelf_select "$ONLY")" || exit 2

require_device
[ "${SHELF_NO_BUILD:-0}" = "1" ] || sh ../shelf/build.sh

# 推送前先核对：要装的服务的二进制都在（缺了指路去构建，别传完才被设备端拒绝）
MISSING=""
for s in $SEL; do
    f="$(bin_src "$s")" || exit 1
    [ -f "$f" ] || MISSING="$MISSING
   $f"
done
if [ -n "$MISSING" ]; then
    echo "!! 缺这些交叉编译产物（要装的服务：$(echo "$SEL" | sed 's/^ //')）：$MISSING"
    echo "   先在 shelf/ 下跑 sh build.sh（会一并编 gateway/notes/enhance），或用 --only 只装已编好的服务。"
    exit 1
fi

STAGE="$(mktemp -d)"; trap 'rm -rf "$STAGE"' EXIT
P="$STAGE/pkg/shelf"
mkdir -p "$P/bin" "$P/systemd" "$P/lo-alias" "$P/xovi"
for s in $SEL; do cp "$(bin_src "$s")" "$P/bin/"; done
cp ../shelf/systemd/* "$P/systemd/"
if [ -d ../gateway/systemd ]; then cp ../gateway/systemd/*.service "$P/systemd/"; fi
for b in wallpaper-serve font-serve; do
    if [ -f "../enhance/$b/$b.service" ]; then cp "../enhance/$b/$b.service" "$P/systemd/"; fi
done
if [ -d ../notes/systemd ]; then cp ../notes/systemd/*.service "$P/systemd/"; fi
cp ../enhance/lo-alias/lo-alias.sh "$P/lo-alias/"
cp ../shelf/install.sh ../shelf/uninstall.sh ../shelf/manifest.sh devlib.sh "$P/"
cp ../shelf/xovi/*.qmd "$P/xovi/"
tar -C "$STAGE/pkg" -cf "$STAGE/shelf-pkg.tar" shelf

REMOTE=/home/root/shelf-pkg
echo "-- 推送到 root@$HOST:$REMOTE/ 并安装"
rssh_in "rm -rf $REMOTE.new && mkdir -p $REMOTE.new && tar -C $REMOTE.new -xf - && [ -f $REMOTE.new/shelf/install.sh ] && rm -rf $REMOTE && mv $REMOTE.new $REMOTE" < "$STAGE/shelf-pkg.tar"
if [ "$HAVE_PW" = "1" ]; then
    printf '%s' "$PASSWORD" | rssh_in "umask 077; cat > $REMOTE/.pw"
    REMOTE_ARGS="$REMOTE_ARGS --password-file $REMOTE/.pw"
fi
# REMOTE_ARGS 每个词已 shquote，要在远端展开。install.sh 读完密码文件即删；它没跑起来（ssh 断了等）时这里兜底清，
# 不让密码明文留在设备上
# 兜底清理与安装同一次连接（设备端 install.sh 退出后无论成败都 rm；省一次 ssh）
rc=0
rssh "sh $REMOTE/shelf/install.sh$REMOTE_ARGS; rc=\$?; rm -f $REMOTE/.pw; exit \$rc" || rc=$?
[ "$rc" -eq 0 ] || { echo "!! 设备端 install.sh 退出码 $rc（见上面的输出；载荷仍在 $REMOTE，可 ssh 上去重跑 sh $REMOTE/shelf/install.sh）"; exit "$rc"; }
# host 侧 HTTPS 探测（设备 busybox wget 做不了自签）：无密码应 401
if command -v curl >/dev/null 2>&1; then
    code="$(curl -sk -o /dev/null -w '%{http_code}' --max-time 5 "https://$HOST/api/services" || true)"
    echo "-- HTTPS 探测：无密码 ${code:-000}（期望 401）；浏览器开 https://$HOST/ 登录（首次默认密码 shelf，登录后强制改）"
else
    echo "-- 本机没有 curl，跳过 HTTPS 探测；浏览器开 https://$HOST/"
fi
