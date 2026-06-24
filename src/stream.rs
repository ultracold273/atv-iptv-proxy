use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamProxyConfig {
    pub udpxy_base_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamResolveError {
    MissingUdpxy { source_url: String },
}

impl std::fmt::Display for StreamResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingUdpxy { source_url } => {
                write!(f, "udpxy is required for multicast stream {source_url}")
            }
        }
    }
}

impl std::error::Error for StreamResolveError {}

pub fn resolve_stream_url(
    source_url: &str,
    config: &StreamProxyConfig,
) -> Result<String, StreamResolveError> {
    let Some((scheme, address)) = multicast_parts(source_url) else {
        return Ok(source_url.to_string());
    };
    let Some(base) = config
        .udpxy_base_url
        .as_deref()
        .map(normalize_udpxy)
        .filter(|s| !s.is_empty())
    else {
        return Err(StreamResolveError::MissingUdpxy {
            source_url: format!("{scheme}://{address}"),
        });
    };
    Ok(format!("{base}/udp/{address}"))
}

fn multicast_parts(source_url: &str) -> Option<(&'static str, &str)> {
    let lower = source_url.to_ascii_lowercase();
    if lower.starts_with("igmp://") {
        Some(("igmp", &source_url[7..]))
    } else if lower.starts_with("rtp://") {
        Some(("rtp", &source_url[6..]))
    } else {
        None
    }
}

fn normalize_udpxy(value: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_igmp_through_udpxy() {
        let cfg = StreamProxyConfig {
            udpxy_base_url: Some("openwrt:4022".into()),
        };
        assert_eq!(
            "http://openwrt:4022/udp/239.1.1.1:8000",
            resolve_stream_url("igmp://239.1.1.1:8000", &cfg).unwrap()
        );
    }

    #[test]
    fn rewrites_rtp_and_tolerates_scheme_and_slash() {
        let cfg = StreamProxyConfig {
            udpxy_base_url: Some("http://openwrt:4022/".into()),
        };
        assert_eq!(
            "http://openwrt:4022/udp/239.1.1.2:8000",
            resolve_stream_url("rtp://239.1.1.2:8000", &cfg).unwrap()
        );
    }

    #[test]
    fn passes_http_through() {
        let cfg = StreamProxyConfig::default();
        assert_eq!(
            "http://example.test/live.m3u8",
            resolve_stream_url("http://example.test/live.m3u8", &cfg).unwrap()
        );
    }

    #[test]
    fn multicast_without_udpxy_is_error() {
        let err =
            resolve_stream_url("igmp://239.1.1.1:8000", &StreamProxyConfig::default()).unwrap_err();
        assert!(err.to_string().contains("udpxy"));
    }
}
