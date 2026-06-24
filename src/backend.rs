use crate::cache::Channel;
use crate::stream::{resolve_stream_url, StreamProxyConfig};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct BackendChannel {
    number: u32,
    name: String,
    #[serde(rename = "streamUrl")]
    stream_url: String,
    #[serde(rename = "channelCode")]
    channel_code: Option<String>,
}

pub fn fetch_channels(url: &str, stream: &StreamProxyConfig) -> Result<Vec<Channel>, String> {
    let text = ureq::get(url)
        .call()
        .map_err(|e| format!("backend request failed: {e}"))?
        .into_string()
        .map_err(|e| format!("backend body failed: {e}"))?;
    parse_backend_channels(&text, stream)
}

pub fn parse_backend_channels(
    text: &str,
    stream: &StreamProxyConfig,
) -> Result<Vec<Channel>, String> {
    let raw: Vec<BackendChannel> =
        serde_json::from_str(text).map_err(|e| format!("backend channel JSON invalid: {e}"))?;
    raw.into_iter()
        .map(|ch| {
            let stream_url =
                resolve_stream_url(&ch.stream_url, stream).map_err(|e| e.to_string())?;
            Ok(Channel {
                number: ch.number,
                name: ch.name,
                stream_url,
                channel_code: ch.channel_code,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_rewrites_backend_channels() {
        let cfg = StreamProxyConfig {
            udpxy_base_url: Some("openwrt:4022".into()),
        };
        let channels = parse_backend_channels(
            r#"[{"number":1,"name":"CCTV-1","streamUrl":"igmp://239.0.0.1:8000","channelCode":"ch1"}]"#,
            &cfg,
        ).unwrap();
        assert_eq!(
            "http://openwrt:4022/udp/239.0.0.1:8000",
            channels[0].stream_url
        );
        assert_eq!(Some("ch1".to_string()), channels[0].channel_code);
    }
}
