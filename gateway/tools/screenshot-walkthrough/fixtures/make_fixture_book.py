#!/usr/bin/env python3
"""生成一份 ink-serve 条目库 fixture（notecore::model::Book 的 JSON 形状），供截图走查
覆盖「浏览/整理/回收站」各状态用——不是真实摄取产物，纯手写 JSON 直接落盘绕开 .rm 解析管线。"""
import json
import sys
import time

now = int(time.time() * 1000)


def entry(id_, page, page_index, chapter, chapter_title, status, **kw):
    e = {
        "id": id_,
        "page": page,
        "page_index": page_index,
        "chapter": chapter,
        "chapter_title": chapter_title,
        "subhead": None,
        "quote": None,
        "ink": None,
        "drafts": [],
        "text": None,
        "style": "body",
        "ask_ai": False,
        "question": None,
        "answer": None,
        "status": status,
        "destination": "both",
        "created": now,
        "updated": now,
    }
    e.update(kw)
    return e


def quote(text, color="yellow"):
    return {"id": f"q-{abs(hash(text))}", "text": text, "color": color, "rects": [[10.0, 20.0, 300.0, 40.0]]}


book = {
    "uuid": "screenshot-fixture-book-0001",
    "title": "人骨拼图占位书名一",
    "author": "作者甲",
    "chapters": ["第1章", "第2章", "第3章", "第4章"],
    "entries": [
        # 浏览页：新探测到、还没决定的（Mined）
        entry("e1", "page-uuid-1", 0, 0, "第1章", "mined", quote=quote("这是一条刚探测到的勾画，还没决定要不要转笔记。")),
        entry("e2", "page-uuid-1", 1, 0, "第1章", "mined", quote=quote("第二条纯勾画条目，测试浏览页多条渲染。")),
        # 整理页·未导出：Pending/Draft/Reviewed
        entry("e3", "page-uuid-2", 0, 1, "第2章", "pending", ink={"strokes": ["s1", "s2"], "bbox": [0, 0, 100, 50], "hash": "hash-e3", "crop": ""}),
        entry(
            "e4", "page-uuid-2", 1, 1, "第2章", "draft",
            ink={"strokes": ["s3"], "bbox": [0, 0, 100, 50], "hash": "hash-e4", "crop": ""},
            drafts=[{"text": "这是一份等待校对的转写草稿。", "backend": "qwen3-vl-plus", "at": now, "hash": "hash-e4"}],
        ),
        entry(
            "e5", "page-uuid-3", 0, 2, "第3章", "reviewed",
            ink={"strokes": ["s4"], "bbox": [0, 0, 100, 50], "hash": "hash-e5", "crop": ""},
            drafts=[{"text": "草稿版本", "backend": "qwen3-vl-plus", "at": now, "hash": "hash-e5"}],
            text="这是已经校对定稿的正文，用来测试整理页已导出/未导出双 tab 的定稿态渲染。",
            style="bullet",
        ),
        # 带「问 AI」的条目
        entry(
            "e6", "page-uuid-3", 1, 2, "第3章", "reviewed",
            quote=quote("原文引用，旁边配了一个问 AI 的问题。"),
            text="这条附带了问AI的问答测试。",
            ask_ai=True,
            question="这段话的核心观点是什么？",
            answer={"text": "这是一段占位的 AI 回答文本，用来测试问答区渲染。", "backend": "qwen-plus", "at": now, "brief": "brief"},
        ),
        # 回收站：Skipped / Revoked / Archived
        entry("e7", "page-uuid-4", 0, 3, "第4章", "skipped", quote=quote("用户点了不需要，进回收站。")),
        entry(
            "e8", "page-uuid-4", 1, 3, "第4章", "revoked",
            ink={"strokes": ["s5"], "bbox": [0, 0, 100, 50], "hash": "hash-e8", "crop": ""},
            text="笔迹已从设备页面删除，条目留痕进回收站。",
        ),
        entry(
            "e9", "page-uuid-4", 2, 3, "第4章", "archived",
            text="用户在整理页点了不要了，测试回收站第三种终态。",
            drafts=[{"text": "草稿", "backend": "qwen3-vl-plus", "at": now, "hash": "hash-e9"}],
        ),
    ],
    "page_mtimes": {},
}

if __name__ == "__main__":
    with open(sys.argv[1], "w", encoding="utf-8") as f:
        json.dump(book, f, ensure_ascii=False, indent=2)
    print(f"-- 写好 {sys.argv[1]}（{len(book['entries'])} 条条目）")
