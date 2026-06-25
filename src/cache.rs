use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Program {
    pub code: String,
    pub name: String,
    pub start: String,
    pub end: String,
    #[serde(rename = "isLive")]
    pub is_live: bool,
    #[serde(rename = "isReplayable")]
    pub is_replayable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EpgResponse {
    pub data: Vec<Program>,
    pub cache: CacheMeta,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelCache {
    channels: Vec<Channel>,
    cached_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EpgCacheKey {
    pub channel_code: String,
    pub date_offset: i32,
}

#[derive(Debug, Clone)]
struct EpgCacheEntry {
    programs: Vec<Program>,
    cached_at: u64,
}

#[derive(Debug, Clone, Default)]
pub struct EpgCache {
    entries: HashMap<EpgCacheKey, EpgCacheEntry>,
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

impl EpgCache {
    pub fn is_fresh(&self, key: &EpgCacheKey, ttl_seconds: u64, now: u64) -> bool {
        self.entries
            .get(key)
            .map(|entry| now.saturating_sub(entry.cached_at) <= ttl_seconds)
            .unwrap_or(false)
    }

    pub fn update(&mut self, key: EpgCacheKey, programs: Vec<Program>, now: u64) {
        self.entries.insert(
            key,
            EpgCacheEntry {
                programs,
                cached_at: now,
            },
        );
    }

    pub fn response(
        &self,
        key: &EpgCacheKey,
        ttl_seconds: u64,
        stale: bool,
    ) -> Option<EpgResponse> {
        let entry = self.entries.get(key)?;
        Some(EpgResponse {
            data: entry.programs.clone(),
            cache: CacheMeta {
                stale,
                cached_at: entry.cached_at,
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

    #[test]
    fn epg_cache_tracks_keys_and_stale_response() {
        let mut cache = EpgCache::default();
        let key = EpgCacheKey {
            channel_code: "ch1".into(),
            date_offset: 0,
        };
        cache.update(
            key.clone(),
            vec![Program {
                code: "p1".into(),
                name: "News".into(),
                start: "2026-06-07T08:00:00+08:00".into(),
                end: "2026-06-07T09:00:00+08:00".into(),
                is_live: true,
                is_replayable: false,
            }],
            100,
        );

        assert!(cache.is_fresh(&key, 10, 109));
        assert!(!cache.is_fresh(&key, 10, 111));
        let resp = cache.response(&key, 10, true).unwrap();
        assert!(resp.cache.stale);
        assert_eq!("p1", resp.data[0].code);
    }
}
