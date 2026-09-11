/* enhance/shared/ 四个纯工具文件（scan.c/pattern.c/trampoline_aarch64.c/trampoline_patch.c）
 * 的 host 侧单元测试——2026-09-16 补（全项目现状核查 P1 条目：这几个文件此前被
 * hl-snap/handwriting-stroke 两个"涉及 xovi/mprotect 的高危代码路径"共用，却完全零自动化
 * 覆盖，只能靠真机验证）。
 *
 * 不需要真机、不需要交叉编译——四个文件本身就是纯 C11、不依赖任何 xovi/xochitl 符号：
 *   · scan.c 的 cj_find_exec_module 本来就设计成接受构造好的 maps_content 字符串（见其
 *     头注"方便单元测试用构造好的字符串跑"），不用真的读 /proc/self/maps。
 *   · pattern.c 是纯字节匹配算法，喂内存里的字节数组即可。
 *   · trampoline_aarch64.c 的 cj_build_far_jump 是纯指令编码，不执行生成的指令，只检查
 *     编码结果的位模式——生成的是 ARM64 指令，host（x86_64）CPU 不认得也不会去执行它，
 *     这里只验证"编码规则本身对不对"，不验证"跑起来对不对"（那是真机验证的范畴）。
 *   · trampoline_patch.c 的 cj_patch_target 同理：用 host 上 mmap 出的一块可读写可执行内存
 *     模拟"目标函数所在的页"，验证 patch 完之后内存里的字节内容符合预期（覆盖处=远跳转到
 *     handler、stub=原始字节+跳回原地址），同样不执行、只检查字节。
 *
 * 跑法：cd enhance/shared && make test（host gcc，不需要 aarch64 交叉工具链）。
 */
#include "../pattern.h"
#include "../scan.h"
#include "../trampoline_aarch64.h"
#include "../trampoline_patch.h"

#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

static int failures = 0;
#define CHECK(cond) do { \
    if (!(cond)) { \
        fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); \
        failures++; \
    } \
} while (0)

/* ── pattern.c ─────────────────────────────────────────────────────────── */

static void test_pattern_unique_hit(void) {
    const uint8_t hay[] = {0x11, 0x22, 0x33, 0x44, 0x55, 0x66};
    const uint8_t pat[] = {0x33, 0x44};
    uintptr_t addr = 0;
    CHECK(cj_find_unique_pattern(hay, sizeof hay, pat, sizeof pat, &addr) == 1);
    CHECK(addr == (uintptr_t)(hay + 2));
}

static void test_pattern_zero_hits_fails(void) {
    const uint8_t hay[] = {0x11, 0x22, 0x33};
    const uint8_t pat[] = {0xAA, 0xBB};
    uintptr_t addr = 0xdeadbeef;
    CHECK(cj_find_unique_pattern(hay, sizeof hay, pat, sizeof pat, &addr) == 0);
    CHECK(addr == 0xdeadbeef); /* 0 次命中不该写 *out_addr */
}

static void test_pattern_multi_hits_fails(void) {
    /* 特征码在同一段字节里出现两次——白皮书教训：宁可放弃也不要在不确定地址上 patch。 */
    const uint8_t hay[] = {0xAA, 0xBB, 0x01, 0x02, 0xAA, 0xBB};
    const uint8_t pat[] = {0xAA, 0xBB};
    uintptr_t addr = 0;
    size_t first = 0;
    CHECK(cj_find_unique_pattern(hay, sizeof hay, pat, sizeof pat, &addr) == 0);
    CHECK(cj_count_pattern(hay, sizeof hay, pat, sizeof pat, (uintptr_t *)&first) == 2);
}

static void test_pattern_degenerate_inputs(void) {
    const uint8_t hay[] = {0x11, 0x22};
    uintptr_t addr = 0;
    /* pattern 比 haystack 长 */
    const uint8_t long_pat[] = {0x11, 0x22, 0x33};
    CHECK(cj_find_unique_pattern(hay, sizeof hay, long_pat, sizeof long_pat, &addr) == 0);
    /* pattern_len == 0 */
    CHECK(cj_find_unique_pattern(hay, sizeof hay, hay, 0, &addr) == 0);
}

