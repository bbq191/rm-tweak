#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════
# 安装/卸载/部署脚本的本机模拟测试（不碰真机；2026-09-20 脚本审计后新增）。
#
# 做法：PATH 里放 tests/stubs/ 下的假 ssh/scp/systemctl/mount/dmsetup/id/sleep/curl/journalctl/rcc/python3，
# 临时目录当"设备"（$CJ_SIM_ROOT：home/root、usr/lib/systemd/system、proc/…）。假 ssh 把远端命令直接在本机沙箱里
# 执行，设备端脚本（devlib.sh / shelf/install.sh / 各 heredoc 脚本）因此跑的是**真代码**，只是 rootfs/systemd/
# mount 被桩住并写日志（$CJ_SIM_LOG），可断言"有没有 remount rw、最后一次 mount 是不是 ro、有没有 xovi/start"。
#
# 覆盖：devlib 各函数 · shelf install 幂等/缺载荷不留半成品/rw 窗口失败恢复 ro/只重启有变化的服务 ·
#       shelf uninstall 与 install 清单对称（含 mkdir-agent qmd、lo-alias.sh、shelf-uninstall、旧命名遗留、--only、--purge）·
#       deploy.sh（密码含特殊字符、shelf-pkg 换位）· hl-snap 部署（原子落位/备份不进 extensions.d/DEFER）·
#       xovi-apply 与 sidebar-entry 的 H1 判定（xovi 已生效 → restart，绝不 xovi/start）·
#       install-all → uninstall-all 整轮对称 · 静态守卫（remount,rw / xovi/start 只许出现在库里）。
# 一键：sh packaging/tests/run_sim_tests.sh     （也由 pytest 的 test_install_scripts_sim.py 调用，CI 会跑）
# ═══════════════════════════════════════════════════════════════════════════
set -u
# 保险：如果真是 root 在跑，拒绝（stub 的 id 会骗过脚本，而脚本里有 remount/rm 等真命令走假 PATH，但保守起见不冒险）
if [ "$(/usr/bin/id -u)" = "0" ] && [ -z "${CJ_SIM_ALLOW_ROOT:-}" ]; then
    echo "拒绝以 root 跑模拟测试（设 CJ_SIM_ALLOW_ROOT=1 强行跑）"; exit 2
fi
REAL_HOME="$HOME"
# 所有 XDG_* 一律指进沙箱（脚本会读它们；不设的话 install/uninstall --purge 会碰到真实用户目录——
# 2026-09-20 首版就踩过：--purge 测试清了开发机的 ~/.config/shelf 等，见 new_sandbox 里的导出与末尾的"真实 HOME 未被触碰"守卫）
guard_paths() { echo "$REAL_HOME/.config/shelf $REAL_HOME/.local/share/shelf $REAL_HOME/.local/state/shelf $REAL_HOME/.local/lib/shelf $REAL_HOME/cangjie-backups $REAL_HOME/.local/bin/shelf-uninstall $REAL_HOME/.cangjie-stage $REAL_HOME/.cangjie-pending-apply /run/cangjie-pending-apply"; }
GUARD_BEFORE=""
for g in $(guard_paths); do [ -e "$g" ] && GUARD_BEFORE="$GUARD_BEFORE $g"; done
HERE="$(cd "$(dirname "$0")" && pwd)"
PKG="$(cd "$HERE/.." && pwd)"
REPO="$(cd "$PKG/.." && pwd)"
STUBS="$HERE/stubs"
TMPBASE="$(mktemp -d)"
CREATED_FILES="$TMPBASE/created-files.txt"; : > "$CREATED_FILES"
cleanup() {
    while read -r f; do rm -f "$f"; rmdir -p "$(dirname "$f")" 2>/dev/null; done < "$CREATED_FILES"
    # 还原被挪走的真交叉编译产物（见 mk_dummy_targets）
    if [ -f "$TMPBASE/real-targets.txt" ]; then
        while read -r f; do mkdir -p "$(dirname "$REPO/$f")"; mv "$TMPBASE/real-targets/$f" "$REPO/$f"; done < "$TMPBASE/real-targets.txt"
    fi
    rm -rf "$TMPBASE"
}
trap cleanup EXIT

PASS=0; FAIL=0
ok()   { PASS=$((PASS + 1)); echo "  ok   $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  FAIL $1"; }
check() { # 描述 命令…（命令成功=通过）
    d="$1"; shift
    if "$@" >/dev/null 2>&1; then ok "$d"; else bad "$d"; fi
}
sig_eq() { # 描述 期望sig 实际sig：不等时打印 diff（便于定位）
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1"; diff <(printf '%s\n' "$2") <(printf '%s\n' "$3") | head -20 | sed 's/^/       /'; fi
}
section() { echo; echo "── $1"; }

# ── 沙箱 ──
new_sandbox() {
    R="$(mktemp -d "$TMPBASE/sim.XXXXXX")"
    export CJ_SIM_ROOT="$R" CJ_SIM_LOG="$R/log.txt"
    mkdir -p "$R/home/root/xovi/extensions.d" "$R/home/root/xovi/exthome/qt-resource-rebuilder" "$R/home/root/xovi/exthome/appload" \
             "$R/home/root/.local/bin" "$R/usr/lib/systemd/system" "$R/proc/4242" "$R/usr/bin" "$R/tmp/shelf-0/shelf/services"
    : > "$CJ_SIM_LOG"
    B="$R/home/root/.local/bin"; Q="$R/home/root/xovi/exthome/qt-resource-rebuilder"
    : > "$R/home/root/xovi/xovi.so"
    printf '#!/bin/sh\necho XOVI_START >> "%s"\n' "$CJ_SIM_LOG" > "$R/home/root/xovi/start"; chmod +x "$R/home/root/xovi/start"
    echo "fake xochitl $$" > "$R/usr/bin/xochitl"
    echo '{}' > "$R/tmp/shelf-0/shelf/services/book.json"
    xovi_live off
    export HOME="$R/home/root" XDG_CONFIG_HOME="$R/home/root/.config" XDG_DATA_HOME="$R/home/root/.local/share" XDG_STATE_HOME="$R/home/root/.local/state" XDG_CACHE_HOME="$R/home/root/.cache" SHELF_REG_DIR="$R/tmp/shelf-0/shelf/services" CJ_SYSD="$R/usr/lib/systemd/system" CJ_PROC="$R/proc"
    export CJ_APPLY_GRACE=0 CJ_HEALTH_SLEEP=0 CJ_RETRY_SLEEP=0 CJ_APPLY_VERIFY=0   # 部署后等重启+自动核对另有专门用例
    # 待生效标记目录进沙箱（默认 /run/cangjie-pending-apply 是真实系统路径）
    export CJ_PENDING_DIR="$R/run/cangjie-pending"
    unset CJ_SIM_VERITY CJ_SIM_RW_FAIL CJ_SIM_INACTIVE CJ_SIM_SCP_CORRUPT CJ_HOME CJ_XOVI CJ_BACKUP_DIR CJ_BACKUP_KEEP CJ_STAGE_DIR CJ_PENDING_FALLBACK
}
# xovi_live on|off：模拟 xochitl 进程里有/没有 LD_PRELOAD=xovi.so，及 maps 里有无扩展
xovi_live() {
    if [ "$1" = on ]; then
        printf 'FOO=1\0LD_PRELOAD=/home/root/xovi/xovi.so\0XOVI_ROOT=/x\0' > "$R/proc/4242/environ"
        printf '7f00 r-xp /home/root/xovi/xovi.so\n7f01 r-xp /home/root/xovi/extensions.d/hl-snap.so\n7f02 r-xp hw-stroke.so\n' > "$R/proc/4242/maps"
    else
        printf 'FOO=1\0' > "$R/proc/4242/environ"
        printf '7f00 r-xp /home/root/xovi/xovi.so\n7f01 r-xp hl-snap.so\n7f02 r-xp hw-stroke.so\n' > "$R/proc/4242/maps"
    fi
}
run() { PATH="$STUBS:$PATH" "$@"; }
count_log() { grep -c -- "$1" "$CJ_SIM_LOG" 2>/dev/null; }
# 设备文件树签名：只看常规文件与符号链接（路径 + 内容 md5 / 链接目标），排除 proc/日志/备份/用户数据
tree_sig() {
    ( cd "$R" && find ./home ./usr \( -type f -o -type l \) 2>/dev/null | sort | grep -v -e '^./home/root/cangjie-backups/' | while read -r f; do
        if [ -L "$f" ]; then echo "L $f -> $(readlink "$f")"; else echo "F $f $(md5sum < "$f" | cut -c1-32)"; fi
      done )
}
last_mount() { grep '^mount ' "$CJ_SIM_LOG" | tail -n 1; }

# ── shelf 载荷 ──
SVCS="gateway book koreader font wallpaper ink transcribe mind note"
svc_of() { [ "$1" = gateway ] && echo gateway || echo "$1-serve"; }
mk_payload() { # DIR
    P="$1"; mkdir -p "$P/bin" "$P/systemd" "$P/lo-alias" "$P/xovi"
    for s in $SVCS; do
        b="$(svc_of "$s")"
        printf '#!/bin/sh\necho "%s $*" >> "%s"\n' "$b" "$CJ_SIM_LOG" > "$P/bin/$b"; chmod +x "$P/bin/$b"
        printf '[Unit]\nDescription=%s\n[Service]\nExecStart=/x/%s\n[Install]\nWantedBy=shelf.target\n' "$b" "$b" > "$P/systemd/$b.service"
    done
    printf '[Unit]\nDescription=t\n[Install]\nWantedBy=multi-user.target\n' > "$P/systemd/shelf.target"
    echo '#!/bin/sh' > "$P/lo-alias/lo-alias.sh"
    for q in font-menu-dynamic.qmd font-menu-dynamic-3.27.qmd shelf-trash-agent.qmd shelf-mkdir-agent.qmd shelf-comic-margins.qmd reader-page-turn.qmd; do echo "qmd $q v1" > "$P/xovi/$q"; done
    cp "$REPO/shelf/install.sh" "$REPO/shelf/uninstall.sh" "$REPO/shelf/manifest.sh" "$PKG/devlib.sh" "$P/"
}

# ═══════════════════════════ 1. devlib 单元 ═══════════════════════════
section "devlib.sh：rw 窗口 / 原子替换 / 备份轮转 / xochitl 判定"
new_sandbox
# shellcheck disable=SC1091
. "$PKG/devlib.sh"
export PATH="$STUBS:$PATH"

: > "$CJ_SIM_LOG"; body_fail() { return 1; }; body_ok() { return 0; }
cj_with_rootfs_rw body_fail; rc=$?
check "with_rootfs_rw：body 失败 → 返回非 0" test "$rc" -ne 0
check "with_rootfs_rw：body 失败后最后一次 mount 是 remount,ro" test "$(last_mount)" = "mount -o remount,ro /"
: > "$CJ_SIM_LOG"; cj_with_rootfs_rw body_ok; rc=$?
check "with_rootfs_rw：body 成功 → 0 且最后 mount 是 ro" test "$rc" -eq 0 -a "$(last_mount)" = "mount -o remount,ro /"
: > "$CJ_SIM_LOG"; CJ_SIM_RW_FAIL=1 cj_with_rootfs_rw body_ok; rc=$?
check "with_rootfs_rw：remount rw 失败 → 非 0、不跑 body、无 ro 恢复调用" test "$rc" -ne 0 -a "$(count_log 'remount,ro')" = 0
# body 被信号杀死（模拟中途死亡）：仍恢复 ro，返回非 0
: > "$CJ_SIM_LOG"
body_kill() { kill -KILL "$BASHPID"; }
cj_with_rootfs_rw body_kill >/dev/null 2>&1; rc=$?
check "with_rootfs_rw：body 被 SIGKILL → 非 0，且仍恢复 ro" test "$rc" -ne 0 -a "$(last_mount)" = "mount -o remount,ro /"
# 调用方 shell 自己在 rw 窗口里收到 SIGPIPE（ssh 断开后写输出）：仍恢复 ro
: > "$CJ_SIM_LOG"
sh -c ". '$PKG/devlib.sh'; body_pipe() { kill -PIPE \$\$; }; cj_with_rootfs_rw body_pipe" >/dev/null 2>&1
check "with_rootfs_rw：调用方 shell 收到 SIGPIPE → 仍恢复 ro" test "$(last_mount)" = "mount -o remount,ro /"

# install_usr_unit：源缺失 → 不 remount；verity → 3 且不 remount；成功 → wants 链接；幂等
: > "$CJ_SIM_LOG"; cj_install_usr_unit x.service "$R/nope" multi-user.target.wants; rc=$?
check "install_usr_unit：源缺失 → 1，且从未 remount rw" test "$rc" -eq 1 -a "$(count_log 'remount,rw')" = 0
echo '[Unit]' > "$R/x.service.src"
: > "$CJ_SIM_LOG"; CJ_SIM_VERITY=1 cj_install_usr_unit x.service "$R/x.service.src" multi-user.target.wants; rc=$?
check "install_usr_unit：dm-verity 激活 → 3，不 remount" test "$rc" -eq 3 -a "$(count_log 'remount')" = 0
: > "$CJ_SIM_LOG"; cj_install_usr_unit x.service "$R/x.service.src" multi-user.target.wants >/dev/null; rc=$?
check "install_usr_unit：成功装单元+wants 链接" test "$rc" -eq 0 -a -f "$CJ_SYSD/x.service" -a -L "$CJ_SYSD/multi-user.target.wants/x.service"
: > "$CJ_SIM_LOG"; cj_install_usr_unit x.service "$R/x.service.src" multi-user.target.wants >/dev/null
check "install_usr_unit：再装一次 → 幂等，不 remount" test "$(count_log 'remount')" = 0
cj_remove_usr_unit x.service multi-user.target.wants >/dev/null
check "remove_usr_unit：单元与链接都删了" test ! -e "$CJ_SYSD/x.service" -a ! -L "$CJ_SYSD/multi-user.target.wants/x.service"
cj_install_usr_unit y.service "$R/x.service.src" - >/dev/null
check "install_usr_unit：wants='-' 不建开机链接" test -f "$CJ_SYSD/y.service" -a ! -e "$CJ_SYSD/multi-user.target.wants/y.service"

# safe_replace：内容、无残留、失败保留旧文件、相同内容不动
echo new > "$R/src.bin"; echo old > "$R/dst.bin"
cj_safe_replace "$R/src.bin" "$R/dst.bin" "$R/stg" 755
check "safe_replace：内容替换 + REPLACED=1" test "$(cat "$R/dst.bin")" = new -a "$CJ_REPLACED" = 1
check "safe_replace：暂存目录里无残留临时文件" test -z "$(ls -A "$R/stg")"
cj_safe_replace "$R/src.bin" "$R/dst.bin" "$R/stg" 755
check "safe_replace：内容相同 → REPLACED=0" test "$CJ_REPLACED" = 0
cj_safe_replace "$R/none" "$R/dst.bin" "$R/stg" 755 >/dev/null; rc=$?
check "safe_replace：源不存在 → 1，旧文件不变" test "$rc" -eq 1 -a "$(cat "$R/dst.bin")" = new

# 备份轮转：只删自己生成的严格时间戳命名、保留最近 N 份、不动手工命名/大文件
export CJ_BACKUP_DIR="$R/bk" CJ_BACKUP_KEEP=3 CJ_BACKUP_MAXBYTES=100
mkdir -p "$R/bk"
for i in 1 2 3 4 5; do echo "v$i" > "$R/bk/foo.so.bak.pre-2026010$i-000000"; done
echo manual > "$R/bk/foo.so.bak.pre-manual"
echo other > "$R/bk/bar.so.bak.pre-20260101-000000"
head -c 500 /dev/zero > "$R/bk/foo.so.bak.pre-20250101-000000"   # 最老，但超过 MAXBYTES → 不动
mkdir "$R/bk/keepdir"; echo u > "$R/bk/keepdir/userdata"
echo x > "$R/foo.so"; cj_backup_file "$R/foo.so" >/dev/null
left="$(find "$R/bk" -maxdepth 1 -name 'foo.so.bak.pre-????????-??????' | wc -l)"
check "轮转：foo.so 的时间戳备份只剩 KEEP=3 份 + 1 份超大不动" test "$left" -eq 4
check "轮转：手工命名/别的文件/其它目录/用户数据 全部保留" test -f "$R/bk/foo.so.bak.pre-manual" -a -f "$R/bk/bar.so.bak.pre-20260101-000000" -a -f "$R/bk/keepdir/userdata"
check "轮转：超过大小上限的旧备份不被删" test -f "$R/bk/foo.so.bak.pre-20250101-000000"
check "轮转：最老的小备份被删、最新的保留" test ! -f "$R/bk/foo.so.bak.pre-20260101-000000" -a -f "$R/bk/foo.so.bak.pre-20260105-000000"
# 目录型备份轮转：只删"里面全是常规文件"的
for i in 1 2 3 4 5; do mkdir "$R/bk/shelf-2026020$i-000000"; echo b > "$R/bk/shelf-2026020$i-000000/f"; done
mkdir "$R/bk/shelf-20260101-000000"; mkdir "$R/bk/shelf-20260101-000000/sub"   # 含子目录 → 不动
cj_bk_prune shelf- d
n="$(find "$R/bk" -maxdepth 1 -name 'shelf-????????-??????' | wc -l)"
check "目录轮转：保留 KEEP 份 + 含子目录的那份不动" test "$n" -eq 4 -a -d "$R/bk/shelf-20260101-000000/sub"
unset CJ_BACKUP_DIR CJ_BACKUP_KEEP CJ_BACKUP_MAXBYTES

