use crate::auth::{hash_secret, ClientToken};
use crate::stream::StreamProxyConfig;
use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub listen: String,
    pub admin_password_hash: String,
    pub channel_cache_ttl_seconds: u64,
    pub backend_channels_url: Option<String>,
    pub stream: StreamProxyConfig,
    pub tokens: Vec<ClientToken>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8088".into(),
            admin_password_hash: hash_secret("admin"),
            channel_cache_ttl_seconds: 3600,
            backend_channels_url: None,
            stream: StreamProxyConfig::default(),
            tokens: Vec::new(),
        }
    }
}

impl ProxyConfig {
    pub fn load_or_default(path: &Path) -> io::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path)?;
        serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn save_atomic(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = unique_tmp_path(path);
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, path).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
    }
}

fn unique_tmp_path(path: &Path) -> std::path::PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.json");
    path.with_file_name(format!(".{file_name}.{pid}.{nanos}.tmp"))
}
