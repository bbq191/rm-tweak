CC ?= gcc

# host 侧单测（不需要 aarch64 交叉工具链）：这四个文件本身是纯 C11，不依赖任何 xovi/
# xochitl 符号（scan.c 的 cj_find_exec_module 本来就接受构造好的字符串代替真的
# /proc/self/maps，见其头注），可以直接在宿主机编、跑。真正的 aarch64 产物仍由各消费方
# （../hl-snap/Makefile、../handwriting-stroke/Makefile）自己的 aarch64 目标编译，
# 这里不重复。
SRC = scan.c pattern.c trampoline_aarch64.c trampoline_patch.c
CFLAGS_TEST = -std=gnu17 -Wall -Wextra -g -I.

.PHONY: test clean

test: tests/test_shared
	./tests/test_shared

tests/test_shared: $(SRC) tests/test_shared.c
	$(CC) $(CFLAGS_TEST) $(SRC) tests/test_shared.c -o tests/test_shared

clean:
	rm -f tests/test_shared