# xochitl_apply 的判定（H1 + 2026-09-25 一律整机重启）
: > "$CJ_SIM_LOG"; xovi_live on; out="$(cj_xochitl_apply 2>&1)"
check "xochitl_apply：xovi 已生效 → systemctl reboot，绝不停/重启 xochitl、绝不 xovi/start" test "$(count_log 'systemctl reboot')" = 1 -a -z "$(grep -E 'systemctl (stop|start|restart) xochitl' "$CJ_SIM_LOG")" -a "$(count_log XOVI_START)" = 0
check "xochitl_apply：重启前打印\"打断阅读\"提示，并打印 CJ-APPLY-REBOOTING 给 host 侧认" test -n "$(printf '%s' "$out" | grep '打断')" -a -n "$(printf '%s' "$out" | grep -x 'CJ-APPLY-REBOOTING')"
: > "$CJ_SIM_LOG"; xovi_live off; : > "$CJ_SYSD/xovi-reenable.service"; cj_xochitl_apply >/dev/null 2>&1
check "xochitl_apply：xovi 没生效但装了 xovi-reenable → 也是整机重启（开机自动恢复 xovi），不 xovi/start" test "$(count_log 'systemctl reboot')" = 1 -a "$(count_log XOVI_START)" = 0
rm -f "$CJ_SYSD/xovi-reenable.service"
: > "$CJ_SIM_LOG"; xovi_live off; cj_xochitl_apply >/dev/null 2>&1
check "xochitl_apply：xovi 没生效且没装 xovi-reenable → 才跑 xovi/start" test "$(count_log XOVI_START)" = 1 -a "$(count_log 'systemctl reboot')" = 0 -a "$(count_log 'systemctl restart xochitl')" = 0
mkdir -p "$CJ_PENDING_FALLBACK"; : > "$CJ_PENDING_FALLBACK/hl-snap"   # 退路目录（/home，重启不清）里有标记
: > "$CJ_SIM_LOG"; xovi_live on; cj_xochitl_apply >/dev/null 2>&1
check "xochitl_apply：整机重启前清掉 /home 退路目录里的待生效标记（否则设备回来又判\"待生效\"→ 重启循环）" test -z "$(ls -A "$CJ_PENDING_FALLBACK" 2>/dev/null)"
rm "$HOME/xovi/start"; xovi_live off; cj_xochitl_apply >/dev/null 2>&1; rc=$?
check "xochitl_apply：没有 xovi/start 也没 xovi → 失败而不是乱重启" test "$rc" -ne 0
xovi_live on
check "count_maps：只输出一个整数（grep -c 为 0 时不出 '0\\n0'）" test "$(cj_count_maps nonexistent-tag 4242)" = 0
check "count_maps：有匹配" test "$(cj_count_maps hl-snap 4242)" -ge 1

# 待生效标记（A1）
rm -rf "$CJ_PENDING_DIR"
check "pending：初始无标记" test -z "$(cj_pending_list)"
cj_pending_mark hl-snap; cj_pending_mark shelf-qmd
check "pending：mark 两次后 list 有两项" test "$(cj_pending_list | sort | tr '\n' ' ')" = "hl-snap shelf-qmd "
cj_pending_clear
check "pending：clear 后为空且标记目录已删" test -z "$(cj_pending_list)" -a ! -e "$CJ_PENDING_DIR"
mkdir -p "$R/ro"; chmod 500 "$R/ro"; OLD_PD="$CJ_PENDING_DIR"; CJ_PENDING_DIR="$R/ro/x/y"
cj_pending_mark fb-test >/dev/null
check "pending：主目录写不了 → 落退路目录，list 仍能看到（宁可多重启也不能漏）" test "$(cj_pending_list)" = fb-test -a -f "$CJ_PENDING_FALLBACK/fb-test"
cj_pending_clear; CJ_PENDING_DIR="$OLD_PD"; chmod 700 "$R/ro"
check "pending：clear 也清退路目录" test ! -e "$CJ_PENDING_FALLBACK"

# 内容没变不备份 / 载荷目录清理 / 暂存清理 / 卸载单元的 verity 宽松语义
CJ_BACKUP_DIR="$R/bk2"; CJ_BACKUP_KEEP=5; CJ_BACKUP_MAXBYTES=67108864   # 上面的轮转测试 unset 过这几个变量
echo a > "$R/s1"; echo a > "$R/d1"; cj_backup_if_differs "$R/s1" "$R/d1"
check "backup_if_differs：内容相同 → 不备份" test -z "$(ls -A "$R/bk2" 2>/dev/null)"
echo b > "$R/s1"; cj_backup_if_differs "$R/s1" "$R/d1" >/dev/null
check "backup_if_differs：内容不同 → 备份了旧内容" test "$(cat "$R"/bk2/d1.bak.pre-* 2>/dev/null)" = a
cj_backup_if_differs "$R/s1" "$R/not-exist"
check "backup_if_differs：目标不存在 → 无事可做" test "$(ls "$R/bk2" | wc -l)" -eq 1
CJ_BACKUP_DIR="$CJ_HOME/cangjie-backups"

mkdir -p "$CJ_HOME/pkg-x/deploy"; echo 1 > "$CJ_HOME/pkg-x/a.so"; echo 1 > "$CJ_HOME/pkg-x/deploy/i.sh"; echo keep > "$CJ_HOME/pkg-x/mine.txt"
cj_rm_payload "$CJ_HOME/pkg-x" a.so deploy/i.sh deploy
check "rm_payload：只删已知文件；目录里有别的东西 → 目录与别的文件都保留" test ! -e "$CJ_HOME/pkg-x/a.so" -a ! -e "$CJ_HOME/pkg-x/deploy" -a -f "$CJ_HOME/pkg-x/mine.txt"
rm "$CJ_HOME/pkg-x/mine.txt"; cj_rm_payload "$CJ_HOME/pkg-x" a.so
check "rm_payload：全是已知文件 → 目录也删" test ! -e "$CJ_HOME/pkg-x"
mkdir -p "$R/outside"; echo x > "$R/outside/f"
cj_rm_payload "$R/outside" f >/dev/null; rc=$?
check "rm_payload：$CJ_HOME 之外的路径 → 拒绝（返回 1），文件不动" test "$rc" -eq 1 -a -f "$R/outside/f"
ln -s "$R/outside" "$CJ_HOME/lnk"; cj_rm_payload "$CJ_HOME/lnk" f >/dev/null; rc=$?
check "rm_payload：目标是符号链接 → 拒绝，链接指向的内容不动" test "$rc" -eq 1 -a -f "$R/outside/f"
rm -f "$CJ_HOME/lnk"

mkdir -p "$CJ_STAGE_DIR"; cj_stage_cleanup
check "stage_cleanup：空暂存目录被删" test ! -e "$CJ_STAGE_DIR"
mkdir -p "$CJ_STAGE_DIR"; : > "$CJ_STAGE_DIR/x"; cj_stage_cleanup
check "stage_cleanup：暂存目录非空 → 保留" test -f "$CJ_STAGE_DIR/x"
rm -rf "$CJ_STAGE_DIR"

cj_install_usr_unit v.service "$R/x.service.src" multi-user.target.wants >/dev/null
CJ_SIM_VERITY=1 cj_uninstall_usr_unit v.service multi-user.target.wants >"$R/o.txt" 2>&1; rc=$?
check "uninstall_usr_unit：dm-verity 激活 → 视为非失败(0)、单元仍在、如实说明重启后可能被拉起" test "$rc" -eq 0 -a -f "$CJ_SYSD/v.service" -a -n "$(grep '重启设备后' "$R/o.txt")"
cj_uninstall_usr_unit v.service multi-user.target.wants >/dev/null; rc=$?
check "uninstall_usr_unit：正常 → 0，单元与链接都删了" test "$rc" -eq 0 -a ! -e "$CJ_SYSD/v.service" -a ! -L "$CJ_SYSD/multi-user.target.wants/v.service"

# ═══════════════════════════ 2. shelf install / uninstall ═══════════════════════════
trap cleanup EXIT   # devlib 的 cj_with_rootfs_rw 会清掉 EXIT trap（被 source 进本 shell 时），这里重新挂上
section "shelf/install.sh：幂等 / 缺载荷不留半成品 / rw 窗口失败恢复 ro"
new_sandbox
PL="$R/payload"; mk_payload "$PL"
PRE_SIG="$(tree_sig)"
: > "$CJ_SIM_LOG"
run sh "$PL/install.sh" --no-systemd >/dev/null 2>&1
check "install --no-systemd：不 remount、不写 /usr" test "$(count_log 'remount')" = 0 -a -z "$(ls -A "$CJ_SYSD")"
new_sandbox; PL="$R/payload"; mk_payload "$PL"; PRE_SIG="$(tree_sig)"
: > "$CJ_SIM_LOG"
run sh "$PL/install.sh" >"$R/out1.txt" 2>&1; rc=$?
check "install 全量：退出 0" test "$rc" -eq 0
check "install：9 个服务二进制都在" test -x "$B/gateway" -a -x "$B/book-serve" -a -x "$B/note-serve" -a -x "$B/wallpaper-serve"
check "install：辅助脚本 lo-alias.sh / shelf-uninstall / 库 已装" test -x "$B/lo-alias.sh" -a -x "$B/shelf-uninstall" -a -f "$R/home/root/.local/lib/shelf/manifest.sh" -a -f "$R/home/root/.local/lib/shelf/devlib.sh"
check "install：单元 + shelf.target + wants 链接" test -f "$CJ_SYSD/gateway.service" -a -L "$CJ_SYSD/shelf.target.wants/book-serve.service" -a -L "$CJ_SYSD/multi-user.target.wants/shelf.target"
check "install：五个 qmd（字体/回收站/建夹/漫画页边距/阅读器翻页）都在 qrr 目录" test -f "$Q/font-menu-dynamic.qmd" -a -f "$Q/shelf-trash-agent.qmd" -a -f "$Q/shelf-mkdir-agent.qmd" -a -f "$Q/shelf-comic-margins.qmd" -a -f "$Q/reader-page-turn.qmd"
check "install：rw 窗口只开一次、最后一次 mount 是 ro" test "$(count_log 'remount,rw')" = 1 -a "$(last_mount)" = "mount -o remount,ro /"
check "install：不跑 xovi/start、不重启 xochitl（只打印提示）" test "$(count_log XOVI_START)" = 0 -a "$(count_log 'restart xochitl')" = 0
check "install：qmd 生效提示指路整机重启（deploy-xovi-apply.sh / reboot），不教 systemctl restart xochitl" test -n "$(grep '生效需整机重启' "$R/out1.txt")" -a -z "$(grep 'systemctl restart xochitl' "$R/out1.txt")"
check "install：qmd 有变化 → 记了待生效标记 shelf-qmd（A1）" test -f "$CJ_PENDING_DIR/shelf-qmd"
check "install：暂存目录已清" test ! -e "$R/home/root/.cangjie-stage"
SIG1="$(tree_sig)"
: > "$CJ_SIM_LOG"; run sh "$PL/install.sh" >"$R/out2.txt" 2>&1; rc=$?
check "install 第二次：退出 0" test "$rc" -eq 0
sig_eq "install 第二次：文件树逐字节不变（幂等）" "$SIG1" "$(tree_sig)"
check "install 第二次：不 remount、不重启任何服务、不留空备份目录" test "$(count_log 'remount')" = 0 -a "$(count_log 'systemctl restart')" = 0 -a -z "$(ls -A "$R/home/root/cangjie-backups" 2>/dev/null)"
check "install 第二次：qmd 没变 → 不再提示\"生效需整机重启\"（A1）" test -z "$(grep '生效需整机重启' "$R/out2.txt")"
rm -rf "$CJ_PENDING_DIR"; echo "qmd font v2" > "$PL/xovi/font-menu-dynamic.qmd"
xovi_live on; run sh "$PL/install.sh" >"$R/out3.txt" 2>&1
check "install：qmd 变了 + xovi 已生效 → 提示整机重启，不教 xovi/start 也不教 restart xochitl" test -n "$(grep '生效需整机重启' "$R/out3.txt")" -a -z "$(grep -e 'xovi/start' -e 'restart xochitl' "$R/out3.txt")"
check "install：qmd 变了 → 重新记待生效标记" test -f "$CJ_PENDING_DIR/shelf-qmd"
# 更新一个二进制：只重启它，且备份进 cangjie-backups
echo '#!/bin/sh' > "$PL/bin/font-serve"; echo '# v2' >> "$PL/bin/font-serve"
: > "$CJ_SIM_LOG"; run sh "$PL/install.sh" >/dev/null 2>&1
check "install：更新 font-serve → 只重启 font-serve.service" test "$(count_log 'systemctl restart')" = 1 -a "$(count_log 'restart font-serve.service')" = 1
check "install：旧 font-serve 备份进 cangjie-backups/shelf-*/" test -n "$(ls "$R"/home/root/cangjie-backups/shelf-*/font-serve 2>/dev/null)"

# 缺载荷：什么都不写
new_sandbox; PL="$R/payload"; mk_payload "$PL"; rm "$PL/bin/note-serve"; PRE_SIG="$(tree_sig)"; : > "$CJ_SIM_LOG"
run sh "$PL/install.sh" >"$R/out.txt" 2>&1; rc=$?
check "缺 bin/note-serve：退出非 0" test "$rc" -ne 0
sig_eq "缺 bin/note-serve：文件树完全未变（无半成品混装）" "$PRE_SIG" "$(tree_sig)"
check "缺 bin/note-serve：从未 remount" test "$(count_log remount)" = 0
new_sandbox; PL="$R/payload"; mk_payload "$PL"; rm "$PL/systemd/mind-serve.service"; PRE_SIG="$(tree_sig)"; : > "$CJ_SIM_LOG"
run sh "$PL/install.sh" >"$R/out.txt" 2>&1; rc=$?
check "缺 mind-serve.service：退出非 0、从未 remount（旧版会在 rw 状态半路退出）" test "$rc" -ne 0 -a "$(count_log remount)" = 0
sig_eq "缺 mind-serve.service：文件树未变" "$PRE_SIG" "$(tree_sig)"
# rw 窗口内失败：单元源不可读 → cp 失败 → 仍恢复 ro
new_sandbox; PL="$R/payload"; mk_payload "$PL"; chmod 000 "$PL/systemd/book-serve.service"; : > "$CJ_SIM_LOG"
run sh "$PL/install.sh" >"$R/out.txt" 2>&1; rc=$?
chmod 644 "$PL/systemd/book-serve.service"
check "rw 窗口内 cp 失败：退出非 0，且最后一次 mount 是 remount,ro（H2）" test "$rc" -ne 0 -a "$(last_mount)" = "mount -o remount,ro /"
# verity
new_sandbox; PL="$R/payload"; mk_payload "$PL"; : > "$CJ_SIM_LOG"
CJ_SIM_VERITY=1 run sh "$PL/install.sh" >"$R/out.txt" 2>&1
check "dm-verity 激活：不写 /usr、不 remount，二进制照装" test "$(count_log remount)" = 0 -a -x "$R/home/root/.local/bin/gateway" -a -z "$(ls -A "$CJ_SYSD")"
# 未知服务令牌
new_sandbox; PL="$R/payload"; mk_payload "$PL"
run sh "$PL/install.sh" --only bogus >/dev/null 2>&1; rc=$?
check "--only 未知令牌：退出 2、不写任何东西" test "$rc" -eq 2 -a ! -e "$R/home/root/.local/bin/gateway"
# 旧命名迁移
new_sandbox; PL="$R/payload"; mk_payload "$PL"
echo old > "$CJ_SYSD/shelf-gateway.service"; ln -s ../shelf-gateway.service "$CJ_SYSD/multi-user.target.wants-x" 2>/dev/null
echo old > "$R/home/root/.local/bin/shelf-gateway"; echo old > "$R/home/root/.local/bin/cangjie-lo-alias.sh"
run sh "$PL/install.sh" >/dev/null 2>&1
check "M7 旧命名迁移：shelf-gateway 单元/二进制、cangjie-lo-alias.sh 被清，且备份进 cangjie-backups" test ! -e "$CJ_SYSD/shelf-gateway.service" -a ! -e "$R/home/root/.local/bin/shelf-gateway" -a ! -e "$R/home/root/.local/bin/cangjie-lo-alias.sh" -a -f "$(ls "$R"/home/root/cangjie-backups/shelf-*/shelf-gateway | head -n 1)"
rm -f "$CJ_SYSD/multi-user.target.wants-x"
# 密码文件
new_sandbox; PL="$R/payload"; mk_payload "$PL"; printf '%s' "p a\"s;s\$x" > "$R/pw"
run sh "$PL/install.sh" --password-file "$R/pw" >/dev/null 2>&1
check "--password-file：密码原样传给 gateway passwd（特殊字符不被 shell 解释）" grep -Fq "gateway passwd p a\"s;s\$x" "$CJ_SIM_LOG"
check "--password-file：文件读后即删" test ! -e "$R/pw"
new_sandbox; PL="$R/payload"; mk_payload "$PL"; rm "$PL/bin/gateway"; printf 'pw' > "$R/pw"
run sh "$PL/install.sh" --password-file "$R/pw" >/dev/null 2>&1
check "--password-file：载荷校验失败退出时密码文件也已删" test ! -e "$R/pw"

section "shelf/uninstall.sh：与 install 清单对称"
new_sandbox; PL="$R/payload"; mk_payload "$PL"; PRE_SIG="$(tree_sig)"
run sh "$PL/install.sh" >/dev/null 2>&1
echo old > "$CJ_SYSD/shelf-gateway.service"; echo old > "$R/home/root/.local/bin/shelf-gateway"
echo "user note" > "$R/home/root/.local/share/shelf/user-file.txt"
mkdir -p "$R/home/root/.local/state/notes"; echo keep > "$R/home/root/.local/state/notes/entries.json"
: > "$CJ_SIM_LOG"
run sh "$R/home/root/.local/bin/shelf-uninstall" >"$R/out.txt" 2>&1; rc=$?
check "uninstall 全量：退出 0" test "$rc" -eq 0
POST_SIG="$(tree_sig | grep -v -e 'home/root/\.config/shelf/' -e 'home/root/\.local/share/shelf/' -e 'home/root/\.local/state/shelf/' -e 'home/root/\.local/state/notes/')"
sig_eq "uninstall：装过的每个文件都被删（文件树回到安装前，仅剩用户数据）" "$PRE_SIG" "$POST_SIG"
check "uninstall：comic-margins qmd 也删了" test ! -e "$Q/shelf-comic-margins.qmd"
check "uninstall：reader-page-turn qmd 也删了" test ! -e "$Q/reader-page-turn.qmd"
check "uninstall：mkdir-agent qmd / lo-alias.sh / shelf-uninstall / 库 / 旧命名遗留 全清" test ! -e "$Q/shelf-mkdir-agent.qmd" -a ! -e "$B/lo-alias.sh" -a ! -e "$B/shelf-uninstall" -a ! -e "$R/home/root/.local/lib/shelf" -a ! -e "$CJ_SYSD/shelf-gateway.service" -a ! -e "$B/shelf-gateway"
check "uninstall：用户数据（含 share/shelf 里的用户文件、笔记线数据）保留" test -f "$R/home/root/.local/share/shelf/user-file.txt" -a -f "$R/home/root/.local/state/notes/entries.json"
check "uninstall：壁纸还原调用了 wallpaper-serve disable" grep -q 'wallpaper-serve disable' "$CJ_SIM_LOG"
check "uninstall：rw 窗口后最后一次 mount 是 ro" test "$(last_mount)" = "mount -o remount,ro /"
# --only
new_sandbox; PL="$R/payload"; mk_payload "$PL"; run sh "$PL/install.sh" >/dev/null 2>&1
run sh "$R/home/root/.local/bin/shelf-uninstall" --only book >/dev/null 2>&1
check "uninstall --only book：book 的二进制/单元/两个 qmd 删了，其它服务与共享件保留" test ! -e "$B/book-serve" -a ! -e "$CJ_SYSD/book-serve.service" -a ! -e "$Q/shelf-trash-agent.qmd" -a ! -e "$Q/shelf-mkdir-agent.qmd" -a -x "$B/gateway" -a -x "$B/lo-alias.sh" -a -x "$B/shelf-uninstall" -a -f "$Q/font-menu-dynamic.qmd"
run sh "$R/home/root/.local/bin/shelf-uninstall" --only bogus >/dev/null 2>&1; rc=$?
check "uninstall --only 未知令牌：退出 2" test "$rc" -eq 2
# --purge
new_sandbox; PL="$R/payload"; mk_payload "$PL"; run sh "$PL/install.sh" >/dev/null 2>&1
mkdir -p "$R/home/root/.local/state/notes"; echo keep > "$R/home/root/.local/state/notes/e.json"; echo d > "$R/home/root/.local/share/shelf/d.txt"
run sh "$R/home/root/.local/bin/shelf-uninstall" --purge >/dev/null 2>&1
check "uninstall --purge：只清 shelf 三个 XDG 目录，笔记线数据仍在" test ! -e "$R/home/root/.config/shelf" -a ! -e "$R/home/root/.local/share/shelf" -a ! -e "$R/home/root/.local/state/shelf" -a -f "$R/home/root/.local/state/notes/e.json"
# 用符号链接冒充 shelf 数据目录：拒绝清除
new_sandbox; PL="$R/payload"; mk_payload "$PL"; run sh "$PL/install.sh" >/dev/null 2>&1
mkdir -p "$R/precious"; echo keep > "$R/precious/f"; rm -rf "$R/home/root/.local/share/shelf"; ln -s "$R/precious" "$R/home/root/.local/share/shelf"
run sh "$R/home/root/.local/bin/shelf-uninstall" --purge >/dev/null 2>&1
check "uninstall --purge：数据目录是符号链接 → 拒绝，目标内容不动" test -f "$R/precious/f"

