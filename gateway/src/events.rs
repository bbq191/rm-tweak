//! 事件汇聚（Fan-in）：网关维护一个总线，给浏览器/CLI 一条 `GET /api/events` SSE；每个领域服务各起一条
//! loopback 长连接订阅它的 `GET /events`，收到的事件补上 `svc` 后转发。服务没起 / 重启 → 3 s 后重连（阻塞等待，零轮询 CPU）。
//! 另监听注册表目录（inotify，tmpfs）：服务注册/注销时发 `{"area":"manage"}`，网页管理台与 tab 列表据此刷新。
//! 设计约束（用户 2026-09-06）：不轮询、不监听全盘、日志写入不触发——事件只来自服务代码里的变更点与这两处 inotify。
use rmsvc_core::events::{parse_sse_line, EventBus};
use rmsvc_core::paths::Paths;
use rmsvc_core::registry;
use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::time::Duration;

pub struct Hub {
    pub bus: Arc<EventBus>,
}

impl Hub {
    /// 起所有后台线程：每个模块一条订阅线程 + 注册表目录监听。
    pub fn spawn(paths: Arc<Paths>) -> Hub {
        let bus = Arc::new(EventBus::new());
        for m in crate::manage::MODULES {
            let (bus, paths, seg, svc) = (bus.clone(), paths.clone(), m.seg, m.service);
            std::thread::spawn(move || subscribe_loop(&bus, &paths, seg, svc));
        }
        {
            let (bus, dir) = (bus.clone(), paths.services_dir());
            std::thread::spawn(move || {
                rmsvc_core::fswatch::watch_debounced(&dir, Duration::from_millis(500), |_| bus.publish("manage", "services"));
            });
        }
        Hub { bus }
    }
}

/// 一条服务的订阅循环：找注册表 → 连 `/events` → 逐行转发；断了睡 3 s 重来。
fn subscribe_loop(bus: &EventBus, paths: &Paths, seg: &str, svc: &str) {
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(3)).build(); // 读不设超时：SSE 靠心跳保活
    loop {
        let Some(info) = registry::find(paths, svc) else {
            std::thread::sleep(Duration::from_secs(3));
            continue;
        };
        match agent.get(&format!("{}/events", info.base_url())).call() {
            Ok(resp) => {
                let reader = BufReader::new(resp.into_reader());
                for line in reader.lines().map_while(Result::ok) {
                    if let Some(json) = parse_sse_line(&line) {
                        bus.publish_raw(&tag_svc(json, seg));
                    }
                }
            }
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// 给服务发来的事件补 `"svc":"<seg>"`（URL 段名，与网页 tab / `/api/<seg>` 一致）。
pub fn tag_svc(json: &str, seg: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(mut v) => {
            if let Some(o) = v.as_object_mut() {
                o.insert("svc".into(), serde_json::Value::String(seg.into()));
            }
            v.to_string()
        }
        Err(_) => serde_json::json!({"svc": seg, "raw": json}).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tags_svc_and_tolerates_bad_json() {
        let t = tag_svc(r#"{"area":"books","kind":"staging","at":1}"#, "books");
        let v: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["svc"], "books");
        assert_eq!(v["kind"], "staging");
        let bad = tag_svc("not json", "fonts");
        assert!(bad.contains(r#""svc":"fonts""#) && bad.contains("raw"));
    }
}
