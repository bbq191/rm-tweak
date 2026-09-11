//! `$XDG_CONFIG_HOME/notes/note.json`。缺省即可用；首启写出缺省文件供用户改。
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct NoteConfig {
    /// xochitl web 主机（`/upload`），与书架同一套设备事实（USB/WiFi 都常驻可达，见
    /// `rmsvc_core::xochitl::DEFAULT_HOST` 文档）。
    pub xochitl_host: String,
    /// `/upload` 超时；笔记本单页文档很小，不用给大书那么长。
    pub upload_timeout_secs: u64,
}

impl Default for NoteConfig {
    fn default() -> Self {
        NoteConfig { xochitl_host: rmsvc_core::xochitl::DEFAULT_HOST.into(), upload_timeout_secs: 60 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_partial_override() {
        let c = NoteConfig::default();
        assert_eq!(c.xochitl_host, rmsvc_core::xochitl::DEFAULT_HOST);
        let partial: NoteConfig = serde_json::from_str(r#"{"uploadTimeoutSecs": 120}"#).unwrap();
        assert_eq!(partial.upload_timeout_secs, 120);
        assert_eq!(partial.xochitl_host, rmsvc_core::xochitl::DEFAULT_HOST, "没给的字段落缺省");
    }
}
