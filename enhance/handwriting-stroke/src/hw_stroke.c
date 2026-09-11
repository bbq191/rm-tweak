/* hw-stroke —— CJK 手写笔迹渲染优化。
 *
 * 第一轮真机实验（factor 统一缩放）验证过"改变宽笔画几何生成函数的入口宽度，
 * 能不能真的影响渲染出来的笔迹粗细"这件事本身可行（真机截图肉眼确认）。
 *
 * 第二轮：笔尖角度模型（西式书法笔工具的经典公式，社区调研见白皮书 §03f）：
 *   宽度 ×= min_ratio + (1-min_ratio) × |sin(运笔方向角 − 笔尖固定角度)|
 * 运笔方向角**不依赖点结构里的方向字节**（拿真机诊断 hook 实测过：日常用的
 * "中粗钢笔"走的是 `bVar16<4` 那条纯线性分支，压根不会往点结构方向字段那条
 * 路径走）——改成在 `FUN_00f47530` 自己内部本来就维护的"上一个点坐标"
 * （`ctx+0x48`/`ctx+0x4c`）现算 `atan2(dy,dx)`，这段状态是这个函数自己的、
 * 所有笔型都统一走到，不挑分支。
 *
 * hook 目标 `FUN_00f47530`（变宽笔画的几何生成器）是从 `ShapesOverlay::
 * updateImage`（`FUN_008bbb80`）反编译直接跟踪下来的，不是靠 vtable 成员关系
 * 猜的——`VaryingGenerator_WidthLength::generate()`（`FUN_00f401f0`）那次反编译
 * 简化过头、从未被真实调用点引用过，已经在 ../README.md 里勘误，这次不拿它当
 * hook 目标。
 *
 * 第三轮：中文毛笔"提按"预研（运笔速度代理）。真实硬件压感字节（点结构
 * offset 0xD）确认存在、确认不是垃圾数据（两次真机验证：接触瞬态平滑爬坡、
 * 专测轻重力度时分布也是真实连续变化的），但接进 `FUN_00f47530` 这个 hook
 * 失败了——诊断 hook 挂在 `FUN_00f3f9d0` 入口读到的压感样本里，落在"会调用
 * FUN_00f47530"那个 bVar16 分支的极少（3449 个有效样本里只有 2 个），但同一
 * 时段 `FUN_00f47530` 自己却触发了 686 次——说明 `FUN_00f47530` 的调用大部分
 * 没有经过我们锁定的这一份 `FUN_00f3f9d0`（`updateImage` 反编译时就见过
 * `FUN_00f3f9d0` 有 6 个不同调用点，很可能"正在画的实时预览"和"提交进笔记本"
 * 走不同路径，只有部分路径经过这一份）。跨函数传值这条路暂时放弃，改成完全
 * 在 `FUN_00f47530` 自己 hook 内部算：复用已有的"上一个点坐标"缓存算出两点
 * 距离（运笔速度代理）——慢/顿笔→粗，快/带过→细，跟笔尖角度模型同一个 hook、
 * 同一套"效果强度按基础宽度挂钩"的思路，不依赖任何跨函数状态。压感诊断 hook
 * （`cj_hw_dispatch_handler`）保留，纯只读不参与行为，留给以后想再查"到底还
 * 有哪些路径能到 FUN_00f47530"用。
 *
 * 复用 enhance/shared/ 下的通用工具文件（跟 enhance/hl-snap 同一份独立副本，见
 * 那边的头注）；通用 trampoline 安装（cj_patch_target）同样在 shared/ 里
 * （2026-09-15 前这里跟 hl_snap.c 各有一份逐字节重复的 patch_target/
 * make_call_through_stub，全量代码审查审出后收进 enhance/shared/trampoline_patch.c，
 * 不是本次功能定制逻辑）。
 *
 * `FUN_00f47530(float x, float y, void *ctx)` 是标准 AAPCS64 调用约定
 * （两个 float 走 s0/s1，一个指针走 x0），handler 签名照抄这个约定，不需要
 * 处理特殊 ABI。函数入口第一件事就是 `*(float*)((char*)ctx+4) * 0.5` 算半宽，
 * `(char*)ctx+4` 就是"当前点宽度"，调用方（FUN_00f3f9d0）刚存进去、这个函数
 * 刚读出来就用——在这里改这个值最简单可靠。
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <stdbool.h>
#include <math.h>

#include "scan.h"
#include "pattern.h"
#include "trampoline_aarch64.h"
#include "trampoline_patch.h"

#define TARGET_MODULE_SUFFIX "/usr/bin/xochitl"
#define CJ_DATA_DIR "/home/root/.local/share/cangjie-ime"
#define CJ_READING_QOL_PATH CJ_DATA_DIR "/reading-qol.json"

/* FUN_00f47530 前 20 字节是纯栈/寄存器操作（paciasp/stp x29,x30/mov x29,sp/
 * stp x19,x20/mov x19,x0），没有分支、没有 PC 相对寻址，安全可 patch——但这
 * 20 字节本身太通用（拿真机 3.28.0.172 二进制实测，这个形状在全文件里命中
 * 113 次！常见 C++ 方法序言长这样的太多了），不能只拿这 20 字节当特征码。
 * 签名延长到 32 字节（再加 3 条指令：ldrb w0,[x0,#0x5a] / stp d12,d13,[sp,
 * #0x40] / fmov s13,s1，offset 0x5a 这种具体立即数足够把命中收窄到 1），真机
 * 实测确认唯一命中——签名比 PATCH_LEN 长没问题，patch_target 依然只覆盖前
 * PATCH_LEN(20) 字节，多出来的字节只用来提高定位唯一性，不参与 patch。 */
