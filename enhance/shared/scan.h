#pragma once
#include <stddef.h>
#include <stdint.h>

/* 解析 /proc/self/maps，找路径以 module_path_suffix 结尾、且带执行权限（r-xp）
 * 的第一段映射，连同紧跟其后的续段合并成一段，取得它在当前进程地址空间里的运行时
 * 基址和大小。
 *
 * 之所以能这么简单——我们自己的扩展 .so 是被 dlopen() 进 xochitl 进程本身的
 * （xovi 的注入机制就是把 xovi.so 挂在 xochitl 前面，xovi.so 再 dlopen() 各扩展，
 * 参见白皮书 10.2 节），跟 xochitl 共享同一个地址空间，读 /proc/self/maps 就是
 * 在读 xochitl 自己的映射表，不需要 ptrace 一类跨进程手段。
 *
 * 一个 ELF 可执行文件通常只有一个 .text 对应的 PT_LOAD 可执行段，体现为 maps 里
 * 连续的一段 r-xp 映射。但每装一个 hook，cj_patch_target 的 mprotect 都会把目标
 * 所在页切成独立的 rwxp 段（xochitl 的代码段因此被切成 r-xp / rwxp / r-xp …）；
 * 若只取第一段，后装的扩展就找不到地址比已 patch 页更高的目标（2026-09-24 审查，
 * 白皮书 §04「hook 安全性」）。所以找到第一段后，把紧随其后、同一路径、地址首尾
 * 相接、可读可执行（r-xp 或 rwxp）的续段一并算进来；遇到任何不满足的行就停。
 * 不相接的第二处同名映射不合并（仍只取第一处）。
 *
 * 返回 1 = 找到（*out_base / *out_size 有效），0 = 没找到。
 * maps_content 允许调用方直接传入已经读好的 /proc/self/maps 全文内容，方便单元
 * 测试用构造好的字符串跑，不需要真的操作 /proc；传 NULL 时函数会自己读
 * /proc/self/maps。
 */
int cj_find_exec_module(const char *module_path_suffix,
                         const char *maps_content /* 可为 NULL */,
                         uintptr_t *out_base, size_t *out_size);
