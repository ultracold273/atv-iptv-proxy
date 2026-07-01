use crate::auth::{hash_secret, normalize_token_ids, ClientToken};
use crate::stream::StreamProxyConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    #[serde(default = "default_channel_cache_ttl_seconds")]
    pub channel_cache_ttl_seconds: u64,
    #[serde(default = "default_epg_cache_ttl_seconds")]
    pub epg_cache_ttl_seconds: u64,
    pub backend_channels_url: Option<String>,
    pub provider: Option<ProviderConfig>,
    #[serde(default)]
    pub pairing: PairingConfig,
    #[serde(default)]
    pub stream: StreamProxyConfig,
    #[serde(default)]
    pub tokens: Vec<ClientToken>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderConfig {
    pub user_id: String,
    pub password: String,
    pub stb_id: String,
    pub local_ip: String,
    pub local_mac: String,
    pub auth_server_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingConfig {
    #[serde(default = "default_pairing_session_ttl_seconds")]
    pub session_ttl_seconds: u64,
    #[serde(default = "default_pairing_poll_interval_seconds")]
    pub poll_interval_seconds: u64,
    #[serde(default)]
    pub create_rate_limit: RateLimitConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RateLimitConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_pairing_rate_limit_window_seconds")]
    pub window_seconds: u64,
    #[serde(default = "default_pairing_rate_limit_max_requests")]
    pub max_requests: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelNumberOverride {
    #[serde(default)]
    pub name: Option<String>,
    pub number: u32,
}

pub type ChannelNumberOverrides = HashMap<String, ChannelNumberOverride>;

pub fn load_channel_number_overrides(path: Option<&Path>) -> io::Result<ChannelNumberOverrides> {
    let Some(path) = path else {
        return Ok(ChannelNumberOverrides::default());
    };
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8088".into(),
            admin_password_hash: hash_secret("admin"),
            channel_cache_ttl_seconds: default_channel_cache_ttl_seconds(),
            epg_cache_ttl_seconds: default_epg_cache_ttl_seconds(),
            backend_channels_url: None,
            provider: None,
            pairing: PairingConfig::default(),
            stream: StreamProxyConfig::default(),
            tokens: Vec::new(),
        }
    }
}

fn default_channel_cache_ttl_seconds() -> u64 {
    3600
}

fn default_epg_cache_ttl_seconds() -> u64 {
    300
}

fn default_pairing_session_ttl_seconds() -> u64 {
    300
}

fn default_pairing_poll_interval_seconds() -> u64 {
    2
}

fn default_pairing_rate_limit_window_seconds() -> u64 {
    60
}

fn default_pairing_rate_limit_max_requests() -> u32 {
    10
}

impl Default for PairingConfig {
    fn default() -> Self {
        Self {
            session_ttl_seconds: default_pairing_session_ttl_seconds(),
            poll_interval_seconds: default_pairing_poll_interval_seconds(),
            create_rate_limit: RateLimitConfig::default(),
        }
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            window_seconds: default_pairing_rate_limit_window_seconds(),
            max_requests: default_pairing_rate_limit_max_requests(),
        }
    }
}

impl ProxyConfig {
    pub fn load_or_default(path: &Path) -> io::Result<Self> {
        if !path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("config file not found: {}", path.display()),
            ));
        }
        let text = fs::read_to_string(path)?;
        let mut config: Self = serde_json::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        normalize_token_ids(&mut config.tokens);
        Ok(config)
    }

    pub fn validate_startup(&self) -> Result<(), String> {
        validate_admin_password_hash(&self.admin_password_hash)
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

fn validate_admin_password_hash(value: &str) -> Result<(), String> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err("admin_password_hash must start with sha256:".to_string());
    };
    if hex.contains("replace") {
        return Err("admin_password_hash is still the example placeholder".to_string());
    }
    if value == hash_secret("admin") {
        return Err("admin_password_hash must not use the default password 'admin'".to_string());
    }
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(
            "admin_password_hash must be sha256: followed by 64 hex characters".to_string(),
        );
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_channel_code_overrides() {
        let path =
            std::env::temp_dir().join(format!("atv-channel-overrides-{}.json", std::process::id()));
        fs::write(
            &path,
            r#"{
              "ch1": {"name": "News", "number": 5},
              "ch2": {"number": 9}
            }"#,
        )
        .unwrap();

        let overrides = load_channel_number_overrides(Some(&path)).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(Some(5), overrides.get("ch1").map(|entry| entry.number));
        assert_eq!(
            Some("News"),
            overrides.get("ch1").and_then(|entry| entry.name.as_deref())
        );
        assert_eq!(Some(9), overrides.get("ch2").map(|entry| entry.number));
    }

    #[test]
    fn startup_rejects_unset_admin_password() {
        for hash in [
            "",
            "sha256:replace-with-hash-from-admin-tool",
            "sha256:not-hex",
            &hash_secret("admin"),
        ] {
            let config = ProxyConfig {
                admin_password_hash: hash.to_string(),
                ..ProxyConfig::default()
            };
            assert!(config.validate_startup().is_err());
        }
    }

    #[test]
    fn startup_accepts_configured_admin_password() {
        let config = ProxyConfig {
            admin_password_hash: hash_secret("configured-password"),
            ..ProxyConfig::default()
        };
        assert!(config.validate_startup().is_ok());
    }

    #[test]
    fn pairing_config_defaults_keep_rate_limit_disabled() {
        let config: ProxyConfig = serde_json::from_str(
            r#"{
              "listen": "127.0.0.1:8088",
              "admin_password_hash": "sha256:8a012f93fc727f163d806c746dd1180e3d1fa0f1c7bfaef86a0372d1dd332d39",
              "backend_channels_url": null,
              "provider": null
            }"#,
        )
        .unwrap();

        assert_eq!(300, config.pairing.session_ttl_seconds);
        assert_eq!(2, config.pairing.poll_interval_seconds);
        assert!(!config.pairing.create_rate_limit.enabled);
        assert_eq!(60, config.pairing.create_rate_limit.window_seconds);
        assert_eq!(10, config.pairing.create_rate_limit.max_requests);
    }

    #[test]
    fn load_backfills_missing_token_ids() {
        let path =
            std::env::temp_dir().join(format!("atv-config-token-id-{}.json", std::process::id()));
        fs::write(
            &path,
            format!(
                r#"{{
                  "listen": "127.0.0.1:8088",
                  "admin_password_hash": "{}",
                  "backend_channels_url": null,
                  "provider": null,
                  "tokens": [{{
                    "name": "living-room",
                    "hash": "{}",
                    "created_at": 1,
                    "last_seen_at": null,
                    "enabled": true
                  }}]
                }}"#,
                hash_secret("configured-password"),
                hash_secret("raw-token")
            ),
        )
        .unwrap();

        let config = ProxyConfig::load_or_default(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(1, config.tokens.len());
        assert!(config.tokens[0].id.starts_with("tok_"));
    }
}
