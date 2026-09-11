-- settings.reader.lua 补丁（deep-merge 进设备 settings.reader.lua）。键名全部以设备上 KOReader v2026.07.1 的源码与
-- 真机快照为准（2026-09-24 按源码逐个核过）；语义：标量覆盖、表递归、"__DELETE__" 删键。
-- 两套阅读方案的分工：
--   · 本文件 = 全局设置 = 文字书方案（任何没有单书设置的书都用它）；
--   · 漫画方案 = directory_defaults.patch.lua（books/漫画/ 下的新书首次打开时套用的单书设置）
--               + profiles.patch.lua（打开/关闭漫画时自动切换状态栏）。
-- 默认阅读字体不在本文件：cre_font 由 KOReader 菜单选，勿在此猜。
return {
    -- ── 脚注回得去 ──
    footnote_link_in_popup = true,           -- 点脚注底部弹窗，不跳页
    link_prefer_footnote = true,             -- 中文书脚注常无 epub:type，放宽判定
    swipe_to_go_back = true,                 -- 单指左→右滑回上一位置
    larger_tap_area_to_follow_links = true,  -- 7.3″ 上标太小，放大命中区

    -- ── 刷新/屏闪 ──
    wf_level = 1,                            -- 翻页 CONTENT 波形（3=全 FAST 会把抗锯齿二值化=锯齿）
    full_refresh_count = 16,                 -- 文字页每 16 页全刷一次清残影
    avoid_flashing_ui = true,                -- 菜单/弹窗不全刷
    -- 带图片的页 KOReader 缺省就每页全刷（refresh_on_pages_with_images 缺省开），漫画不必另设"每页全刷"。
    color_rendering = true,                  -- 与设备现状一致（Move 彩屏，彩色封面/彩页要它）

    -- ── 中文排版（文字书全局缺省；已打开过的书各自存了设置，不受影响）──
    floating_punctuation = true,             -- 悬挂标点（crengine 独有）
    copt_line_spacing = 115,                 -- 行距 115%（缺省 100 对中文偏挤；7.3″ 屏再大会少行）
    copt_font_base_weight = 0.5,             -- 字重 +0.5：墨水屏上细宋体偏灰，略加粗更清楚（可在底栏「字重」随时改）
    copt_embedded_fonts = 0,                 -- 不用书自带字体，统一用选定的中文字体（书内嵌字体常是子集，缺字）
    text_lang_fallback = "zh-CN",            -- 书没标语言时按中文断行（原为 en-US）

    -- ── 状态栏（文字书）：保持现状；漫画打开时由配置档切到「漫画」预设（隐藏），关书切回「文字」──
    footer = {
        reclaim_height = true,
        progress_style_thin = true,
        progress_style_thin_height = 3,
        battery = false,
        time = false,
        page_progress = true,
        pages_left = true,
        percentage = true,
        book_time_to_read = true,            -- 需要「统计」插件（下面已启用），否则显示 N/A
        chapter_time_to_read = true,
    },
    footer_presets = {
        ["文字"] = {
            footer = {
                ["align"] = "center",
                ["all_at_once"] = false,
                ["auto_refresh_time"] = false,
                ["battery"] = false,
                ["battery_hide_threshold"] = 100,
                ["book_author"] = false,
                ["book_author_max_width_pct"] = 30,
                ["book_chapter"] = false,
                ["book_chapter_max_width_pct"] = 30,
                ["book_time_to_read"] = true,
                ["book_title"] = false,
                ["book_title_max_width_pct"] = 30,
                ["bookmark_count"] = false,
                ["bottom_horizontal_separator"] = false,
                ["chapter_progress"] = false,
                ["chapter_progress_bar"] = false,
                ["chapter_time_to_read"] = true,
                ["container_bottom_padding"] = 1,
                ["container_height"] = 14,
                ["disable_progress_bar"] = false,
                ["disabled"] = false,
                ["frontlight"] = false,
                ["hide_empty_generators"] = false,
                ["initial_marker"] = false,
                ["invert_progress_direction"] = false,
                ["item_prefix"] = "icons",
                ["items_separator"] = "bar",
                ["lock_tap"] = false,
                ["mem_usage"] = false,
                ["page_progress"] = true,
                ["page_turning_inverted"] = false,
                ["pages_left"] = true,
                ["pages_left_book"] = false,
                ["pages_left_includes_current_page"] = false,
                ["percentage"] = true,
                ["progress_bar_min_width_pct"] = 20,
                ["progress_bar_position"] = "alongside",
                ["progress_margin"] = false,
                ["progress_margin_width"] = 10,
                ["progress_pct_format"] = "0",
                ["progress_style_thick_height"] = 7,
                ["progress_style_thin"] = true,
                ["progress_style_thin_height"] = 3,
                ["reclaim_height"] = true,
                ["skim_widget_on_hold"] = false,
                ["text_font_bold"] = false,
                ["text_font_face"] = "./fonts/noto/NotoSans-Regular.ttf",
                ["text_font_size"] = 14,
                ["time"] = false,
                ["toc_markers"] = true,
                ["toc_markers_width"] = 2,
                ["wifi_status"] = false,
            },
            reader_footer_mode = 4,
            reader_footer_custom_text = "KOReader",
            reader_footer_custom_text_repetitions = "1",
        },
        ["漫画"] = {
            footer = {
                ["align"] = "center",
                ["all_at_once"] = false,
                ["auto_refresh_time"] = false,
                ["battery"] = false,
                ["battery_hide_threshold"] = 100,
                ["book_author"] = false,
                ["book_author_max_width_pct"] = 30,
                ["book_chapter"] = false,
                ["book_chapter_max_width_pct"] = 30,
                ["book_time_to_read"] = true,
                ["book_title"] = false,
                ["book_title_max_width_pct"] = 30,
                ["bookmark_count"] = false,
                ["bottom_horizontal_separator"] = false,
                ["chapter_progress"] = false,
                ["chapter_progress_bar"] = false,
                ["chapter_time_to_read"] = true,
                ["container_bottom_padding"] = 1,
                ["container_height"] = 14,
                ["disable_progress_bar"] = true,
                ["disabled"] = false,
                ["frontlight"] = false,
                ["hide_empty_generators"] = false,
                ["initial_marker"] = false,
                ["invert_progress_direction"] = false,
                ["item_prefix"] = "icons",
                ["items_separator"] = "bar",
                ["lock_tap"] = false,
                ["mem_usage"] = false,
                ["page_progress"] = true,
                ["page_turning_inverted"] = false,
                ["pages_left"] = true,
                ["pages_left_book"] = false,
                ["pages_left_includes_current_page"] = false,
                ["percentage"] = true,
                ["progress_bar_min_width_pct"] = 20,
                ["progress_bar_position"] = "alongside",
                ["progress_margin"] = false,
                ["progress_margin_width"] = 10,
                ["progress_pct_format"] = "0",
                ["progress_style_thick_height"] = 7,
                ["progress_style_thin"] = true,
                ["progress_style_thin_height"] = 3,
                ["reclaim_height"] = true,
                ["skim_widget_on_hold"] = false,
                ["text_font_bold"] = false,
                ["text_font_face"] = "./fonts/noto/NotoSans-Regular.ttf",
                ["text_font_size"] = 14,
                ["time"] = false,
                ["toc_markers"] = true,
                ["toc_markers_width"] = 2,
                ["wifi_status"] = false,
            },
            reader_footer_mode = 0,
            reader_footer_custom_text = "KOReader",
            reader_footer_custom_text_repetitions = "1",
        },
    },

    -- ── 漫画方案的自动切换（配置档内容见 profiles.patch.lua）──
    -- 打开 books/漫画/ 下的书 → 执行「漫画」配置档；关闭时 → 执行「文字」配置档恢复。
    -- 打开 books/小说/ 下的书也执行一次「文字」，防上次漫画中途崩溃没走到关书那一步。
    profiles_autoexec = {
        ReaderReadyAll = {
            ["漫画"] = { filepath = "/books/漫画/" },
            ["文字"] = { filepath = "/books/小说/" },
        },
        CloseDocumentAll = {
            ["文字"] = { filepath = "/books/漫画/" },
        },
    },

    -- ── 插件：值为 true = 禁用；"__DELETE__" = 从禁用表里拿掉（启用）──
    plugins_disabled = {
        -- 需要启用
        statistics = "__DELETE__",           -- 状态栏"本章/全书剩余时间"靠它算（原先禁用 → 一直显示 N/A）
        vocabbuilder = "__DELETE__",         -- 查词自动进生词本；书架 koreader-serve 从它的数据库导入生词到笔记线
        -- 新增禁用：与本机用法无关
        hello = true,                        -- 示例插件
        coverimage = true,                   -- 把封面写成屏保图；reMarkable 上屏保由 xochitl 管，KOReader 屏保已关
        keepalive = true,                    -- 手动防休眠
        bookshortcuts = true,                -- 书籍快捷方式（未使用）
        cloudstorage = true,                 -- 网盘（书由书架网页推送）
        opds = true,                         -- OPDS 书目下载（同上）
        kosync = true,                       -- 进度同步服务器（未配置）
        timesync = true,                     -- NTP 对时（设备已由 chrony 对时）
        autostandby = true,                  -- Kobo 待机；auto_standby_timeout_seconds 本就是 -1
        batterystat = true,                  -- 电量统计（状态栏也没显示电量）
        hotkeys = true,                      -- 实体键/键盘快捷键：Move 无实体翻页键，appload 0.6.0 下键码也不对
        externalkeyboard = true,             -- USB 外接键盘
        archiveviewer = true,                -- 在文件管理器里翻压缩包；书只有 EPUB/PDF
    },
    vocabulary_builder = {
        enabled = true,                      -- 查词后自动加入生词本
        with_context = true,
    },
}
