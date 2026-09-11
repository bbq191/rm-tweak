#!/bin/sh
# ═══════════════════════════════════════════════════════════════════════════
# cang-jie 全新设备统一安装器（host 侧编排，2026-09-11 新写）。
#
# 只编排、不重新实现任何构建/传输逻辑——依次调用十个已经各自独立可用的部署脚本：
#   packaging/deploy-chrony-cn.sh           国内 NTP（跟 xovi/vellum 无关）
#   packaging/deploy-chrony-boot-wakelock.sh 开机头几十秒防自动休眠打断 chronyd 首次校时（跟
#                                            xovi/vellum 无关；根因见该 .service 头注）
#   packaging/deploy-timezone-cn.sh         默认时区 Asia/Shanghai（跟 xovi/vellum 无关）
#   packaging/deploy-battop.sh              电池刺客（跟 xovi/vellum 无关）
#   packaging/deploy-xovi-persist.sh        xovi 开机持久化恢复链（需要 vellum add xovi）
#   packaging/deploy-hl-snap.sh             荧光笔 CJK 精确吸附（需要 vellum add xovi；只落盘）
#   packaging/deploy-handwriting-stroke.sh  CJK 手写笔迹渲染优化（需要 vellum add xovi；只落盘）
#   packaging/deploy-sidebar-entry.sh       Sidebar 一级直达 KOReader/WeRead 入口（需要
#                                           qt-resource-rebuilder；缺了自动跳过不阻塞；只落盘）
#   packaging/deploy.sh                     shelf 本体+网关+笔记线+两个领域服务（不需要 xovi）
#   packaging/deploy-xovi-apply.sh          统一跑一次 xovi/start，重注入上面落盘的全部内容
# 装前先过固件安全门（sha256(/usr/bin/xochitl) 比对 firmware-allowlist.txt），避免在没验证
# 过注入定位的固件上装错。
#
# ⚠️ xovi/start 只跑一次，放在最后：xovi/start 是全量重启 xochitl 重新扫描/注入 extensions.d
# 全部内容，没有"只重载一个扩展"的机制（系统增强线白皮书有过专门论证）。hl-snap/handwriting-
# stroke 各自的设备端 install.sh 本来会各自跑一次 xovi/start；这里改用 DEFER_XOVI_START=1
# 环境变量让它们只落盘、不重启（见各自脚本内 --no-restart 选项），全部落盘完只在
# deploy-xovi-apply.sh 这一步统一跑一次。原因：短时间内多次重启 xochitl 会撞它的
# watchdog+StartLimit，真机验证过触发过一次意外整机重启（2026-09-11，hl-snap 和
# handwriting-stroke 两步各自跑一次 xovi/start 时）。
#
# 明确不做的事（范围外，见 packaging/README.md「前置条件」「已知缺口」）：
#   · 不装 vellum/xovi/qt-resource-rebuilder/appload 本体、不侧载 KOReader——这些是全新设备
#     共同的手动前置条件，本脚本只在缺失时把报错原样透出，不代为安装。
#   · 不装中文化（输入法/候选栏/UI 汉化）——这条功能线未包含在本仓库
#     版本控制，需要单独手动跑。
#   · 不装 wifi-watch 常驻看护。
# 对称卸载见 packaging/uninstall-all.sh（2026-09-16 补）。
#
# 用法：./install-all.sh [host] [--force]
#     [--skip chrony-cn,chrony-boot-wakelock,timezone-cn,battop,xovi-persist,hl-snap,handwriting-stroke,sidebar-entry,shelf,xovi-apply]
#   host    默认 10.11.99.1（USB）
#   --force 固件不在白名单也强装（会自动把当前哈希追加进 firmware-allowlist.txt）
#   --skip  逗号分隔，跳过指定的安装步骤
# ═══════════════════════════════════════════════════════════════════════════
set -eu
cd "$(dirname "$0")"

HOST="${1:-10.11.99.1}"; [ $# -gt 0 ] && shift
FORCE=0
SKIP=""
for a in "$@"; do
    case "$a" in
        --force) FORCE=1 ;;
        --skip=*) SKIP="${a#--skip=}" ;;
        --skip) ;;
        *) if [ "${_prev:-}" = "--skip" ]; then SKIP="$a"; else echo "!! 未知参数：$a"; exit 2; fi ;;
    esac
    case "$a" in --skip) _prev="$a" ;; *) _prev="" ;; esac
done

skip_has() { case ",$SKIP," in *",$1,"*) return 0 ;; *) return 1 ;; esac; }

