#!/bin/sh
# EPUB 清洗流（C2）+ series 命名（C3）：Calibre 拍平网页级 CSS、规整中文排版，
# 输出名按 series 模板自拼（落到 xochitl visibleName 实现丛书排序）。
# 产物保留 EPUB 流式特性（字号可调 + PKM 全功能），推送前过 check_output.py。
#
# 用法: wash_epub.sh 输入书 [输出目录]
#   输入接受 calibre 认的一切格式（epub/azw3/mobi/fb2...），输出一律 .epub。
#   EPUB 带 encryption.xml 时先过 strip_pseudo_drm.py：只加密样式/字体的「伪 DRM」（多看
#   dkagent.css 那类，《飘》）自动剥掉继续洗；加密了正文的真 DRM 明确报错停下（退出码 3）。
#   AZW3/MOBI 来源务必过 check_output.py（双 id 扫描）再推送——设备端
#   collapse_dup_id_attrs 兜底缺位时，双 id = xochitl 整章白屏。
# 环境变量:
#   WASH_AUTOTOC=1  从 h1/h2 重建目录（默认保留书内原 TOC；目录坏掉的书才开）
#   WASH_LINEARIZE_TABLES=1  表格拍平成线性文本（954px 宽屏放不下多列表的书才开；真表格别开）
#   WASH_KEEP_PARA_SPACING=1  保留原书段间距（诗集/剧本这类靠空行分节的书）
#   WASH_OPTIMIZE_BIN=路径  指定 epub-optimize（缺省依次找 PATH、shelf/target/release/）
#   WASH_NO_OPTIMIZE=1  跳过设备优化步
#
# 末步叠加**设备端同一优化器**（host CLI `epub-optimize`，`cd shelf && cargo build --release -p bookconv --bin epub-optimize`）：脚注互指拆环 + duokan 标记换可点上标 + 远程图内联 + 全书 id 去重 +
# 图片降采样 + e-ink 提对比。产物落母版库时带优化标记（full），网页不再显示「优化」按钮——所以这一步必须在 host
# 做完再推，否则用户以为已优化而注释在设备上点不动（Calibre 会把同文件锚写成 part0004.html#x 并保留真 img
# 标记+回链 2-环，v5 优化器专治此形态；设备端母版库「优化」是同一函数，漏了也能在网页补点）。
# 产物自带 META-INF/com.cangjie.optimized 标记，设备 autoopt 不会再优化一遍。找不到二进制则跳过并提示。
#
# 取舍（详见阅读白皮书 §11）:
#   --filter-css 拍平字体/字号/颜色/对齐锁（含独立 .css，补设备端优化器只剥
#   内联 style 的已知遗漏）；不拍 margin/padding（伤 blockquote/列表缩进）。
#   不开 Smarten Punctuation（西文直引号替换，规范中文无益、混排有配对风险）。
#   行距不注入（xochitl 自己有行距设置，嵌死会打架）；只注首行缩进。
#
# 按 Move 屏（954×1696@264ppi）校准的三处（2026-09-02 真机渲染量出来的，见白皮书 §11 第四批）：
#   --margin-* 0        Calibre 默认 5pt 会写进 body(.calibre{margin:0 5pt}) 和 @page，xochitl 照吃、
#                       与设备自己的边距叠加（渲染量得正文列两边各窄 5pt）。边距交给 xochitl 设置。
#   --remove-paragraph-spacing (+indent 2em)  段落 margin/padding 上下归零 + 首行缩进 2em（替代原
#                       extra-css 缩进）。原书 p{margin:0.5em/1em} 在对话密集页吃掉约 1/4 竖向空间
#                       （20 行的页上 5.7 行是空隙）；中文排版惯例本就是缩进不空行。居中/右对齐段不缩进。
#   --output-profile generic_eink_hd 保持：其 screen_size=10000×10000 即 Calibre 不缩图，图片交给末步
#                       epub-optimize 的朝向安全规则（长边≤1696 且短边≤954）；换 ipad3 之类会按横屏盒
#                       把竖图高压到 1536 欠采样。字号/列宽由 xochitl 阅读设置决定，内容层无杠杆。
set -eu

in=${1:?用法: wash_epub.sh 输入书 [输出目录]}
outdir=${2:-.}

here=$(cd "$(dirname "$0")" && pwd)