static const uint8_t PROLOGUE_HW_QUAD[] = {
    0x3f, 0x23, 0x03, 0xd5, 0xfd, 0x7b, 0xb9, 0xa9, 0xfd, 0x03, 0x00, 0x91,
    0xf3, 0x53, 0x01, 0xa9, 0xf3, 0x03, 0x00, 0xaa, 0x00, 0x68, 0x41, 0x39,
    0xec, 0x37, 0x04, 0x6d, 0x2d, 0x40, 0x20, 0x1e,
};

/* FUN_00f3f9d0 前 32 字节（paciasp/sub sp,#0x130/cmp w2,#1/stp x29,x30/add
 * x29,sp,#0x40/stp x19,x20/mov x19,x1/stp x21,x22），全是纯栈/寄存器操作，
 * 拿真机 3.28.0.172 二进制实测：12 字节起即唯一命中，这里用满 32 字节留
 * 余量。这是逐点渲染分派函数——诊断 hook 用，探查真机写字实际走哪个笔型
 * 分支，见 cj_hw_dispatch_handler 头注。 */
static const uint8_t PROLOGUE_HW_DISPATCH[] = {
    0x3f, 0x23, 0x03, 0xd5, 0xff, 0xc3, 0x04, 0xd1, 0x5f, 0x04, 0x00, 0x71,
    0xfd, 0x7b, 0x04, 0xa9, 0xfd, 0x03, 0x01, 0x91, 0xf3, 0x53, 0x05, 0xa9,
    0xf3, 0x03, 0x01, 0xaa, 0xf5, 0x5b, 0x06, 0xa9,
};

/* FUN_00f4c8d0——第四轮调查找到的第二个"变宽几何生成器"：headless 反编译
 * 核实过真实签名 `(float x, float y, undefined2 *ctx)`，ctx 里宽度字段偏移
 * `param_3+2`（undefined2 单位=2 字节，即字节偏移 4）——跟 FUN_00f47530 的
 * `ctx+4` 是同一个约定，可以直接复用同一套效果计算逻辑。这是 bVar16==6 分支
 * 直接调用、bVar16==5 分支经 FUN_00f4d190 写完宽度后间接调用（FUN_00f4d190
 * 自己也把宽度写进同一个 ctx+4，见白皮书）的目标，两个笔型分支都覆盖到。
 * 前 20 字节（paciasp/sub sp,#0xc0/stp x29,x30/add x29,sp,#0x20/stp x19,x20）
 * 纯栈操作、没有分支/PC 相对寻址，patch 安全——**同一轮反编译还核实了另一个
 * 候选 FUN_00f4f430（bVar16==3 分支），它前 20 字节第 3 条指令就是条件分支
 * （cbz），PC 相对寻址在 memcpy 进 stub 后地址会算错，这个不安全，没有拿来
 * 当 hook 目标**（项目纪律：分支/PC 相对寻址一律排除，跟 FUN_00f47530 当初
 * 的筛选标准一致）。32 字节签名真机 3.28.0.172 二进制实测唯一命中。 */