static void test_pattern_masked_wildcard(void) {
    /* 两处候选，中间一字节不同（模拟 BL 相对偏移随固件版本漂移）；掩码通配掉那个字节后
     * 变成"两处都命中"，符合韧性重构的设计（跨版本仍能唯一命中同一段代码结构）——但如果
     * 这两处本来就是两个不同函数体的巧合重叠，掩码放太宽会导致真的出现两次命中，测试
     * 用后一种场景验证"掩码放宽后命中数从 1 变多"这条边界确实会被判定失败，不是被吞掉。
     */
    const uint8_t hay[] = {0x90, 0x11, 0xA0, 0x90, 0x22, 0xA0};
    const uint8_t pat[] = {0x90, 0x00, 0xA0};
    const uint8_t mask[] = {0xFF, 0x00, 0xFF}; /* 中间字节通配 */
    uintptr_t addr = 0;
    CHECK(cj_find_unique_pattern_masked(hay, sizeof hay, pat, mask, sizeof pat, &addr) == 0);
    size_t first = 0;
    CHECK(cj_count_pattern_masked(hay, sizeof hay, pat, mask, sizeof pat, (uintptr_t *)&first) == 2);

    /* 同一份数据不带掩码（精确匹配）——两处中间字节不同，精确匹配只命中一次。 */
    const uint8_t exact_pat[] = {0x90, 0x11, 0xA0};
    CHECK(cj_find_unique_pattern(hay, sizeof hay, exact_pat, sizeof exact_pat, &addr) == 1);
    CHECK(addr == (uintptr_t)hay);
}

static void test_pattern_fast_path_matches_bytewise_loop(void) {
    /* 精确匹配走 memchr 快路径，全 0xFF 掩码走逐字节循环——两者语义相同，差分对拍：
     * 小字母表随机数据（大量重叠/相邻命中）、各种长度，含命中落在首尾两端的情况。 */
    static uint8_t hay[4096];
    uint32_t x = 7;
    for (int round = 0; round < 400; round++) {
        size_t hlen = 1 + (size_t)(round * 37) % sizeof hay;
        size_t plen = 1 + (size_t)round % 6;
        for (size_t i = 0; i < hlen; i++) { x = x * 1103515245u + 12345u; hay[i] = (uint8_t)((x >> 16) % 3); }
        uint8_t pat[6], full[6];
        for (size_t i = 0; i < plen; i++) { x = x * 1103515245u + 12345u; pat[i] = (uint8_t)((x >> 16) % 3); full[i] = 0xFF; }
        if (round % 5 == 0 && hlen >= plen) memcpy(hay + hlen - plen, pat, plen); /* 命中贴尾 */
        if (round % 7 == 0 && hlen >= plen) memcpy(hay, pat, plen);               /* 命中贴头 */
        uintptr_t fa = 0, fb = 0;
        size_t a = cj_count_pattern(hay, hlen, pat, plen, &fa);
        size_t b = cj_count_pattern_masked(hay, hlen, pat, full, plen, &fb);
        CHECK(a == b);
        if (a > 0) CHECK(fa == fb);
    }
}

/* ── scan.c ────────────────────────────────────────────────────────────── */

static void test_scan_finds_matching_exec_mapping(void) {
    const char *maps =
        "55aa00000000-55aa00010000 r--p 00000000 08:01 1  /usr/bin/xochitl\n"
        "55aa00010000-55aa00030000 r-xp 00010000 08:01 1  /usr/bin/xochitl\n"
        "55aa00030000-55aa00040000 rw-p 00030000 08:01 1  /usr/bin/xochitl\n";
    uintptr_t base = 0;
    size_t size = 0;
    CHECK(cj_find_exec_module("xochitl", maps, &base, &size) == 1);
    CHECK(base == 0x55aa00010000ULL);
    CHECK(size == 0x20000);
}

static void test_scan_suffix_mismatch_not_found(void) {
    const char *maps = "55aa00010000-55aa00030000 r-xp 00010000 08:01 1  /usr/bin/otherapp\n";
    uintptr_t base = 0;
    size_t size = 0;
    CHECK(cj_find_exec_module("xochitl", maps, &base, &size) == 0);
}

static void test_scan_non_exec_mapping_skipped(void) {
    /* 路径匹配但不是可执行映射（r--p）——不该被当成命中。 */
    const char *maps = "55aa00010000-55aa00030000 r--p 00010000 08:01 1  /usr/bin/xochitl\n";
    uintptr_t base = 0;
    size_t size = 0;
    CHECK(cj_find_exec_module("xochitl", maps, &base, &size) == 0);
}

