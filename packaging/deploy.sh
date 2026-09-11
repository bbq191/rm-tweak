#!/bin/sh
# host 侧一键部署书架+网关+笔记线+两个 enhance 领域服务到设备。2026-09-11 从 shelf/deploy.sh
# 搬到这里——它编排的是跨 shelf/gateway/enhance/notes 四个目录的一整套安装，本质上是"全项目安装
# 编排"的一部分，该跟 packaging/ 放一起，不该继续散在 shelf/ 下（shelf/build.sh、shelf/install.sh、
# shelf/uninstall.sh 没有跟着搬，见 packaging/README.md）。
#   组载荷（bin/ systemd/ lo-alias/ xovi/ install.sh uninstall.sh）→ tar-over-ssh → 设备端 install.sh。
# 用法：./deploy.sh [host] [install.sh 的参数…]      host 默认 10.11.99.1
#   环境 SHELF_NO_BUILD=1 跳过交叉编译（直接用 target/ 里现成产物）
set -eu
cd "$(dirname "$0")"
HOST="${1:-10.11.99.1}"; [ $# -gt 0 ] && shift
TARGET=aarch64-unknown-linux-musl
BINS="book-serve koreader-serve"
GATEWAY_BINS="gateway"   # 网关（../gateway）2026-09-11 正名搬顶层，二进制与单元一并打进载荷
ENHANCE_BINS="wallpaper-serve font-serve"   # 2026-09-11 从 shelf 挪进 ../enhance/，单元跟着各自目录走
NOTES_BINS="ink-serve transcribe-serve mind-serve note-serve"   # 笔记线（../notes）二进制与单元一并打进载荷

[ "${SHELF_NO_BUILD:-0}" = "1" ] || sh ../shelf/build.sh
STAGE="$(mktemp -d)"; trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/shelf/bin" "$STAGE/shelf/systemd" "$STAGE/shelf/lo-alias" "$STAGE/shelf/xovi"
for b in $BINS; do cp "../shelf/target/$TARGET/release/$b" "$STAGE/shelf/bin/"; done
cp ../shelf/systemd/* "$STAGE/shelf/systemd/"
for b in $GATEWAY_BINS; do
    [ -f "../gateway/target/$TARGET/release/$b" ] && cp "../gateway/target/$TARGET/release/$b" "$STAGE/shelf/bin/"
done
[ -d ../gateway/systemd ] && cp ../gateway/systemd/*.service "$STAGE/shelf/systemd/"
for b in $ENHANCE_BINS; do
    [ -f "../enhance/$b/target/$TARGET/release/$b" ] && cp "../enhance/$b/target/$TARGET/release/$b" "$STAGE/shelf/bin/"
    [ -f "../enhance/$b/$b.service" ] && cp "../enhance/$b/$b.service" "$STAGE/shelf/systemd/"
done
for b in $NOTES_BINS; do
    [ -f "../notes/target/$TARGET/release/$b" ] && cp "../notes/target/$TARGET/release/$b" "$STAGE/shelf/bin/"
done
[ -d ../notes/systemd ] && cp ../notes/systemd/*.service "$STAGE/shelf/systemd/"
cp ../enhance/lo-alias/lo-alias.sh "$STAGE/shelf/lo-alias/"
cp ../shelf/install.sh ../shelf/uninstall.sh "$STAGE/shelf/"
cp ../shelf/xovi/*.qmd "$STAGE/shelf/xovi/"
echo "-- 推送到 root@$HOST:/home/root/shelf-pkg/ 并安装"
tar -C "$STAGE" -cf - shelf | ssh "root@$HOST" 'rm -rf /home/root/shelf-pkg && mkdir -p /home/root/shelf-pkg && tar -C /home/root/shelf-pkg -xf -'
# shellcheck disable=SC2029  # 参数就是要在远端展开
ssh "root@$HOST" "sh /home/root/shelf-pkg/shelf/install.sh $*"
# host 侧 HTTPS 探测（设备 busybox wget 做不了自签）：无密码应 401；默认密码 shelf 若仍必改应 403（首登必改），否则 200
code="$(curl -sk -o /dev/null -w '%{http_code}' --max-time 5 "https://$HOST/api/services" || echo 000)"
ok="$(curl -sk -o /dev/null -w '%{http_code}' --max-time 5 -u "shelf:shelf" "https://$HOST/api/services" || echo 000)"
case "$ok" in
    403) echo "-- HTTPS 探测：无密码 $code（期望 401），默认密码 shelf → 403（首登必改）；浏览器开 https://$HOST/ 用 shelf 登录后设新密码" ;;
    200) echo "-- HTTPS 探测：无密码 $code（期望 401），默认密码仍可用但未强制改（异常，检查 gateway.json）" ;;
    *) echo "-- HTTPS 探测：无密码 $code（期望 401），默认密码 $ok（已自定义密码则为 401 正常）；浏览器开 https://$HOST/" ;;
esac
