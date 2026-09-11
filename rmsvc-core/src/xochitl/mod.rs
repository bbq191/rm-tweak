//! 原生 xochitl 书库免重启注入。**剥离移植**自旧项目 device-core `inject.rs` 的真机验证结论
//! （书架不引用旧 crate，此处独立实现）：
//! - `POST http://<host>/upload`（multipart 字段 `file`）免重启进库；xochitl web 只绑 USB 网口，
//!   设备端靠 lo/usb1 别名让 `10.11.99.1` 常驻可达。
//! - **GET-then-upload 归档**：`GET /documents/<folder-uuid>` 设"当前文件夹"是全局服务端状态，
//!   之后的 `/upload` 落进该文件夹（metadata.parent 会被忽略）。因为是全局状态，进程内的"设文件夹 → 上传"用一把锁串成一对。
//! - **防复制风暴**：大书 `/upload` 处理慢 → 408/读超时但文档已创建，此类错误**绝不重试**。
use std::io::{Cursor, Read, Write};
use std::path::Path;

// 书库 `.metadata`/`.content` 的只读查询单独一个子模块（不碰 HTTP），对外路径不变。
mod library;
pub use library::*;

pub const DEFAULT_HOST: &str = "10.11.99.1";

/// 上传结果：`Delivered`=确认成功；`LikelyDelivered`=超时但很可能已创建（别重试）。
#[derive(Debug, PartialEq)]
pub enum Delivery {
    Delivered(String),
    LikelyDelivered(String),
}

pub struct Xochitl {
    agent: ureq::Agent,
    host: String,
    library_dir: std::path::PathBuf,
}

impl Xochitl {
    /// `library_dir`=书库目录（用于按名找文件夹）；`timeout_secs` 建议 300（大书）。
    pub fn new(host: &str, library_dir: &Path, timeout_secs: u64) -> Xochitl {
        // 连接 10s 即判"未送达"（:80 没绑/USB 未就绪，可安全重试）；整体 timeout 给大书处理留足（超时但已送达
        // 由 upload_likely_delivered 识别、绝不重试）。
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build();
        Xochitl { agent, host: host.to_string(), library_dir: library_dir.to_path_buf() }
    }

    /// `/upload` 是否可达（不真上传，GET 根页）。
    pub fn reachable(&self) -> bool {
        self.agent.get(&format!("http://{}/", self.host)).timeout(std::time::Duration::from_secs(3)).call().is_ok()
    }

    /// 按 visibleName 找非回收站文件夹 uuid。
    pub fn find_folder(&self, name: &str) -> Option<String> {
        find_folder_by_name(&self.library_dir, name)
    }

    /// 给定文档 uuid，查它当前所在的设备文件夹 uuid（空串＝根）；查不到／在回收站 → `None`。
    pub fn parent_folder(&self, uuid: &str) -> Option<String> {
        parent_folder_of(&self.library_dir, uuid)
    }

    /// 在 `folder` 范围内给 `base_name` 去重，撞名就加数字后缀。
    pub fn unique_name(&self, folder: &str, base_name: &str) -> String {
        unique_document_name(&self.library_dir, folder, base_name)
    }

    /// 书库目录（`<uuid>.{metadata,content,epub,pdf}` 所在）。
    pub fn library_dir(&self) -> &Path {
        &self.library_dir
    }

    fn set_folder(&self, folder_uuid: &str) -> bool {
        let path = if folder_uuid.is_empty() { "documents/".to_string() } else { format!("documents/{folder_uuid}") };
        self.agent.get(&format!("http://{}/{}", self.host, path)).call().is_ok()
    }

    /// 上传进指定名字的文件夹（找不到→书库根，best-effort）。数据已经在内存里（漫画拆分份、
    /// note-serve 笔记本 zip 这类合成产物）用这个；落地文件直接上传用 [`Self::upload_file`]，
    /// 别自己先 `fs::read` 整个再传进来。
    pub fn upload(&self, data: &[u8], filename: &str, content_type: &str, folder_name: &str) -> Result<Delivery, String> {
        self.upload_body(Cursor::new(data), data.len() as u64, filename, content_type, folder_name)
    }