static void test_scan_anonymous_mapping_skipped(void) {
    /* 匿名可执行映射（无路径字段，比如 JIT 页）——没有 path token，不该崩溃、按未命中处理。 */
    const char *maps =
        "55aa00010000-55aa00030000 r-xp 00000000 00:00 0 \n"
        "55aa00040000-55aa00050000 r-xp 00010000 08:01 1  /usr/bin/xochitl\n";
    uintptr_t base = 0;
    size_t size = 0;
    CHECK(cj_find_exec_module("xochitl", maps, &base, &size) == 1);
    CHECK(base == 0x55aa00040000ULL);
}

static void test_scan_returns_first_match(void) {
    /* 两段路径都匹配后缀的可执行映射——函数文档承诺"取第一段"。 */
    const char *maps =
        "10000-20000 r-xp 0 08:01 1  /a/xochitl\n"
        "30000-40000 r-xp 0 08:01 1  /b/xochitl\n";
    uintptr_t base = 0;
    size_t size = 0;
    CHECK(cj_find_exec_module("xochitl", maps, &base, &size) == 1);
    CHECK(base == 0x10000ULL);
    CHECK(size == 0x10000); /* 不相接的第二处不合并 */
}

static void test_scan_merges_segments_split_by_hooks(void) {
    /* 2026-09-24 真机形状：先装的扩展把 0xf03000、0xf47000 两页 mprotect 成 rwxp，
     * xochitl 代码段被切成 5 段。后装的扩展必须仍能扫到整段（含高地址目标）。 */
    const char *maps =
        "aaaa00000000-aaaa00400000 r--p 00000000 b3:02 77  /usr/bin/xochitl\n"
        "aaaa00400000-aaaa00f03000 r-xp 00400000 b3:02 77  /usr/bin/xochitl\n"
        "aaaa00f03000-aaaa00f04000 rwxp 00f03000 b3:02 77  /usr/bin/xochitl\n"
        "aaaa00f04000-aaaa00f47000 r-xp 00f04000 b3:02 77  /usr/bin/xochitl\n"
        "aaaa00f47000-aaaa00f48000 rwxp 00f47000 b3:02 77  /usr/bin/xochitl\n"
        "aaaa00f48000-aaaa01200000 r-xp 00f48000 b3:02 77  /usr/bin/xochitl\n"
        "aaaa01200000-aaaa01300000 r--p 01200000 b3:02 77  /usr/bin/xochitl\n"
        "aaaa01300000-aaaa01310000 rw-p 01300000 b3:02 77  /usr/bin/xochitl\n";
    uintptr_t base = 0;
    size_t size = 0;
    CHECK(cj_find_exec_module("/usr/bin/xochitl", maps, &base, &size) == 1);
    CHECK(base == 0xaaaa00400000ULL);
    CHECK(size == 0xaaaa01200000ULL - 0xaaaa00400000ULL); /* 到 r--p 为止，不含只读数据段 */
}

static void test_scan_merge_stops_on_gap_other_file_or_unreadable(void) {
    uintptr_t base = 0;
    size_t size = 0;
    /* 中间有空洞：只取到空洞前 */
    const char *gap =
        "10000-20000 r-xp 0 08:01 1  /usr/bin/xochitl\n"
        "21000-30000 r-xp 0 08:01 1  /usr/bin/xochitl\n";
    CHECK(cj_find_exec_module("xochitl", gap, &base, &size) == 1);
    CHECK(base == 0x10000ULL && size == 0x10000);
    /* 紧接的是别的文件（后缀也匹配）：不合并 */
    const char *other =
        "10000-20000 r-xp 0 08:01 1  /usr/bin/xochitl\n"
        "20000-30000 r-xp 0 08:01 2  /opt/xochitl\n";
    CHECK(cj_find_exec_module("xochitl", other, &base, &size) == 1);
    CHECK(base == 0x10000ULL && size == 0x10000);
    /* 紧接段不可读（--xp）：不合并，扫描范围始终可读 */
    const char *xonly =
        "10000-20000 r-xp 0 08:01 1  /usr/bin/xochitl\n"
        "20000-30000 --xp 0 08:01 1  /usr/bin/xochitl\n"
        "30000-40000 r-xp 0 08:01 1  /usr/bin/xochitl\n";
    CHECK(cj_find_exec_module("xochitl", xonly, &base, &size) == 1);
    CHECK(base == 0x10000ULL && size == 0x10000);
    /* 紧接的是匿名映射：不合并 */
    const char *anon =
        "10000-20000 r-xp 0 08:01 1  /usr/bin/xochitl\n"
        "20000-30000 rwxp 0 00:00 0 \n";
    CHECK(cj_find_exec_module("xochitl", anon, &base, &size) == 1);
    CHECK(base == 0x10000ULL && size == 0x10000);
    /* 最后一行没有换行符的续段也能合并 */
    const char *noeol =
        "10000-20000 r-xp 0 08:01 1  /usr/bin/xochitl\n"
        "20000-30000 rwxp 0 08:01 1  /usr/bin/xochitl";
    CHECK(cj_find_exec_module("xochitl", noeol, &base, &size) == 1);
    CHECK(base == 0x10000ULL && size == 0x20000);
}

