use crate::cache::{ChannelCache, EpgCache};
use crate::config::{ProviderConfig, ProxyConfig};
use crate::pairing::PairingStore;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct AppState {
    pub(super) config_path: PathBuf,
    pub(super) channel_number_overrides_path: Option<PathBuf>,
    pub(super) config: Mutex<ProxyConfig>,
    pub(super) cache: Mutex<ChannelCache>,
    pub(super) epg_cache: Mutex<EpgCache>,
    pub(super) ctc_session: Mutex<Option<CachedCtcSession>>,
    pub(super) pairing: Mutex<PairingStore>,
}

#[derive(Debug, Clone)]
pub(super) struct CachedCtcSession {
    pub(super) provider: ProviderConfig,
    pub(super) session: crate::ctc::LoginSession,
}

impl AppState {
    pub(super) fn new(
        config_path: PathBuf,
        channel_number_overrides_path: Option<PathBuf>,
        config: ProxyConfig,
    ) -> Self {
        Self {
            config_path,
            channel_number_overrides_path,
            config: Mutex::new(config),
            cache: Mutex::new(ChannelCache::default()),
            epg_cache: Mutex::new(EpgCache::default()),
            ctc_session: Mutex::new(None),
            pairing: Mutex::new(PairingStore::default()),
        }
    }
}
