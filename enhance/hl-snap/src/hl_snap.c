/* hl-snap —— 荧光笔汉字精确吸附，独立 xovi 扩展。
 *
 * 只做一件事：CJK 划线时命中区间不向两边扩张成整行（划哪吸哪），跟拼音输入法/
 * UI 汉化/EF/KBS 那些 hook 完全脱钩——2026-09-09 从 chinese-ime/langhook（那边
 * 是完整中文化线，4800+ 行、一堆互不相关的 hook 混一个文件）里独立拆出来，逻辑
 * 逐字节照抄自 `chinese-ime/langhook/src/hook_init.c` 的 Step HL2 代码段（真机
 * 2026-08-23 验证过），不是重新发明。
 *
 * 复用 chinese-ime/langhook 已经模块化出来的三个纯工具文件（scan.c/pattern.c/
 * trampoline_aarch64.c——特征码扫描 + ARM64 远跳转 trampoline，本来就是跟"装的
 * 是哪个 hook"无关的通用基础设施，不是"老项目的业务逻辑"）；不复制、不修改，
 * 用相对路径引用，跟 shelf 的 bookconv 被 reading/device-rs 路径引用是同一个
 * 已有先例（"跨目录路径依赖可行"）。patch_target/make_call_through_stub 这两个
 * 通用 trampoline 安装函数本身很小（~60 行），逐字节复制过来，让这个扩展完全
 * 自包含、不依赖 chinese-ime/langhook 里任何跟 IME 相关的代码。
 *
 * 跟原来在 chinese-ime/langhook 里不一样的地方：_xovi_shouldLoad 的固件兼容性
 * 判据直接就是这个 hook 自己的目标函数特征码（不再借用 setLanguageCode 那个跟
 * IME 相关的判据）——这个扩展的兼容性只取决于它自己要 patch 的那个函数还在不
 * 在，不需要跟输入法是否兼容挂钩。
 */
#include <stdio.h>
#include <string.h>
#include <errno.h>
#include <sys/mman.h>
#include <unistd.h>
#include <stdint.h>
#include <stdbool.h>

#include "scan.h"
#include "pattern.h"
#include "trampoline_aarch64.h"

#define TARGET_MODULE_SUFFIX "/usr/bin/xochitl"
#define CJ_DATA_DIR "/home/root/.local/share/cangjie-ime"
#define CJ_READING_QOL_PATH CJ_DATA_DIR "/reading-qol.json"

/* Step HL2：荧光笔"吸整行"元凶——命中区间向两边扩张的函数（.169 FUN_00f05ad0，
 * 中文实测走这条：对每个子区间调 FUN_00f052f0 扩 start/end）→ 整行；对无空格
 * 中文就是"划一小段吸整行"的病根。前 20 字节纯栈/寄存器可安全 patch；offset 32
 * 的 CBZ 在 patch 区外、同版本固定，精确匹配。x0=scene，x1=range 向量。
 * 逐字节抄自 chinese-ime/langhook/src/hook_init.c 的 PROLOGUE_HL_EXPAND，
 * 2026-09-09 真机在 3.28.0.172 上复核过这段特征码仍唯一命中（不是固件迁移偏移
 * 漂移的问题，见记忆 hlsnapcjk-langhook-missing-2026-09）。 */
static const uint8_t PROLOGUE_HL_EXPAND[] = {
    0x3f, 0x23, 0x03, 0xd5, 0xfd, 0x7b, 0xbd, 0xa9, 0xfd, 0x03, 0x00, 0x91,
    0xf5, 0x13, 0x00, 0xf9, 0xf5, 0x03, 0x00, 0xaa, 0x20, 0x00, 0x40, 0xf9,
    0xf3, 0x53, 0x01, 0xa9, 0xf4, 0x03, 0x01, 0xaa, 0x80, 0x00, 0x00, 0xb4,
    0x01, 0x00, 0x40, 0xb9,
};
#define CJ_FAR_JUMP_LEN_LOCAL (5 * 4)
#define PATCH_LEN CJ_FAR_JUMP_LEN_LOCAL /* 覆盖目标函数开头的字节数，跟远跳转指令长度一致 */

