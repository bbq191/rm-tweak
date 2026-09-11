#pragma once
#include <stddef.h>

/* 通用 trampoline 安装：用 cj_build_far_jump() 把目标函数开头 patch_len 字节覆盖成一条跳去
 * handler 的远跳转；*out_stub 拿到一份"保留被覆盖字节 + 跳回 target_addr+patch_len"的可执行
 * 调用桩，handler 想调用原始逻辑时跳去 *out_stub 即可（`call_through` 套路）。
 *
 * 逐字节抄自 chinese-ime/langhook/src/hook_init.c 的 make_call_through_stub/patch_target
 * ——2026-09-15 从 hl_snap.c / hw_stroke.c 两份逐字节重复的独立拷贝（当初各自"逐字节抄自
 * hook_init.c"）收进这里，不做任何逻辑改动：唯一的参数化是把原来硬编码的 `PATCH_LEN` 宏改成
 * 显式参数（两边调用方传的值本来就都是 `CJ_FAR_JUMP_LEN`，行为不变），把原来硬编码的日志前缀
 * （`[hl-snap]`/`[hw-stroke]`）改成 `tag` 参数（调用方传自己的名字，日志输出文本不变）。
 *
 * 失败（mmap/mprotect 失败）返回 0 且已经打印过一行 `[tag] ... 放弃 hook（safe mode）` 到
 * stderr，调用方应放弃这个 hook（这台设备/这个固件版本安全放弃，不崩、不重试）。成功返回 1，
 * `*out_stub` 是新分配的可执行内存（不使用后不用释放——生命周期等于插件的进程生命周期）。
 */
int cj_patch_target(void *target_addr, void *handler, size_t patch_len, const char *tag, void **out_stub);
