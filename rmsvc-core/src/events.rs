//! 事件总线 + SSE（Server-Sent Events）流：服务在**变更发生处**发事件（上传/优化/落库/删除/轮换/inbox 追平），
//! 网关汇聚后推给浏览器与 host CLI，网页不轮询也能即时刷新（2026-09-06 用户："不喜欢轮询，要事件通知"）。
//! - `EventBus`：进程内广播，订阅者各持一个有界通道（满了丢事件——事件只是"该刷新了"的信号，不携带状态）；
//! - `SseStream`：把通道包成 `Read`，交给 HTTP 层（`http::respond_stream`：接管 socket、一帧一 flush，绕开 tiny_http 的 8 KB chunked 缓冲）；20 s 无事件发一行注释心跳，
//!   浏览器 `EventSource` 断线（WiFi 掉 / 设备休眠醒来）会自动重连；
//! - 事件格式一行 JSON：`{"area":"books","kind":"staging","at":<unix秒>}`，网关转发时补 `"svc"`。
use crate::http::Reply;
use std::io::Read;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Mutex;
use std::time::Duration;

/// 心跳间隔（小于常见反向代理/浏览器的空闲超时）。
pub const KEEPALIVE: Duration = Duration::from_secs(20);
const QUEUE: usize = 64;

#[derive(Default)]
pub struct EventBus {
    subs: Mutex<Vec<SyncSender<String>>>,
}

impl EventBus {
    pub fn new() -> EventBus {
        EventBus::default()
    }

    /// 订阅：返回 SSE 可读流（掉线后由 HTTP 层 drop，总线在下次 publish 时清掉死订阅者）。
    pub fn subscribe(&self) -> SseStream {
        let (tx, rx) = sync_channel::<String>(QUEUE);
        self.subs.lock().unwrap_or_else(|e| e.into_inner()).push(tx);
        SseStream { rx, pending: Vec::new() }
    }

    /// 发一条事件（`area`=UI 区域：books/koreader/fonts/wallpapers/manage；`kind`=细分）。
    pub fn publish(&self, area: &str, kind: &str) {
        let at = crate::clock::now_secs();
        self.publish_raw(&serde_json::json!({"area": area, "kind": kind, "at": at}).to_string());
    }

    /// 转发已成型的 JSON 行（网关汇聚用）。
    pub fn publish_raw(&self, json_line: &str) {
        let frame = format!("data: {json_line}\n\n");
        let mut subs = self.subs.lock().unwrap_or_else(|e| e.into_inner());
        subs.retain(|tx| match tx.try_send(frame.clone()) {
            Ok(()) | Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Disconnected(_)) => false,
        });
    }

    #[cfg(test)]
    pub fn subscribers(&self) -> usize {
        self.subs.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// `GET /events` 的回执：`text/event-stream` 流式响应。
    pub fn sse_reply(&self) -> Reply {
        Reply::stream("text/event-stream; charset=utf-8", Box::new(self.subscribe())).with_header("Cache-Control", "no-cache").with_header("X-Accel-Buffering", "no")
    }
}

/// 把事件通道包成阻塞 `Read`：无事件时等 `KEEPALIVE` 后吐一行注释心跳。
pub struct SseStream {
    rx: Receiver<String>,
    pending: Vec<u8>,
}

impl Read for SseStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending.is_empty() {
            match self.rx.recv_timeout(KEEPALIVE) {
                Ok(frame) => self.pending = frame.into_bytes(),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => self.pending = b": ping\n\n".to_vec(),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }
        let n = self.pending.len().min(buf.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

/// 解析一行 SSE 文本：`data: {...}` → JSON 行；心跳/空行 → None。
pub fn parse_sse_line(line: &str) -> Option<&str> {
    let line = line.trim_end_matches(['\r', '\n']);
    line.strip_prefix("data:").map(str::trim).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_reaches_subscribers_and_drops_dead_ones() {
        let bus = EventBus::new();
        let mut a = bus.subscribe();
        let b = bus.subscribe();
        assert_eq!(bus.subscribers(), 2);
        bus.publish("books", "staging");
        let mut buf = [0u8; 2048];
        let n = a.read(&mut buf).unwrap();
        let s = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(s.starts_with("data: {") && s.ends_with("}\n\n") && s.contains(r#""area":"books""#) && s.contains(r#""kind":"staging""#), "{s}");
        drop(b);
        bus.publish("fonts", "fonts");
        assert_eq!(bus.subscribers(), 1, "死订阅者在下次 publish 时清掉");
        assert_eq!(parse_sse_line("data: {\"a\":1}\n"), Some("{\"a\":1}"));
        assert_eq!(parse_sse_line(": ping\n"), None);
        assert_eq!(parse_sse_line(""), None);
    }

    #[test]
    fn stream_reads_in_chunks_and_ends_when_bus_dropped() {
        let bus = EventBus::new();
        let mut s = bus.subscribe();
        bus.publish_raw("{\"x\":1}");
        let mut small = [0u8; 4];
        assert_eq!(s.read(&mut small).unwrap(), 4);
        assert_eq!(&small, b"data");
        let mut rest = [0u8; 64];
        let n = s.read(&mut rest).unwrap();
        assert_eq!(std::str::from_utf8(&rest[..n]).unwrap(), ": {\"x\":1}\n\n");
        drop(bus);
        assert_eq!(s.read(&mut rest).unwrap(), 0, "总线没了 → EOF");
    }
}