static const uint8_t PROLOGUE_HW_QUAD2[] = {
    0x3f, 0x23, 0x03, 0xd5, 0xff, 0x03, 0x03, 0xd1, 0xfd, 0x7b, 0x02, 0xa9,
    0xfd, 0x83, 0x00, 0x91, 0xf3, 0x53, 0x03, 0xa9, 0xf3, 0x03, 0x00, 0xaa,
    0x00, 0xa0, 0x41, 0x39, 0xee, 0x3f, 0x0a, 0x6d,
};

#define PATCH_LEN CJ_FAR_JUMP_LEN /* 覆盖目标函数开头的字节数，跟远跳转指令长度一致 */

/* 通用 trampoline 安装（cj_patch_target）在 enhance/shared/trampoline_patch.c，
 * 2026-09-15 全量代码审查发现这里跟 hl_snap.c 逐字节重复后收进去了，见该文件头注。 */

/* ---- 宽度实验本体 ---- */

typedef void (*orig_hw_quad_fn_t)(float, float, void *);
static orig_hw_quad_fn_t g_orig_hw_quad_call_through = NULL;

/* 宽度缩放系数：1.0 = 不改变行为（默认，fail-safe）。第一轮真机验证先保持
 * 默认值确认"零写入、纯诊断"这一步的日志/数值符合预期，再手动改配置到
 * 2.0/0.5 验证"改这条渲染路径能真实影响笔迹粗细"这件事本身是否可行——两步
 * 验证用同一份构建，不用重新编译部署，改配置文件即可。 */
static float g_width_factor = 1.0f;

/* 笔尖角度模型（西式书法笔工具经典公式，社区调研见白皮书 §03f）：
 *   宽度 ×= min_ratio + (1-min_ratio) × |sin(运笔方向角 − 笔尖固定角度)|
 * min_ratio=1.0 时公式恒等于 1（不管方向角多少，乘数都是 1），等价于关闭——
 * 默认值，fail-safe。角度用角度制存、用的时候转弧度，方便手改配置文件。 */
static float g_nib_angle_deg = 45.0f;
static float g_nib_min_ratio = 1.0f;

/* 真机实测发现：钢笔（细笔画，w 大概 1.5~7.5）跟毛笔（粗笔画，w 能到 67）
 * 走的是完全同一条代码分支、同一套公式——用户反馈"钢笔上效果不好、毛笔上
 * 还行"，根因不是分支不同，是同一个比例摆动在细笔画上显得突兀、在粗笔画上
 * 才像书法笔触该有的过渡。不按笔型分支（两者本来就没区别），改成效果强度
 * 跟基础宽度挂钩——细笔画自动趋近"关闭"，粗笔画自动趋近完整强度。 */
static float g_nib_width_low = 6.0f;   /* w 低于这个值，效果趋近关闭 */
static float g_nib_width_high = 20.0f; /* w 高于这个值，效果满强度（g_nib_min_ratio） */

/* 提按（运笔速度代理）模型：
 *   宽度 ×= min_ratio + (1-min_ratio) × (1 − clamp((len-len_low)/(len_high-len_low),0,1))
 * len 是相邻两点的欧氏距离（同一个 hook 里本来就要算，笔尖角度模型也用它）。
 * len 小（慢/顿笔）→ 比例趋近 1.0（粗，"按"）；len 大（快/带过）→ 比例趋近
 * min_ratio（细，"提"）。min_ratio=1.0 时公式恒等于 1，等价于关闭——默认值，
 * fail-safe。效果强度同样按基础宽度挂钩（复用 g_nib_width_low/high 这组阈值，
 * 语义是共通的"多细的笔画不该有任何顿挫效果"，不单独开一组配置项）。
 * len_low/high 单位是像素/采样点，真机诊断没有直接采过 len 的分布，先给一个
 * 保守起点，效果不理想就手改配置调。 */
static float g_speed_min_ratio = 1.0f; /* 默认关闭，fail-safe——跟 g_nib_min_ratio 同一个约定 */
static float g_speed_len_low = 1.0f;
static float g_speed_len_high = 8.0f;

