CC ?= gcc
AARCH64_CC ?= aarch64-linux-gnu-gcc

# 特征码扫描 + ARM64 远跳转 trampoline 三个纯工具文件（跟"装哪个 hook"无关的通用
# 基础设施）。2026-09-11 起改为**剥离移植**（独立副本，不再路径引用 chinese-ime/
# langhook/src——那边挪出仓库后不再适合当活跃路径依赖），见 ../shared/PROVENANCE.md。
LANGHOOK_SRC_DIR = ../shared
SHARED_SRC = $(LANGHOOK_SRC_DIR)/scan.c $(LANGHOOK_SRC_DIR)/pattern.c $(LANGHOOK_SRC_DIR)/trampoline_aarch64.c $(LANGHOOK_SRC_DIR)/trampoline_patch.c
CFLAGS_COMMON = -std=gnu17 -Wall -Wextra -g -ffile-prefix-map=$(CURDIR)=. -I$(LANGHOOK_SRC_DIR)

.PHONY: aarch64 glue clean

# xovi 扩展胶水：xovigen 从 hl-snap.xovi 生成 xovi section 元数据（同 chinese-ime/
# langhook 的 xovigen 用法，白皮书 2.4 记过流程）。需要 asivery/xovi 的
# xovigen.py——构建前 clone 它，XOVI_DIR 指向克隆路径。
XOVI_DIR ?= ../../../xovi
XOVIGEN = $(XOVI_DIR)/util/xovigen.py

aarch64: xovi_glue.c
	$(AARCH64_CC) $(CFLAGS_COMMON) -shared -fPIC \
		$(SHARED_SRC) src/hl_snap.c xovi_glue.c \
		-o hl-snap.so
	aarch64-linux-gnu-nm -D hl-snap.so | grep -iE "_xovi_construct|_xovi_shouldLoad" || true

# 胶水 xovi_glue.{c,h} 已提交进仓库：这条规则不依赖 .xovi 的 mtime，只在胶水缺失时才生成——
# 否则 checkout 后 .xovi 的 mtime 恰好更新就会去跑 xovigen，本机没有 xovi clone 时构建失败，
# 部署脚本随即退回仓库里已提交的（可能是旧源码编出的）.so（2026-09-24 审查）。改了 .xovi 用
# `make glue XOVI_DIR=<clone>` 显式重新生成。
xovi_glue.c:
	python3 $(XOVIGEN) -o xovi_glue.c -H xovi_glue.h -a aarch64 hl-snap.xovi

glue:
	python3 $(XOVIGEN) -o xovi_glue.c -H xovi_glue.h -a aarch64 hl-snap.xovi

clean:
	rm -f hl-snap.so xovi_glue.c xovi_glue.h
