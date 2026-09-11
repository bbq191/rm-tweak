#include "trampoline_aarch64.h"

void cj_build_far_jump(uint32_t out[5], const void *target) {
    uint64_t addr = (uint64_t)(uintptr_t)target;
    uint32_t part0 = (uint32_t)((addr >> 0) & 0xFFFF);
    uint32_t part1 = (uint32_t)((addr >> 16) & 0xFFFF);
    uint32_t part2 = (uint32_t)((addr >> 32) & 0xFFFF);
    uint32_t part3 = (uint32_t)((addr >> 48) & 0xFFFF);

    out[0] = 0xD2800010u | (part0 << 5); /* movz x16, #part0            */
    out[1] = 0xF2A00010u | (part1 << 5); /* movk x16, #part1, lsl #16   */
    out[2] = 0xF2C00010u | (part2 << 5); /* movk x16, #part2, lsl #32   */
    out[3] = 0xF2E00010u | (part3 << 5); /* movk x16, #part3, lsl #48   */
    out[4] = 0xD61F0200u;                /* br x16                     */
}