/* ⚠️ 曾经试过按笔型标签（`*(byte*)(lVar7+0x70)`，FUN_00f3f9d0 里的 bVar16）
 * 精确排除钢笔，撤回了——那个字段是 FUN_00f3f9d0 自己 `lVar7` 对象上的，
 * 钢笔走的 `bVar16<4` 分支用 `plVar6`（另一个指针）做虚函数调用，从没验证过
 * 这条路径下 FUN_00f47530 收到的 ctx 就是同一个 lVar7——真机实测大号
 * paintbrush 时这个假设直接崩了（读出来的"笔型"在 0~255 上随机跳，w 却是
 * 正常连续变化的真实数据），说明 ctx+0x70 对这条路径读的是无关内存，不是
 * 真的笔型标签。**只留纯宽度渐变**（ctx+4/0x48/0x4c/0x5a 是 FUN_00f47530
 * 自己定义并使用的字段，不管上游从哪条路径调进来，这几个偏移都可靠——
 * FUN_00f47530 自己的反编译代码直接用到它们，不是借别的函数的解读）。见
 * 白皮书 §03f「已知局限」。 */

/* 从已经读进内存的 reading-qol.json 内容里找一个浮点字段，找不到/解析失败
 * 不改 *out（调用方决定初始默认值）。仿 hl_snap.c 的极简字段扫描手法，不引
 * JSON 库——跟 cj_hw_refresh_config 里三个字段共用，避免三份重复的
 * strstr+strtod 样板。 */
static void cj_hw_read_float_key(const char *buf, const char *key, float *out) {
    const char *p = strstr(buf, key);
    if (!p) return;
    p += strlen(key);
    while (*p == ':' || *p == ' ' || *p == '\t' || *p == '"') p++;
    char *end = NULL;
    double v = strtod(p, &end);
    if (end == p) return; /* 没解析出数字，保持原值 */
    *out = (float)v;
}

/* 运行时开关：从 reading-qol.json 读 hwStrokeWidthFactor/hwStrokeNibAngleDeg/
 * hwStrokeNibMinRatio。fail-safe：文件缺失/字段缺失/解析失败 → 不改，保持
 * 当前值（初始默认=不改变行为）；解出来的值超出合理范围也拒绝，不写入。
 * 划一笔才调，非热路径，每次读一次即可。 */
static void cj_hw_refresh_config(void) {
    FILE *f = fopen(CJ_READING_QOL_PATH, "rb");
    if (!f) return;
    char buf[4096];
    size_t n = fread(buf, 1, sizeof(buf) - 1, f);
    fclose(f);
    buf[n] = '\0';

    float width_factor = g_width_factor;
    cj_hw_read_float_key(buf, "\"hwStrokeWidthFactor\"", &width_factor);
    if (width_factor > 0.0f && width_factor <= 10.0f) { /* ≤0 会让笔画消失，过大可能撑爆渲染/裁剪逻辑 */
        g_width_factor = width_factor;
    }

    float nib_angle = g_nib_angle_deg;
    cj_hw_read_float_key(buf, "\"hwStrokeNibAngleDeg\"", &nib_angle);
    if (nib_angle >= -360.0f && nib_angle <= 360.0f) {
        g_nib_angle_deg = nib_angle;
    }

    float nib_ratio = g_nib_min_ratio;
    cj_hw_read_float_key(buf, "\"hwStrokeNibMinRatio\"", &nib_ratio);
    if (nib_ratio >= 0.0f && nib_ratio <= 1.0f) { /* >1 或 <0 都没有物理意义（不是"最小比例"了） */
        g_nib_min_ratio = nib_ratio;
    }

    float width_low = g_nib_width_low;
    cj_hw_read_float_key(buf, "\"hwStrokeNibWidthLow\"", &width_low);
    if (width_low >= 0.0f) g_nib_width_low = width_low;

    float width_high = g_nib_width_high;
    cj_hw_read_float_key(buf, "\"hwStrokeNibWidthHigh\"", &width_high);
    if (width_high >= 0.0f) g_nib_width_high = width_high;

    float speed_ratio = g_speed_min_ratio;
    cj_hw_read_float_key(buf, "\"hwStrokeSpeedMinRatio\"", &speed_ratio);
    if (speed_ratio >= 0.0f && speed_ratio <= 1.0f) g_speed_min_ratio = speed_ratio;

    float speed_len_low = g_speed_len_low;
    cj_hw_read_float_key(buf, "\"hwStrokeSpeedLenLow\"", &speed_len_low);
    if (speed_len_low >= 0.0f) g_speed_len_low = speed_len_low;

    float speed_len_high = g_speed_len_high;
    cj_hw_read_float_key(buf, "\"hwStrokeSpeedLenHigh\"", &speed_len_high);
    if (speed_len_high >= 0.0f) g_speed_len_high = speed_len_high;
}

