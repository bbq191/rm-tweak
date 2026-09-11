-- settings/directory_defaults.lua 补丁（deep-merge）：漫画方案的单书设置。
-- 机制（docsettingtweak 插件，源码 plugins/docsettingtweak.koplugin/main.lua）：某本书**第一次**被打开时，若它在下面某个
-- 目录（或其子目录）里，就用这张表作为这本书的初始设置；表里没写的键照常取全局设置。已经打开过的书不受影响——
-- 要让旧书也用上，在书的「…」菜单里「重置设置」后重开。路径必须是绝对路径、不带结尾斜杠。
-- 键名以设备上 KOReader v2026.07.1 的 creoptions.lua / 单书 metadata.*.lua 为准。
return {
    ["/home/root/xovi/exthome/appload/koreader/books/漫画"] = {
        inverse_reading_order = true,        -- 日漫从右往左：点左侧=下一页、右侧=上一页，滑动方向同步反转
                                             -- （KOReader 不读 OPF 的 page-progression-direction，得手动设；
                                             --  国漫/美漫这类从左往右的，放到 漫画/ 之外或打开后在菜单里关掉）
        copt_h_page_margins = { 0, 0 },      -- 左右页边距 0：画面尽量铺满 7.3″ 屏
        copt_t_page_margin = 0,              -- 上边距 0
        copt_b_page_margin = 0,              -- 下边距 0（状态栏由配置档隐藏，腾出的高度也给画面）
        copt_sync_t_b_page_margins = 0,
        copt_smooth_scaling = 1,             -- 图片缩放用「最佳」算法：大图缩到屏幕尺寸时网点/线稿不糊不锯齿（比「快速」多花点 CPU）
        copt_status_line = 1,                -- 关 crengine 顶部标题栏（1=关）
        copt_view_mode = 0,                  -- 翻页模式（不是滚动）
    },
}
