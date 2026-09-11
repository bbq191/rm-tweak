//! book-serve —— 书架·母版库 + 落原生（loopback 8790）。
//! 所有内容源（网页上传 / 抓网文 / scp 进 inbox）原样落**母版库**；优化与落库（投 xochitl）是母版库里
//! 各自独立的动作（`staging.rs`）。自有 inbox 队列（XDG state）+ inotify 追平。不读写旧项目任何路径。
//! 网页 tab 「传书」是网关固定页（不由本服务注册），本服务不挂 tab。
mod agent_failures;
mod api;
mod comic_margins;
mod config;
mod mkdir;
mod ops;
mod pending_queue;
mod render_check;
mod service_state;
mod sidecar;
mod spool;
mod staging;
mod reading_direction;
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
    // 追平：inotify 防抖监听（常驻线程）先起，再扫一遍启动前就在 inbox 里的——先监听后扫描，扫描期间落进来的文件
    // 不会漏。启动扫描遇到"还在写"的文件（见 `State::process_inbox_counting_deferred`）隔一个静止时长再扫（有上限）；
    // 它们写完时的事件若恰好落在监听起来之前，也不会因此永远躺在 inbox 里。
    {
        let st = st.clone();
        std::thread::spawn(move || {
            let inbox = st.spool.inbox();
            rmsvc_core::fswatch::watch_debounced(&inbox, std::time::Duration::from_secs(8), |_| {
                st.process_inbox(None);
            });
        });
    }
    {
        let st = st.clone();
        std::thread::spawn(move || {
            // 最多重扫 12 轮（约 1 分钟）：一直在写的大文件之后由它写完时的事件接手，这里不陪着空转。
            for _ in 0..12 {
                if st.process_inbox_counting_deferred(None).1 == 0 {
                    break;
                }
                std::thread::sleep(st.inbox_settle);
            }
        });
    }
    println!("[book-serve] 母版库 {}；xochitl {}", st.staging.dir().display(), st.cfg.xochitl_host);
    if let Err(e) = service::run(&SPEC, &bind, &paths, api::router(st)) {
        eprintln!("[book-serve] {e}");
        std::process::exit(1);
    }
}