/* ctx 里 FUN_00f47530 自己维护的状态字段（跟点结构/点结构的方向字节无关，
 * 这个函数自己算自己的，见文件头注）：
 *   ctx+0x48/ctx+0x4c：上一个点的 x/y（float）
 *   ctx+0x5a：是不是这一笔的第一个点（非 0 = 已经有上一个点了）
 * offset 来自反编译交叉核对过的 FUN_00f47530 反汇编，不是猜的——跟"笔型
 * 标签"那次不一样，这几个字段是 FUN_00f47530 自己的代码直接用到、自己
 * 读写的，不管上游从哪条路径调进来都可靠（见上面的勘误说明）。 */
#define HW_CTX_LAST_X_OFF  0x48
#define HW_CTX_LAST_Y_OFF  0x4c
#define HW_CTX_HAS_PREV_OFF 0x5a

/* 相邻两点的位移，笔尖角度模型/提按速度模型共用（避免两份重复的
 * last_x/last_y 读取 + sqrt）。have_delta=0 表示笔画起点（没有上一个点），
 * 两个效果各自决定"没有方向/速度信息时"要不要直接放弃（目前都是直接放弃，
 * 等价于该点不受影响）。 */
typedef struct {
    int have_delta;
    float dx, dy, len;
} cj_hw_delta_t;

static cj_hw_delta_t cj_hw_compute_delta(float x, float y, void *ctx) {
    cj_hw_delta_t d = {0, 0.0f, 0.0f, 0.0f};
    if (*((uint8_t *)ctx + HW_CTX_HAS_PREV_OFF) == 0) return d; /* 笔画起点 */

    float last_x = *(float *)((uint8_t *)ctx + HW_CTX_LAST_X_OFF);
    float last_y = *(float *)((uint8_t *)ctx + HW_CTX_LAST_Y_OFF);
    d.dx = x - last_x;
    d.dy = y - last_y;
    /* __builtin_sqrtf 编译成 ARM64 原生 FSQRT 指令，不经过 libm 符号——
     * sqrtf() 本身需要 GLIBC_2.43，设备 libm 没这么新，dlopen 直接解析失败
     * （真机实测撞到过，atan2f 也是同一个坑，见下面注释）。 */
    d.len = __builtin_sqrtf(d.dx * d.dx + d.dy * d.dy);
    d.have_delta = 1;
    return d;
}

/* 按基础宽度把 min_ratio 插值成这一点实际用的"最小比例"——w 越粗越接近
 * min_ratio（满强度），w 越细越接近 1.0（趋近关闭）。笔尖角度模型/提按速度
 * 模型共用同一套"多细的笔画不该有顿挫效果"逻辑，不重复实现。 */
static float cj_hw_width_scaled_min_ratio(float w, float min_ratio,
                                           float width_low, float width_high) {
    if (min_ratio >= 1.0f) return 1.0f;
    float span = width_high - width_low;
    float t = (span > 0.0f) ? (w - width_low) / span : 1.0f;
    if (t < 0.0f) t = 0.0f;
    if (t > 1.0f) t = 1.0f;
    return 1.0f - t * (1.0f - min_ratio);
}

static float cj_hw_nib_ratio(float w, const cj_hw_delta_t *d) {
    if (g_nib_min_ratio >= 1.0f) return 1.0f; /* 关闭，省后面的计算 */
    if (!d->have_delta) return 1.0f; /* 笔画起点，没有上一个点算不出方向 */

    float effective_min_ratio =
        cj_hw_width_scaled_min_ratio(w, g_nib_min_ratio, g_nib_width_low, g_nib_width_high);
    if (effective_min_ratio >= 1.0f) return 1.0f; /* 这支笔太细，效果已经趋近 0，省下面的三角函数 */
    if (d->len == 0.0f) return 1.0f; /* 同一个点，方向未定义 */

    /* 不用 atan2f 求出真实角度再减——同样是 GLIBC_2.43 符号版本问题。用
     * sin(a-b)=sin(a)cos(b)-cos(a)sin(b) 展开绕开，sin(方向角)=dy/len、
     * cos(方向角)=dx/len，剩下只用 sinf/cosf/fabsf（GLIBC_2.17，跟其它
     * 符号一个级别，真机验证过能加载）。 */
    float sin_dir = d->dy / d->len;
    float cos_dir = d->dx / d->len;
    float nib_angle_rad = g_nib_angle_deg * (float)M_PI / 180.0f;
    float sin_diff = sin_dir * cosf(nib_angle_rad) - cos_dir * sinf(nib_angle_rad);
    float s = fabsf(sin_diff);
    return effective_min_ratio + (1.0f - effective_min_ratio) * s;
}