    /// 直接流式上传一个磁盘文件——内容全程不整体读进内存，只在 `send_multipart` 里按块过一遍
    /// （2026-09-19 OOM 审计：`Staging::deliver()` 落库不拆分路径曾经 `fs::read` 整本＋这里内部
    /// 再克隆一份拼 multipart body，峰值能到原文件 2 倍+；改流式后这条路径不再囤整本字节）。
    pub fn upload_file(&self, path: &Path, filename: &str, content_type: &str, folder_name: &str) -> Result<Delivery, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("打开 {}: {e}", path.display()))?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        self.upload_body(std::io::BufReader::new(file), len, filename, content_type, folder_name)
    }

    /// **突破网页上传的体积上限**（xochitl `/upload` 约 100MB 硬限，超了直接断连）：先上传 `placeholder`
    /// （几 KB 的占位文档，EPUB 要带真书名和封面，见 `bookconv::placeholder`）让 xochitl 建好条目，再把磁盘上
    /// 那个文件原子替换成 `path` 的真文件。2026-09-20 真机验证：PDF 154MB/349 页、EPUB 153MB 都能打开。
    ///
    /// - **EPUB**：删掉占位的渲染缓存 `.pdf`/`.epubindex`，用户第一次打开时 xochitl 重新渲染（146MB 实测约 25s，
    ///   之后走缓存），页数、`sizeInBytes` 等届时自己更新。
    /// - **PDF**：`.content` 里有逐页 UUID 表/页数/大小，必须一并改成真文件的（`pdf_pages` 由调用方给），
    ///   否则只显示占位的页数。列表里的页数/大小要用户点开后才刷新（真机观察）。
    ///
    /// 只能在 xochitl 数据目录本机可写时用（book-serve 跑在设备上）。返回新文档 uuid。失败时占位文档可能
    /// 已留在书库里（回执里说明），不做危险的"回滚删除"。
    pub fn upload_large_file(&self, path: &Path, filename: &str, content_type: &str, folder_name: &str, placeholder: &[u8], pdf_pages: Option<usize>) -> Result<String, String> {
        let ext = filename.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if ext != "epub" && ext != "pdf" {
            return Err("只有 EPUB/PDF 能走大文件通道".into());
        }
        let dir = self.library_dir.clone();
        if !dir.is_dir() {
            return Err(format!("xochitl 书库目录不可写（{}），大文件通道只能在设备上用", dir.display()));
        }
        let want_len = std::fs::metadata(path).map_err(|e| format!("读 {} 失败: {e}", path.display()))?.len();
        // 整个"传占位 → 按占位字节数认领 → 换成真文件"串行：认领只凭"刚进库 + 大小等于占位"，PDF 占位是同一份固定字节，
        // 两本大 PDF 同时走这条路时会认领到同一个 uuid——一本被覆盖进别人的条目、另一份占位永远留在书库里。锁要拿到
        // 替换完成：替换前那份文件仍是占位大小，后来者照样会认错（2026-09-25 第四轮审计）。
        static LARGE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _serial = crate::sync::lock(&LARGE);
        let since = crate::clock::now_ms().saturating_sub(2_000);
        self.upload(placeholder, filename, content_type, folder_name)?;
        // 等 xochitl 建好条目（`.metadata` + 占位文件都落地），按占位字节数确认是"我们这一份"而不是别人同时传的。
        let mut uuid = None;
        for _ in 0..100 {
            if let Some(d) = find_documents_since(&dir, since).into_iter().find(|d| std::fs::metadata(dir.join(format!("{}.{ext}", d.uuid))).map(|m| m.len() == placeholder.len() as u64).unwrap_or(false)) {
                uuid = Some(d.uuid);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let uuid = uuid.ok_or("占位文档已上传，但 20 秒内没在书库里找到它（未替换成真文件）")?;
        let tmp = dir.join(format!("{uuid}.{ext}.new"));
        let dest = dir.join(format!("{uuid}.{ext}"));
        let fail = |e: String| {
            let _ = std::fs::remove_file(&tmp);
            format!("{e}（书库里可能留有占位文档「{filename}」，请手动删除）")
        };
        std::fs::copy(path, &tmp).map_err(|e| fail(format!("复制大文件失败: {e}")))?;
        let got = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
        if got != want_len {
            return Err(fail(format!("复制后大小不符（{got} ≠ {want_len}）")));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        if ext == "epub" {
            let _ = std::fs::remove_file(dir.join(format!("{uuid}.pdf")));
            let _ = std::fs::remove_file(dir.join(format!("{uuid}.epubindex")));
        } else if let Some(n) = pdf_pages {
            rewrite_pdf_content(&dir, &uuid, n, want_len).map_err(fail)?;
        }
        std::fs::rename(&tmp, &dest).map_err(|e| fail(format!("替换文件失败: {e}")))?;
        Ok(uuid)
    }

    fn upload_body(&self, body: impl Read, body_len: u64, filename: &str, content_type: &str, folder_name: &str) -> Result<Delivery, String> {
        let folder = if folder_name.is_empty() { String::new() } else { self.find_folder(folder_name).unwrap_or_default() };
        // "设当前文件夹 → /upload" 必须成对、不被打断：当前文件夹是 xochitl 服务端的**全局**状态，两次投递并发时
        // （网关允许 3 本小书同时处理）A 设完文件夹、B 又设了自己的，A 的书就落进 B 的文件夹。进程内所有
        // `Xochitl` 实例共用一把锁（static），把这一对串起来；只锁上传本身，拆分投递等渲染的间隙不占锁。
        // 跨进程（note-serve 也会投笔记本）仍可能交错，这把锁管不到（2026-09-24 审计）。
        static UPLOAD: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _serial = crate::sync::lock(&UPLOAD);
        self.set_folder(&folder);
        match send_multipart(&self.agent, &self.host, body, body_len, filename, content_type) {
            Ok(resp) => Ok(Delivery::Delivered(resp)),
            Err(e) if upload_likely_delivered(&e) => Ok(Delivery::LikelyDelivered(e)),
            Err(e) => Err(e),
        }
    }
}

/// 把占位 PDF 的 `.content` 改成真 PDF 的：页数、逐页 UUID 表、`redirectionPageMap`、`sizeInBytes`。
fn rewrite_pdf_content(dir: &Path, uuid: &str, pages: usize, size: u64) -> Result<(), String> {
    let path = dir.join(format!("{uuid}.content"));
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| format!("读 .content 失败: {e}"))?).map_err(|e| format!(".content 不是合法 JSON: {e}"))?;
    let obj = v.as_object_mut().ok_or(".content 不是对象")?;
    obj.insert("pageCount".into(), pages.into());
    obj.insert("originalPageCount".into(), pages.into());
    obj.insert("pages".into(), (0..pages).map(|_| serde_json::Value::String(uuid::Uuid::new_v4().to_string())).collect::<Vec<_>>().into());
    obj.insert("redirectionPageMap".into(), (0..pages).collect::<Vec<_>>().into());
    obj.insert("sizeInBytes".into(), size.to_string().into());
    let tmp = dir.join(format!("{uuid}.content.new"));
    std::fs::write(&tmp, serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?).map_err(|e| format!("写 .content 失败: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("替换 .content 失败: {e}"))
}

/// 流式发一份 multipart `/upload` 请求：`body`（文件内容，长度已知 `body_len`）不整体缓冲，
/// 用 `Cursor(头).chain(body).chain(Cursor(尾))` 直接喂给 `ureq`；显式给 `Content-Length` 让
/// `ureq` 按已知长度发送而不是退化成 chunked（`ureq::Request::send` 文档：调用方可设
/// `Content-Length`，设了就不用 chunked）——线上字节序列跟改动前逐字节相同，只是不再囤在一个
/// `Vec<u8>` 里。
fn send_multipart(agent: &ureq::Agent, host: &str, body: impl Read, body_len: u64, filename: &str, content_type: &str) -> Result<String, String> {
    let boundary = format!("----shelf{}", uuid::Uuid::new_v4().simple());
    let mut header = Vec::new();
    let filename = header_safe_filename(filename);
    write!(header, "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n")
        .map_err(|e| e.to_string())?;
    let footer = format!("\r\n--{boundary}--\r\n").into_bytes();
    let total_len = header.len() as u64 + body_len + footer.len() as u64;
    let reader = Cursor::new(header).chain(body).chain(Cursor::new(footer));
    let resp = agent
        .post(&format!("http://{host}/upload"))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .set("Content-Length", &total_len.to_string())
        .send(reader);
    match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(c, r)) => Err(format!("HTTP {c}: {}", r.into_string().unwrap_or_default())),
        Err(e) => Err(format!("上传失败: {e}")),
    }
}

