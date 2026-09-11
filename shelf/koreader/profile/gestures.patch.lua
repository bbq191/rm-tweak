-- gestures.lua 补丁：长按左上角 = 退出 KOReader（防睡眠卡死 #14348）。结构与 2026-09-03 设备快照一致
-- （顶层 gesture_reader / gesture_fm 两上下文，键 hold_top_left_corner = { exit = true }）。
return {
    gesture_reader = { hold_top_left_corner = { exit = true } },
    gesture_fm     = { hold_top_left_corner = { exit = true } },
}