/* 提按（运笔速度代理）：len 越小（慢/顿笔）比例越接近 1.0（粗），len 越大
 * （快/带过）比例越接近 effective_min_ratio（细）。见文件头注「第三轮」，
 * 为什么放弃真实硬件压感、改用这个代理信号。 */
static float cj_hw_speed_ratio(float w, const cj_hw_delta_t *d) {
    if (g_speed_min_ratio >= 1.0f) return 1.0f;
    if (!d->have_delta) return 1.0f;

    float effective_min_ratio =
        cj_hw_width_scaled_min_ratio(w, g_speed_min_ratio, g_nib_width_low, g_nib_width_high);
    if (effective_min_ratio >= 1.0f) return 1.0f;

    float span = g_speed_len_high - g_speed_len_low;
    float t = (span > 0.0f) ? (d->len - g_speed_len_low) / span : 0.0f;
    if (t < 0.0f) t = 0.0f;
    if (t > 1.0f) t = 1.0f; /* t=0(慢) → 比例 1.0；t=1(快) → 比例 effective_min_ratio */
    return 1.0f - t * (1.0f - effective_min_ratio);
}

/* 效果计算+写值本体，两个 hook 目标（FUN_00f47530/FUN_00f4c8d0）共用——两边
 * 反编译核实过的 ctx 布局（+4=宽度、+0x48/+0x4c=上一点坐标、+0x5a=has_prev）
 * 完全一致，不是巧合，是同一族"变宽几何生成器"共用的调用约定。日志前缀区分
 * 是哪个目标调用的，方便真机排查哪个笔型分支实际在起作用。 */
static void cj_hw_apply_effects(const char *tag, float x, float y, void *ctx) {
    cj_hw_refresh_config();

    float *width_ptr = (float *)((uint8_t *)ctx + 4);
    float w = *width_ptr;
    cj_hw_delta_t delta = cj_hw_compute_delta(x, y, ctx);
    float nib_ratio = cj_hw_nib_ratio(w, &delta);
    float speed_ratio = cj_hw_speed_ratio(w, &delta);

    fprintf(stderr, "[hw-stroke:%s] x=%.1f y=%.1f w=%.4f factor=%.3f nib_ratio=%.3f speed_ratio=%.3f len=%.2f\n",
            tag, (double)x, (double)y, (double)w, (double)g_width_factor, (double)nib_ratio,
            (double)speed_ratio, (double)delta.len);

    if (g_width_factor != 1.0f || nib_ratio != 1.0f || speed_ratio != 1.0f) {
        *width_ptr = w * g_width_factor * nib_ratio * speed_ratio;
    }
}

static void cj_hw_quad_handler(float x, float y, void *ctx) {
    cj_hw_apply_effects("f47530", x, y, ctx);
    if (g_orig_hw_quad_call_through) {
        g_orig_hw_quad_call_through(x, y, ctx);
    }
}

static orig_hw_quad_fn_t g_orig_hw_quad2_call_through = NULL;

static void cj_hw_quad2_handler(float x, float y, void *ctx) {
    cj_hw_apply_effects("f4c8d0", x, y, ctx);
    if (g_orig_hw_quad2_call_through) {
        g_orig_hw_quad2_call_through(x, y, ctx);
    }
}

static void cj_install_hw_quad_hook(uintptr_t target) {
    void *stub = NULL;
    if (!cj_patch_target((void *)target, (void *)cj_hw_quad_handler, PATCH_LEN, "hw-stroke", &stub)) {
        fprintf(stderr, "[hw-stroke] 变宽几何 hook 安装失败（safe mode）\n");
        return;
    }
    g_orig_hw_quad_call_through = (orig_hw_quad_fn_t)stub;
    fprintf(stderr, "[hw-stroke] 变宽几何 hook 安装完成 @ %p（factor=%.3f）\n",
            (void *)target, (double)g_width_factor);
}

/* 第二个几何生成器 hook（bVar16==5/6 分支，见 PROLOGUE_HW_QUAD2 头注）——
 * 独立、尽力而为：找不到目标只跳过它自己，不拖累主 hook（跟诊断 hook 同一个
 * 原则）。 */