# ---- 伪 DRM 预处理（只对 EPUB；用系统 python3，别进 uv venv）
cleanup=""
case $in in
*.epub|*.EPUB)
    if unzip -l "$in" 2>/dev/null | grep -q "META-INF/encryption.xml"; then
        stripped="$outdir/.wash-stripped-$$.epub"
        if python3 "$here/strip_pseudo_drm.py" "$in" "$stripped"; then
            in=$stripped
            cleanup=$stripped
        else
            rc=$?
            rm -f "$stripped"
            echo "错误: 输入是加密 EPUB（真 DRM），两条 lane 都进不去" >&2
            exit "$rc"
        fi
    fi
    ;;
esac

# ---- C3 命名：{series}{series_index:02d} - {title}.epub，无 series 退化 {title}.epub
meta=$(ebook-meta "$in")
title=$(printf '%s\n' "$meta" | sed -n 's/^Title  *: *//p' | head -n 1)
series=$(printf '%s\n' "$meta" | sed -n 's/^Series  *: *//p' | head -n 1)
[ -n "$title" ] || { title=$(basename "$in"); title=${title%.*}; }
if [ -n "$series" ]; then
    # ebook-meta 的 Series 形如 "三体 #2.0"
    s_name=${series% #*}
    s_idx=${series##*#}
    s_idx=${s_idx%%.*}
    name=$(printf '%s%02d - %s' "$s_name" "$s_idx" "$title")
else
    name=$title
fi
# 文件名净化：/ 和控制字符不可入名
name=$(printf '%s' "$name" | tr '/\n\t' '---')
out="$outdir/$name.epub"
# 防输出撞输入：PDF 重排产物落在 <work>/X.epub，wash 又按同标题算出 <work>/X.epub → ebook-convert
# 报 "Input file is the same as the output file"。规范化两边路径比对，撞了就换个名（调用方按 mtime 取最新）。
canon() { d=$(dirname "$1"); printf '%s/%s' "$(cd "$d" 2>/dev/null && pwd || printf '%s' "$d")" "$(basename "$1")"; }
if [ "$(canon "$out")" = "$(canon "$in")" ]; then
    out="$outdir/$name.washed.epub"
fi

autotoc_args=""
if [ "${WASH_AUTOTOC:-0}" = "1" ]; then
    autotoc_args='--use-auto-toc --level1-toc //h:h1 --level2-toc //h:h2'
fi

extra_args=""
if [ "${WASH_LINEARIZE_TABLES:-0}" = "1" ]; then
    extra_args="$extra_args --linearize-tables"
fi
if [ "${WASH_KEEP_PARA_SPACING:-0}" = "1" ]; then
    extra_css='p{text-indent:2em}'
else
    extra_args="$extra_args --remove-paragraph-spacing --remove-paragraph-spacing-indent-size 2"
    extra_css=''
fi

# shellcheck disable=SC2086  # autotoc_args/extra_args 有意按词拆分
ebook-convert "$in" "$out" \
    --output-profile generic_eink_hd \
    --filter-css font-family,font-size,color,background-color,text-align \
    --margin-top 0 --margin-bottom 0 --margin-left 0 --margin-right 0 \
    --extra-css "$extra_css" \
    $autotoc_args $extra_args

[ -n "$cleanup" ] && rm -f "$cleanup"

# ---- 末步：设备优化器（与设备端 optimize_epub 同一函数）
if [ "${WASH_NO_OPTIMIZE:-0}" != "1" ]; then
    optbin=${WASH_OPTIMIZE_BIN:-}
    [ -n "$optbin" ] || optbin=$(command -v epub-optimize 2>/dev/null || true)
    [ -n "$optbin" ] || { [ -x "$here/../../target/release/epub-optimize" ] && optbin="$here/../../target/release/epub-optimize"; }
    if [ -n "$optbin" ]; then
        tmp="$out.opt.tmp"
        if "$optbin" "$out" "$tmp"; then
            mv -f "$tmp" "$out"
        else
            rm -f "$tmp"
            echo "警告: epub-optimize 失败，产物保持 Calibre 原样（注释在设备上可能点不动）" >&2
        fi
    else
        echo "警告: 未找到 epub-optimize（cd shelf && cargo build --release -p bookconv --bin epub-optimize），跳过设备优化步" >&2
    fi
fi

echo "产物: $out"