# ═══════════════════════════ 3. 部署脚本（假 ssh/scp）═══════════════════════════
section "packaging/deploy.sh：密码特殊字符 / shelf-pkg 换位"
mk_dummy_targets() {
    T=aarch64-unknown-linux-musl
    for f in shelf/target/$T/release/book-serve shelf/target/$T/release/koreader-serve gateway/target/$T/release/gateway \
             enhance/wallpaper-serve/target/$T/release/wallpaper-serve enhance/font-serve/target/$T/release/font-serve \
             notes/target/$T/release/ink-serve notes/target/$T/release/transcribe-serve notes/target/$T/release/mind-serve notes/target/$T/release/note-serve; do
        # 仓库里已有**真**交叉编译产物（开发机上构建过）时先挪走再造假的：否则 deploy.sh 会把真 aarch64 二进制装进
        # 沙箱去执行（Exec format error）。结束时 cleanup 无条件还原。
        if [ -e "$REPO/$f" ] && ! head -c 2 "$REPO/$f" 2>/dev/null | grep -q '^#!'; then
            mkdir -p "$TMPBASE/real-targets/$(dirname "$f")"
            mv "$REPO/$f" "$TMPBASE/real-targets/$f"
            echo "$f" >> "$TMPBASE/real-targets.txt"
        fi
        if [ ! -e "$REPO/$f" ]; then
            mkdir -p "$(dirname "$REPO/$f")"
            printf '#!/bin/sh\necho "%s $*" >> "%s"\n' "$(basename "$f")" '$CJ_SIM_LOG' > "$REPO/$f"
            # 让 stub 二进制写日志：日志路径运行期取 CJ_SIM_LOG（单引号里的 $CJ_SIM_LOG 在设备上由环境展开）
            sed -i 's|"\$CJ_SIM_LOG"|"${CJ_SIM_LOG:-/dev/null}"|' "$REPO/$f"
            chmod +x "$REPO/$f"; echo "$REPO/$f" >> "$CREATED_FILES"
        fi
    done
}
mk_dummy_targets
new_sandbox
( cd "$PKG" && SHELF_NO_BUILD=1 run sh deploy.sh 127.0.0.1 --only book,font --password 'p a"s;s$x' ) >"$R/out.txt" 2>&1; rc=$?
[ "$rc" -eq 0 ] || sed 's/^/     | /' "$R/out.txt" | tail -12   # 失败时把 deploy.sh 的输出末尾带出来，便于定位
check "deploy.sh：--only book,font + 含空格/引号/分号的密码 → 退出 0" test "$rc" -eq 0
check "deploy.sh：密码原样到达 gateway passwd（没被远端 shell 解释）" grep -Fq 'gateway passwd p a"s;s$x' "$CJ_SIM_LOG"
check "deploy.sh：密码不出现在任何 ssh 命令行里" test -z "$(grep '^ssh' "$CJ_SIM_LOG" | grep -F 's;s')"
check "deploy.sh：设备上无 .pw 残留、无 shelf-pkg.new 残留、shelf-pkg 完整" test ! -e "$R/home/root/shelf-pkg/.pw" -a ! -e "$R/home/root/shelf-pkg.new" -a -f "$R/home/root/shelf-pkg/shelf/install.sh" -a -f "$R/home/root/shelf-pkg/shelf/manifest.sh" -a -f "$R/home/root/shelf-pkg/shelf/devlib.sh"
check "deploy.sh：只装了 gateway/book/font 三个服务（--only 透传）" test -x "$R/home/root/.local/bin/book-serve" -a -x "$R/home/root/.local/bin/font-serve" -a ! -e "$R/home/root/.local/bin/note-serve"
# 换位保护：载荷里没有 install.sh 时不能把现成 shelf-pkg 清掉（直接测远端换位命令的语义）
mkdir -p "$R/home/root/shelf-pkg/shelf"; echo keepme > "$R/home/root/shelf-pkg/shelf/marker"
empty="$R/empty.tar"; mkdir -p "$R/emptyroot/shelf"; tar -C "$R/emptyroot" -cf "$empty" shelf
( cd "$R/home/root" && PATH="$STUBS:$PATH" sh -c "ssh -o x=y root@h 'rm -rf /home/root/shelf-pkg.new && mkdir -p /home/root/shelf-pkg.new && tar -C /home/root/shelf-pkg.new -xf - && [ -f /home/root/shelf-pkg.new/shelf/install.sh ] && rm -rf /home/root/shelf-pkg && mv /home/root/shelf-pkg.new /home/root/shelf-pkg' < '$empty'" ) >/dev/null 2>&1
check "shelf-pkg 换位：新载荷缺 install.sh → 旧 shelf-pkg 原样保留" test -f "$R/home/root/shelf-pkg/shelf/marker"

