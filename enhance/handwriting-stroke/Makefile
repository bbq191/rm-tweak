CC ?= gcc
AARCH64_CC ?= aarch64-linux-gnu-gcc

# 特征码扫描 + ARM64 远跳转 trampoline 三个纯工具文件，跟 enhance/hl-snap 同一份
# 独立副本（2026-09-11 起不再路径引用 chinese-ime/langhook/src），见
# ../shared/PROVENANCE.md。
LANGHOOK_SRC_DIR = ../shared
SHARED_SRC = $(LANGHOOK_SRC_DIR)/scan.c $(LANGHOOK_SRC_DIR)/pattern.c $(LANGHOOK_SRC_DIR)/trampoline_aarch64.c $(LANGHOOK_SRC_DIR)/trampoline_patch.c
# -fno-math-errno：让 GCC 把 sqrtf 内联成 ARM64 原生 FSQRT 指令而不是调 libm——
# libm 的 sqrtf() 符号需要 GLIBC_2.43，设备上的 libm 版本没这么新，dlopen 会
# 解析失败（真机实测撞到过，见 src/hw_stroke.c 注释）。
CFLAGS_COMMON = -std=gnu17 -Wall -Wextra -g -fno-math-errno -I$(LANGHOOK_SRC_DIR)

.PHONY: aarch64 clean

# xovi 扩展胶水：xovigen 从 hw-stroke.xovi 生成 xovi section 元数据（同
# chinese-ime/langhook / enhance/hl-snap 的 xovigen 用法）。需要 asivery/xovi
# 的 xovigen.py——构建前 clone 它，XOVI_DIR 指向克隆路径。
XOVI_DIR ?= ../../../xovi
XOVIGEN = $(XOVI_DIR)/util/xovigen.py

aarch64: xovi_glue.c
	$(AARCH64_CC) $(CFLAGS_COMMON) -shared -fPIC \
		$(SHARED_SRC) src/hw_stroke.c xovi_glue.c \
		-lm -o hw-stroke.so
	aarch64-linux-gnu-nm -D hw-stroke.so | grep -iE "_xovi_construct|_xovi_shouldLoad" || true

xovi_glue.c: hw-stroke.xovi
	python3 $(XOVIGEN) -o xovi_glue.c -H xovi_glue.h -a aarch64 hw-stroke.xovi

clean:
	rm -f hw-stroke.so xovi_glue.c xovi_glue.h