echo "═══ 固件安全门（root@$HOST）═══"
REMOTE_HASH="$(ssh "root@$HOST" 'sha256sum /usr/bin/xochitl' | awk '{print $1}')"
if [ -z "$REMOTE_HASH" ]; then
    echo "!! 没拿到 /usr/bin/xochitl 的 sha256（ssh 连不上，或设备上没有这个文件？）"
    exit 1
fi
if grep -q "^${REMOTE_HASH}[[:space:]]" firmware-allowlist.txt; then
    LABEL="$(awk -v h="$REMOTE_HASH" '$1==h{$1=""; print; exit}' firmware-allowlist.txt)"
    echo "-- 固件命中白名单：$LABEL"
elif [ "$FORCE" = "1" ]; then
    echo "⚠️  固件不在白名单（sha256=$REMOTE_HASH），--force 强装——追加进 firmware-allowlist.txt"
    echo "$REMOTE_HASH  (--force 追加，未验证，$(date +%Y-%m-%d))" >> firmware-allowlist.txt
else
    echo "!! 固件不在白名单（sha256=$REMOTE_HASH）。"
    echo "   这台设备的 xochitl 没有在这套安装脚本上验证过注入定位，qmd/hook 偏移可能对不上。"
    echo "   确认这台设备的固件确实跟已验证过的版本一致，要强装就加 --force（会自动记录这个哈希）。"
    exit 1
fi

INSTALLED=""
FAILED=""

run_step() {
    name="$1"; script="$2"
    if skip_has "$name"; then
        echo; echo "-- 跳过 $name（--skip）"
        return 0
    fi
    echo; echo "═══ $name ═══"
    if sh "$script" "$HOST"; then
        INSTALLED="$INSTALLED $name"
    else
        echo "!! $name 失败（见上面这一步的原始报错）"
        FAILED="$FAILED $name"
    fi
}

# 顺序：先两个跟 xovi/vellum 完全无关的独立配置项（早点做，出问题跟后面几步互不牵连）；
# 再 battop（同样跟 xovi 无关）；再三个依赖 xovi/vellum 已就绪的（xovi-persist 需要
# /home/root/xovi/start 存在，跟 hl-snap/handwriting-stroke 同一前提，放一起）；shelf 最重；
# xovi-apply 放最后，统一跑一次 xovi/start（见上面头注为什么不让每一步各自跑）。
run_step chrony-cn ./deploy-chrony-cn.sh
run_step chrony-boot-wakelock ./deploy-chrony-boot-wakelock.sh
run_step timezone-cn ./deploy-timezone-cn.sh
run_step battop ./deploy-battop.sh
run_step xovi-persist ./deploy-xovi-persist.sh
export DEFER_XOVI_START=1   # hl-snap/handwriting-stroke/sidebar-entry 只落盘，不各自触发 xochitl 重启
run_step hl-snap ./deploy-hl-snap.sh
run_step handwriting-stroke ./deploy-handwriting-stroke.sh
run_step sidebar-entry ./deploy-sidebar-entry.sh
unset DEFER_XOVI_START
run_step shelf ./deploy.sh
run_step xovi-apply ./deploy-xovi-apply.sh

echo
echo "═══════════════════════════════════════════════════════════"
echo "已安装：${INSTALLED:-（无）}"
if [ -n "$FAILED" ]; then
    echo "❌ 失败：$FAILED —— 看对应步骤上面的原始报错，不会自动重试"
fi
echo "─── 不在本脚本范围内，需要手动处理 ───"
echo "· vellum/xovi/qt-resource-rebuilder/appload 引导（若 xovi-persist/hl-snap/handwriting-stroke/"
echo "    xovi-apply 因缺 xovi.so 失败）：设备上先跑 vellum add xovi qt-resource-rebuilder"
echo "· KOReader：通过 appload 侧载，本脚本不代装"
echo "· WeRead（可选第三方 app）：本脚本不代装，需要自己下载官方发行包 SSH 装；装了的话"
echo "    sidebar-entry 这步会自动探测到、把 Sidebar 入口换成带 WeRead 的两项版本"
echo "· 中文化（输入法/候选栏/UI 汉化）：这条链路未包含在本仓库，"
echo "    没有回到 git 版本控制，需要去那边手动编译 + 跑 deploy/install.sh"
echo "· wifi-watch 常驻看护：这次没有一并恢复，见 README「已知缺口」"
echo "═══════════════════════════════════════════════════════════"
[ -z "$FAILED" ]
