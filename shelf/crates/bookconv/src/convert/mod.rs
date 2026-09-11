//! 多格式 → xochitl 可读格式转换（块3 阅读 · 补内容源）。
//!
//! **使用方（2026-09-05 起）只有 `reading/device-rs`**（ingest / 旧上传页）：书架 shelf 的杂格式进原生统一走电脑 Calibre，
//! 漫画只出 CBZ 给 KOReader（不投原生），设备端不再调本模块转换；`direct_content_type` 仍被母版库落库门控复用。
//!
//! xochitl 原生只开 EPUB/PDF：**文本类 → EPUB、漫画类 → PDF**，再走已验证的 `/upload`
//! 免重启注入书库（见 device-core inject）。设计要点 = 各转换器互不耦合、统一收敛到
//! `Converted` 结果类型，摄入层（ingest）对 CBZ/FB2/… 一视同仁，不认具体格式。
//!
//! 支持格式（全设备端纯 Rust）：CBZ→PDF、FB2→EPUB、MOBI6→EPUB（.mobi/.prc/.azw）、
//! **AZW3(KF8)→EPUB（.azw3，clean-room 自研，见 kf8）**。
//! **降级（少见/受限，走 host，不进设备二进制）**：
//! - HUFF/CDIC 压缩的 AZW3（少见）：kf8 明确拒绝 → host `calibre ebook-convert x.azw3 x.epub` 丢 inbox。
//! - CBR：`unrar` 依赖 unrar_sys 编 C++ + RARLAB 受限许可（撞零 C 依赖 + 许可证铁律）→ host `unar`/`7z x` 解成 .cbz 丢 inbox（CBZ→PDF 接手）。

pub mod cbz;
pub mod common;
pub mod fb2;
pub mod kf8;
pub mod mobi;
pub mod palm;
pub mod pdfwrite;

/// xochitl `/upload` 接受的落地类型。
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ContentType {
    Epub,
    Pdf,
}
impl ContentType {
    pub fn mime(self) -> &'static str {
        match self {
            ContentType::Epub => "application/epub+zip",
            ContentType::Pdf => "application/pdf",
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            ContentType::Epub => "epub",
            ContentType::Pdf => "pdf",
        }
    }
}

/// 转换产物：新书字节 + 落地类型 + 建议书名（不含扩展名，供 /upload 落文件名）。
pub struct Converted {
    pub data: Vec<u8>,
    pub content_type: ContentType,
    pub title: String,
}

/// 墨水屏色调处理档（仅作用于漫画 CBZ→PDF；其余格式忽略）。reading 线由「系统增强→漫画省刷新」开关控制；shelf 不再用。
/// `Off`=原样（默认）；`Mono`=黑白页转 1-bit Floyd–Steinberg 抖动（触发面板更轻的 mono 波形、缩体积；
/// 真彩页按饱和度阈值保留彩色）。真机坐实内容层能换更轻波形=减闪，见白皮书。
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum EinkTone {
    #[default]
    Off,
    Mono,
}

/// 该文件扩展名是否为「可转换的源格式」（用于摄入层筛选、跳过 epub/pdf/其他）。
/// .mobi/.prc = 旧 MOBI6；.azw3/.azw = KF8（设备端纯 Rust 解析，见 kf8）。
pub fn is_convertible(filename: &str) -> bool {
    let l = filename.to_ascii_lowercase();
    l.ends_with(".cbz")
        || l.ends_with(".fb2")
        || l.ends_with(".mobi")
        || l.ends_with(".prc")
        || l.ends_with(".azw3")
        || l.ends_with(".azw")
}

/// 已是 xochitl 可直读格式（EPUB/PDF）→ 不转换、直接 `/upload`。返回其落地类型。
pub fn direct_content_type(filename: &str) -> Option<ContentType> {
    let l = filename.to_ascii_lowercase();
    if l.ends_with(".epub") {
        Some(ContentType::Epub)
    } else if l.ends_with(".pdf") {
        Some(ContentType::Pdf)
    } else {
        None
    }
}

/// 摄入层是否接收该文件：可转换源 OR 可直传的 epub/pdf。
pub fn is_ingestible(filename: &str) -> bool {
    is_convertible(filename) || direct_content_type(filename).is_some()
}