static void cj_install_hw_quad2_hook(uintptr_t target) {
    void *stub = NULL;
    if (!cj_patch_target((void *)target, (void *)cj_hw_quad2_handler, PATCH_LEN, "hw-stroke", &stub)) {
        fprintf(stderr, "[hw-stroke] 第二几何 hook 安装失败（safe mode，不影响主 hook）\n");
        return;
    }
    g_orig_hw_quad2_call_through = (orig_hw_quad_fn_t)stub;
    fprintf(stderr, "[hw-stroke] 第二几何 hook 安装完成 @ %p\n", (void *)target);
}

/* ---- 探查 FUN_00f3f9d0：纯诊断，零行为改动，不改任何值 ----
 *
 * 用户提出"CJK 顿挫不是单纯粗细问题"、查了社区（西式书法笔尖角度模型/中文
 * 毛笔提按时序模型）之后，下一步要确认笔尖角度模型能不能接上——公式是
 * `宽度 = 基础宽度 × |sin(方向角 − 笔尖固定角度)|`，方向角字节就在点结构
 * `offset 0xC`，FUN_00f3f9d0 已经在某个笔型分支里把它解出来存进
 * `*(float*)(ctx+8)`/`*(float*)(ctx+0xc)`（sin/cos，见反编译），但这是**条件
 * 触发的**（某标志位打开才算），不确定真机日常写字用的笔型会不会走到这条
 * 分支。这个诊断 hook 只读 `bVar16`（`*(byte*)(*param_1+0x70)`，决定走哪个
 * 宽度分支的笔型标签）和 `param_3`，原样调用穿透桩，不碰任何值——先把
 * "真机写字到底走哪条分支"坐实，再决定要不要在 cj_hw_quad_handler 里去读
 * ctx+8/ctx+0xc。
 *
 * 压感诊断（2026-09-10 追加，中文毛笔"提按"效果预研）：headless 反编译
 * FUN_00f3f9d0 全函数体核实过——唯一会走到 FUN_00f47530 的默认分支里，
 * 硬件压感字节（点结构 offset 0xD，README「14 字节点结构」表）会被读出来
 * 转成 0~1 浮点，但存进的是 `plVar6+0x74`，不是我们 hook 收到的 `ctx`
 * （`ctx = plVar6[1]`，另一个对象）——照搬 `ctx` 偏移读压感会重蹈 bVar16
 * 那次跨对象猜偏移的覆辙。这次不读 `plVar6`/`ctx` 任何字段：`当前点在原始
 * 点数组里的地址 = param_2[3] + (param_2[4]*0xe - 0xe)` 这条算法在
 * FUN_00f3f9d0 所有 bVar16 分支（3/`<4`/5/6/默认）里完全一样，是从
 * `FUN_00f3f9d0` 自己的入参（`param_2`/`param_3`）直接算出来的，跟
 * `bVar16`/`plVar6`/`ctx` 身份完全无关，不是"猜"，是原样照抄反编译里逐分支
 * 重复出现的同一段代码。`param_3==1` 是另一种"初始化记录"调用，不走这套
 * 点数组寻址（反编译里这条分支直接用 `param_2[3]+10`，不乘点序号），跳过
 * 不读；`param_2[4]==0` 同理跳过（真实函数自己也在这个值上提前 return）。
 * 纯只读、不写任何值——验证过这个字节确实跟着运笔轻重变化（不是垃圾数据），
 * 但真机数据显示 FUN_00f47530 的调用大部分不经过这一份 FUN_00f3f9d0（见文件
 * 头注「第三轮」），跨函数传值那条路放弃了，宽度公式改用同一个 hook 内部
 * 就能算的运笔速度代理。这个诊断 hook 原样保留——纯只读，不拖累任何已生效
 * 的行为，留给以后想再查"FUN_00f47530 到底还有哪些调用路径"用。 */

typedef void (*orig_hw_dispatch_fn_t)(void *, void *, int);
static orig_hw_dispatch_fn_t g_orig_hw_dispatch_call_through = NULL;

static void cj_hw_dispatch_handler(void *param_1, void *param_2, int param_3) {
    long lVar7 = *(long *)param_1;
    uint8_t bVar16 = *(uint8_t *)(lVar7 + 0x70);

    int have_pressure = 0;
    uint8_t pressure_raw = 0;
    if (param_3 != 1) {
        uint64_t *p2 = (uint64_t *)param_2;
        uint64_t point_idx = p2[4];
        if (point_idx != 0) {
            uint8_t *point_base = (uint8_t *)(p2[3] + point_idx * 0xe - 0xe);
            pressure_raw = point_base[0xd];
            have_pressure = 1;
        }
    }
    if (have_pressure) {
        fprintf(stderr, "[hw-stroke-dispatch] param_3=%d bVar16=%u pressure=%u\n",
                param_3, (unsigned)bVar16, (unsigned)pressure_raw);
    } else {
        fprintf(stderr, "[hw-stroke-dispatch] param_3=%d bVar16=%u pressure=N/A\n",
                param_3, (unsigned)bVar16);
    }

    if (g_orig_hw_dispatch_call_through) {
        g_orig_hw_dispatch_call_through(param_1, param_2, param_3);
    }
}