/// multipart 头里的 `filename="…"` 不能含 `"` 与 CR/LF：母版库文件名只校验"单段"（`fs::plain_name`），
/// 带引号的书名（`他说"好".pdf`）此前原样拼进头里，xochitl 解析到第一个 `"` 就截断，书库里显示成半截名；
/// 带换行则直接把后面的内容当成新的头。`"` 换成 `'`、CR/LF 换成空格，其余字节原样（中文照旧直传，
/// 与改动前一致——xochitl 按 UTF-8 读这个字段，真机一直这么传）。
fn header_safe_filename(name: &str) -> String {
    name.chars().map(|c| match c {
        '"' => '\'',
        '\r' | '\n' => ' ',
        c => c,
    }).collect()
}

/// 错误是否属于"很可能已送达"（408/读超时且非连接阶段）。
pub fn upload_likely_delivered(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    (e.contains("408") || e.contains("timed out") || e.contains("timeout")) && !e.contains("connect")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 假 xochitl：`GET /documents/..` 回 200；`POST /upload` 解出 multipart 里的文件部分，落成
    /// `<uuid>.{ext}` + `.metadata`（+ EPUB 的渲染缓存 `.pdf`/`.epubindex` 与 PDF 的 `.content`），回 201。
    fn fake_xochitl(lib: std::path::PathBuf) -> String {
        fake_xochitl_delayed(lib, std::time::Duration::ZERO)
    }

    /// 同 [`fake_xochitl`]，但条目在回应之后 `delay` 才落盘（真 xochitl 导入是异步的，回完 `/upload` 过一会才出现 `.metadata`）。
    fn fake_xochitl_delayed(lib: std::path::PathBuf, delay: std::time::Duration) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap().to_string();
        std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                if req.method() == &tiny_http::Method::Post {
                    let mut body = Vec::new();
                    std::io::Read::read_to_end(req.as_reader(), &mut body).unwrap();
                    let start = body.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                    let head = String::from_utf8_lossy(&body[..start]).to_string();
                    let fname = head.split("filename=\"").nth(1).unwrap().split('"').next().unwrap().to_string();
                    let end = body.windows(2).rposition(|w| w == b"\r\n").map(|_| body.len()).unwrap();
                    let tail = body.windows(4).rposition(|w| w == b"\r\n--").unwrap_or(end);
                    let file = &body[start..tail];
                    let uuid = uuid::Uuid::new_v4().to_string();
                    let ext = fname.rsplit('.').next().unwrap().to_string();
                    let (file, lib, created) = (file.to_vec(), lib.clone(), crate::clock::now_ms());
                    let materialize = move || {
                        std::fs::write(lib.join(format!("{uuid}.{ext}")), file).unwrap();
                        std::fs::write(lib.join(format!("{uuid}.metadata")), format!(r#"{{"type":"DocumentType","visibleName":"{fname}","parent":"","createdTime":"{created}"}}"#)).unwrap();
                        if ext == "epub" {
                            std::fs::write(lib.join(format!("{uuid}.pdf")), b"render-cache").unwrap();
                            std::fs::write(lib.join(format!("{uuid}.epubindex")), b"idx").unwrap();
                        } else {
                            std::fs::write(lib.join(format!("{uuid}.content")), r#"{"fileType":"pdf","pageCount":1,"pages":["x"],"redirectionPageMap":[0],"sizeInBytes":"5"}"#).unwrap();
                        }
                    };
                    if delay.is_zero() {
                        materialize();
                    } else {
                        std::thread::spawn(move || {
                            std::thread::sleep(delay);
                            materialize();
                        });
                    }
                    let _ = req.respond(tiny_http::Response::from_string(r#"{"status":"Upload successful"}"#).with_status_code(201));
                } else {
                    let _ = req.respond(tiny_http::Response::from_string("[]"));
                }
            }
        });
        addr
    }

    /// 回归：并发投递到不同文件夹，每本书都落进自己要的文件夹（"设当前文件夹 → /upload" 不被别的投递插队）。
    /// 假 xochitl 记下每次 `GET /documents/<uuid>` 设的当前文件夹，`POST /upload` 时把"文件名 → 当前文件夹"记下来。
    #[test]
    fn concurrent_uploads_land_in_their_own_folders() {
        let lib = tempfile::tempdir().unwrap();
        for (uuid, name) in [("fa", "甲"), ("fb", "乙")] {
            std::fs::write(lib.path().join(format!("{uuid}.metadata")), format!(r#"{{"type":"CollectionType","visibleName":"{name}","parent":""}}"#)).unwrap();
        }
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap().to_string();
        let landed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
        let l2 = landed.clone();
        std::thread::spawn(move || {
            let mut current = String::new();
            for mut req in server.incoming_requests() {
                if req.method() == &tiny_http::Method::Post {
                    let mut body = Vec::new();
                    std::io::Read::read_to_end(req.as_reader(), &mut body).unwrap();
                    let text = String::from_utf8_lossy(&body).to_string();
                    let fname = text.split("filename=\"").nth(1).unwrap().split('"').next().unwrap().to_string();
                    l2.lock().unwrap().push((fname, current.clone()));
                } else {
                    current = req.url().trim_start_matches("/documents/").trim_start_matches('/').to_string();
                    std::thread::sleep(std::time::Duration::from_millis(2)); // 拉大"设完文件夹到上传"之间的窗口
                }
                let _ = req.respond(tiny_http::Response::from_string("{}"));
            }
        });
        let x = std::sync::Arc::new(Xochitl::new(&addr, lib.path(), 10));
        let hs: Vec<_> = [("甲", "fa"), ("乙", "fb")]
            .into_iter()
            .map(|(folder, _)| {
                let x = x.clone();
                std::thread::spawn(move || {
                    for i in 0..10 {
                        x.upload(b"data", &format!("{folder}-{i}.pdf"), "application/pdf", folder).unwrap();
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        let got = landed.lock().unwrap().clone();
        assert_eq!(got.len(), 20);
        for (fname, folder) in got {
            let want = if fname.starts_with('甲') { "fa" } else { "fb" };
            assert_eq!(folder, want, "{fname} 落错了文件夹");
        }
    }

    #[test]
    fn upload_large_file_swaps_epub_and_drops_render_cache() {
        let lib = tempfile::tempdir().unwrap();
        let addr = fake_xochitl(lib.path().to_path_buf());
        let x = Xochitl::new(&addr, lib.path(), 10);
        let big = lib.path().join("big-source.bin");
        let big_bytes = vec![7u8; 300_000];
        std::fs::write(&big, &big_bytes).unwrap();
        let uuid = x.upload_large_file(&big, "书 - 02卷.epub", "application/epub+zip", "", b"PLACEHOLDER-EPUB", None).unwrap();
        assert_eq!(std::fs::read(lib.path().join(format!("{uuid}.epub"))).unwrap(), big_bytes, "占位必须被真文件替换");
        assert!(!lib.path().join(format!("{uuid}.pdf")).exists(), "占位的渲染缓存必须删掉，让 xochitl 重新渲染");
        assert!(!lib.path().join(format!("{uuid}.epubindex")).exists());
        assert!(!lib.path().join(format!("{uuid}.epub.new")).exists(), "不留临时文件");
    }

    /// 回归：两本大 PDF 同时走大文件通道（占位字节相同），各自认领到自己的条目、内容不串。
    #[test]
    fn concurrent_large_pdf_uploads_claim_distinct_entries() {
        let lib = tempfile::tempdir().unwrap();
        let addr = fake_xochitl_delayed(lib.path().to_path_buf(), std::time::Duration::from_millis(300));
        let x = std::sync::Arc::new(Xochitl::new(&addr, lib.path(), 10));
        let hs: Vec<_> = (0..3u8)
            .map(|i| {
                let (x, dir) = (x.clone(), lib.path().to_path_buf());
                std::thread::spawn(move || {
                    let src = dir.join(format!("src-{i}.bin"));
                    let bytes = vec![i + 1; 8_000_000 + i as usize * 1000];
                    std::fs::write(&src, &bytes).unwrap();
                    let uuid = x.upload_large_file(&src, &format!("大书{i}.pdf"), "application/pdf", "", b"%PDF-placeholder", Some(3)).unwrap();
                    (uuid, bytes)
                })
            })
            .collect();
        let got: Vec<(String, Vec<u8>)> = hs.into_iter().map(|h| h.join().unwrap()).collect();
        let uuids: std::collections::HashSet<_> = got.iter().map(|(u, _)| u.clone()).collect();
        assert_eq!(uuids.len(), 3, "每本认领到不同条目");
        for (uuid, bytes) in got {
            assert_eq!(std::fs::read(lib.path().join(format!("{uuid}.pdf"))).unwrap(), bytes, "条目里是自己的内容");
        }
    }

    #[test]
    fn upload_large_file_pdf_rewrites_content_pages() {
        let lib = tempfile::tempdir().unwrap();
        let addr = fake_xochitl(lib.path().to_path_buf());
        let x = Xochitl::new(&addr, lib.path(), 10);
        let big = lib.path().join("big-source.bin");
        std::fs::write(&big, vec![9u8; 123_456]).unwrap();
        let uuid = x.upload_large_file(&big, "漫画.pdf", "application/pdf", "", b"%PDF-placeholder", Some(349)).unwrap();
        assert_eq!(std::fs::metadata(lib.path().join(format!("{uuid}.pdf"))).unwrap().len(), 123_456);
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(lib.path().join(format!("{uuid}.content"))).unwrap()).unwrap();
        assert_eq!(v["pageCount"], 349);
        assert_eq!(v["pages"].as_array().unwrap().len(), 349);
        assert_eq!(v["sizeInBytes"], "123456");
    }

    #[test]
    fn upload_large_file_rejects_other_formats_and_missing_library() {
        let lib = tempfile::tempdir().unwrap();
        let x = Xochitl::new("127.0.0.1:1", lib.path(), 1);
        assert!(x.upload_large_file(Path::new("/x"), "a.cbz", "x", "", b"p", None).unwrap_err().contains("EPUB/PDF"));
        let y = Xochitl::new("127.0.0.1:1", Path::new("/nonexistent-lib"), 1);
        assert!(y.upload_large_file(Path::new("/x"), "a.epub", "x", "", b"p", None).unwrap_err().contains("只能在设备上用"));
    }

    #[test]
    fn rewrite_pdf_content_updates_pages_map_and_size_keeping_other_fields() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("u1.content"),
            r#"{"fileType":"pdf","pageCount":3,"originalPageCount":3,"pages":["a","b","c"],"redirectionPageMap":[0,1,2],"sizeInBytes":"99","zoomMode":"bestFit"}"#,
        )
        .unwrap();
        rewrite_pdf_content(d.path(), "u1", 349, 154_367_634).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.path().join("u1.content")).unwrap()).unwrap();
        assert_eq!(v["pageCount"], 349);
        assert_eq!(v["originalPageCount"], 349);
        assert_eq!(v["pages"].as_array().unwrap().len(), 349);
        assert_eq!(v["redirectionPageMap"].as_array().unwrap().len(), 349);
        assert_eq!(v["redirectionPageMap"][348], 348);
        assert_eq!(v["sizeInBytes"], "154367634");
        assert_eq!(v["zoomMode"], "bestFit", "其它字段必须原样保留");
        let ids: std::collections::HashSet<_> = v["pages"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        assert_eq!(ids.len(), 349, "逐页 UUID 必须互不相同");
    }

    #[test]
    fn header_filename_strips_quotes_and_newlines_only() {
        assert_eq!(header_safe_filename("他说\"好\".pdf"), "他说'好'.pdf");
        assert_eq!(header_safe_filename("a\r\nX-Evil: 1.epub"), "a  X-Evil: 1.epub");
        assert_eq!(header_safe_filename("镖人 - 01卷.epub"), "镖人 - 01卷.epub", "普通名字原样");
    }

    #[test]
    fn classifies_upload_errors() {
        assert!(upload_likely_delivered("HTTP 408: 408 request timeout"));
        assert!(upload_likely_delivered("上传失败: timed out reading response"));
        assert!(!upload_likely_delivered("上传失败: Connection refused (os error 111)"));
        assert!(!upload_likely_delivered("上传失败: connect timed out"));
    }
}

/// 2026-09-19 OOM 审计：`upload`/`upload_file` 改流式发送体后，线上字节序列应该跟改动前的
/// "整块 Vec 拼 body" 写法完全一致，只是不再整块囤内存。起一个最小 HTTP mock（GET 任意路径回
/// 200 空体，POST /upload 把收到的原始 body 送回 channel）验证这一点——不需要真连 xochitl。
#[cfg(test)]
mod upload_tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// 强制每个请求 `Connection: close`（下一个请求另起连接），mock 逻辑不用管 keep-alive 复用；
    /// `upload`/`upload_file` 先各发一次 `set_folder` 的 GET 再发 POST /upload，遇到 POST 就把
    /// body 送回 channel 并停止收连接。
    fn mock_xochitl() -> (String, mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                    break;
                }
                let mut content_length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; content_length];
                reader.read_exact(&mut body).unwrap();
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
                let is_post = request_line.starts_with("POST");
                if is_post {
                    let _ = tx.send(body);
                    break;
                }
            }
        });
        (format!("127.0.0.1:{}", addr.port()), rx)
    }

    fn assert_well_formed_upload(body: &[u8], data: &[u8], filename: &str, content_type: &str) {
        let text = String::from_utf8_lossy(body);
        assert!(text.contains(&format!("filename=\"{filename}\"")), "{text}");
        assert!(text.contains(&format!("Content-Type: {content_type}")), "{text}");
        assert!(body.windows(data.len().max(1)).any(|w| w == data), "body 应该原样包含完整数据");
        assert!(text.trim_end().ends_with("--"), "multipart 尾部 boundary 收尾要完整");
    }

    #[test]
    fn upload_streams_in_memory_data_as_well_formed_multipart() {
        let t = tempfile::tempdir().unwrap();
        let (host, rx) = mock_xochitl();
        let x = Xochitl::new(&host, t.path(), 5);
        let data = b"hello epub bytes, this is the whole book content".to_vec();
        let r = x.upload(&data, "book.epub", "application/epub+zip", "");
        assert!(matches!(r, Ok(Delivery::Delivered(_))), "{r:?}");
        let body = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_well_formed_upload(&body, &data, "book.epub", "application/epub+zip");
    }

    #[test]
    fn upload_file_streams_disk_file_as_well_formed_multipart() {
        let t = tempfile::tempdir().unwrap();
        let data = b"streamed straight from disk, never buffered whole in memory".to_vec();
        let path = t.path().join("src.epub");
        std::fs::write(&path, &data).unwrap();
        let (host, rx) = mock_xochitl();
        let x = Xochitl::new(&host, t.path(), 5);
        let r = x.upload_file(&path, "book.epub", "application/epub+zip", "");
        assert!(matches!(r, Ok(Delivery::Delivered(_))), "{r:?}");
        let body = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_well_formed_upload(&body, &data, "book.epub", "application/epub+zip");
    }
}