static void test_scan_real_proc_self_maps(void) {
    /* maps_content=NULL 走真读 /proc/self/maps：找测试程序自己的代码段，范围应包含 main。 */
    char exe[512];
    ssize_t n = readlink("/proc/self/exe", exe, sizeof exe - 1);
    CHECK(n > 0);
    if (n <= 0) return;
    exe[n] = '\0';
    uintptr_t base = 0;
    size_t size = 0;
    CHECK(cj_find_exec_module(exe, NULL, &base, &size) == 1);
    uintptr_t fn = (uintptr_t)&test_scan_real_proc_self_maps;
    CHECK(fn >= base && fn < base + size);
}

static void test_scan_real_kernel_split_by_mprotect(void) {
    /* 用真内核复现"hook 把代码段切成三段"：把一个 4 页的临时文件以 r-x 私有映射，再把第 2 页
     * mprotect 成 rwx（跟 cj_patch_target 同一个调用），/proc/self/maps 里就会出现
     * r-xp / rwxp / r-xp 三行。合并后的范围必须覆盖全部 4 页。 */
    long page = sysconf(_SC_PAGESIZE);
    char path[] = "/tmp/cj-scan-split-XXXXXX";
    int fd = mkstemp(path);
    CHECK(fd >= 0);
    if (fd < 0) return;
    CHECK(ftruncate(fd, page * 4) == 0);
    uint8_t *m = mmap(NULL, (size_t)page * 4, PROT_READ | PROT_EXEC, MAP_PRIVATE, fd, 0);
    close(fd);
    CHECK(m != MAP_FAILED);
    if (m != MAP_FAILED) {
        CHECK(mprotect(m + page, (size_t)page, PROT_READ | PROT_WRITE | PROT_EXEC) == 0);
        uintptr_t base = 0;
        size_t size = 0;
        CHECK(cj_find_exec_module(path, NULL, &base, &size) == 1);
        CHECK(base == (uintptr_t)m);
        CHECK(size == (size_t)page * 4);
        munmap(m, (size_t)page * 4);
    }
    unlink(path);
}

static void test_scan_degenerate_range_rejected(void) {
    /* end <= start 的畸形行不当命中（否则 size 下溢成天文数字，扫描越界）。 */
    const char *maps = "20000-10000 r-xp 0 08:01 1  /usr/bin/xochitl\n";
    uintptr_t base = 0;
    size_t size = 0;
    CHECK(cj_find_exec_module("xochitl", maps, &base, &size) == 0);
}

/* ── trampoline_aarch64.c ──────────────────────────────────────────────── */

/* 按头注文档的编码规则独立解码回地址：只信"固定操作码位不变 + 立即数字段按 movz/movk 的
 * hw 分段位置摆放"这两条不变式（cj_build_far_jump 头注承诺的内容），不是照抄实现反过来凑。
 */
static uint64_t decode_far_jump(const uint32_t insn[5]) {
    CHECK((insn[0] & 0xFFE0001Fu) == 0xD2800010u); /* movz x16, #imm16          */
    CHECK((insn[1] & 0xFFE0001Fu) == 0xF2A00010u); /* movk x16, #imm16, lsl #16 */
    CHECK((insn[2] & 0xFFE0001Fu) == 0xF2C00010u); /* movk x16, #imm16, lsl #32 */
    CHECK((insn[3] & 0xFFE0001Fu) == 0xF2E00010u); /* movk x16, #imm16, lsl #48 */
    CHECK(insn[4] == 0xD61F0200u);                 /* br x16                   */
    uint64_t part0 = (insn[0] >> 5) & 0xFFFFu;
    uint64_t part1 = (insn[1] >> 5) & 0xFFFFu;
    uint64_t part2 = (insn[2] >> 5) & 0xFFFFu;
    uint64_t part3 = (insn[3] >> 5) & 0xFFFFu;
    return part0 | (part1 << 16) | (part2 << 32) | (part3 << 48);
}