static void cj_install_hw_dispatch_hook(uintptr_t target) {
    void *stub = NULL;
    if (!cj_patch_target((void *)target, (void *)cj_hw_dispatch_handler, PATCH_LEN, "hw-stroke", &stub)) {
        fprintf(stderr, "[hw-stroke] 分派诊断 hook 安装失败（safe mode，不影响变宽几何 hook）\n");
        return;
    }
    g_orig_hw_dispatch_call_through = (orig_hw_dispatch_fn_t)stub;
    fprintf(stderr, "[hw-stroke] 分派诊断 hook 安装完成 @ %p\n", (void *)target);
}

/* ---- xovi 扩展入口 ---- */

/* 固件兼容性判据是"变宽几何"这个主 hook 自己的目标特征码——这个是已经真机
 * 验证过、真正改变行为的 hook，决定整个扩展加不加载。诊断 hook（探查
 * FUN_00f3f9d0）是独立的、尽力而为的——找不到目标只跳过它自己，不拖累主
 * hook（跟 chinese-ime/langhook 8 个 hook 各自独立 install 同一个原则）。 */
char _xovi_shouldLoad(void) {
    uintptr_t base = 0, addr = 0;
    size_t size = 0;
    if (!cj_find_exec_module(TARGET_MODULE_SUFFIX, NULL, &base, &size)) {
        fprintf(stderr, "[hw-stroke] _xovi_shouldLoad: 找不到 xochitl 映射 → 拒绝加载(裸启原生)\n");
        return 0;
    }
    if (!cj_find_unique_pattern((const uint8_t *)base, size, PROLOGUE_HW_QUAD,
                                 sizeof(PROLOGUE_HW_QUAD), &addr)) {
        fprintf(stderr, "[hw-stroke] _xovi_shouldLoad: 变宽几何函数特征码未唯一命中"
                "(未知固件) → 拒绝加载(裸启原生)\n");
        return 0;
    }
    fprintf(stderr, "[hw-stroke] _xovi_shouldLoad: 固件兼容(FUN_00f47530@0x%lx) → 加载\n",
            (unsigned long)addr);
    return 1;
}

void _xovi_construct(void) {
    uintptr_t base = 0, addr = 0;
    size_t size = 0;
    if (!cj_find_exec_module(TARGET_MODULE_SUFFIX, NULL, &base, &size)) return;
    if (!cj_find_unique_pattern((const uint8_t *)base, size, PROLOGUE_HW_QUAD,
                                 sizeof(PROLOGUE_HW_QUAD), &addr)) {
        return; /* _xovi_shouldLoad 已经打过日志，这里不重复 */
    }
    cj_hw_refresh_config();
    cj_install_hw_quad_hook(addr);

    /* 以下都是独立、尽力而为的 hook：找不到目标只跳过它自己，不拖累上面已经
     * 装好的主 hook（跟 chinese-ime/langhook 每个 hook 各自独立 install 同一个
     * 原则）。 */
    uintptr_t quad2_addr = 0;
    if (cj_find_unique_pattern((const uint8_t *)base, size, PROLOGUE_HW_QUAD2,
                                sizeof(PROLOGUE_HW_QUAD2), &quad2_addr)) {
        cj_install_hw_quad2_hook(quad2_addr);
    } else {
        fprintf(stderr, "[hw-stroke] 第二几何函数特征码未唯一命中，跳过（不影响主 hook）\n");
    }

    uintptr_t dispatch_addr = 0;
    if (cj_find_unique_pattern((const uint8_t *)base, size, PROLOGUE_HW_DISPATCH,
                                sizeof(PROLOGUE_HW_DISPATCH), &dispatch_addr)) {
        cj_install_hw_dispatch_hook(dispatch_addr);
    } else {
        fprintf(stderr, "[hw-stroke] 分派诊断函数特征码未唯一命中，跳过（不影响变宽几何 hook）\n");
    }
}