section "packaging/deploy-hl-snap.sh：原子落位 / 备份不进 extensions.d / DEFER"
new_sandbox; EXT="$R/home/root/xovi/extensions.d"
echo OLDSO > "$EXT/hl-snap.so"; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_SKIP_BUILD=1 DEFER_XOVI_START=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "hl-snap DEFER：退出 0" test "$rc" -eq 0
check "hl-snap：设备上的 .so 与仓库产物 md5 一致" test "$(md5sum < "$EXT/hl-snap.so")" = "$(md5sum < "$REPO/enhance/hl-snap/hl-snap.so")"
check "hl-snap：旧 .so 备份进 cangjie-backups，extensions.d 里只有 hl-snap.so（无 .bak/.new/临时文件）" test -f "$(ls "$R"/home/root/cangjie-backups/hl-snap.so.bak.pre-* | head -n 1)" -a "$(ls -A "$EXT")" = "hl-snap.so"
check "hl-snap DEFER：不重启 xochitl、不 xovi/start" test "$(count_log 'restart xochitl')" = 0 -a "$(count_log XOVI_START)" = 0
check "hl-snap：暂存/推送目录里带了 xovi-ext-install.sh 与 devlib.sh" test -f "$R/home/root/hl-snap/deploy/xovi-ext-install.sh" -a -f "$R/home/root/hl-snap/deploy/devlib.sh"
check "hl-snap：reading-qol.json 首次装才建" test -f "$R/home/root/.local/share/cangjie-ime/reading-qol.json"
echo '{"user":"setting"}' > "$R/home/root/.local/share/cangjie-ime/reading-qol.json"
( cd "$PKG" && CJ_SKIP_BUILD=1 DEFER_XOVI_START=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >/dev/null 2>&1
check "hl-snap 重复部署：已有的 reading-qol.json 不被覆盖" grep -q '"user":"setting"' "$R/home/root/.local/share/cangjie-ime/reading-qol.json"
# 非 DEFER：xovi 已生效 → 整机重启（2026-09-25 起；H1 仍是绝不 xovi/start）
xovi_live on; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_SKIP_BUILD=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "hl-snap 非 DEFER + xovi 已生效：systemctl reboot（不停/不重启 xochitl）、绝不 xovi/start、退出 0 并提示回来后核对" test "$rc" -eq 0 -a "$(count_log 'systemctl reboot')" = 1 -a -z "$(grep -E 'systemctl (stop|start|restart) xochitl' "$CJ_SIM_LOG")" -a "$(count_log XOVI_START)" = 0 -a -n "$(grep 'verify-on-device.sh 127.0.0.1' "$R/out.txt")"
xovi_live off; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_SKIP_BUILD=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >/dev/null 2>&1
check "hl-snap 非 DEFER + xovi 未生效：走 xovi/start" test "$(count_log XOVI_START)" = 1
# md5 传输损坏
new_sandbox; EXT="$R/home/root/xovi/extensions.d"; echo OLDSO > "$EXT/hl-snap.so"
( cd "$PKG" && CJ_SIM_SCP_CORRUPT=hl-snap.so CJ_SKIP_BUILD=1 DEFER_XOVI_START=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "scp 传输损坏：md5 对不上 → 非 0，extensions.d 里旧 .so 原样、无残留" test "$rc" -ne 0 -a "$(cat "$EXT/hl-snap.so")" = OLDSO -a "$(ls -A "$EXT")" = "hl-snap.so"

section "H3：运行中 xochitl 正映射着的扩展 .so 不当场换 → 待换入区；生效＝换入后主动整机重启（2026-09-25 起）"
new_sandbox; EXT="$R/home/root/xovi/extensions.d"; SOP="$R/home/root/.cangjie-stage/so-pending"
echo OLDSO > "$EXT/hl-snap.so"; xovi_live on; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_SKIP_BUILD=1 DEFER_XOVI_START=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "H3 DEFER：extensions.d 里仍是旧版，新版在待换入区（不在 extensions.d）" test "$rc" -eq 0 -a "$(cat "$EXT/hl-snap.so")" = OLDSO -a -f "$SOP/hl-snap.so" -a "$(ls -A "$EXT")" = "hl-snap.so"
check "H3 DEFER：记了待生效标记、没动 xochitl、没重启设备" test -n "$(ls -A "$CJ_PENDING_DIR" 2>/dev/null)" -a -z "$(grep -E 'systemctl (stop|start|restart) xochitl' "$CJ_SIM_LOG")" -a "$(count_log 'systemctl reboot')" = 0
: > "$CJ_SIM_LOG"
( cd "$PKG" && run sh deploy-xovi-apply.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "H3 apply：换入 → systemctl reboot；不停/不重启 xochitl、没有 xovi/start、退出 0" test "$rc" -eq 0 -a "$(count_log 'systemctl reboot')" = 1 -a -z "$(grep -E 'systemctl (stop|start|restart) xochitl' "$CJ_SIM_LOG")" -a "$(count_log XOVI_START)" = 0
check "H3 apply：换入后 md5 与仓库产物一致，待换入区清空，extensions.d 无残留" test "$(md5sum < "$EXT/hl-snap.so")" = "$(md5sum < "$REPO/enhance/hl-snap/hl-snap.so")" -a ! -e "$SOP" -a "$(ls -A "$EXT")" = "hl-snap.so"
check "H3 apply：提示设备在重启并给出核对命令，没跑重启后健康检查" test -n "$(grep '设备正在整机重启' "$R/out.txt")" -a -n "$(grep 'verify-on-device.sh 127.0.0.1' "$R/out.txt")" -a -z "$(grep 'is-active :' "$R/out.txt")"
# 非 DEFER 同理：一次部署内换入 + 整机重启
echo OLDSO > "$EXT/hl-snap.so"; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_SKIP_BUILD=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "H3 非 DEFER：整机重启一次且新版已就位" test "$rc" -eq 0 -a "$(count_log 'systemctl reboot')" = 1 -a -z "$(grep -E 'systemctl (stop|start|restart) xochitl' "$CJ_SIM_LOG")" -a "$(md5sum < "$EXT/hl-snap.so")" = "$(md5sum < "$REPO/enhance/hl-snap/hl-snap.so")"
# 过时的待换入版本：新部署与已装的相同 → 撤掉
mkdir -p "$SOP"; echo STALE > "$SOP/hl-snap.so"
( cd "$PKG" && CJ_SKIP_BUILD=1 DEFER_XOVI_START=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >/dev/null 2>&1
check "H3：与已装相同的部署撤掉过时的待换入版本" test ! -e "$SOP/hl-snap.so"
# 部署后自动核对（2026-09-25）：等设备断开 → 回来 → 跑 verify-on-device.sh，退出码跟随核对结果
: > "$CJ_SIM_LOG"; mkdir -p "$CJ_PENDING_DIR"; : > "$CJ_PENDING_DIR/shelf-qmd"
( cd "$PKG" && CJ_APPLY_VERIFY=1 CJ_REBOOT_DOWN_WAIT=0 CJ_REBOOT_UP_WAIT=0 CJ_REBOOT_SETTLE=0 run sh deploy-xovi-apply.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "部署后自动核对：排上重启后等设备回来并跑 verify-on-device.sh，退出码跟随核对（沙箱固件哈希不在白名单 → 1）" test "$rc" -eq 1 -a -n "$(grep '设备已回来，核对' "$R/out.txt")" -a -n "$(grep '^VERIFY-SUMMARY' "$R/out.txt")"
# 映射着已删文件（刚卸载过）也一样，--force 也一样
xovi_live on; printf '7f03 r-xp %s (deleted)\n' "$EXT/hw-stroke.so" >> "$R/proc/4242/maps"; : > "$CJ_SIM_LOG"
( cd "$PKG" && run sh deploy-xovi-apply.sh 127.0.0.1 --force ) >"$R/out.txt" 2>&1; rc=$?
check "H3：刚卸载过（maps 带 (deleted)）+ --force → 同样只整机重启" test "$rc" -eq 0 -a "$(count_log 'systemctl reboot')" = 1 -a -z "$(grep -E 'systemctl (stop|start|restart) xochitl' "$CJ_SIM_LOG")"

section "packaging/deploy-handwriting-stroke.sh（同一份数据驱动流程）"
new_sandbox; EXT="$R/home/root/xovi/extensions.d"
( cd "$PKG" && CJ_SKIP_BUILD=1 DEFER_XOVI_START=1 run sh deploy-handwriting-stroke.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "hw-stroke DEFER：落位 hw-stroke.so，配置键是 hwStrokeWidthFactor" test "$rc" -eq 0 -a -f "$EXT/hw-stroke.so" -a -n "$(grep hwStrokeWidthFactor "$R/home/root/.local/share/cangjie-ime/reading-qol.json")"

section "packaging/deploy-xovi-apply.sh：H1 + A1（无待生效改动不重启）"
new_sandbox; echo x > "$R/home/root/xovi/extensions.d/hl-snap.so"; xovi_live on; : > "$CJ_SIM_LOG"
mkdir -p "$CJ_PENDING_DIR"; : > "$CJ_PENDING_DIR/hl-snap"
( cd "$PKG" && run sh deploy-xovi-apply.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "xovi-apply：有待生效标记 + xovi 已生效 → 整机重启（不停/不重启 xochitl），绝不 xovi/start（2026-09-20 事故）" test "$rc" -eq 0 -a "$(count_log 'systemctl reboot')" = 1 -a -z "$(grep -E 'systemctl (stop|start|restart) xochitl' "$CJ_SIM_LOG")" -a "$(count_log XOVI_START)" = 0
check "xovi-apply：输出里有\"打断\"提示" grep -q '打断' "$R/out.txt"
check "xovi-apply：重启成功后待生效标记已清空" test -z "$(ls -A "$CJ_PENDING_DIR" 2>/dev/null)"
: > "$CJ_SIM_LOG"
( cd "$PKG" && run sh deploy-xovi-apply.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "xovi-apply：没有待生效标记且 xovi 已生效 → 不重启 xochitl、退出 0（A1）" test "$rc" -eq 0 -a "$(count_log 'restart xochitl')" = 0 -a "$(count_log XOVI_START)" = 0
check "xovi-apply：无改动时说明原因并提示 --force" grep -q -e '不重启 xochitl' -e '--force' "$R/out.txt"
: > "$CJ_SIM_LOG"
( cd "$PKG" && run sh deploy-xovi-apply.sh 127.0.0.1 --force ) >"$R/out.txt" 2>&1; rc=$?
check "xovi-apply --force：无标记也生效一次（整机重启，不 xovi/start）" test "$rc" -eq 0 -a "$(count_log 'systemctl reboot')" = 1 -a -z "$(grep -E 'systemctl (stop|start|restart) xochitl' "$CJ_SIM_LOG")" -a "$(count_log XOVI_START)" = 0
xovi_live off; : > "$CJ_SIM_LOG"
( cd "$PKG" && run sh deploy-xovi-apply.sh 127.0.0.1 ) >/dev/null 2>&1
check "xovi-apply：xovi 未生效（哪怕没标记）→ 走 xovi/start" test "$(count_log XOVI_START)" = 1 -a "$(count_log 'restart xochitl')" = 0
new_sandbox; rm "$R/home/root/xovi/start" "$R/home/root/xovi/xovi.so"; xovi_live off; : > "$CJ_SIM_LOG"
( cd "$PKG" && run sh deploy-xovi-apply.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "xovi-apply：设备没装 xovi 且无待生效改动 → 跳过、退出 0，不重启" test "$rc" -eq 0 -a "$(count_log 'restart xochitl')" = 0 -a "$(count_log XOVI_START)" = 0
( cd "$PKG" && run sh deploy-xovi-apply.sh 127.0.0.1 --bogus ) >"$R/out.txt" 2>&1; rc=$?
check "xovi-apply：未知参数 → 退出 2" test "$rc" -eq 2

section "packaging/deploy-usr-unit（wifi-watch / chrony-boot-wakelock / xovi-persist）"
new_sandbox; : > "$CJ_SIM_LOG"
( cd "$PKG" && run sh deploy-wifi-watch.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "wifi-watch：单元 + wants 链接 + ~/.local/bin/wifi-watch.sh + 已 restart" test "$rc" -eq 0 -a -f "$CJ_SYSD/wifi-watch.service" -a -L "$CJ_SYSD/multi-user.target.wants/wifi-watch.service" -a -x "$R/home/root/.local/bin/wifi-watch.sh" -a "$(count_log 'restart wifi-watch.service')" = 1
: > "$CJ_SIM_LOG"; ( cd "$PKG" && run sh deploy-wifi-watch.sh 127.0.0.1 ) >/dev/null 2>&1
check "wifi-watch 再装一次：不 remount、不重启服务（幂等）" test "$(count_log remount)" = 0 -a "$(count_log 'restart wifi-watch.service')" = 0
new_sandbox; : > "$CJ_SIM_LOG"; CJ_SIM_VERITY=1 bash -c "cd '$PKG' && PATH='$STUBS:'\$PATH sh deploy-chrony-boot-wakelock.sh 127.0.0.1" >"$R/out.txt" 2>&1; rc=$?
check "chrony-boot-wakelock + dm-verity：退出 0（跳过非失败）、不 remount、不写 /usr" test "$rc" -eq 0 -a "$(count_log remount)" = 0 -a -z "$(ls -A "$CJ_SYSD")"
new_sandbox; rm "$R/home/root/xovi/start"; ( cd "$PKG" && run sh deploy-xovi-persist.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "xovi-persist：没装 xovi 本体 → 失败，且不写 /usr、不 remount" test "$rc" -ne 0 -a "$(count_log remount)" = 0 -a -z "$(ls -A "$CJ_SYSD")"

section "packaging/deploy-battop.sh：有意不开机自启 / 原子替换 / 保留用户关闭状态"
new_sandbox; echo BIN1 > "$R/battop.bin"; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_BATTOP_BIN="$R/battop.bin" run sh deploy-battop.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "battop 首装：二进制+单元就位，起服务" test "$rc" -eq 0 -a "$(cat "$R/home/root/battop/battop")" = BIN1 -a -f "$CJ_SYSD/battop.service" -a "$(count_log 'restart battop.service')" = 1
check "battop：不建 wants 链接、不 systemctl enable（有意不开机自启）" test ! -e "$CJ_SYSD/multi-user.target.wants/battop.service" -a "$(count_log 'systemctl enable')" = 0
echo BIN2 > "$R/battop.bin"; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_BATTOP_BIN="$R/battop.bin" run sh deploy-battop.sh 127.0.0.1 ) >/dev/null 2>&1
check "battop 更新：旧二进制备份进 cangjie-backups；旧的在跑 → restart 载入新版；没有先 stop 再 scp" test "$(cat "$R/home/root/battop/battop")" = BIN2 -a -n "$(ls "$R"/home/root/cangjie-backups/battop.bak.pre-* 2>/dev/null)" -a "$(count_log 'systemctl stop')" = 0 -a "$(count_log 'restart battop.service')" = 1
: > "$CJ_SIM_LOG"; ( cd "$PKG" && CJ_BATTOP_BIN="$R/battop.bin" run sh deploy-battop.sh 127.0.0.1 ) >/dev/null 2>&1
check "battop 重复部署同一份二进制：不重启（不重复触发 cgroup 迁移）、不再堆备份" test "$(count_log 'restart battop.service')" = 0 -a "$(ls "$R"/home/root/cangjie-backups/battop.bak.pre-* | wc -l)" -eq 1
echo BIN3 > "$R/battop.bin"; : > "$CJ_SIM_LOG"
CJ_SIM_INACTIVE=1 bash -c "cd '$PKG' && CJ_BATTOP_BIN='$R/battop.bin' PATH='$STUBS:'\$PATH sh deploy-battop.sh 127.0.0.1" >/dev/null 2>&1
check "battop 已装但当前停着（用户在网页关了）→ 更新二进制但不擅自启动" test "$(cat "$R/home/root/battop/battop")" = BIN3 -a "$(count_log 'restart battop.service')" = 0
new_sandbox; CJ_SIM_VERITY=1 bash -c "cd '$PKG' && CJ_BATTOP_BIN='$R/battop.bin' PATH='$STUBS:'\$PATH sh deploy-battop.sh 127.0.0.1" >/dev/null 2>&1
check "battop + dm-verity：不 remount（补上原先缺的 verity 门）" test "$(count_log remount)" = 0

section "packaging/deploy-sidebar-entry.sh"
new_sandbox; Q="$R/home/root/xovi/exthome/qt-resource-rebuilder"; echo OLDQMD > "$Q/koreader-sidebar-entry.qmd"
( cd "$PKG" && DEFER_XOVI_START=1 PATH="$STUBS:$PATH" sh deploy-sidebar-entry.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "sidebar-entry DEFER：qmd/rcc 落位，旧 qmd 备份进 cangjie-backups，qrr 目录里无 .bak/暂存残留" test "$rc" -eq 0 -a -f "$Q/cangjie-icons.rcc" -a -n "$(ls "$R"/home/root/cangjie-backups/koreader-sidebar-entry.qmd.bak.pre-* 2>/dev/null)" -a -z "$(find "$Q" -maxdepth 1 \( -name '*.bak*' -o -name '*.new*' \) )" -a ! -e "$R/home/root/.cangjie-stage"
check "sidebar-entry DEFER：不重启 xochitl" test "$(count_log 'restart xochitl')" = 0 -a "$(count_log XOVI_START)" = 0
xovi_live on; : > "$CJ_SIM_LOG"
( cd "$PKG" && PATH="$STUBS:$PATH" sh deploy-sidebar-entry.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "sidebar-entry 非 DEFER + xovi 已生效：整机重启，绝不 xovi/start" test "$rc" -eq 0 -a "$(count_log 'systemctl reboot')" = 1 -a -z "$(grep -E 'systemctl (stop|start|restart) xochitl' "$CJ_SIM_LOG")" -a "$(count_log XOVI_START)" = 0
new_sandbox; CJ_SIM_SCP_CORRUPT=cangjie-icons.rcc bash -c "cd '$PKG' && DEFER_XOVI_START=1 PATH='$STUBS:'\$PATH sh deploy-sidebar-entry.sh 127.0.0.1" >/dev/null 2>&1; rc=$?
check "sidebar-entry rcc 传输损坏：非 0，qrr 目录里没有 rcc/qmd（H3 同类：坏文件不进自动加载路径）" test "$rc" -ne 0 -a ! -e "$R/home/root/xovi/exthome/qt-resource-rebuilder/cangjie-icons.rcc"

# ═══════════════════════════ 4. install-all → uninstall-all 整轮对称 ═══════════════════════════
section "install-all → uninstall-all 整轮"
new_sandbox
echo BATTOPBIN > "$R/battop.bin"
export CJ_BATTOP_BIN="$R/battop.bin" CJ_SKIP_BUILD=1 SHELF_NO_BUILD=1
export CJ_ALLOWLIST_LOCAL="$R/allow.local.txt"
ALLOW_MD5="$(md5sum < "$PKG/firmware-allowlist.txt")"
PRE_SIG="$(tree_sig)"; : > "$CJ_SIM_LOG"
( cd "$PKG" && run sh install-all.sh 127.0.0.1 --skip chrony-cn,timezone-cn ) >"$R/out.txt" 2>&1; rc=$?
check "install-all 固件不在白名单：拒装（退出非 0）、不 remount" test "$rc" -ne 0 -a "$(count_log remount)" = 0
sig_eq "install-all 固件不在白名单：什么都没装" "$PRE_SIG" "$(tree_sig)"
xovi_live on
( cd "$PKG" && run sh install-all.sh 127.0.0.1 --force --skip chrony-cn,timezone-cn ) >"$R/out.txt" 2>&1; rc=$?
check "install-all --force：整轮退出 0" test "$rc" -eq 0
check "install-all --force：未验证哈希写进本机 allowlist.local，被 git 跟踪的白名单文件不变（L5）" test -s "$R/allow.local.txt" -a "$ALLOW_MD5" = "$(md5sum < "$PKG/firmware-allowlist.txt")"
# 扩展 .so 有变化且 xochitl 正映射着 → 走 H3 的 stop → 换入 → start；否则 restart。两者合计恰好一轮。
cycles="$(count_log 'systemctl reboot')"
check "install-all：整轮下来 xovi 已生效 → 只在最后整机重启一次，全程不停/不重启 xochitl、没有 xovi/start（H1）" test "$cycles" = 1 -a -z "$(grep -E 'systemctl (stop|start|restart) xochitl' "$CJ_SIM_LOG")" -a "$(count_log XOVI_START)" = 0
check "install-all：hl-snap/hw-stroke 落进 extensions.d，且目录里只有它俩" test -f "$R/home/root/xovi/extensions.d/hl-snap.so" -a -f "$R/home/root/xovi/extensions.d/hw-stroke.so" -a "$(ls "$R/home/root/xovi/extensions.d" | wc -l)" -eq 2
W="$CJ_SYSD/multi-user.target.wants"
check "install-all：wifi-watch（M1）/ xovi-reenable / chrony-boot-wakelock 单元在 /usr 且有 wants 链接" test -L "$W/wifi-watch.service" -a -L "$W/xovi-reenable.service" -a -L "$W/chrony-boot-wakelock.service"
check "install-all：battop 单元在 /usr 但没有 wants 链接（有意不开机自启）" test -f "$CJ_SYSD/battop.service" -a ! -e "$W/battop.service" -a ! -L "$W/battop.service"
check "install-all：qmd 全部就位（sidebar/字体/回收站/建夹）" test -f "$R/home/root/xovi/exthome/qt-resource-rebuilder/koreader-sidebar-entry.qmd" -a -f "$R/home/root/xovi/exthome/qt-resource-rebuilder/shelf-mkdir-agent.qmd" -a -f "$R/home/root/xovi/exthome/qt-resource-rebuilder/font-menu-dynamic.qmd"
check "install-all：每次 rw 窗口都以 ro 收尾" test "$(last_mount)" = "mount -o remount,ro /"
SIG_INSTALLED="$(tree_sig)"
: > "$CJ_SIM_LOG"
( cd "$PKG" && run sh install-all.sh 127.0.0.1 --skip chrony-cn,timezone-cn ) >"$R/out2.txt" 2>&1; rc=$?
check "install-all 第二遍（固件已在本机白名单）：退出 0" test "$rc" -eq 0
sig_eq "install-all 第二遍：文件树不变（幂等）" "$SIG_INSTALLED" "$(tree_sig)"
check "install-all 第二遍：不 remount rootfs（单元都已是最新）、仍无 xovi/start" test "$(count_log remount)" = 0 -a "$(count_log XOVI_START)" = 0
check "install-all 第二遍：这轮没改任何 xovi 相关文件 → 不重启 xochitl（A1，重复跑不再闪屏）" test "$(count_log 'restart xochitl')" = 0
: > "$CJ_SIM_LOG"
( cd "$PKG" && run sh install-all.sh 127.0.0.1 --skip chrony-cn,timezone-cn --force-apply ) >"$R/out2b.txt" 2>&1; rc=$?
check "install-all --force-apply：无改动也生效一次（整机重启，不 xovi/start）" test "$rc" -eq 0 -a "$(count_log 'systemctl reboot')" = 1 -a -z "$(grep -E 'systemctl (stop|start|restart) xochitl' "$CJ_SIM_LOG")" -a "$(count_log XOVI_START)" = 0
( cd "$PKG" && run sh install-all.sh 127.0.0.1 --skip chrony-cn,timezone-cn,bogus-step ) >"$R/out3.txt" 2>&1
check "--skip 未知步骤名：给出警告而不是静默" grep -q '不是已知步骤名' "$R/out3.txt"

: > "$CJ_SIM_LOG"
( cd "$PKG" && run sh uninstall-all.sh 127.0.0.1 ) >"$R/uout.txt" 2>&1; rc=$?
check "uninstall-all：整轮退出 0" test "$rc" -eq 0
POST_SIG="$(tree_sig | grep -v \
    -e 'home/root/\.config/shelf/' -e 'home/root/\.local/share/shelf/' -e 'home/root/\.local/state/shelf/' \
    -e 'home/root/battop/' -e 'home/root/\.local/share/cangjie-ime/')"
sig_eq "uninstall-all：文件树回到安装前（含 deploy 推送的载荷 pkg-*/hl-snap/hw-stroke/shelf-pkg 与暂存目录全清；仅剩用户数据/battop 数据/配置）" "$PRE_SIG" "$POST_SIG"
check "uninstall-all：暂存目录也清了" test ! -e "$R/home/root/.cangjie-stage"
check "uninstall-all：不重启 xochitl、不 xovi/start" test "$(count_log 'restart xochitl')" = 0 -a "$(count_log XOVI_START)" = 0
check "uninstall-all：最后一次 mount 是 ro" test "$(last_mount)" = "mount -o remount,ro /"
( cd "$PKG" && run sh uninstall-all.sh 127.0.0.1 ) >"$R/uout2.txt" 2>&1; rc=$?
check "uninstall-all 第二遍：仍退出 0（幂等）" test "$rc" -eq 0
: > "$CJ_SIM_LOG"
( cd "$PKG" && run sh uninstall-all.sh 127.0.0.1 --purge --skip shelf ) >/dev/null 2>&1
check "uninstall-all --purge：battop 目录才被清" test ! -e "$R/home/root/battop"
unset CJ_BATTOP_BIN CJ_SKIP_BUILD SHELF_NO_BUILD CJ_ALLOWLIST_LOCAL

# ═══════════════════════════ 6. 2026-09-22 审计新增 ═══════════════════════════
section "参数解析 / --dry-run / -h / 设备不可达"
new_sandbox; : > "$CJ_SIM_LOG"
( cd "$PKG" && run sh install-all.sh --dry-run ) >"$R/out.txt" 2>&1; rc=$?
check "install-all --dry-run：退出 0、不发起任何 ssh/scp" test "$rc" -eq 0 -a "$(count_log '^ssh')" = 0 -a "$(count_log '^scp')" = 0
missing=""; for st in chrony-cn chrony-boot-wakelock timezone-cn battop wifi-watch xovi-persist hl-snap handwriting-stroke sidebar-entry shelf xovi-apply; do grep -q "═══ $st ═══" "$R/out.txt" || missing="$missing $st"; done
check "install-all --dry-run：11 个步骤都在计划里" test -z "$missing"
( cd "$PKG" && run sh install-all.sh -h ) >"$R/out.txt" 2>&1; rc=$?
check "install-all -h：退出 0、打印用法、不连设备" test "$rc" -eq 0 -a -n "$(grep '用法' "$R/out.txt")" -a "$(count_log '^ssh')" = 0
( cd "$PKG" && run sh install-all.sh 127.0.0.1 --purge ) >/dev/null 2>&1; rc1=$?
( cd "$PKG" && run sh install-all.sh 127.0.0.1 --bogus ) >/dev/null 2>&1; rc2=$?
( cd "$PKG" && run sh uninstall-all.sh 127.0.0.1 --force ) >/dev/null 2>&1; rc3=$?
( cd "$PKG" && run sh uninstall-all.sh 127.0.0.1 --force-apply ) >/dev/null 2>&1; rc4=$?
check "install-all --purge / 未知参数、uninstall-all --force/--force-apply：退出 2，且没连设备" test "$rc1" -eq 2 -a "$rc2" -eq 2 -a "$rc3" -eq 2 -a "$rc4" -eq 2 -a "$(count_log '^ssh')" = 0
( cd "$PKG" && run sh uninstall-all.sh --dry-run --purge ) >"$R/out.txt" 2>&1; rc=$?
check "uninstall-all --dry-run：退出 0、不连设备" test "$rc" -eq 0 -a "$(count_log '^ssh')" = 0
order="$(for st in shelf sidebar-entry handwriting-stroke hl-snap xovi-persist wifi-watch battop chrony-boot-wakelock; do grep -n "═══ $st ═══" "$R/out.txt" | cut -d: -f1; done | tr '\n' ' ')"
sorted="$(printf '%s\n' $order | sort -n | tr '\n' ' ')"
check "uninstall-all：步骤按 install-all 的逆序执行（shelf 最先、chrony-boot-wakelock 最后）" test -n "$order" -a "$order" = "$sorted"
check "uninstall-all：配置覆写/纯动作步骤（chrony-cn/timezone-cn/xovi-apply）不在卸载计划里" test -z "$(grep -e '═══ chrony-cn ═══' -e '═══ timezone-cn ═══' -e '═══ xovi-apply ═══' "$R/out.txt")"
( cd "$PKG" && run sh deploy-wifi-watch.sh --bogus ) >/dev/null 2>&1; rc1=$?
( cd "$PKG" && run sh deploy-wifi-watch.sh a b ) >/dev/null 2>&1; rc2=$?
( cd "$PKG" && run sh deploy-hl-snap.sh -h ) >"$R/out.txt" 2>&1; rc3=$?
check "薄 deploy 脚本：未知选项/多余参数 → 退出 2；-h → 退出 0 并打印用法；都没连设备" test "$rc1" -eq 2 -a "$rc2" -eq 2 -a "$rc3" -eq 0 -a -n "$(grep '用法' "$R/out.txt")" -a "$(count_log '^ssh')" = 0

# 设备连不上：每个会连设备的入口都先给可读的报错并在动手前退出（不留半成品、不 scp）
new_sandbox; export CJ_BATTOP_BIN="$R/battop.bin" CJ_SKIP_BUILD=1 SHELF_NO_BUILD=1; echo B > "$R/battop.bin"
unreach_ok=1
for sc in "install-all.sh 127.0.0.1 --force" "uninstall-all.sh 127.0.0.1" deploy-hl-snap.sh deploy-handwriting-stroke.sh deploy-wifi-watch.sh deploy-xovi-persist.sh deploy-chrony-boot-wakelock.sh deploy-battop.sh deploy-sidebar-entry.sh deploy-xovi-apply.sh deploy-chrony-cn.sh deploy-timezone-cn.sh deploy.sh; do
    : > "$CJ_SIM_LOG"
    case "$sc" in *" "*) cmd="$sc" ;; *) cmd="$sc 127.0.0.1" ;; esac
    CJ_SIM_SSH_FAIL=1 bash -c "cd '$PKG' && PATH='$STUBS:'\$PATH sh $cmd" >"$R/out.txt" 2>&1; rc=$?
    if [ "$rc" -eq 0 ] || ! grep -q '连不上' "$R/out.txt" || [ "$(count_log '^scp')" != 0 ]; then unreach_ok=0; echo "       未达预期：$sc rc=$rc"; fi
done
check "13 个入口在 ssh 不通时：退出非 0、打印\"连不上\"与下一步、没有 scp" test "$unreach_ok" = 1
unset CJ_BATTOP_BIN CJ_SKIP_BUILD SHELF_NO_BUILD

section "设备预检（磁盘空间）"
new_sandbox; export CJ_ALLOWLIST_LOCAL="$R/allow.local.txt"; PRE_SIG="$(tree_sig)"
CJ_MIN_FREE_KB=999999999999 bash -c "cd '$PKG' && PATH='$STUBS:'\$PATH sh install-all.sh 127.0.0.1 --force" >"$R/out.txt" 2>&1; rc=$?
check "预检：/home 空间不足 → install-all 退出非 0、报剩余空间" test "$rc" -ne 0 -a -n "$(grep '装不下' "$R/out.txt")"
sig_eq "预检：空间不足时一个文件都没装" "$PRE_SIG" "$(tree_sig)"
unset CJ_ALLOWLIST_LOCAL

section "deploy.sh：选项当首参 / 推送前核对二进制 / 密码兜底清理"
new_sandbox
( cd "$PKG" && SHELF_NO_BUILD=1 run sh deploy.sh --only book ) >"$R/out.txt" 2>&1; rc=$?
[ "$rc" -eq 0 ] || sed 's/^/     | /' "$R/out.txt" | tail -8
check "deploy.sh --only book（首参是选项 → host 用默认）：退出 0，只装 gateway+book" test "$rc" -eq 0 -a -x "$R/home/root/.local/bin/book-serve" -a -x "$R/home/root/.local/bin/gateway" -a ! -e "$R/home/root/.local/bin/font-serve"
T=aarch64-unknown-linux-musl; HOLD="$TMPBASE/hold-note-serve"
mv "$REPO/notes/target/$T/release/note-serve" "$HOLD"
new_sandbox
( cd "$PKG" && SHELF_NO_BUILD=1 run sh deploy.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "deploy.sh：缺 note-serve 产物 → 推送前退出非 0、指路 build.sh，设备上什么都没推" test "$rc" -ne 0 -a -n "$(grep 'build.sh' "$R/out.txt")" -a -n "$(grep 'note-serve' "$R/out.txt")" -a ! -e "$R/home/root/shelf-pkg" -a "$(count_log 'tar')" = 0
( cd "$PKG" && SHELF_NO_BUILD=1 run sh deploy.sh 127.0.0.1 --only book,font ) >/dev/null 2>&1; rc=$?
check "deploy.sh：缺的服务不在 --only 里 → 照常部署" test "$rc" -eq 0 -a -x "$R/home/root/.local/bin/font-serve"
mv "$HOLD" "$REPO/notes/target/$T/release/note-serve"
new_sandbox
( cd "$PKG" && SHELF_NO_BUILD=1 run sh deploy.sh 127.0.0.1 --bogus-flag --password 'topsecret' ) >"$R/out.txt" 2>&1; rc=$?
check "deploy.sh：设备端 install.sh 失败（未知参数）→ 退出非 0，密码临时文件不残留" test "$rc" -ne 0 -a ! -e "$R/home/root/shelf-pkg/.pw" -a -z "$(grep -rl topsecret "$R/home" 2>/dev/null)"
( cd "$PKG" && SHELF_NO_BUILD=1 run sh deploy.sh 127.0.0.1 --only bogus ) >/dev/null 2>&1; rc=$?
check "deploy.sh：--only 未知令牌 → 本机就拒绝（退出 2）" test "$rc" -eq 2

section "shelf：verity 下已有单元照常重启 / 卸载保留二进制 / dry-run / 幂等不 remount"
new_sandbox; PL="$R/payload"; mk_payload "$PL"; run sh "$PL/install.sh" >/dev/null 2>&1
echo "# v2" >> "$PL/bin/font-serve"; echo "# v2" >> "$PL/bin/note-serve"
rm -f "$CJ_SYSD/note-serve.service" "$CJ_SYSD/shelf.target.wants/note-serve.service"   # note 没有单元
: > "$CJ_SIM_LOG"; CJ_SIM_VERITY=1 run sh "$PL/install.sh" >"$R/out.txt" 2>&1
check "install + dm-verity + 已有单元：换了二进制的服务照常重启（载入新版），不 remount" test "$(count_log 'restart font-serve.service')" = 1 -a "$(count_log remount)" = 0
check "install + dm-verity：没有单元的服务不去 systemctl restart（无意义）" test "$(count_log 'restart note-serve.service')" = 0
check "install + dm-verity：服务没被拉起时报的是 verity 专属说明而不是笼统\"有服务未起\"" test -n "$(grep 'dm-verity' "$R/out.txt" | grep -v '✋')" -o -n "$(grep '手动跑' "$R/out.txt")"

new_sandbox; PL="$R/payload"; mk_payload "$PL"; run sh "$PL/install.sh" >/dev/null 2>&1
SIG_BEFORE="$(tree_sig)"; : > "$CJ_SIM_LOG"
run sh "$R/home/root/.local/bin/shelf-uninstall" --dry-run --purge >"$R/out.txt" 2>&1; rc=$?
check "uninstall --dry-run：退出 0、列出会删的路径、不改任何文件、不 remount、不停服务" test "$rc" -eq 0 -a -n "$(grep '会删：.*gateway' "$R/out.txt")" -a -n "$(grep -- '--purge 会删' "$R/out.txt")" -a "$(count_log remount)" = 0 -a "$(count_log 'systemctl disable')" = 0
sig_eq "uninstall --dry-run：文件树不变" "$SIG_BEFORE" "$(tree_sig)"
: > "$CJ_SIM_LOG"; CJ_SIM_VERITY=1 run sh "$R/home/root/.local/bin/shelf-uninstall" >"$R/out.txt" 2>&1; rc=$?
check "uninstall + dm-verity：退出 0，/usr 单元还在 → **保留**二进制与 shelf-uninstall（否则重启后服务缺二进制反复失败重启）" test "$rc" -eq 0 -a -f "$CJ_SYSD/gateway.service" -a -x "$R/home/root/.local/bin/gateway" -a -x "$R/home/root/.local/bin/book-serve" -a -x "$R/home/root/.local/bin/shelf-uninstall"
check "uninstall + dm-verity：qmd 照删、输出说明原因" test ! -e "$Q/shelf-mkdir-agent.qmd" -a -n "$(grep '保留二进制' "$R/out.txt")"
: > "$CJ_SIM_LOG"; run sh "$R/home/root/.local/bin/shelf-uninstall" >/dev/null 2>&1; rc=$?
check "可写后再跑一次卸载：收敛到干净（单元、二进制、shelf-uninstall 都清）" test "$rc" -eq 0 -a ! -e "$CJ_SYSD/gateway.service" -a ! -e "$R/home/root/.local/bin/gateway" -a ! -e "$R/home/root/.local/bin/shelf-uninstall"
new_sandbox; PL="$R/payload"; mk_payload "$PL"; run sh "$PL/install.sh" >/dev/null 2>&1
run sh "$R/home/root/.local/bin/shelf-uninstall" >/dev/null 2>&1
: > "$CJ_SIM_LOG"; run sh "$PL/uninstall.sh" >"$R/out.txt" 2>&1; rc=$?
check "uninstall 第二次（已无残留）：退出 0，且不 remount rootfs（幂等）" test "$rc" -eq 0 -a "$(count_log remount)" = 0

section "uninstall-all：载荷清理保守 / wifi-watch 在 verity 下留脚本 / 备份不因重复部署被挤掉"
new_sandbox; export CJ_SKIP_BUILD=1 CJ_BATTOP_BIN="$R/battop.bin"; echo B > "$R/battop.bin"
( cd "$PKG" && DEFER_XOVI_START=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >/dev/null 2>&1
echo "user file" > "$R/home/root/hl-snap/mine.txt"
( cd "$PKG" && run sh uninstall-all.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "uninstall-all：载荷目录里有不认识的文件 → 只删已知文件，目录与该文件保留" test "$rc" -eq 0 -a -f "$R/home/root/hl-snap/mine.txt" -a ! -e "$R/home/root/hl-snap/hl-snap.so" -a ! -e "$R/home/root/hl-snap/deploy"
new_sandbox
( cd "$PKG" && run sh deploy-wifi-watch.sh 127.0.0.1 ) >/dev/null 2>&1
( cd "$PKG" && run sh deploy-wifi-watch.sh 127.0.0.1 ) >/dev/null 2>&1
( cd "$PKG" && run sh deploy-wifi-watch.sh 127.0.0.1 ) >/dev/null 2>&1
check "wifi-watch 重复部署 3 次：脚本内容没变 → 没有堆备份（旧版每次都备份，5 次就把真旧版挤出保留窗口）" test -z "$(ls "$R"/home/root/cangjie-backups/wifi-watch.sh.bak.pre-* 2>/dev/null)"
echo "tampered" >> "$R/home/root/.local/bin/wifi-watch.sh"; : > "$CJ_SIM_LOG"
( cd "$PKG" && run sh deploy-wifi-watch.sh 127.0.0.1 ) >/dev/null 2>&1
check "wifi-watch 设备上的脚本被改过 → 备份一份、恢复为仓库版本并重启服务" test "$(ls "$R"/home/root/cangjie-backups/wifi-watch.sh.bak.pre-* 2>/dev/null | wc -l)" -eq 1 -a -z "$(grep tampered "$R/home/root/.local/bin/wifi-watch.sh")" -a "$(count_log 'restart wifi-watch.service')" = 1
: > "$CJ_SIM_LOG"; CJ_SIM_VERITY=1 bash -c "cd '$PKG' && PATH='$STUBS:'\$PATH sh uninstall-all.sh 127.0.0.1" >"$R/out.txt" 2>&1; rc=$?
check "uninstall-all + dm-verity：退出 0；wifi-watch 单元删不掉 → 保留 wifi-watch.sh（否则服务每 10 秒起一次并失败）" test "$rc" -eq 0 -a -f "$CJ_SYSD/wifi-watch.service" -a -x "$R/home/root/.local/bin/wifi-watch.sh" -a "$(count_log remount)" = 0
: > "$CJ_SIM_LOG"; ( cd "$PKG" && run sh uninstall-all.sh 127.0.0.1 ) >/dev/null 2>&1
check "uninstall-all 可写后再跑：wifi-watch 单元与脚本都清掉" test ! -e "$CJ_SYSD/wifi-watch.service" -a ! -e "$R/home/root/.local/bin/wifi-watch.sh"
new_sandbox
( cd "$PKG" && SHELF_NO_BUILD=1 run sh deploy.sh 127.0.0.1 --only book ) >/dev/null 2>&1
CJ_SIM_VERITY=1 bash -c "cd '$PKG' && PATH='$STUBS:'\$PATH sh uninstall-all.sh 127.0.0.1" >"$R/out.txt" 2>&1; rc=$?
check "uninstall-all + dm-verity：shelf 二进制被保留 → shelf-pkg 载荷（可写后重卸的退路）也保留" test "$rc" -eq 0 -a -x "$R/home/root/.local/bin/gateway" -a -f "$R/home/root/shelf-pkg/shelf/uninstall.sh"
( cd "$PKG" && run sh uninstall-all.sh 127.0.0.1 ) >/dev/null 2>&1; rc=$?
check "uninstall-all 可写后再跑：shelf 卸干净，shelf-pkg 载荷随后删除" test "$rc" -eq 0 -a ! -e "$R/home/root/.local/bin/gateway" -a ! -e "$R/home/root/shelf-pkg"
unset CJ_SKIP_BUILD CJ_BATTOP_BIN

section "chrony-cn / timezone-cn（路径覆盖；只测非 overlay 与 verity 路径，overlay 底层改写只能真机验证）"
new_sandbox; : > "$R/mounts"; CONF="$R/chrony.conf"
printf 'driftfile /var/lib/chrony/drift\nserver time1.google.com iburst\nserver time2.google.com iburst\nmakestep 1.0 3\n' > "$CONF"
RE_ETC_BEFORE="$(md5sum /etc/chrony.conf 2>/dev/null | cut -c1-32)$(readlink -f /etc/localtime 2>/dev/null)"
: > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_CHRONY_CONF="$CONF" CJ_MOUNTS="$R/mounts" CJ_BACKUP_DIR="$R/cbk" CJ_TMPDIR="$R" run sh deploy-chrony-cn.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "chrony-cn：退出 0；google 服务器换成 4 个国内的，driftfile/makestep 等其它行保留" test "$rc" -eq 0 -a "$(grep -c '^server ' "$CONF")" -eq 4 -a -z "$(grep google "$CONF")" -a -n "$(grep '^driftfile' "$CONF")" -a -n "$(grep '^makestep' "$CONF")" -a -n "$(grep '^server ntp.aliyun.com' "$CONF")"
check "chrony-cn：非 overlay 路径不 remount/bind；有改动 → 重启 chronyd" test "$(count_log '^mount')" = 0 -a "$(count_log 'restart chronyd')" = 1
SUM1="$(md5sum < "$CONF")"; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_CHRONY_CONF="$CONF" CJ_MOUNTS="$R/mounts" CJ_BACKUP_DIR="$R/cbk" CJ_TMPDIR="$R" run sh deploy-chrony-cn.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "chrony-cn 第二次：幂等（文件不变、已同步 → 不重启 chronyd、退出 0）" test "$rc" -eq 0 -a "$SUM1" = "$(md5sum < "$CONF")" -a "$(count_log 'restart chronyd')" = 0
printf 'driftfile /x\nmakestep 1 3\n' > "$CONF"
( cd "$PKG" && CJ_CHRONY_CONF="$CONF" CJ_MOUNTS="$R/mounts" CJ_BACKUP_DIR="$R/cbk" CJ_TMPDIR="$R" run sh deploy-chrony-cn.sh 127.0.0.1 ) >/dev/null 2>&1
check "chrony-cn：原配置里一条 server 都没有（用 pool）→ 追加 4 条，而不是每次都\"改不动\"" test "$(grep -c '^server ' "$CONF")" -eq 4
( cd "$PKG" && CJ_SIM_UNSYNCED=1 CJ_CHRONY_CONF="$CONF" CJ_MOUNTS="$R/mounts" CJ_BACKUP_DIR="$R/cbk" CJ_TMPDIR="$R" bash -c "PATH='$STUBS:'\$PATH sh deploy-chrony-cn.sh 127.0.0.1" ) >"$R/out.txt" 2>&1; rc=$?
check "chrony-cn：时钟没同步 → 退出 2 并提示看 journalctl" test "$rc" -eq 2 -a -n "$(grep journalctl "$R/out.txt")"
printf 'overlay /etc overlay rw 0 0\n' > "$R/mounts-ov"; printf 'driftfile /x\nserver a.google.com iburst\n' > "$CONF"; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_SIM_VERITY=1 CJ_CHRONY_CONF="$CONF" CJ_MOUNTS="$R/mounts-ov" CJ_BACKUP_DIR="$R/cbk" CJ_TMPDIR="$R" bash -c "PATH='$STUBS:'\$PATH sh deploy-chrony-cn.sh 127.0.0.1" ) >"$R/out.txt" 2>&1; rc=$?
check "chrony-cn：overlay + dm-verity → 只改 /etc 视图（重启会丢）、不 remount、退出 0" test "$rc" -eq 0 -a "$(count_log remount)" = 0 -a "$(grep -c '^server ntp' "$CONF")" -ge 1 -a -n "$(grep 'dm-verity' "$R/out.txt")"

ZI="$R/Shanghai"; echo TZDATA > "$ZI"; LT="$R/localtime"; echo old > "$LT"
( cd "$PKG" && CJ_ZONEINFO="$ZI" CJ_LOCALTIME="$LT" CJ_MOUNTS="$R/mounts" CJ_BACKUP_DIR="$R/cbk" CJ_TMPDIR="$R" run sh deploy-timezone-cn.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "timezone-cn：退出 0，/etc/localtime（沙箱里的）指向 Asia/Shanghai" test "$rc" -eq 0 -a "$(readlink "$LT")" = "$ZI"
( cd "$PKG" && CJ_ZONEINFO="$ZI" CJ_LOCALTIME="$LT" CJ_MOUNTS="$R/mounts" CJ_BACKUP_DIR="$R/cbk" CJ_TMPDIR="$R" run sh deploy-timezone-cn.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "timezone-cn 第二次（已是目标时区）：仍退出 0（旧版曾因末尾 [ ] && echo 让幂等分支误判失败）" test "$rc" -eq 0
rm -f "$LT"; ( cd "$PKG" && CJ_ZONEINFO="$R/no-such-zone" CJ_LOCALTIME="$LT" CJ_MOUNTS="$R/mounts" CJ_BACKUP_DIR="$R/cbk" CJ_TMPDIR="$R" run sh deploy-timezone-cn.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "timezone-cn：设备镜像没有 zoneinfo → 警告并跳过（退出 0，不拖垮整轮安装）、不建 localtime" test "$rc" -eq 0 -a ! -e "$LT" -a -n "$(grep '跳过' "$R/out.txt")"
RE_ETC_AFTER="$(md5sum /etc/chrony.conf 2>/dev/null | cut -c1-32)$(readlink -f /etc/localtime 2>/dev/null)"
check "chrony/timezone 测试没有碰开发机真实的 /etc/chrony.conf 与 /etc/localtime" test "$RE_ETC_BEFORE" = "$RE_ETC_AFTER"

# ═══════════════════════════ 7. 2026-09-24 审计新增 ═══════════════════════════
section "2026-09-24：待换入 .so 与卸载 / 重启设备后 / 重复部署不重复备份"
HLSO="$REPO/enhance/hl-snap/hl-snap.so"
new_sandbox; EXT="$R/home/root/xovi/extensions.d"; SOP="$R/home/root/.cangjie-stage/so-pending"
echo OLDSO > "$EXT/hl-snap.so"; xovi_live on
( cd "$PKG" && CJ_SKIP_BUILD=1 DEFER_XOVI_START=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >"$R/out1.txt" 2>&1
( cd "$PKG" && CJ_SKIP_BUILD=1 DEFER_XOVI_START=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >"$R/out2.txt" 2>&1; rc=$?
check "同一新版重复 DEFER 部署（还没重启）：第二次不再备份、说明已在待换入区、仍记待生效" test "$rc" -eq 0 -a -z "$(grep '已备份' "$R/out2.txt")" -a -n "$(grep '已在待换入区' "$R/out2.txt")" -a -f "$SOP/hl-snap.so" -a -f "$CJ_PENDING_DIR/hl-snap"
# 模拟"放进待换入区后设备重启过"：/run 里的标记没了，待换入区（/home）还在
rm -rf "$CJ_PENDING_DIR"; : > "$CJ_SIM_LOG"
( cd "$PKG" && run sh deploy-xovi-apply.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "设备重启后（标记已清、待换入区还在）：xovi-apply 仍判定需要生效 → 换入 → 整机重启（旧版会说\"不重启\"，新版永远换不进去）" test "$rc" -eq 0 -a "$(count_log 'systemctl reboot')" = 1 -a "$(md5sum < "$EXT/hl-snap.so")" = "$(md5sum < "$HLSO")" -a ! -e "$SOP"
# 卸载时待换入区里还有新版：必须一起撤掉，否则下一次重启 xochitl 又把它装回 extensions.d
new_sandbox; EXT="$R/home/root/xovi/extensions.d"; SOP="$R/home/root/.cangjie-stage/so-pending"
echo OLDSO > "$EXT/hl-snap.so"; xovi_live on
( cd "$PKG" && CJ_SKIP_BUILD=1 DEFER_XOVI_START=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >/dev/null 2>&1
( cd "$PKG" && run sh uninstall-all.sh 127.0.0.1 --skip shelf ) >"$R/uout.txt" 2>&1; rc=$?
check "uninstall-all：待换入区里的 hl-snap.so 一并撤掉、暂存目录清空" test "$rc" -eq 0 -a ! -e "$EXT/hl-snap.so" -a ! -e "$SOP" -a ! -e "$R/home/root/.cangjie-stage"
check "uninstall-all：xochitl 还加载着被删的 .so → 提示整机重启、别 systemctl restart" grep -q 'reboot' "$R/uout.txt"
: > "$CJ_SIM_LOG"; ( cd "$PKG" && run sh deploy-xovi-apply.sh 127.0.0.1 --force ) >/dev/null 2>&1
check "uninstall-all 之后再重启 xochitl：卸掉的扩展没有被换回 extensions.d" test ! -e "$EXT/hl-snap.so"

# 关键区（换入 → 清标记 → 排重启）里 ssh 断开（SIGPIPE）：忽略信号、照样排上重启，不能换了一半就停
new_sandbox; SOP="$R/home/root/.cangjie-stage/so-pending"; mkdir -p "$SOP"; echo NEWSO > "$SOP/hl-snap.so"; xovi_live on; : > "$CJ_SIM_LOG"
CJ_SIM_PIPE_ON_REBOOT=1 PATH="$STUBS:$PATH" sh -c ". '$PKG/devlib.sh'; cj_xochitl_apply" >/dev/null 2>&1; rc=$?
check "关键区里连接断了（SIGPIPE）：仍然换入、排上 reboot、退出 0" test "$rc" -eq 0 -a "$(count_log 'systemctl reboot')" = 1 -a "$(cat "$R/home/root/xovi/extensions.d/hl-snap.so" 2>/dev/null)" = NEWSO

section "2026-09-24：单独跑的部署没有变化就不重启 xochitl"
new_sandbox; EXT="$R/home/root/xovi/extensions.d"; cp "$HLSO" "$EXT/hl-snap.so"; xovi_live on; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_SKIP_BUILD=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "hl-snap 单独跑：与已装相同 + 已加载 + 无待生效 → 不重启 xochitl、退出 0" test "$rc" -eq 0 -a "$(grep -c -e 'restart xochitl' -e 'stop xochitl' -e XOVI_START "$CJ_SIM_LOG")" = 0 -a -n "$(grep '已是最新' "$R/out.txt")"
mkdir -p "$CJ_PENDING_DIR"; : > "$CJ_PENDING_DIR/shelf-qmd"; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_SKIP_BUILD=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >/dev/null 2>&1; rc=$?
check "hl-snap 单独跑：自己没变但有别的待生效改动（shelf-qmd）→ 照常整机重启一次并清标记" test "$rc" -eq 0 -a "$(count_log 'systemctl reboot')" = 1 -a -z "$(ls -A "$CJ_PENDING_DIR" 2>/dev/null)"
xovi_live off; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_SKIP_BUILD=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >/dev/null 2>&1
check "hl-snap 单独跑：没变但 xovi 还没生效 → 照常 xovi/start" test "$(count_log XOVI_START)" = 1
new_sandbox; xovi_live on
( cd "$PKG" && DEFER_XOVI_START=1 PATH="$STUBS:$PATH" sh deploy-sidebar-entry.sh 127.0.0.1 ) >/dev/null 2>&1
( cd "$PKG" && run sh deploy-xovi-apply.sh 127.0.0.1 ) >/dev/null 2>&1
: > "$CJ_SIM_LOG"
( cd "$PKG" && PATH="$STUBS:$PATH" sh deploy-sidebar-entry.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "sidebar-entry 单独跑：qmd/rcc 没变且无待生效 → 不重启 xochitl、退出 0" test "$rc" -eq 0 -a "$(grep -c -e 'restart xochitl' -e 'stop xochitl' -e XOVI_START "$CJ_SIM_LOG")" = 0 -a -n "$(grep '已是最新' "$R/out.txt")"

section "2026-09-24：wifi-watch 在 verity 下更新脚本 / shelf --no-systemd 不空等 / shelf_select"
new_sandbox
( cd "$PKG" && run sh deploy-wifi-watch.sh 127.0.0.1 ) >/dev/null 2>&1
echo "# old" >> "$R/home/root/.local/bin/wifi-watch.sh"; : > "$CJ_SIM_LOG"
CJ_SIM_VERITY=1 bash -c "cd '$PKG' && PATH='$STUBS:'\$PATH sh deploy-wifi-watch.sh 127.0.0.1" >"$R/out.txt" 2>&1; rc=$?
check "wifi-watch + dm-verity + 单元是以前装的：脚本更新后重启服务用上新脚本，不 remount、退出 0" test "$rc" -eq 0 -a "$(count_log 'restart wifi-watch.service')" = 1 -a "$(count_log remount)" = 0 -a -z "$(grep '# old' "$R/home/root/.local/bin/wifi-watch.sh")"
: > "$CJ_SIM_LOG"; CJ_SIM_VERITY=1 bash -c "cd '$PKG' && PATH='$STUBS:'\$PATH sh deploy-wifi-watch.sh 127.0.0.1" >/dev/null 2>&1
check "wifi-watch + dm-verity：脚本没变 → 不重启" test "$(count_log 'restart wifi-watch.service')" = 0
new_sandbox; PL="$R/payload"; mk_payload "$PL"; : > "$CJ_SIM_LOG"
CJ_SIM_INACTIVE=1 run sh "$PL/install.sh" --no-systemd >/dev/null 2>&1; rc=$?
check "shelf install --no-systemd（服务都没在跑）：不空等 10 轮健康检查（is-active 只查一轮 + 汇总一轮）、退出 0" test "$rc" -eq 0 -a "$(count_log 'systemctl is-active')" -le 18
# shellcheck disable=SC1091
check "shelf_select：空 → 全部；去重且网关在最前；未知令牌 → 返回 2" bash -c ". '$REPO/shelf/manifest.sh'; [ \"\$(shelf_select '')\" = \"\$SHELF_ALL\" ] && [ \"\$(shelf_select 'book,font,book,gateway')\" = 'gateway book font' ] && { shelf_select 'book,nope' 2>/dev/null; [ \$? -eq 2 ]; }"

section "2026-09-24：前置条件不满足的\"跳过\"在汇总里单列，不混进\"已安装\""
new_sandbox; rm -rf "$R/home/root/xovi/exthome/appload"; export CJ_ALLOWLIST_LOCAL="$R/allow.local.txt"
( cd "$PKG" && run sh install-all.sh 127.0.0.1 --force --skip chrony-cn,timezone-cn,battop,wifi-watch,xovi-persist,chrony-boot-wakelock,hl-snap,handwriting-stroke,shelf ) >"$R/out.txt" 2>&1; rc=$?
check "install-all：没装 appload → sidebar-entry 记进\"前置条件不满足\"并写明原因，不在\"已安装\"里、整轮退出 0" test "$rc" -eq 0 -a -n "$(grep '前置条件不满足' "$R/out.txt" | head -n 1)" -a -n "$(grep 'sidebar-entry：设备没装 appload' "$R/out.txt")" -a -z "$(grep '^已安装：.*sidebar-entry' "$R/out.txt")"
new_sandbox
CJ_SIM_VERITY=1 bash -c "cd '$PKG' && PATH='$STUBS:'\$PATH && . ./lib.sh && HOST=127.0.0.1 && run_step chrony-boot-wakelock sh ./deploy-chrony-boot-wakelock.sh 127.0.0.1 >/dev/null 2>&1; echo \"D=\$DONE|N=\$NOTAPPL\"" >"$R/out.txt" 2>&1
check "run_step：dm-verity 下单元从没装过 → 记为\"前置条件不满足\"而不是已安装" test -n "$(grep '^D=|N=' "$R/out.txt")" -a -n "$(grep '^   chrony-boot-wakelock：dm-verity' "$R/out.txt")"
unset CJ_ALLOWLIST_LOCAL

# ═══════════════════════════ 8. 2026-09-25：部署后健康核对 verify-on-device.sh ═══════════════════════════
section "2026-09-25：verify-on-device.sh —— host 端判定（--from 离线喂构造的采集文本）"
new_sandbox; export CJ_ALLOWLIST_LOCAL="$R/allow.local.txt"
FW_OK="$(awk '!/^#/ && /3\.28\.0\.172/ { print $1; exit }' "$PKG/firmware-allowlist.txt")"
VSVCS="gateway book-serve koreader-serve font-serve wallpaper-serve ink-serve transcribe-serve mind-serve note-serve"
# 一台"健康设备"的采集文本（字段用 | 写，转成 TAB）：扩展都映射且无 (deleted)、qmd 都在且早于 xochitl 启动、
# 9 个服务 active、端口都在听、单元都在、无告警、空间充足
mk_dump() {
    {
        echo "VERSION|1"; echo "NOW|2000000000"; echo "UPTIME|100000.00"; echo "FW_SHA|$FW_OK"; echo "FW_VER|3.28.0.172"
        echo "X_ACTIVE|active"; echo "X_PID|4242"; echo "X_NRESTARTS|0"; echo "X_START_MONO|7400000"; echo "X_XOVI|1"
        echo "XMAP|/home/root/xovi/extensions.d/hl-snap.so|3|0"; echo "XMAP|/home/root/xovi/extensions.d/hw-stroke.so|3|0"
        echo "HAS_XOVI|1"; echo "EXT_FILE|hl-snap.so|1999000000"; echo "EXT_FILE|hw-stroke.so|1999000000"
        echo "HAS_QRR|1"
        for q in font-menu-dynamic.qmd shelf-trash-agent.qmd shelf-mkdir-agent.qmd shelf-comic-margins.qmd reader-page-turn.qmd koreader-sidebar-entry.qmd cangjie-icons.rcc; do echo "QRR_FILE|$q|1999000000"; done
        echo "HAS_APPLOAD|1"
        for s in $VSVCS; do echo "UNIT|svc|$s.service|1|1|active|0|5000|10240|20480|7600000|-"; done
        echo "UNIT|target|shelf.target|1|1|active|-|-|-|-|-|-"
        echo "UNIT|svc|wifi-watch.service|1|1|active|0|5001|512|600|7000000|-"
        echo "UNIT|opt|battop.service|1|1|inactive|0|0|-|-|0|-"
        echo "UNIT|once|xovi-reenable.service|1|1|active|-|-|-|-|-|-"
        echo "UNIT|once|chrony-boot-wakelock.service|1|-|inactive|-|-|-|-|-|-"
        echo "PORTINFO|1"; echo "LISTEN|443|0.0.0.0"
        for p in 8790 8791 8792 8793 8795 8796 8797 8798; do echo "LISTEN|$p|127.0.0.1"; done
        echo "JOURNAL|1"; echo "XLOG|hookok:hl-snap|1"; echo "XLOG|hookok:hw-stroke|3"; echo "XLOG|hookfail:hl-snap|0"
        echo "XLOG|mark:reader-page-turn.qmd|2"; echo "FLIGHT_ABSENT|1"; echo "DF_HOME|3000000"; echo "END|1"
    } | tr '|' '\t'
}
# vr ARGS…：跑 verify-on-device.sh，输出进 $R/vout.txt，返回退出码；vsum KEY：从汇总行取 ok/warn/fail/result
vr() { ( cd "$PKG" && run sh verify-on-device.sh "$@" ) >"$R/vout.txt" 2>&1; }
vsum() { sed -n "s/^VERIFY-SUMMARY .*$1=\([A-Z0-9]*\).*/\1/p" "$R/vout.txt"; }
vline() { grep -F -- "$1" "$R/vout.txt" | head -n 1; }
mk_dump > "$R/ok.dump"
: > "$CJ_SIM_LOG"; vr 127.0.0.1 --from "$R/ok.dump"; rc=$?
check "健康采集：退出 0、result=PASS、无 ⚠/✗" test "$rc" -eq 0 -a "$(vsum result)" = PASS -a "$(vsum warn)" = 0 -a "$(vsum fail)" = 0
check "--from：不连设备（没有任何 ssh/scp）" test "$(count_log '^ssh')" = 0 -a "$(count_log '^scp')" = 0
check "健康采集：扩展行带 maps 段数与 hook「安装完成」次数" test -n "$(vline '✓ 扩展 hw-stroke.so：已映射 3 段，日志 hook「安装完成」×3')"
check "健康采集：qmd 有日志佐证时注明（CJ-PAGE-TURN: loaded ×2）" test -n "$(vline 'CJ-PAGE-TURN: loaded」×2')"
check "健康采集：battop inactive 不算问题（有意不开机自启）" test -n "$(vline '✓ battop.service：inactive')"
check "健康采集：最后一行是机器可读汇总" test "$(tail -n 1 "$R/vout.txt" | cut -d' ' -f1)" = VERIFY-SUMMARY

# 逐项坏掉：期望的级别与退出码
vcase() { # 描述 期望退出码(0/1) 期望行里的子串 sed表达式…（作用在健康采集上）
    vc_d="$1"; vc_rc="$2"; vc_s="$3"; shift 3
    mk_dump > "$R/case.dump"
    for vc_e in "$@"; do sed -i "$vc_e" "$R/case.dump"; done
    vr 127.0.0.1 --from "$R/case.dump"; vc_got=$?
    if [ "$vc_got" -eq "$vc_rc" ] && [ -n "$(vline "$vc_s")" ]; then ok "$vc_d"; else bad "$vc_d（rc=$vc_got）"; grep -e '✗' -e '⚠' "$R/vout.txt" | head -n 5 | sed 's/^/       /'; fi
}
T=$'\t'
vcase "扩展 .so 在 maps 里是 (deleted) → ✗、退出 1" 1 "✗ 扩展 hw-stroke.so：maps 里是 (deleted)" "s#^XMAP${T}\(.*hw-stroke.so\)${T}3${T}0#XMAP${T}\1${T}3${T}1#"
vcase "extensions.d 里有非 .so 文件（备份）→ ✗" 1 "✗ 扩展目录异物 hl-snap.so.bak" "\$a EXT_FILE${T}hl-snap.so.bak${T}1"
vcase "extensions.d 里有 .crashed 标记 → 只 ⚠" 0 "⚠ 扩展 hl-snap.so.crashed" "\$a EXT_FILE${T}hl-snap.so.crashed${T}1"
vcase "扩展在目录里但没被映射 → ⚠" 0 "⚠ 扩展 hl-snap.so：在 extensions.d 里但没被当前 xochitl 映射" "/^XMAP${T}.*hl-snap/d"
vcase "当前 xochitl 日志有「hook 未安装」→ 该扩展 ✗" 1 "✗ 扩展 hl-snap.so：已映射 3 段，但当前 xochitl 日志有「hook 未安装」×2" "s/^XLOG${T}hookfail:hl-snap${T}0/XLOG${T}hookfail:hl-snap${T}2/"
vcase "待换入区有 .so → ⚠、退出 0（result=WARN）" 0 "⚠ 待换入区 so-pending：有 hw-stroke.so" "\$a PENDING${T}so-pending:hw-stroke.so"
check "  └ result=WARN" test "$(vsum result)" = WARN
vcase "有待生效标记 → ⚠" 0 "⚠ 待生效标记：shelf-qmd" "\$a PENDING${T}shelf-qmd"
vcase "xovi 没在 xochitl 里生效 → ✗" 1 "✗ xovi：xochitl 没带 xovi" "s/^X_XOVI${T}1/X_XOVI${T}0/"
vcase "设备没装 xovi → 只 ⚠" 0 "⚠ xovi：设备上没有 xovi.so" "s/^X_XOVI${T}1/X_XOVI${T}0/" "s/^HAS_XOVI${T}1/HAS_XOVI${T}0/" "/^XMAP/d" "/^EXT_FILE/d"
vcase "xochitl NRestarts>0 → ⚠" 0 "⚠ xochitl：active，MainPID=4242，NRestarts=2" "s/^X_NRESTARTS${T}0/X_NRESTARTS${T}2/"
vcase "xochitl 不是 active → ✗" 1 "✗ xochitl：is-active=failed" "s/^X_ACTIVE${T}active/X_ACTIVE${T}failed/"
vcase "固件哈希不在白名单 → ✗" 1 "✗ 固件：IMG_VERSION=3.28.0.172，sha256=deadbeef 不在白名单" "s/^FW_SHA${T}.*/FW_SHA${T}deadbeef/"
echo "cafebabe  (--force 追加，未验证，2026-09-25)" > "$CJ_ALLOWLIST_LOCAL"
vcase "固件哈希只在本机 --force 白名单 → ⚠" 0 "⚠ 固件：IMG_VERSION=3.28.0.172，sha256 只在本机" "s/^FW_SHA${T}.*/FW_SHA${T}cafebabe/"
rm -f "$CJ_ALLOWLIST_LOCAL"
vcase "开机不久 → ⚠（提示查意外整机重启）" 0 "⚠ 开机时长：1分40秒" "s/^UPTIME${T}.*/UPTIME${T}100.5/"
vcase "qmd 缺失（所属服务已装）→ ✗" 1 "✗ reader-page-turn.qmd：缺失" "/^QRR_FILE${T}reader-page-turn.qmd/d"
vcase "qmd 比 xochitl 进程新 → ⚠ 待重启" 0 "⚠ shelf-mkdir-agent.qmd：文件比当前 xochitl 进程新" "s/^QRR_FILE${T}shelf-mkdir-agent.qmd${T}.*/QRR_FILE${T}shelf-mkdir-agent.qmd${T}1999999999/"
vcase "book-serve 没装 → 不期望它的 qmd/端口，单元缺失只 ⚠" 0 "⚠ book-serve.service：没装" "/^QRR_FILE${T}\(shelf-\|reader-page\)/d" "/^LISTEN${T}8790/d" "s/^UNIT${T}svc${T}book-serve.service${T}1${T}1/UNIT${T}svc${T}book-serve.service${T}0${T}0/"
check "  └ 没有报它的 qmd 缺失" test -z "$(vline '✗ reader-page-turn.qmd')"
vcase "没装 qt-resource-rebuilder → qmd 整节只一条 ⚠" 0 "⚠ qt-resource-rebuilder" "s/^HAS_QRR${T}1/HAS_QRR${T}0/" "/^QRR_FILE/d"
vcase "旧命名遗留 qmd → ⚠" 0 "⚠ font-menu-dynamic-3.27.qmd：旧命名遗留" "\$a QRR_FILE${T}font-menu-dynamic-3.27.qmd${T}1"
vcase "单元文件不在但载荷在（OTA 冲掉）→ ✗" 1 "✗ gateway.service：单元文件不在" "s/^UNIT${T}svc${T}gateway.service${T}1/UNIT${T}svc${T}gateway.service${T}0/"
vcase "服务 inactive → ✗" 1 "✗ ink-serve.service：is-active=failed" "s/^UNIT${T}svc${T}ink-serve.service${T}1${T}1${T}active/UNIT${T}svc${T}ink-serve.service${T}1${T}1${T}failed/"
vcase "服务 NRestarts>0 → ⚠" 0 "⚠ mind-serve.service：active，但 NRestarts=3" "s/^UNIT${T}svc${T}mind-serve.service${T}1${T}1${T}active${T}0/UNIT${T}svc${T}mind-serve.service${T}1${T}1${T}active${T}3/"
vcase "服务峰值内存超阈值 → ⚠" 0 "⚠ book-serve.service：active，峰值内存偏高" "s/^\(UNIT${T}svc${T}book-serve.service${T}1${T}1${T}active${T}0${T}5000${T}10240${T}\)20480/\1900000/"
vcase "服务很晚才启动 → 注明开机后被重启过" 0 "开机后 1小时0分 才启动（开机后被重启过）" "s/^\(UNIT${T}svc${T}note-serve.service${T}.*${T}\)7600000${T}-\$/\13600000000${T}-/"
vcase "端口没人听 → ✗" 1 "✗ 8796（transcribe-serve）：没有进程在监听" "/^LISTEN${T}8796/d"
vcase "领域服务监听 0.0.0.0 → ⚠" 0 "⚠ 8791（koreader-serve）：监听 0.0.0.0" "s/^LISTEN${T}8791${T}127.0.0.1/LISTEN${T}8791${T}0.0.0.0/"
vcase "网关只听回环 → ⚠" 0 "⚠ 443（gateway）：只监听 127.0.0.1" "s/^LISTEN${T}443${T}0.0.0.0/LISTEN${T}443${T}127.0.0.1/"
vcase "单元里 --bind 的端口优先于兜底表" 0 "✓ 8899（font-serve）" "s/^\(UNIT${T}svc${T}font-serve.service${T}.*${T}\)-\$/\18899/" "\$a LISTEN${T}8899${T}127.0.0.1"
vcase "journal 有 panic → ✗" 1 "✗ panic：1 条相关日志；最近一条：thread 'main' panicked at src/x.rs" "\$a ALERT${T}J${T}panic${T}1${T}thread 'main' panicked at src/x.rs"
vcase "dmesg 与 journal 同一次 OOM 各一份 → 取较大计数、样例用 journal" 1 "✗ OOM：3 条相关日志；最近一条：J-sample" "\$a ALERT${T}K${T}oom${T}3${T}K-sample" "\$a ALERT${T}J${T}oom${T}2${T}J-sample"
vcase "SHELF-MKDIR 超时只 ⚠" 0 "⚠ SHELF-MKDIR 超时：4 条相关日志" "\$a ALERT${T}J${T}mkdir-timeout${T}4${T}SHELF-MKDIR: transfer timeout after 9000ms"
vcase "/home 空间不足 → ✗" 1 "✗ /home：剩余 10.0MB" "s/^DF_HOME${T}.*/DF_HOME${T}10240/"
vcase "/home 空间偏紧 → ⚠" 0 "⚠ /home：剩余 100.0MB" "s/^DF_HOME${T}.*/DF_HOME${T}102400/"
vcase "飞行记录仪：打印最后几行" 0 "│ 2026-09-25 03:00 freeze?" "/^FLIGHT_ABSENT/d" "\$a FLIGHT_MTIME${T}1999999000" "\$a FLIGHT${T}2026-09-25 03:00 freeze?"
vcase "采集不完整（缺 END）→ ✗" 1 "✗ 采集结果：不完整" "/^END/d"
printf 'garbage\n' > "$R/bad.dump"; vr 127.0.0.1 --from "$R/bad.dump"; rc=$?
check "不是采集文本 → ✗、退出 1" test "$rc" -eq 1 -a -n "$(vline '缺 VERSION 行')"
# --json：合法 JSON，计数与文本模式一致
mk_dump | sed "\$a PENDING${T}so-pending:x.so" > "$R/j.dump"
vr 127.0.0.1 --from "$R/j.dump"; tw="$(vsum warn)"; tf="$(vsum fail)"
vr 127.0.0.1 --from "$R/j.dump" --json; rc=$?
if command -v python3 >/dev/null 2>&1; then
    jsum="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["warn"], d["fail"], d["result"], all(set(i)=={"level","section","title","msg","detail"} for i in d["items"]))' "$R/vout.txt" 2>&1)"
    check "--json：合法 JSON，warn/fail 计数与文本模式一致、result=WARN" test "$rc" -eq 0 -a "$jsum" = "$tw $tf WARN True"
else
    check "--json：输出以 { 开头（本机无 python3，不做完整解析）" test "$(cut -c1 "$R/vout.txt" | head -n 1)" = "{"
fi
# 参数解析：错参数退出 2 且不连设备
: > "$CJ_SIM_LOG"
vr --bogus; rc1=$?; vr a b; rc2=$?; vr --json --dump; rc3=$?; vr --from "$R/nope.dump"; rc4=$?; vr --flight-lines x; rc5=$?
vr -h; rc6=$?
check "verify：未知参数/两个 host/--json+--dump/--from 不存在/--flight-lines 非数字 → 退出 2；-h → 0；都没连设备" test "$rc1$rc2$rc3$rc4$rc5$rc6" = "222220" -a "$(count_log '^ssh')" = 0
unset CJ_ALLOWLIST_LOCAL

section "2026-09-25：verify-on-device.sh —— 设备端采集（假 ssh 跑真采集脚本）：只读、一次连接、内核命令行 panic=N 不误报"
new_sandbox; export CJ_ALLOWLIST_LOCAL="$R/allow.local.txt"; xovi_live on
mkdir -p "$R/proc/net"
echo "100000.00 1.00" > "$R/proc/uptime"
printf 'Name:\tx\nVmHWM:\t  20480 kB\nVmRSS:\t  10240 kB\n' > "$R/proc/4242/status"
{ echo "  sl  local_address rem_address   st"
  echo "   0: 00000000:01BB 00000000:0000 0A 0"; n=1
  for p in 8790 8791 8792 8793 8795 8796 8797 8798; do printf '   %d: 0100007F:%04X 00000000:0000 0A 0\n' "$n" "$p"; n=$((n + 1)); done
  echo "   9: 0100007F:1F90 0100007F:D431 01 0"; } > "$R/proc/net/tcp"   # 最后一行是已建立连接（st=01），不算监听
printf '  sl  local_address                         remote_address                        st\n   0: 0000000000000000FFFF00000100007F:22B6 00000000000000000000000000000000:0000 0A 0\n' > "$R/proc/net/tcp6"
: > "$R/home/root/xovi/extensions.d/hl-snap.so"; : > "$R/home/root/xovi/extensions.d/hw-stroke.so"
for s in $VSVCS; do : > "$B/$s"; printf '[Service]\nExecStart=/home/root/.local/bin/%s\n' "$s" > "$R/usr/lib/systemd/system/$s.service"; done
for u in shelf.target xovi-reenable.service wifi-watch.service battop.service chrony-boot-wakelock.service; do : > "$R/usr/lib/systemd/system/$u"; done
: > "$B/wifi-watch.sh"; mkdir -p "$R/home/root/battop"; : > "$R/home/root/battop/battop"
for q in font-menu-dynamic.qmd shelf-trash-agent.qmd shelf-mkdir-agent.qmd shelf-comic-margins.qmd reader-page-turn.qmd koreader-sidebar-entry.qmd cangjie-icons.rcc; do : > "$Q/$q"; done
printf 'f1\nf2\nf3\nf4\tx\n' > "$R/host-flight.log"   # 飞行记录仪在宿主机上（09-25 更正）；末行带 TAB：要压成空格，不能错位
export CJ_FLIGHT_LOG="$R/host-flight.log"
cat > "$R/journal.txt" <<'EOF'
Kernel command line: console=ttymxc0,115200 panic=2 rootwait
[hl-snap] 荧光笔EXPAND hook 安装完成 @ 0x1（neuter=0）
[hw-stroke] 变宽几何 hook 安装完成 @ 0x2（factor=1.000）
CJ-PAGE-TURN: loaded
EOF
export CJ_SIM_JOURNAL="$R/journal.txt"
PRE_SIG="$(tree_sig)"; : > "$CJ_SIM_LOG"
vr 127.0.0.1 --flight-lines 2; rc=$?
check "采集：内核命令行里的 panic=2 不算 panic（2026-09-25 真机误报）" test -n "$(vline '✓ panic：无 panic')"
check "采集：全链路跑通、没有采集错误（有 END、单元/端口/qmd/扩展都读到）" test -z "$(vline '采集结果')" -a -n "$(vline '✓ 443（gateway）：监听 0.0.0.0')" -a -n "$(vline '✓ 8790（book-serve）：监听 127.0.0.1')" -a -n "$(vline '✓ 扩展 hl-snap.so：已映射 1 段，日志 hook「安装完成」×1')" -a -n "$(vline '✓ 单元：14/14 个在位')"
check "采集：飞行记录仪只取 --flight-lines 条（最后 2 行）" test -n "$(vline '│ f4 x')" -a -n "$(vline '│ f3')" -a -z "$(vline '│ f2')"
check "采集：/proc/net/tcp6 的 IPv4 映射地址解析成 127.0.0.1（8886 端口）" test -n "$( ( cd "$PKG" && run sh verify-on-device.sh 127.0.0.1 --dump ) 2>/dev/null | grep "^LISTEN${T}8886${T}127.0.0.1$")"
( cd "$PKG" && run sh verify-on-device.sh 127.0.0.1 --dump ) > "$R/host.dump" 2>/dev/null; vr --from "$R/host.dump"
check "--dump 记下真实主机，--from 不给 host 时页头用它（09-25：WiFi 地址被显示成缺省 10.11.99.1）" test -n "$(vline '127.0.0.1（离线：host.dump）')"
unset CJ_FLIGHT_LOG
: > "$CJ_SIM_LOG"; vr 127.0.0.1
sig_eq "采集：设备文件树前后一字不差（只读）" "$PRE_SIG" "$(tree_sig)"
check "采集：没有 start/stop/restart/enable/disable/daemon-reload、没有 mount、没有 scp" test -z "$(grep -E '^systemctl (start|stop|restart|reload|enable|disable|daemon-reload|kill|reset-failed)|^mount |^scp |XOVI_START' "$CJ_SIM_LOG")"
check "采集：一次 ssh 连通检查 + 一次 ssh 采集（不逐项连）" test "$(count_log '^ssh')" = 2 -a "$(count_log '^ssh sh -s')" = 1
check "采集：设备全健康时退出 0（固件桩哈希不在白名单除外，已单独断言）" test "$(vsum fail)" = 1 -a -n "$(vline '✗ 固件')"
# 真 panic / hook 未安装 / dmesg OOM 能从采集里抓到
printf "thread 'main' panicked at gateway/src/main.rs:10:5\n[hw-stroke] _xovi_construct: 找不到 xochitl 映射，hook 未安装\nxochitl: processed more than once!\n" >> "$R/journal.txt"
printf '[  12.3] Out of memory: Killed process 777 (book-serve)\n[    0.0] Kernel command line: panic=2\n' > "$R/dmesg.txt"; export CJ_SIM_DMESG="$R/dmesg.txt"
vr 127.0.0.1; rc=$?
check "采集：真 panic / hook 未安装 / 扩展重复注册 / dmesg 里的 OOM 都判 ✗、退出 1" test "$rc" -eq 1 -a -n "$(vline "✗ panic：1 条相关日志；最近一条：thread 'main' panicked")" -a -n "$(vline '✗ hook 未安装：1 条')" -a -n "$(vline '✗ 扩展重复注册')" -a -n "$(vline '✗ OOM：1 条相关日志；最近一条：[  12.3] Out of memory')"
unset CJ_SIM_JOURNAL CJ_SIM_DMESG
# 服务 inactive / 设备连不上
CJ_SIM_INACTIVE_UNITS="note-serve.service" vr 127.0.0.1
check "采集：某个服务 inactive → 该服务 ✗" test -n "$(vline '✗ note-serve.service：is-active=inactive')"
: > "$CJ_SIM_LOG"; CJ_SIM_SSH_FAIL=1 vr 127.0.0.1; rc=$?
check "采集：设备连不上 → 退出 1、打印\"连不上\"与排查步骤" test "$rc" -eq 1 -a -n "$(vline '连不上')"
# 静态守卫：采集脚本（heredoc）里不许出现任何改设备状态的命令
awk '/<<.DEVICE_SCRIPT.$/ { on = 1; next } /^DEVICE_SCRIPT$/ { on = 0 } on' "$PKG/verify-on-device.sh" | grep -v '^[[:space:]]*#' > "$R/collector.sh"
check "静态守卫：verify 采集脚本非空" test -s "$R/collector.sh"
check "静态守卫：verify 采集脚本不含 systemctl 写操作 / mount / rm / mv / cp / 写文件重定向 / cj_xochitl_apply / 标记增删" test -z "$(grep -nE 'systemctl +(start|stop|restart|reload|enable|disable|daemon-reload|kill|mask|reset-failed)|\bmount\b|(^|[;&|[:space:]])(rm|mv|cp|mkdir|touch|ln|chmod|kill)[[:space:]]|>>|>[[:space:]]*"?\$|cj_xochitl_apply|cj_pending_(mark|clear)|cj_so_(stage|commit|unstage)|xovi/start' "$R/collector.sh")"
unset CJ_ALLOWLIST_LOCAL

# ═══════════════════════════ 9. 2026-09-25 第四轮审计新增 ═══════════════════════════
section "2026-09-25 第四轮：待换入区过时版本 / 重启失败 / battop verity / 连接次数"
HLSO="$REPO/enhance/hl-snap/hl-snap.so"
# 待换入区里有更早一轮的旧版（xovi 暂未生效时直接原子替换）：必须撤掉，否则随后的生效步骤会把它盖回来
new_sandbox; EXT="$R/home/root/xovi/extensions.d"; SOP="$R/home/root/.cangjie-stage/so-pending"
echo OLDSO > "$EXT/hl-snap.so"; mkdir -p "$SOP"; echo STALE > "$SOP/hl-snap.so"; xovi_live off
( cd "$PKG" && CJ_SKIP_BUILD=1 DEFER_XOVI_START=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "xovi 未生效时直接装新版：待换入区里过时的旧版一并撤掉" test "$rc" -eq 0 -a ! -e "$SOP/hl-snap.so" -a "$(md5sum < "$EXT/hl-snap.so")" = "$(md5sum < "$HLSO")"
( cd "$PKG" && run sh deploy-xovi-apply.sh 127.0.0.1 ) >/dev/null 2>&1
check "  └ 随后生效（xovi/start）：extensions.d 里仍是新版，没被过时的待换入版本盖回" test "$(md5sum < "$EXT/hl-snap.so")" = "$(md5sum < "$HLSO")"

# systemctl reboot 本身失败：不能让 host 空等设备重启并把核对结果当成本步结果；标记要补回去，下次还会再试
new_sandbox; EXT="$R/home/root/xovi/extensions.d"; echo x > "$EXT/hl-snap.so"; xovi_live on
mkdir -p "$CJ_PENDING_DIR"; : > "$CJ_PENDING_DIR/shelf-qmd"; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_SIM_REBOOT_FAIL=1 CJ_APPLY_VERIFY=1 CJ_REBOOT_DOWN_WAIT=0 CJ_REBOOT_UP_WAIT=0 CJ_REBOOT_SETTLE=0 run sh deploy-xovi-apply.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "reboot 失败：退出非 0、提示手动 reboot、不去等设备重启/不跑核对" test "$rc" -ne 0 -a -n "$(grep '手动 reboot' "$R/out.txt")" -a -z "$(grep -e '设备已回来' -e '^VERIFY-SUMMARY' "$R/out.txt")"
check "reboot 失败：补回待生效标记（下次 xovi-apply 仍会判定需要生效）" test -n "$(ls -A "$CJ_PENDING_DIR" 2>/dev/null)"

# battop + dm-verity：单元以前装过 → 换了二进制且在跑就重启用上新版（与 wifi-watch 09-24 同一类）；旧 timer 不碰 rootfs
new_sandbox; echo BIN1 > "$R/battop.bin"
( cd "$PKG" && CJ_BATTOP_BIN="$R/battop.bin" run sh deploy-battop.sh 127.0.0.1 ) >/dev/null 2>&1
echo BIN2 > "$R/battop.bin"; : > "$CJ_SYSD/battop.timer"; : > "$CJ_SIM_LOG"
CJ_SIM_VERITY=1 bash -c "cd '$PKG' && CJ_BATTOP_BIN='$R/battop.bin' PATH='$STUBS:'\$PATH sh deploy-battop.sh 127.0.0.1" >"$R/out.txt" 2>&1; rc=$?
check "battop + dm-verity + 单元以前装过：二进制更新后重启服务载入新版，不 remount、退出 0" test "$rc" -eq 0 -a "$(cat "$R/home/root/battop/battop")" = BIN2 -a "$(count_log 'restart battop.service')" = 1 -a "$(count_log remount)" = 0
check "battop + dm-verity：旧 battop.timer 不去删（rootfs 不可写）" test -f "$CJ_SYSD/battop.timer"
new_sandbox; echo BIN1 > "$R/battop.bin"
CJ_SIM_VERITY=1 bash -c "cd '$PKG' && PATH='$STUBS:'\$PATH && . ./lib.sh && HOST=127.0.0.1 && CJ_BATTOP_BIN='$R/battop.bin' run_step battop sh ./deploy-battop.sh 127.0.0.1 >/dev/null 2>&1; echo \"D=\$DONE|N=\$NOTAPPL\"" >"$R/out.txt" 2>&1
check "run_step：battop 在 dm-verity 下单元从没装过 → 记为\"前置条件不满足\"而不是已安装" test -n "$(grep '^D=|N=' "$R/out.txt")" -a -n "$(grep '^   battop：dm-verity' "$R/out.txt")"

# chrony-boot-wakelock：收到 TERM（关机/重启时 systemd 发）要放锁并**退出**，不能接着轮询到 120 秒
WL="$R/wl"; mkdir -p "$WL/bin"; : > "$WL/lock"; : > "$WL/unlock"
printf '#!/bin/sh\necho no\n' > "$WL/bin/timedatectl"; chmod +x "$WL/bin/timedatectl"
WL_CMD="$(sed -n '/^ExecStart=/,/[^\\]$/p' "$PKG/chrony-boot-wakelock.service" | sed -e 's/^ExecStart=\/bin\/sh -c .//' -e 's/\\$//' | tr -d '\n' | sed -e "s/'\$//" -e "s#/sys/power/wake_lock#$WL/lock#g" -e "s#/sys/power/wake_unlock#$WL/unlock#g")"
PATH="$WL/bin:$PATH" sh -c "$WL_CMD" & wl_pid=$!
# 等它真的拿到锁（trap 已挂上）再发 TERM——固定 sleep 在机器繁忙时会抢在 trap 之前，测出假失败
for _i in $(seq 1 50); do [ -s "$WL/lock" ] && break; sleep 0.1; done
kill -TERM "$wl_pid" 2>/dev/null
# sh 在前台 sleep 2 结束后才处理 trap（真机上 systemd 连 sleep 一起 TERM，立即退出）：最多等 6 秒
wl_gone=0; for _i in $(seq 1 30); do kill -0 "$wl_pid" 2>/dev/null || { wl_gone=1; break; }; sleep 0.2; done
kill -KILL "$wl_pid" 2>/dev/null; wait "$wl_pid" 2>/dev/null
check "chrony-boot-wakelock：拿到锁；收到 TERM 后几秒内退出（不再轮询到超时）并放锁" test "$(cat "$WL/lock")" = cangjie-chrony-boot -a "$wl_gone" = 1 -a "$(cat "$WL/unlock")" = cangjie-chrony-boot

# chrony-cn / timezone-cn 的 overlay 分支：写 rootfs 底层失败要报错退出（且恢复 ro），不能打印"已改"
# （假 mount 不真的 bind，沙箱里预先放好 "bind 视图" 目录并设为只读，让写底层失败）
new_sandbox; printf 'overlay /etc overlay rw 0 0\n' > "$R/mounts-ov"
mkdir -p "$R/chrony-cn.rootbind/etc" "$R/timezone-cn.rootbind/etc"
printf 'server a.google.com iburst\n' > "$R/chrony-cn.rootbind/etc/chrony.conf"; chmod 555 "$R/chrony-cn.rootbind/etc"
printf 'server a.google.com iburst\n' > "$R/chrony.conf"; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_CHRONY_CONF="$R/chrony.conf" CJ_MOUNTS="$R/mounts-ov" CJ_BACKUP_DIR="$R/cbk" CJ_TMPDIR="$R" run sh deploy-chrony-cn.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "chrony-cn overlay：写底层失败 → 退出非 0、报错、不说\"已改\"、最后一次 mount 是 ro" test "$rc" -ne 0 -a -n "$(grep '失败（底层未动）' "$R/out.txt")" -a -z "$(grep '底层已改' "$R/out.txt")" -a "$(last_mount)" = "mount -o remount,ro /"
chmod 755 "$R/chrony-cn.rootbind/etc"
echo TZ > "$R/Shanghai"; chmod 555 "$R/timezone-cn.rootbind/etc"; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_ZONEINFO="$R/Shanghai" CJ_LOCALTIME="$R/localtime" CJ_MOUNTS="$R/mounts-ov" CJ_BACKUP_DIR="$R/cbk" CJ_TMPDIR="$R" run sh deploy-timezone-cn.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "timezone-cn overlay：写底层失败 → 退出非 0、报错、不说\"已改\"、最后一次 mount 是 ro" test "$rc" -ne 0 -a -n "$(grep '失败（底层未动）' "$R/out.txt")" -a -z "$(grep '底层已改' "$R/out.txt")" -a "$(last_mount)" = "mount -o remount,ro /"
chmod 755 "$R/timezone-cn.rootbind/etc"

# 连接次数（2026-09-25 合批）：推送一次 ssh 建目录 + 每文件一次 scp + 一次 ssh 取全部 md5；sidebar 探测/落位+生效各合一
new_sandbox; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_SKIP_BUILD=1 DEFER_XOVI_START=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >/dev/null 2>&1; rc=$?
check "hl-snap 部署：4 次 ssh（连通检查/建目录/取 md5/安装）+ 4 次 scp（旧版 13 次 ssh）" test "$rc" -eq 0 -a "$(count_log '^ssh')" = 4 -a "$(count_log '^scp')" = 4
: > "$CJ_SIM_LOG"
( cd "$PKG" && DEFER_XOVI_START=1 run sh deploy-sidebar-entry.sh 127.0.0.1 ) >/dev/null 2>&1; rc=$?
check "sidebar-entry 部署：5 次 ssh（连通/探测/建目录/取 md5/落位）+ 2 次 scp（旧版 10 次 ssh）" test "$rc" -eq 0 -a "$(count_log '^ssh')" = 5 -a "$(count_log '^scp')" = 2
new_sandbox; SOP="$R/home/root/.cangjie-stage"; mkdir -p "$SOP"; echo keep > "$SOP/battop.new"; : > "$CJ_SIM_LOG"
( cd "$PKG" && CJ_SIM_SCP_CORRUPT=hl-snap.so CJ_SKIP_BUILD=1 DEFER_XOVI_START=1 run sh deploy-hl-snap.sh 127.0.0.1 ) >"$R/out.txt" 2>&1; rc=$?
check "合批推送：只有 md5 对不上的那个文件被删，其余已校验的暂存文件保留、没进入安装" test "$rc" -ne 0 -a ! -e "$R/home/root/hl-snap/hl-snap.so" -a -f "$R/home/root/hl-snap/deploy/install.sh" -a -n "$(grep 'md5 对不上：hl-snap.so' "$R/out.txt")" -a -z "$(grep "^ssh sh '.*/deploy/install.sh" "$CJ_SIM_LOG")"
new_sandbox; export CJ_ALLOWLIST_LOCAL="$R/allow.local.txt" CJ_BATTOP_BIN="$R/battop.bin" CJ_SKIP_BUILD=1 SHELF_NO_BUILD=1; echo B > "$R/battop.bin"; : > "$CJ_SIM_LOG"
( cd "$PKG" && run sh install-all.sh 127.0.0.1 --force --skip chrony-cn,timezone-cn ) >/dev/null 2>&1; rc=$?
check "install-all：整轮只做一次连通检查（各步骤不再各自 ssh true）" test "$rc" -eq 0 -a "$(count_log '^ssh true')" = 1
unset CJ_ALLOWLIST_LOCAL CJ_BATTOP_BIN CJ_SKIP_BUILD SHELF_NO_BUILD

# 预检提示与 cj_xochitl_apply 的实际分支一致（xovi 未生效但会装 xovi-reenable → 整机重启，不是"一定 xovi/start"）
new_sandbox; export CJ_ALLOWLIST_LOCAL="$R/allow.local.txt"; xovi_live off
( cd "$PKG" && run sh install-all.sh 127.0.0.1 --force --skip chrony-cn,timezone-cn,battop,wifi-watch,chrony-boot-wakelock,hl-snap,handwriting-stroke,sidebar-entry,shelf,xovi-apply ) >"$R/out.txt" 2>&1
check "预检：xovi 未生效时如实说明'装了 xovi-reenable 就整机重启，否则 xovi/start'" test -n "$(grep 'xovi 尚未生效.*整机重启.*否则 xovi/start' "$R/out.txt")"
unset CJ_ALLOWLIST_LOCAL

# ═══════════════════════════ 5. 静态守卫 / 清单对称 ═══════════════════════════
section "静态守卫"
cd "$REPO" || exit 1
# 1) 只有库/既有的 /etc 覆写脚本可以 remount rw
viol="$(grep -rn 'remount,rw' packaging/*.sh shelf/*.sh enhance/*/install.sh enhance/*/deploy/*.sh 2>/dev/null | grep -v -e '^packaging/devlib.sh' -e '^packaging/chrony-cn.sh' -e '^packaging/timezone-cn.sh' -e ':[0-9]*:[[:space:]]*#' -e 'echo')"
check "remount,rw 只出现在 devlib.sh（带 trap）与 chrony-cn/timezone-cn（自带重试收尾）里" test -z "$viol"
[ -n "$viol" ] && echo "$viol"
# 2) 可执行的 xovi/start 只允许在 devlib.sh
viol="$(grep -rnE '^[[:space:]]*("\$[A-Za-z_]*/start"|/home/root/xovi/start)[[:space:]]*(\|\||;|$)' packaging/*.sh shelf/*.sh enhance/*/install.sh enhance/*/deploy/*.sh 2>/dev/null | grep -v '^packaging/devlib.sh')"
check "xovi/start 不再被任何脚本直接执行（统一走 devlib 的 cj_xochitl_apply）" test -z "$viol"
[ -n "$viol" ] && echo "$viol"
# 3) 步骤表对称：每个步骤都有脚本；非配置类步骤在 uninstall-all 里有 uninstall_ 函数
# shellcheck disable=SC1091
( cd "$PKG" && . ./lib.sh
  for step in $STEP_ORDER; do
      s="$(step_script "$step")" || { echo "步骤 $step 无脚本映射"; exit 1; }
      [ -f "$s" ] || { echo "步骤 $step 的脚本 $s 不存在"; exit 1; }
      if ! word_in "$step" "$STEP_CONFIG_ONLY"; then
          fn="uninstall_$(echo "$step" | tr '-' '_')"
          grep -q "^$fn()" uninstall-all.sh || { echo "步骤 $step 缺 $fn"; exit 1; }
      fi
  done ) >"$TMPBASE/sym.txt" 2>&1
check "install-all/uninstall-all 步骤表对称（每步有脚本；非配置步骤有 uninstall_ 函数）" test ! -s "$TMPBASE/sym.txt"
cat "$TMPBASE/sym.txt"
# 4) shelf 清单对称：manifest 里声明的每个 qmd/辅助脚本，install.sh 与 uninstall.sh 都是靠 manifest 函数处理，不再硬编码
check "shelf/uninstall.sh 不再硬编码 qmd 文件名（走 manifest）" test -z "$(grep -n 'shelf-trash-agent\|shelf-mkdir-agent\|font-menu-dynamic' shelf/uninstall.sh | grep -v '^[0-9]*:#')"
# 5) 备份不进 extensions.d：脚本里不存在把 .bak 写进 extensions.d 的写法
check "没有脚本往 extensions.d 里写 .bak/.new" test -z "$(grep -rn 'extensions.d/[^"]*\.\(bak\|new\)' packaging/*.sh enhance/*/deploy/*.sh 2>/dev/null | grep -v ':[0-9]*:[[:space:]]*#')"

# 6) 不再有 eval（旧 deploy-usr-unit 曾对设备端路径记号做 eval echo 二次展开）
check "packaging/ 与 shelf/ 的 .sh 里没有 eval（非注释行）" test -z "$(grep -n '\beval\b' packaging/*.sh shelf/*.sh 2>/dev/null | grep -v -e ':[0-9]*:[[:space:]]*#')"
# 7) 每个会连设备的 deploy 入口动手前都先 require_device（不通给可读报错，不留半成品）
viol=""
for f in packaging/deploy*.sh; do
    if grep -q -e 'rssh' -e 'dev_script' -e 'push_verified' "$f" && ! grep -q 'require_device' "$f"; then viol="$viol $f"; fi
done
check "每个连设备的 deploy-*.sh 都调用了 require_device" test -z "$viol"
[ -z "$viol" ] || echo "       缺 require_device：$viol"
# 8) 设备端会 rm 的脚本必须有目标核对：rm -rf 只允许出现在带守卫的位置（白名单式点名）
viol="$(grep -n 'rm -rf' packaging/*.sh shelf/*.sh 2>/dev/null | grep -v -e ':[0-9]*:[[:space:]]*#' | grep -v -e 'packaging/tests/' -e 'STAGE' -e 'REMOTE' )"
check "rm -rf 出现处已人工核对：仅 uninstall-all(battop purge / shelf-pkg 载荷)、shelf/uninstall(--purge 三个 XDG 目录，路径/符号链接守卫)" test "$(echo "$viol" | grep -v '^$' | grep -v -e 'packaging/uninstall-all.sh' -e 'shelf/uninstall.sh' | wc -l)" -eq 0

# 9) xovi-reenable.service 不许在 xovi 已生效时重跑 xovi/start（会让运行中的 xochitl SEGV → 整机重启）：
#    必须有 ExecCondition 检查 xochitl 进程是否已映射 xovi.so，且排在 ExecStart 之前
U=packaging/xovi-reenable.service
check "xovi-reenable.service：ExecStart 前有 ExecCondition 检查 xovi.so 是否已生效" test -n "$(grep -n '^ExecCondition=.*xovi\[\.\]so' $U)" -a "$(grep -n '^ExecCondition=' $U | cut -d: -f1 | head -n1)" -lt "$(grep -n '^ExecStart=' $U | cut -d: -f1 | head -n1)"
# 守卫命令本身：xovi.so 已映射 → 条件返回非 0（跳过）；未映射 → 返回 0（执行）。用文件充当 /proc/<pid>/maps
GUARD_CMD='! grep -q "xovi[.]so" MAPS 2>/dev/null'
printf '7f00 r-xp /home/root/xovi/xovi.so\n' > "$R/maps-active"; printf '7f00 r-xp /usr/lib/libc.so\n' > "$R/maps-plain"
check "xovi-reenable 守卫：已生效→跳过" bash -c "${GUARD_CMD//MAPS/$R/maps-active}; [ \$? -ne 0 ]"
check "xovi-reenable 守卫：未生效→执行" bash -c "${GUARD_CMD//MAPS/$R/maps-plain}"
# 开机时换入待换入区（2026-09-25）：ExecStartPre 在 ExecCondition 之后、ExecStart 之前；把单元里的命令抽出来（$$→$、
# /home/root→沙箱）真跑一遍：普通文件挪进 extensions.d 且 755、符号链接不动、目录空了就删、extensions.d 里没有临时文件
check "xovi-reenable.service：换入待换入区的 ExecStartPre 排在 ExecCondition 之后、ExecStart 之前" test "$(grep -n '^ExecCondition=' $U | cut -d: -f1)" -lt "$(grep -n '^ExecStartPre=.*so-pending' $U | cut -d: -f1)" -a "$(grep -n '^ExecStartPre=.*so-pending' $U | cut -d: -f1)" -lt "$(grep -n '^ExecStart=' $U | cut -d: -f1)"
PRE_CMD="$(sed -n "s/^ExecStartPre=-\/bin\/sh -c '\(.*\)'\$/\1/p" $U | sed -e 's/\$\$/$/g' -e "s#/home/root#$R/boot-home#g")"
mkdir -p "$R/boot-home/.cangjie-stage/so-pending" "$R/boot-home/xovi/extensions.d"
echo OLD > "$R/boot-home/xovi/extensions.d/hl-snap.so"; echo NEW > "$R/boot-home/.cangjie-stage/so-pending/hl-snap.so"
ln -s /etc/passwd "$R/boot-home/.cangjie-stage/so-pending/evil.so"
out="$(sh -c "$PRE_CMD" 2>&1)"; rc=$?
check "xovi-reenable 开机换入：新版就位且可执行、符号链接没挪进 extensions.d、提示换入了哪个" test "$rc" -eq 0 -a "$(cat "$R/boot-home/xovi/extensions.d/hl-snap.so")" = NEW -a -x "$R/boot-home/xovi/extensions.d/hl-snap.so" -a ! -e "$R/boot-home/xovi/extensions.d/evil.so" -a "$(ls -A "$R/boot-home/xovi/extensions.d")" = hl-snap.so -a -n "$(printf '%s' "$out" | grep '换入 hl-snap.so')"
rm -f "$R/boot-home/.cangjie-stage/so-pending/evil.so"; out="$(sh -c "$PRE_CMD" 2>&1)"
check "xovi-reenable 开机换入：待换入区空了就删掉目录；没有待换入区时安静退出 0" test ! -e "$R/boot-home/.cangjie-stage/so-pending" -a -z "$out" && sh -c "$PRE_CMD"

# 10) 开机顺序（2026-09-24）：常驻服务不拉 network-online.target（否则开机专门为它们跑 NetworkManager-wait-online，
#     没 WiFi 时等到超时）；都排在 xovi-reenable 之后（xochitl 先带 xovi 起来）；xovi-reenable 必须有启动超时上限
#     （否则它卡住所有服务跟着永远等）；fc-cache 在 font-serve 不在网关。
SVC_UNITS="gateway/systemd/gateway.service shelf/systemd/book-serve.service shelf/systemd/koreader-serve.service enhance/font-serve/font-serve.service enhance/wallpaper-serve/wallpaper-serve.service notes/systemd/ink-serve.service notes/systemd/mind-serve.service notes/systemd/note-serve.service notes/systemd/transcribe-serve.service"
viol=""; for u in $SVC_UNITS; do grep -v '^#' "$u" | grep -q 'network-online' && viol="$viol $u"; done
check "常驻服务单元不依赖 network-online.target" test -z "$viol"
[ -z "$viol" ] || echo "       仍依赖：$viol"
viol=""; for u in $SVC_UNITS; do grep -q '^After=.*xovi-reenable[.]service' "$u" || viol="$viol $u"; done
check "常驻服务单元都 After=xovi-reenable.service" test -z "$viol"
[ -z "$viol" ] || echo "       缺：$viol"
viol=""; for u in $SVC_UNITS; do grep -qE '^(Requires|Wants|BindsTo|Requisite)=.*xochitl' "$u" && viol="$viol $u"; grep -q '^Before=.*xochitl' "$u" && viol="$viol $u"; done
check "常驻服务单元不给 xochitl 加任何依赖/前置（红线）" test -z "$viol"
check "xovi-reenable.service 有启动超时上限（TimeoutStartSec）" grep -q '^TimeoutStartSec=[0-9]' packaging/xovi-reenable.service
check "fc-cache 在 font-serve、不在网关" bash -c "grep -q '^ExecStartPre=.*fc-cache' enhance/font-serve/font-serve.service && ! grep -q 'fc-cache' gateway/systemd/gateway.service"
check "xovi-reenable 守卫：xochitl 不在跑（maps 不存在）→执行" bash -c "${GUARD_CMD//MAPS/$R/nonexistent}"

# 守卫：真实 HOME 下不该出现任何测试产物
GUARD_AFTER=""
for g in $(guard_paths); do [ -e "$g" ] && GUARD_AFTER="$GUARD_AFTER $g"; done
check "真实 HOME 未被测试触碰（无新增 shelf/cangjie-backups/.stage 等目录）" test "$GUARD_BEFORE" = "$GUARD_AFTER"

echo
echo "════ 结果：通过 $PASS，失败 $FAIL ════"
[ "$FAIL" -eq 0 ]
