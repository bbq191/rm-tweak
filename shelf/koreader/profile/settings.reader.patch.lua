-- settings.reader.lua 补丁（deep-merge 进设备 settings.reader.lua）。键与值以 2026-09-03 真机快照
-- （`shelf koreader pull`，KOReader v2026.07.1）为准；语义：标量覆盖、表递归、"__DELETE__" 删键。
-- 默认阅读字体不在本文件：设备快照里没有 cre_font 顶层键（字体在 cre_fonts_recently_selected 列表 + 各书 sdr），
-- 由 KOReader 菜单选，勿在此猜键名。
return {
    -- 脚注回得去
    footnote_link_in_popup = true,           -- 点脚注底部弹窗，不跳页
    link_prefer_footnote = true,             -- 中文书脚注常无 epub:type，放宽判定
    swipe_to_go_back = true,                 -- 单指左→右滑回上一位置
    larger_tap_area_to_follow_links = true,  -- 7.3″ 上标太小，放大命中区
    -- 刷新/屏闪
    wf_level = 1,                            -- 翻页 CONTENT 波形（3=全 FAST 会把抗锯齿二值化=锯齿）
    full_refresh_count = 16,
    avoid_flashing_ui = true,
    color_rendering = false,                 -- 全局关，彩书书内单独开
    -- 中文排版
    floating_punctuation = true,             -- 悬挂标点（crengine 独有）
    -- 状态栏按小屏收紧（轮显项与快照一致）
    footer = {
        reclaim_height = true,
        progress_style_thin = true,
        progress_style_thin_height = 3,
        battery = false,
        time = false,
        page_progress = true,
        pages_left = true,
        percentage = true,
        book_time_to_read = true,
        chapter_time_to_read = true,
    },
}
