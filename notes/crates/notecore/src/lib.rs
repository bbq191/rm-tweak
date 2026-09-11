//! notecore —— 笔记线的领域核心，**纯函数、零 I/O、零网络**，四个服务都只调它：
//! - `model`  条目 / 分区 / 书的数据模型（条目库是唯一事实源，笔记本与 md 都是它的投影）；
//! - `geom`   勾画 ↔ 旁边手写的几何配对（同一页坐标系：笔画聚簇 → 簇找最近的勾画矩形）；
//! - `hash`   簇指纹（笔画点集量化哈希）——增量的根：指纹不变不重识别；
//! - `ingest` 一页解析结果 → 条目草稿，并按增量规则并入已有条目（校对文本永不被覆盖、删笔画只标撤销）。
//! - `koreader` KOReader 高亮/生词 → 条目，跟 `ingest` 平行的另一条摄取入口（零笔画坐标，走
//!   `Entry::set_triage` 已有的"纯勾画直接定稿"快路径），供 ink-serve 的 KOReader 回流用。
//! - `marker` 行首标记的 OCR 路兜底（转写文本开头的 `-`/`1.`/`口` → 样式，并剥掉标记）。
//! - `mdimport` 单篇 markdown → `rmv6::write::Paragraph` 列表（跟 `marker` 方向相反：输入已经是
//!   规范 markdown 语法，不是 OCR 纯文本），供 note-serve"单篇导入"这条独立于条目库的功能用。
//! - `project` 条目库 → 设备笔记本投影（一章 → `rmv6::write::Paragraph` 列表 + 变更指纹），note-serve 专用。
//! - `export`  条目库 → Markdown 导出（一章 → `.md` 文本 + 书索引页），跟 `project` 同一批 live 条目，
//!   产物给 Obsidian 用（note-serve 落盘 + host `notes pull`）。
//!
//! 行首手写约定（`-` / `1.` / `口` / 下划线分区头）的几何判定放 `glyph`，阈值待真机样本标定后补。
pub mod export;
pub mod geom;
pub mod hash;
pub mod ingest;
pub mod koreader;
pub mod marker;
pub mod mdimport;
pub mod model;
pub mod project;
