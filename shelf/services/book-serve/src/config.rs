//! `$XDG_CONFIG_HOME/shelf/book.json`。缺省即可用；首启写出缺省文件供用户改。
use serde::{Deserialize, Serialize};
use rmsvc_core::paths::Paths;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct BookConfig {
    /// 「投入原生书库」未指定文件夹时落进的书库文件夹（visibleName；找不到→书库根）。
    pub library_folder: String,
    /// 网页「批注文件夹」预设对应的文件夹（PDF 手写定稿）。
    pub annot_folder: String,
    /// xochitl web 主机（`/upload`）。
    pub xochitl_host: String,
    /// `/upload` 超时（大书处理慢；超时但已送达会被判 LikelyDelivered、绝不重试）。
    pub upload_timeout_secs: u64,
    /// 投原生的体积门（MB）：xochitl `/upload` 有上限（真机 188MB 被 "multipart body is too large" 拒并断连，60MB 稳），
    /// 超过就不发、直接回执指引分卷。0=不拦。
    pub native_upload_limit_mb: u64,
}

impl Default for BookConfig {
    fn default() -> Self {
        BookConfig { library_folder: "library".into(), annot_folder: "library".into(), xochitl_host: rmsvc_core::xochitl::DEFAULT_HOST.into(), upload_timeout_secs: 300, native_upload_limit_mb: 150 }
    }
}

impl BookConfig {
    pub fn load(paths: &Paths) -> BookConfig {
        rmsvc_core::config::load_or_seed(&paths.service_config("book"))
    }
    /// 体积门（字节）；0=不拦。
    pub fn native_upload_limit_bytes(&self) -> u64 {
        self.native_upload_limit_mb * 1024 * 1024
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_json_fills_defaults_and_ignores_retired_keys() {
        let c: BookConfig = serde_json::from_str(r#"{"libraryFolder":"books","comicMono":true,"optimizeDirectEpub":false}"#).unwrap();
        assert_eq!(c.library_folder, "books");
        assert_eq!(c.upload_timeout_secs, 300);
        assert_eq!(c.native_upload_limit_bytes(), 150 * 1024 * 1024);
    }
}
