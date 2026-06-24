use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Channel {
    pub number: u32,
    pub name: String,
    #[serde(rename = "streamUrl")]
    pub stream_url: String,
    #[serde(rename = "channelCode", skip_serializing_if = "Option::is_none")]
    pub channel_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheMeta {
    pub stale: bool,
    #[serde(rename = "cachedAt")]
    pub cached_at: u64,
    #[serde(rename = "ttlSeconds")]
    pub ttl_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelResponse {
    pub data: Vec<Channel>,
    pub cache: CacheMeta,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelCache {
    channels: Vec<Channel>,
    cached_at: u64,
}

impl ChannelCache {
    pub fn is_fresh(&self, ttl_seconds: u64, now: u64) -> bool {
        !self.channels.is_empty() && now.saturating_sub(self.cached_at) <= ttl_seconds
    }

    pub fn update(&mut self, channels: Vec<Channel>, now: u64) {
        self.channels = channels;
        self.cached_at = now;
    }

    pub fn response(&self, ttl_seconds: u64, stale: bool) -> Option<ChannelResponse> {
        if self.channels.is_empty() {
            return None;
        }
        Some(ChannelResponse {
            data: self.channels.clone(),
            cache: CacheMeta {
                stale,
                cached_at: self.cached_at,
                ttl_seconds,
            },
        })
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_tracks_ttl() {
        let mut cache = ChannelCache::default();
        cache.update(
            vec![Channel {
                number: 1,
                name: "A".into(),
                stream_url: "http://x".into(),
                channel_code: None,
            }],
            100,
        );
        assert!(cache.is_fresh(10, 109));
        assert!(!cache.is_fresh(10, 111));
    }
}
