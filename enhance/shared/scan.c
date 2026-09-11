#include "scan.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <ctype.h>

/* 把 /proc/self/maps 整个读进一块动态分配的缓冲区。这个文件不支持用 fstat 提前
 * 拿到大小（/proc 下大多数文件都这样，内核是按需生成内容的），只能循环读到 EOF，
 * 缓冲区不够就翻倍扩容。调用方用完后负责 free()。
 */
static char *read_whole_file(const char *path) {
    FILE *f = fopen(path, "r");
    if (!f) return NULL;

    size_t cap = 64 * 1024;
    size_t len = 0;
    char *buf = malloc(cap);
    if (!buf) { fclose(f); return NULL; }

    for (;;) {
        if (len == cap) {
            cap *= 2;
            char *grown = realloc(buf, cap);
            if (!grown) { free(buf); fclose(f); return NULL; }
            buf = grown;
        }
        size_t n = fread(buf + len, 1, cap - len, f);
        len += n;
        if (n == 0) break; /* EOF 或出错，都停止 */
    }
    fclose(f);

    if (len == cap) {
        char *grown = realloc(buf, cap + 1);
        if (!grown) { free(buf); return NULL; }
        buf = grown;
    }
    buf[len] = '\0';
    return buf;
}

static int str_ends_with(const char *s, const char *suffix) {
    size_t slen = strlen(s), suflen = strlen(suffix);
    if (suflen > slen) return 0;
    return memcmp(s + slen - suflen, suffix, suflen) == 0;
}

/* 解析单行 /proc/self/maps，格式例：
 *   55d2f1a00000-55d2f1a21000 r-xp 00000000 08:01 123456   /usr/bin/xochitl
 * 成功且是可执行映射、路径匹配后缀，返回 1 并填出 base/end 与完整路径（out_path，
 * 容量 PATH_CAP）；否则返回 0。
 */
#define PATH_CAP 512
static int parse_maps_line(const char *line, const char *module_path_suffix,
                            uintptr_t *out_base, uintptr_t *out_end,
                            char *out_perms /* 容量 8 */, char *out_path /* 容量 PATH_CAP */) {
    unsigned long long start = 0, end = 0;
    char perms[8] = {0};
    int consumed = 0;

    if (sscanf(line, "%llx-%llx %7s%n", &start, &end, perms, &consumed) != 3) {
        return 0;
    }
    /* 只要可执行映射（r-xp / r-x-p 之类，第三个字符是 'x' 就算） */
    if (strlen(perms) < 3 || perms[2] != 'x') return 0;

    /* pathname 是这一行最后一个以空白分隔的字段（如果存在——匿名映射没有这个
     * 字段，直接跳过）。从 sscanf 消费掉的位置往后找最后一段非空白 token。 */
    const char *rest = line + consumed;
    const char *path_start = NULL;
    const char *p = rest;
    while (*p) {
        while (*p == ' ' || *p == '\t') p++;
        if (!*p || *p == '\n') break;
        path_start = p;
        while (*p && *p != ' ' && *p != '\t' && *p != '\n') p++;
    }
    if (!path_start) return 0; /* 匿名映射，没有路径字段 */

    /* path_start 到行尾（去掉换行）就是路径；拷进调用方缓冲区做后缀比较 */
    size_t i = 0;
    while (path_start[i] && path_start[i] != '\n' && i < PATH_CAP - 1) {
        out_path[i] = path_start[i];
        i++;
    }
    out_path[i] = '\0';

    if (!str_ends_with(out_path, module_path_suffix)) return 0;
    if (end <= start) return 0;

    memcpy(out_perms, perms, sizeof(perms));
    *out_base = (uintptr_t)start;
    *out_end = (uintptr_t)end;
    return 1;
}

int cj_find_exec_module(const char *module_path_suffix,
                         const char *maps_content,
                         uintptr_t *out_base, size_t *out_size) {
    char *owned = NULL;
    const char *content = maps_content;
    if (content == NULL) {
        owned = read_whole_file("/proc/self/maps");
        if (!owned) return 0;
        content = owned;
    }

    /* found=0：还在找第一段；found=1：已找到第一段，往后只看紧邻的续段。 */
    int found = 0;
    uintptr_t base = 0, end = 0;
    char first_path[PATH_CAP];
    const char *line_start = content;
    while (*line_start) {
        const char *line_end = strchr(line_start, '\n');
        size_t line_len = line_end ? (size_t)(line_end - line_start) : strlen(line_start);

        char linebuf[1024];
        int parsed = 0;
        uintptr_t seg_start = 0, seg_end = 0;
        char perms[8] = {0};
        char path[PATH_CAP];
        if (line_len < sizeof(linebuf)) {
            memcpy(linebuf, line_start, line_len);
            linebuf[line_len] = '\0';
            parsed = parse_maps_line(linebuf, module_path_suffix, &seg_start, &seg_end, perms, path);
        }
        if (!found) {
            if (parsed) {
                found = 1;
                base = seg_start;
                end = seg_end;
                memcpy(first_path, path, sizeof(first_path));
            }
        } else {
            /* 续段：必须同一文件、地址紧接、可读可执行（hook 把某页 mprotect 成 rwxp 后，
             * 内核把原来的一整段 r-xp 切成 r-xp / rwxp / r-xp 三段——见 scan.h 头注）。
             * 任何一条不满足就停，范围始终是一段连续、可读的内存。 */
            if (!parsed || seg_start != end || perms[0] != 'r' || strcmp(path, first_path) != 0) break;
            end = seg_end;
        }
        if (!line_end) break;
        line_start = line_end + 1;
    }

    if (owned) free(owned);
    if (!found) return 0;
    *out_base = base;
    *out_size = (size_t)(end - base);
    return 1;
}
