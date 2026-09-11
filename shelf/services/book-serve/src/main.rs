//! book-serve —— 书架·母版库 + 落原生（loopback 8790）。
//! 所有内容源（网页上传 / 抓网文 / host `shelf push` / scp 进 inbox）原样落**母版库**；优化与落库（投 xochitl）是母版库里
//! 各自独立的动作（`staging.rs`）。自有 inbox 队列（XDG state）+ inotify 追平。不读写旧项目任何路径。
//! 网页 tab 「传书」是网关固定页（不由本服务注册），本服务不挂 tab。
mod api;
mod config;
mod pending_queue;
mod render_check;
mod service_state;
mod sidecar;
mod spool;
mod staging;
mod trash;

use rmsvc_core::paths::Paths;
use rmsvc_core::service::{self, ServiceSpec};
use std::sync::Arc;

const SPEC: ServiceSpec = ServiceSpec {
    name: "book-serve",
    label: "母版库 / 落原生",
    version: env!("CARGO_PKG_VERSION"),
    default_bind: "127.0.0.1:8790",
    tab: None,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bind = service::parse_bind(&args, SPEC.default_bind);
    let paths = Paths::from_env();
    let st = Arc::new(service_state::State::new(&paths));
    if let Err(e) = paths.ensure().and_then(|_| st.ensure_dirs()) {
        eprintln!("[book-serve] 建目录失败: {e}");
        std::process::exit(1);
    }
    let n = st.spool.recover_orphans();
    if n > 0 {
        println!("[book-serve] 恢复 {n} 个上次未完成的文件回 inbox");
    }
    // 追平线程：启动先扫一遍，再 inotify 防抖等待。
    {
        let st = st.clone();
        std::thread::spawn(move || {
            st.process_inbox(None);
            let inbox = st.spool.inbox();
            rmsvc_core::fswatch::watch_debounced(&inbox, std::time::Duration::from_secs(8), |_| {
                st.process_inbox(None);
            });
        });
    }
    println!("[book-serve] 母版库 {}；书库文件夹 {:?}；xochitl {}", st.staging.dir().display(), st.cfg.library_folder, st.cfg.xochitl_host);
    if let Err(e) = service::run(&SPEC, &bind, &paths, api::router(st)) {
        eprintln!("[book-serve] {e}");
        std::process::exit(1);
    }
}
