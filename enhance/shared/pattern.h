#pragma once
#include <stddef.h>
#include <stdint.h>

/* 在 haystack[0..haystack_len) 里搜索 pattern[0..pattern_len) 这段字节序列。
 *
 * 要求“命中且只命中一次”——0 次或 >1 次都视为失败，返回 0，不写 *out_addr。
 * 这个“唯一性”校验不是可选项：白皮书 10.1 节的实测教训是同一个 __FILE__ 字符串
 * 会被同一编译单元里所有日志调用共享，如果不做唯一性校验，特征码万一在其他函数
 * 里也凑巧出现，会在错误的地址上打 patch，直接导致 xochitl crash。宁可在“找不到
 * /找到多个”的时候直接放弃 hook（对应白皮书 10.3 节的 safe mode 设计），也不要
 * 在不确定的情况下继续。
 *
 * 返回值：1 = 唯一命中（*out_addr 有效），0 = 命中数不是 1（*out_addr 不变）。
 */
int cj_find_unique_pattern(const uint8_t *haystack, size_t haystack_len,
                            const uint8_t *pattern, size_t pattern_len,
                            uintptr_t *out_addr);

/* 内部用，测试也会直接调用：统计命中次数（用于诊断 0 次 / 命中多次这两种失败
 * 分别是什么情况，不只是笼统报“失败”）。first_addr 在至少命中一次时会被写入
 * 第一次命中的地址，方便日志里打印出来供人工核查。
 */
size_t cj_count_pattern(const uint8_t *haystack, size_t haystack_len,
                         const uint8_t *pattern, size_t pattern_len,
                         uintptr_t *first_addr);

/* 带掩码的版本——韧性重构用：AArch64 的 BL/B 是相对跳转，callee 一旦重定位，
 * 指令里编码的相对偏移就变，哪怕被 hook 的函数体逻辑一个字节没改。对这几个字节
 * 做通配，就能让同一段特征码跨固件版本继续唯一命中。
 *
 * mask 与 pattern 等长，逐字节语义：mask[i] == 0 表示 pattern[i] 这个字节通配
 * （不参与比较），mask[i] != 0 表示必须精确匹配。mask == NULL 时退化成精确匹配
 * （跟不带掩码的版本完全等价）。
 *
 * 唯一性校验跟不带掩码的版本一致：0 次或 >1 次都失败。通配会放宽匹配、更容易
 * 撞车，所以掩码要尽量只盖住真正会变的那几个字节（一条 BL = 连续 4 字节），
 * 盖得越多唯一性越脆，必须靠离线对拍两版二进制确认仍然唯一命中。
 */
size_t cj_count_pattern_masked(const uint8_t *haystack, size_t haystack_len,
                                const uint8_t *pattern, const uint8_t *mask,
                                size_t pattern_len, uintptr_t *first_addr);

int cj_find_unique_pattern_masked(const uint8_t *haystack, size_t haystack_len,
                                   const uint8_t *pattern, const uint8_t *mask,
                                   size_t pattern_len, uintptr_t *out_addr);