/* ---- 通用 trampoline 安装（逐字节抄自 hook_init.c，不做任何改动） ---- */

static void *make_call_through_stub(const uint8_t *original_bytes, void *jump_back_target) {
    size_t stub_len = PATCH_LEN + CJ_FAR_JUMP_LEN;
    void *stub = mmap(NULL, stub_len, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (stub == MAP_FAILED) return NULL;

    memcpy(stub, original_bytes, PATCH_LEN);

    uint32_t jump_instrs[5];
    cj_build_far_jump(jump_instrs, jump_back_target);
    memcpy((uint8_t *)stub + PATCH_LEN, jump_instrs, CJ_FAR_JUMP_LEN);

    if (mprotect(stub, stub_len, PROT_READ | PROT_EXEC) != 0) {
        munmap(stub, stub_len);
        return NULL;
    }
    __builtin___clear_cache((char *)stub, (char *)stub + stub_len);
    return stub;
}

static int patch_target(void *target_addr, void *handler, void **out_stub) {
    long pagesize = sysconf(_SC_PAGESIZE);
    if (pagesize <= 0) pagesize = 4096;

    uintptr_t page_base = (uintptr_t)target_addr & ~((uintptr_t)pagesize - 1);
    size_t region_len = (size_t)pagesize;
    if ((((uintptr_t)target_addr - page_base) + PATCH_LEN) > region_len) {
        region_len += (size_t)pagesize;
    }

    if (mprotect((void *)page_base, region_len, PROT_READ | PROT_WRITE | PROT_EXEC) != 0) {
        fprintf(stderr, "[hl-snap] mprotect 失败，放弃 hook（safe mode）：%s\n", strerror(errno));
        return 0;
    }

    void *jump_back_target = (uint8_t *)target_addr + PATCH_LEN;
    void *stub = make_call_through_stub((const uint8_t *)target_addr, jump_back_target);
    if (!stub) {
        fprintf(stderr, "[hl-snap] 调用桩分配失败，放弃 hook（safe mode）\n");
        return 0;
    }
    *out_stub = stub;

    uint32_t jump_to_handler[5];
    cj_build_far_jump(jump_to_handler, handler);
    memcpy(target_addr, jump_to_handler, CJ_FAR_JUMP_LEN);
    __builtin___clear_cache((char *)target_addr, (char *)target_addr + CJ_FAR_JUMP_LEN);

    return 1;
}

/* ---- 荧光笔精确吸附本体（逐字节抄自 hook_init.c 的 Step HL2 代码段） ---- */

typedef void (*orig_hl_expand_fn_t)(long, void *);
static orig_hl_expand_fn_t g_orig_hl_expand_call_through = NULL;
static int g_hl_expand_neuter = 1;   /* 1=CJK 跳过扩张（修复）；0=恢复原生扩张 */

/* glyph 数组第 idx 个字的 QChar 是否属 CJK。scene+8 是 glyph 数组基址。 */
static int cj_hl_glyph_is_cjk(long scene, int idx) {
    if (idx < 0) return 0;
    long garr = 0;
    memcpy(&garr, (const void *)(scene + 8), sizeof(garr));
    if (!garr) return 0;
    uint16_t ch = 0;
    memcpy(&ch, (const void *)(garr + (long)idx * 0x38 + 0x30), sizeof(ch));
    return (ch >= 0x4E00 && ch <= 0x9FFF)   /* CJK 统一表意 */
        || (ch >= 0x3400 && ch <= 0x4DBF)   /* 扩展 A */
        || (ch >= 0xF900 && ch <= 0xFAFF)   /* 兼容表意 */
        || (ch >= 0x3000 && ch <= 0x303F);  /* CJK 标点 */
}

/* 运行时开关：从 reading-qol.json 读 hlSnapCjk 到 g_hl_expand_neuter（设置页
 * 「笔记增强」/shelf 网页「管理→系统增强」都写这同一个键，两边改哪边都算数）。
 * 划线才调，非热路径，每次读一次即可，无需 mtime。极简字段扫描、不引 JSON 库。
 * fail-safe：文件缺失/字段缺失/读失败 → 不改，保持编译期默认（修复开）。 */
static void cj_hl_refresh_config(void) {
    FILE *f = fopen(CJ_READING_QOL_PATH, "rb");
    if (!f) return;
    char buf[4096];
    size_t n = fread(buf, 1, sizeof(buf) - 1, f);
    fclose(f);
    buf[n] = '\0';
    const char *p = strstr(buf, "\"hlSnapCjk\"");
    if (!p) return;
    p += 11;  /* 跳过 "hlSnapCjk" 本身（含两个引号，共 11 字节） */
    while (*p == ':' || *p == ' ' || *p == '\t') p++;
    if (strncmp(p, "true", 4) == 0) g_hl_expand_neuter = 1;
    else if (strncmp(p, "false", 5) == 0) g_hl_expand_neuter = 0;
}

static void cj_hl_expand_handler(long scene, void *rng_v) {
    cj_hl_refresh_config();
    if (g_hl_expand_neuter) {
        uint64_t *v = (uint64_t *)rng_v;
        if (v) {
            int *subs = (int *)(uintptr_t)v[1];
            long count = (long)v[2];
            if (subs && count > 0 && cj_hl_glyph_is_cjk(scene, subs[0])) {
                return;   /* CJK：不调原扩张函数 */
            }
        }
    }
    if (g_orig_hl_expand_call_through) {
        g_orig_hl_expand_call_through(scene, rng_v);
    }
}

static void cj_install_hl_expand_hook(uintptr_t target) {
    void *stub = NULL;
    if (!patch_target((void *)target, (void *)cj_hl_expand_handler, &stub)) {
        fprintf(stderr, "[hl-snap] 荧光笔EXPAND hook 安装失败（safe mode）\n");
        return;
    }
    g_orig_hl_expand_call_through = (orig_hl_expand_fn_t)stub;
    fprintf(stderr, "[hl-snap] 荧光笔EXPAND hook 安装完成 @ %p（neuter=%d）\n",
            (void *)target, g_hl_expand_neuter);
}

/* ---- xovi 扩展入口 ---- */

/* 固件兼容性判据就是这个 hook 自己的目标特征码——不借用别的函数当总闸，这个
 * 扩展只关心它要 patch 的那个函数还在不在。 */
char _xovi_shouldLoad(void) {
    uintptr_t base = 0, addr = 0;
    size_t size = 0;
    if (!cj_find_exec_module(TARGET_MODULE_SUFFIX, NULL, &base, &size)) {
        fprintf(stderr, "[hl-snap] _xovi_shouldLoad: 找不到 xochitl 映射 → 拒绝加载(裸启原生)\n");
        return 0;
    }
    if (!cj_find_unique_pattern((const uint8_t *)base, size, PROLOGUE_HL_EXPAND,
                                 sizeof(PROLOGUE_HL_EXPAND), &addr)) {
        fprintf(stderr, "[hl-snap] _xovi_shouldLoad: 荧光笔扩张函数特征码未唯一命中"
                "(未知固件) → 拒绝加载(裸启原生)\n");
        return 0;
    }
    fprintf(stderr, "[hl-snap] _xovi_shouldLoad: 固件兼容(FUN_00f05ad0@0x%lx) → 加载\n",
            (unsigned long)addr);
    return 1;
}

void _xovi_construct(void) {
    uintptr_t base = 0, addr = 0;
    size_t size = 0;
    if (!cj_find_exec_module(TARGET_MODULE_SUFFIX, NULL, &base, &size)) return;
    if (!cj_find_unique_pattern((const uint8_t *)base, size, PROLOGUE_HL_EXPAND,
                                 sizeof(PROLOGUE_HL_EXPAND), &addr)) {
        return; /* _xovi_shouldLoad 已经打过日志，这里不重复 */
    }
    cj_hl_refresh_config();
    cj_install_hl_expand_hook(addr);
}