static void test_far_jump_roundtrips_addresses(void) {
    const uint64_t addrs[] = {
        0x0ULL, 0x1234ULL, 0x1000ULL, 0x7f00deadbeefULL,
        0xffffffffffffffffULL, 0x0000555512340678ULL,
    };
    for (size_t i = 0; i < sizeof(addrs) / sizeof(addrs[0]); i++) {
        uint32_t out[5];
        cj_build_far_jump(out, (const void *)(uintptr_t)addrs[i]);
        uint64_t decoded = decode_far_jump(out);
        CHECK(decoded == addrs[i]);
    }
}

/* ── trampoline_patch.c（用 host mmap 出的可读写可执行页模拟"目标函数所在的页"）───── */

static void test_patch_target_rewrites_bytes_and_builds_stub(void) {
    long page = sysconf(_SC_PAGESIZE);
    void *region = mmap(NULL, (size_t)page, PROT_READ | PROT_WRITE | PROT_EXEC,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    CHECK(region != MAP_FAILED);
    if (region == MAP_FAILED) return;

    /* 目标"函数"开头填一段跟远跳转等长（20 字节）的假原始指令，patch 完之后应该原样
     * 出现在 stub 前 20 字节里。 */
    uint8_t original[CJ_FAR_JUMP_LEN];
    for (size_t i = 0; i < sizeof original; i++) original[i] = (uint8_t)(0xA0 + i);
    memcpy(region, original, sizeof original);

    void *handler = (void *)(uintptr_t)0x00005555deadbeefULL; /* 假地址，不会真的跳过去 */
    void *stub = NULL;
    int ok = cj_patch_target(region, handler, CJ_FAR_JUMP_LEN, "test", &stub);
    CHECK(ok == 1);
    CHECK(stub != NULL);
    if (!ok || !stub) { munmap(region, (size_t)page); return; }

    /* 1) target 处现在应该是跳去 handler 的远跳转 */
    uint32_t expect_to_handler[5];
    cj_build_far_jump(expect_to_handler, handler);
    CHECK(memcmp(region, expect_to_handler, CJ_FAR_JUMP_LEN) == 0);

    /* 2) stub 前 CJ_FAR_JUMP_LEN 字节 == 原始字节（原样保留，供 handler 想调用原逻辑时用） */
    CHECK(memcmp(stub, original, sizeof original) == 0);

    /* 3) stub 接着的字节是跳回 target+CJ_FAR_JUMP_LEN 的远跳转 */
    uint32_t expect_jump_back[5];
    void *jump_back_target = (uint8_t *)region + CJ_FAR_JUMP_LEN;
    cj_build_far_jump(expect_jump_back, jump_back_target);
    CHECK(memcmp((uint8_t *)stub + CJ_FAR_JUMP_LEN, expect_jump_back, CJ_FAR_JUMP_LEN) == 0);

    munmap(region, (size_t)page);
    /* stub 本身按设计"不使用后不用释放"（见头注），测试也不 munmap 它，跟生产行为一致。 */
}

static void test_patch_target_invalid_address_fails_safely(void) {
    /* 不是任何有效映射的地址——mprotect 该失败，函数该返回 0 且不崩溃（safe mode）。 */
    void *bogus = (void *)(uintptr_t)0x1; /* 未映射、非页对齐 */
    void *stub = NULL;
    int ok = cj_patch_target(bogus, (void *)0x2, CJ_FAR_JUMP_LEN, "test-bogus", &stub);
    CHECK(ok == 0);
}

int main(void) {
    test_pattern_unique_hit();
    test_pattern_zero_hits_fails();
    test_pattern_multi_hits_fails();
    test_pattern_degenerate_inputs();
    test_pattern_masked_wildcard();
    test_pattern_fast_path_matches_bytewise_loop();

    test_scan_finds_matching_exec_mapping();
    test_scan_suffix_mismatch_not_found();
    test_scan_non_exec_mapping_skipped();
    test_scan_anonymous_mapping_skipped();
    test_scan_returns_first_match();
    test_scan_merges_segments_split_by_hooks();
    test_scan_merge_stops_on_gap_other_file_or_unreadable();
    test_scan_degenerate_range_rejected();
    test_scan_real_proc_self_maps();
    test_scan_real_kernel_split_by_mprotect();

    test_far_jump_roundtrips_addresses();

    test_patch_target_rewrites_bytes_and_builds_stub();
    test_patch_target_invalid_address_fails_safely();

    if (failures == 0) {
        printf("OK: 全部通过\n");
        return 0;
    }
    fprintf(stderr, "%d 处断言失败\n", failures);
    return 1;
}
