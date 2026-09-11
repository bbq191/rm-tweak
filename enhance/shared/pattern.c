#include "pattern.h"
#include <string.h>

/* 手写朴素字符串搜索，不依赖 memmem（GNU 扩展，要求 _GNU_SOURCE）——这段代码要
 * 同时在宿主机（跑单元测试）和交叉编译目标（aarch64 目标系统的 libc）上编译，
 * 少一个功能测试宏依赖就少一处环境差异。特征码只有 48 字节、目标内存区最大也
 * 就是 xochitl 的 .text 段（十几 MB），O(n*m) 的朴素搜索在这个规模下够用，不需要
 * KMP/Boyer-Moore 这类更复杂的算法；精确匹配只加了一层 memchr 找首字节（C 标准函数，
 * 不需要功能测试宏），见 cj_count_pattern_masked。
 */
/* 单点匹配：mask==NULL 走 memcmp 快路径；否则逐字节按掩码比较（mask[j]==0 跳过）。 */
static int match_at(const uint8_t *hp, const uint8_t *pattern,
                    const uint8_t *mask, size_t pattern_len) {
    if (mask == NULL) {
        return memcmp(hp, pattern, pattern_len) == 0;
    }
    for (size_t j = 0; j < pattern_len; j++) {
        if (mask[j] != 0 && hp[j] != pattern[j]) return 0;
    }
    return 1;
}

size_t cj_count_pattern_masked(const uint8_t *haystack, size_t haystack_len,
                                const uint8_t *pattern, const uint8_t *mask,
                                size_t pattern_len, uintptr_t *first_addr) {
    size_t count = 0;
    if (pattern_len == 0 || haystack_len < pattern_len) return 0;

    size_t last_start = haystack_len - pattern_len;
    if (mask == NULL) {
        /* 精确匹配快路径：先用 memchr 跳到首字节相同的位置再整段比较——结果与下面的逐字节
         * 循环完全一致（同样统计所有重叠命中、first_addr 同样是最低地址），只是不再对每个
         * 字节都调一次 memcmp。两个扩展加载时各要扫整个 xochitl 代码段若干遍（hl-snap 2 遍、
         * hw-stroke 最多 5 遍），扩展按 -O0 编译，逐字节循环在 host 上约 40ms/16MB。 */
        const uint8_t *p = haystack;
        const uint8_t *last = haystack + last_start;
        while (p <= last) {
            const uint8_t *hit = memchr(p, pattern[0], (size_t)(last - p) + 1);
            if (hit == NULL) break;
            if (memcmp(hit, pattern, pattern_len) == 0) {
                if (count == 0 && first_addr != NULL) {
                    *first_addr = (uintptr_t)hit;
                }
                count++;
            }
            p = hit + 1;
        }
        return count;
    }
    for (size_t i = 0; i <= last_start; i++) {
        if (match_at(haystack + i, pattern, mask, pattern_len)) {
            if (count == 0 && first_addr != NULL) {
                *first_addr = (uintptr_t)(haystack + i);
            }
            count++;
        }
    }
    return count;
}

int cj_find_unique_pattern_masked(const uint8_t *haystack, size_t haystack_len,
                                   const uint8_t *pattern, const uint8_t *mask,
                                   size_t pattern_len, uintptr_t *out_addr) {
    uintptr_t first = 0;
    size_t count = cj_count_pattern_masked(haystack, haystack_len, pattern, mask,
                                           pattern_len, &first);
    if (count != 1) return 0;
    *out_addr = first;
    return 1;
}

/* 不带掩码的版本：薄封装到 masked 版本传 mask=NULL，保持现有调用方不动。 */
size_t cj_count_pattern(const uint8_t *haystack, size_t haystack_len,
                         const uint8_t *pattern, size_t pattern_len,
                         uintptr_t *first_addr) {
    return cj_count_pattern_masked(haystack, haystack_len, pattern, NULL,
                                   pattern_len, first_addr);
}

int cj_find_unique_pattern(const uint8_t *haystack, size_t haystack_len,
                            const uint8_t *pattern, size_t pattern_len,
                            uintptr_t *out_addr) {
    return cj_find_unique_pattern_masked(haystack, haystack_len, pattern, NULL,
                                         pattern_len, out_addr);
}
