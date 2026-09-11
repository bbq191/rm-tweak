-- settings/profiles.lua 补丁（deep-merge）：两个配置档，由 settings.reader.patch.lua 的 profiles_autoexec 按书的路径自动执行。
-- 配置档 = 一组 Dispatcher 动作（源码 frontend/dispatcher.lua）；这里只用 load_footer_preset 切状态栏预设
-- （预设本体在 settings.reader.patch.lua 的 footer_presets）。每次切换会弹一条"已载入预设"的小提示。
return {
    ["漫画"] = {
        load_footer_preset = "漫画",          -- 隐藏状态栏和进度条，画面用满整屏高度
        settings = { name = "漫画" },
    },
    ["文字"] = {
        load_footer_preset = "文字",          -- 恢复文字书状态栏（页码进度、剩余页数、剩余阅读时间）
        settings = { name = "文字" },
    },
}
