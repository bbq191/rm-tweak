//! rmsvc-core —— reMarkable 设备端 Web 服务共享底座（2026-09-11 从 `shelf/crates/shelf-core`
//! 正名搬顶层：`shelf/`、`notes/`、`gateway/` 三方共用，不是 shelf 私有物）。**不引用旧项目
//! 任何 crate**（device-core / weread-device），需要的能力按"剥离移植"独立实现（`xochitl`、
//! `fswatch` 两模块注明来源）。
//!
//! 模块职责（端口/适配器分层：领域模块不碰 HTTP 类型，`http` 是唯一适配层）：
//! - `paths`    XDG 基目录规范的单一路径表（三方共用同一张表，见 shelf/docs）。
//! - `registry` 服务自注册/发现（`$XDG_RUNTIME_DIR/shelf/services/<name>.json`），网关据此拔插。
//! - `multipart` 流式 multipart/form-data 解析（多文件、落盘不进内存，设备 MemoryMax 友好）。
//! - `asset`    资产仓库抽象（Repository）+ 上传流程模板（Template Method），字体/壁纸共用。
//! - `http`     tiny_http 适配：路由、JSON 回执、查询串。
//! - `xochitl`  原生书库免重启注入（`/upload` GET-then-upload 归档、防复制风暴判据）；`xochitl::library` 是书库 `.metadata`/`.content` 的只读查询（找文件夹/去重命名/渲染页数），路径仍走 `xochitl::*`。
//! - `xochitl_conf` xochitl.conf `[General]` 单键读写（休眠屏 `SleepScreenPath`；含凭证，绝不打印行内容）。
//! - `fswatch`  inotify 防抖目录监听（spool 追平）。
//! - `service`  服务启动模板：解析参数→建目录→注册→起服务器。
//! - `config`   服务配置/状态 JSON 读写模板（load/seed/save），收编各服务的 config 复制。
//! - `events`   进程内事件总线 + SSE 流（服务在变更处发事件，网关汇聚推给网页/CLI，网页零轮询）。
//! - `fs`       原子写（tmp→rename）+ unix 权限 + 单段文件名校验 / 同名不覆盖，config/registry/母版库/壁纸池共用。
//! - `formats`  文件格式白名单单一事实源（书籍/字体/词典/图片），UI accept 与服务端上传门同源。
//! - `tls`      私有 CA + 叶证书生成/加载（网关 HTTPS；装一次 CA 免提示）。
//! - `auth`     密码哈希（PBKDF2-HMAC-SHA256，60 万轮）、Basic/Cookie 解析、内存会话表。
//! - `netinfo`  本机 IPv4 表（证书 SAN、mDNS 选址）。
//! - `cache`    单值 TTL 缓存（`/status` 这类重活接口降频，操作后可主动失效）。
//! - `clock`    unix 时间戳唯一出处（秒/毫秒/纳秒、文件 mtime 换算）。
//! - `mdns`     极简 mDNS 应答器（`shelf.local` 伪域名）。
//! - `sync`     容忍 poison 的取锁（`sync::lock`），各服务共用。
pub mod asset;
pub mod auth;
pub mod cache;
pub mod clock;
pub mod config;
pub mod events;
pub mod formats;
pub mod fs;
pub mod fswatch;
pub mod http;
pub mod mdns;
pub mod netinfo;
pub mod multipart;
pub mod paths;
pub mod registry;
pub mod service;
pub mod sync;
pub mod tls;
pub mod ttf;
pub mod xochitl;
pub mod xochitl_conf;