/// **廉价预检**（不做完整转换/解压）：判断该文件设备端能否处理，能则 `Ok`，否则给可读原因。
/// 用于上传时**先验后转**——不支持的（DRM / HUFF-CDIC / CBR / 未知）立即回执，不白跑重活、不阻塞批量。
/// MOBI/AZW3 只解 PalmDB+record0 头（含 DRM/压缩类型），秒级；epub/pdf/cbz/fb2 结构无法廉价预判则放行。
pub fn precheck(filename: &str, data: &[u8]) -> Result<(), String> {
    let l = filename.to_ascii_lowercase();
    if direct_content_type(&l).is_some() {
        return Ok(()); // epub/pdf 直传
    }
    if l.ends_with(".mobi") || l.ends_with(".prc") || l.ends_with(".azw") || l.ends_with(".azw3") {
        // 只解容器头即可判 DRM / HUFF-CDIC（parse_header 早失败，不触发解压）
        let records = palm::parse_palmdb(data)?;
        if records.is_empty() {
            return Err("空文件或非 PalmDB".into());
        }
        palm::parse_header(records[0]).map(|_| ())
    } else if is_convertible(&l) {
        Ok(()) // cbz/fb2 无法廉价预判，交由转换阶段
    } else {
        Err("不支持的格式（支持 EPUB/PDF 直传，AZW3/MOBI/FB2/CBZ 转换）".into())
    }
}

/// 按扩展名分派转换。返回 `None` = 非可转换源（摄入层跳过）；`Some(Err)` = 是源但转换失败。
/// `tone` 仅影响漫画 CBZ→PDF（省刷新档），文本类格式忽略。
pub fn convert_file(filename: &str, data: &[u8], tone: EinkTone) -> Option<Result<Converted, String>> {
    let l = filename.to_ascii_lowercase();
    let title = std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("book")
        .to_string();
    if l.ends_with(".cbz") {
        Some(cbz::cbz_to_pdf(data, tone).map(|data| Converted {
            data,
            content_type: ContentType::Pdf,
            title,
        }))
    } else if l.ends_with(".fb2") {
        // FB2 自带 <book-title>，用它当书名（比源文件名准）；解析失败则回退到文件名。
        Some(fb2::fb2_to_epub(data).map(|(data, book_title)| Converted {
            data,
            content_type: ContentType::Epub,
            title: if book_title.trim().is_empty() { title } else { book_title },
        }))
    } else if l.ends_with(".mobi") || l.ends_with(".prc") || l.ends_with(".azw") {
        // .azw（初代 Kindle）≈ 旧 MOBI6；.mobi/.prc 同。
        Some(mobi::mobi_to_epub(data).map(|(data, book_title)| Converted {
            data,
            content_type: ContentType::Epub,
            title: if book_title.trim().is_empty() { title } else { book_title },
        }))
    } else if l.ends_with(".azw3") {
        // AZW3 = KF8（现代 Kindle）。
        Some(kf8::azw3_to_epub(data).map(|(data, book_title)| Converted {
            data,
            content_type: ContentType::Epub,
            title: if book_title.trim().is_empty() { title } else { book_title },
        }))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_and_ingestible_classification() {
        assert_eq!(direct_content_type("a.epub"), Some(ContentType::Epub));
        assert_eq!(direct_content_type("A.PDF"), Some(ContentType::Pdf));
        assert_eq!(direct_content_type("a.azw3"), None);
        assert!(is_ingestible("x.epub") && is_ingestible("x.pdf"));
        assert!(is_ingestible("x.azw3") && is_ingestible("x.cbz"));
        assert!(!is_ingestible("x.txt") && !is_ingestible("x.docx"));
    }

    #[test]
    fn precheck_rejects_unknown_and_accepts_direct() {
        assert!(precheck("x.epub", b"anything").is_ok()); // 直传不预判内容
        assert!(precheck("x.pdf", b"%PDF").is_ok());
        assert!(precheck("x.txt", b"...").is_err()); // 未知格式
        assert!(precheck("x.mobi", b"tooshort").is_err()); // 坏 PalmDB 早失败
    }
}
