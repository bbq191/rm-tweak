#include "trampoline_patch.h"
#include "trampoline_aarch64.h"

#include <stdio.h>
#include <string.h>
#include <errno.h>
#include <sys/mman.h>
#include <unistd.h>
#include <stdint.h>

static void *make_call_through_stub(const uint8_t *original_bytes, size_t patch_len, void *jump_back_target) {
    size_t stub_len = patch_len + CJ_FAR_JUMP_LEN;
    void *stub = mmap(NULL, stub_len, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (stub == MAP_FAILED) return NULL;

    memcpy(stub, original_bytes, patch_len);

    uint32_t jump_instrs[5];
    cj_build_far_jump(jump_instrs, jump_back_target);
    memcpy((uint8_t *)stub + patch_len, jump_instrs, CJ_FAR_JUMP_LEN);

    if (mprotect(stub, stub_len, PROT_READ | PROT_EXEC) != 0) {
        munmap(stub, stub_len);
        return NULL;
    }
    __builtin___clear_cache((char *)stub, (char *)stub + stub_len);
    return stub;
}

int cj_patch_target(void *target_addr, void *handler, size_t patch_len, const char *tag, void **out_stub) {
    long pagesize = sysconf(_SC_PAGESIZE);
    if (pagesize <= 0) pagesize = 4096;

    uintptr_t page_base = (uintptr_t)target_addr & ~((uintptr_t)pagesize - 1);
    size_t region_len = (size_t)pagesize;
    if ((((uintptr_t)target_addr - page_base) + patch_len) > region_len) {
        region_len += (size_t)pagesize;
    }

    if (mprotect((void *)page_base, region_len, PROT_READ | PROT_WRITE | PROT_EXEC) != 0) {
        fprintf(stderr, "[%s] mprotect 失败，放弃 hook（safe mode）：%s\n", tag, strerror(errno));
        return 0;
    }

    void *jump_back_target = (uint8_t *)target_addr + patch_len;
    void *stub = make_call_through_stub((const uint8_t *)target_addr, patch_len, jump_back_target);
    if (!stub) {
        fprintf(stderr, "[%s] 调用桩分配失败，放弃 hook（safe mode）\n", tag);
        return 0;
    }
    *out_stub = stub;

    uint32_t jump_to_handler[5];
    cj_build_far_jump(jump_to_handler, handler);
    memcpy(target_addr, jump_to_handler, CJ_FAR_JUMP_LEN);
    __builtin___clear_cache((char *)target_addr, (char *)target_addr + CJ_FAR_JUMP_LEN);

    return 1;
}
