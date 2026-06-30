mod admin;
mod api;
mod authz;
mod context;
mod ctc_session;
mod http;
mod policies;
mod state;

pub use state::AppState;

use crate::config::ProxyConfig;
use crate::pairing::PairingError;
use context::RequestContext;
use http::{json, json_error, response_status, Request};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerArgs {
    pub config_path: PathBuf,
    pub channel_number_overrides_path: Option<PathBuf>,
}

impl Default for ServerArgs {
    fn default() -> Self {
        Self {
            config_path: PathBuf::new(),
            channel_number_overrides_path: None,
        }
    }
}

pub fn parse_args<I>(args: I) -> Result<ServerArgs, String>
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    let mut parsed = ServerArgs::default();
    let mut config_path = None;
    let mut args = args.into_iter().map(Into::into);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--config requires a path".to_string())?;
                config_path = Some(PathBuf::from(value));
            }
            "--channel-number-overrides" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--channel-number-overrides requires a path".to_string())?;
                parsed.channel_number_overrides_path = Some(PathBuf::from(value));
            }
            "--help" | "-h" => return Err(usage()),
            _ if arg.starts_with("--config=") => {
                config_path = Some(PathBuf::from(&arg["--config=".len()..]));
            }
            _ if arg.starts_with("--channel-number-overrides=") => {
                parsed.channel_number_overrides_path =
                    Some(PathBuf::from(&arg["--channel-number-overrides=".len()..]));
            }
            _ => return Err(format!("unknown argument: {arg}\n{}", usage())),
        }
    }

    parsed.config_path = config_path.ok_or_else(|| format!("--config is required\n{}", usage()))?;

    Ok(parsed)
}

pub fn usage() -> String {
    "usage: atv-iptv-proxy --config PATH [--channel-number-overrides PATH]".to_string()
}

pub fn run(args: ServerArgs) -> Result<(), String> {
    let config_path = args.config_path;
    let override_path = args.channel_number_overrides_path;
    let config = ProxyConfig::load_or_default(&config_path).map_err(|e| e.to_string())?;
    config.validate_startup()?;
    let listen = config.listen.clone();
    eprintln!(
        "startup: config={} channel_number_overrides={} listen={} provider_configured={} backend_configured={} tokens={}",
        config_path.display(),
        override_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not configured".to_string()),
        listen,
        config.provider.is_some(),
        config.backend_channels_url.is_some(),
        config.tokens.len()
    );
    let state = Arc::new(AppState::new(config_path, override_path, config));
    serve(&listen, state)
}

pub fn serve(addr: &str, state: Arc<AppState>) -> Result<(), String> {
    let listener = TcpListener::bind(addr).map_err(|e| format!("bind {addr}: {e}"))?;
    eprintln!("server: listening on {addr}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = Arc::clone(&state);
                std::thread::spawn(move || {
                    let _ = handle_stream(stream, &state);
                });
            }
            Err(e) => eprintln!("accept failed: {e}"),
        }
    }
    Ok(())
}

fn handle_stream(mut stream: TcpStream, state: &AppState) -> std::io::Result<()> {
    let peer_addr = stream.peer_addr().ok();
    let peer = peer_addr
        .map(|addr| addr.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let mut buf = vec![0u8; 64 * 1024];
    let n = stream.read(&mut buf)?;
    if n == 0 {
        eprintln!("request: peer={peer} empty_read=true");
        return Ok(());
    }
    let request = String::from_utf8_lossy(&buf[..n]);
    let response = handle_request_with_context(&request, peer_addr, state);
    eprintln!(
        "response: peer={} status={} bytes={}",
        peer,
        response_status(&response),
        response.len()
    );
    stream.write_all(response.as_bytes())
}

pub fn handle_request(raw: &str, state: &AppState) -> String {
    handle_request_with_context(raw, None, state)
}

pub fn handle_request_with_context(
    raw: &str,
    peer_addr: Option<SocketAddr>,
    state: &AppState,
) -> String {
    let started = Instant::now();
    let request = match Request::parse(raw) {
        Ok(req) => req,
        Err(msg) => {
            eprintln!("request: parse_failed error={msg}");
            return json_error(400, "bad_request", &msg);
        }
    };
    let ctx = RequestContext::new(request, peer_addr);
    eprintln!(
        "request: method={} path={}",
        ctx.request.method, ctx.request.path
    );
    let response = match (ctx.request.method.as_str(), ctx.request.path.as_str()) {
        ("GET", "/health") => json(200, &serde_json::json!({"ok": true})),
        ("GET", "/admin") => admin::page(),
        ("GET", "/admin/api/v1/status") => admin::status(&ctx.request, state),
        ("GET", "/admin/api/v1/config") => admin::config_get(&ctx.request, state),
        ("GET", "/admin/api/v1/tokens") => admin::tokens_get(&ctx.request, state),
        ("POST", "/admin/config") => admin::config_update(&ctx.request, state),
        ("POST", "/admin/tokens") => admin::create_token(&ctx.request, state),
        ("DELETE", "/admin/tokens") => admin::delete_token(&ctx.request, state),
        ("GET", "/admin/api/v1/pairing/sessions") => admin::pairing_sessions(&ctx.request, state),
        ("POST", "/admin/api/v1/pairing/approve") => admin::pairing_approve(&ctx.request, state),
        ("POST", "/admin/api/v1/pairing/reject") => admin::pairing_reject(&ctx.request, state),
        ("POST", "/api/v1/pairing/sessions") => api::pairing_create(&ctx, state),
        ("GET", "/api/v1/channels") => api::channels(&ctx.request, state),
        ("GET", "/api/v1/epg/day") => api::epg_day(&ctx.request, state),
        _ if ctx.request.method == "GET"
            && ctx.request.path.starts_with("/api/v1/pairing/sessions/") =>
        {
            api::pairing_poll(&ctx.request, state)
        }
        _ => json_error(404, "not_found", "not found"),
    };
    eprintln!(
        "request: method={} path={} status={} elapsed_ms={}",
        ctx.request.method,
        ctx.request.path,
        response_status(&response),
        started.elapsed().as_millis()
    );
    response
}

fn pairing_error(error: PairingError) -> String {
    json_error(error.status, error.code, error.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{hash_secret, ClientToken};
    use crate::cache::{now_secs, ChannelCache, EpgCache, EpgCacheKey};
    use crate::config::{ProviderConfig, ProxyConfig};
    use crate::pairing::PairingStore;
    use crate::server::state::CachedCtcSession;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    fn state(config: ProxyConfig) -> AppState {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("atv-proxy-test-{}-{id}.json", now_secs()));
        let _ = fs::remove_file(&path);
        AppState {
            config_path: path,
            channel_number_overrides_path: None,
            config: Mutex::new(config),
            cache: Mutex::new(ChannelCache::default()),
            epg_cache: Mutex::new(EpgCache::default()),
            ctc_session: Mutex::new(None),
            pairing: Mutex::new(PairingStore::default()),
        }
    }

    fn response_body(response: &str) -> &str {
        response
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .unwrap_or("")
    }

    #[test]
    fn health_is_public() {
        let state = state(ProxyConfig::default());
        assert!(handle_request("GET /health HTTP/1.1\r\n\r\n", &state).contains("200 OK"));
    }

    #[test]
    fn admin_page_is_served() {
        let state = state(ProxyConfig::default());
        let resp = handle_request("GET /admin HTTP/1.1\r\n\r\n", &state);
        assert!(resp.contains("200 OK"));
        assert!(resp.contains("text/html; charset=utf-8"));
        assert!(resp.contains("ATV IPTV Proxy Admin"));
        assert!(resp.contains("id=\"loginPage\""));
        assert!(resp.contains("id=\"adminPage\" hidden"));
        assert!(resp.contains("sessionStorage.setItem(PASSWORD_KEY"));
    }

    #[test]
    fn parses_startup_paths() {
        let args = parse_args([
            "--config",
            "/etc/atv-iptv-proxy/config.json",
            "--channel-number-overrides=/etc/atv-iptv-proxy/channel-number-overrides.json",
        ])
        .unwrap();

        assert_eq!(
            PathBuf::from("/etc/atv-iptv-proxy/config.json"),
            args.config_path
        );
        assert_eq!(
            Some(PathBuf::from(
                "/etc/atv-iptv-proxy/channel-number-overrides.json"
            )),
            args.channel_number_overrides_path
        );
    }

    #[test]
    fn startup_config_path_is_required() {
        let err = parse_args(std::iter::empty::<&str>()).unwrap_err();
        assert!(err.contains("--config is required"));
    }

    #[test]
    fn channels_requires_token() {
        let state = state(ProxyConfig::default());
        let resp = handle_request("GET /api/v1/channels HTTP/1.1\r\n\r\n", &state);
        assert!(resp.contains("401 Unauthorized"));
    }

    #[test]
    fn epg_requires_token() {
        let state = state(ProxyConfig::default());
        let resp = handle_request(
            "GET /api/v1/epg/day?channelCode=ch1 HTTP/1.1\r\n\r\n",
            &state,
        );
        assert!(resp.contains("401 Unauthorized"));
    }

    #[test]
    fn admin_can_create_token() {
        let state = state(ProxyConfig {
            admin_password_hash: hash_secret("pw"),
            ..ProxyConfig::default()
        });
        let resp = handle_request(
            "POST /admin/tokens HTTP/1.1\r\nx-admin-password: pw\r\n\r\nname=living-room",
            &state,
        );
        assert!(resp.contains("200 OK"));
        assert!(resp.contains("atv_living-room_"));
        assert_eq!(1, state.config.lock().unwrap().tokens.len());
    }

    #[test]
    fn admin_config_response_redacts_provider_password() {
        let state = state(ProxyConfig {
            admin_password_hash: hash_secret("pw"),
            provider: Some(ProviderConfig {
                user_id: "user".into(),
                password: "secret-password".into(),
                stb_id: "stb".into(),
                local_ip: "192.0.2.10".into(),
                local_mac: "00:11:22:33:44:55".into(),
                auth_server_url: "http://auth.example".into(),
            }),
            ..ProxyConfig::default()
        });

        let resp = handle_request(
            "GET /admin/api/v1/config HTTP/1.1\r\nx-admin-password: pw\r\n\r\n",
            &state,
        );

        assert!(resp.contains("200 OK"));
        assert!(resp.contains("\"passwordConfigured\":true"));
        assert!(!resp.contains("secret-password"));
    }

    #[test]
    fn admin_token_list_does_not_expose_hashes() {
        let mut cfg = ProxyConfig {
            admin_password_hash: hash_secret("pw"),
            ..ProxyConfig::default()
        };
        cfg.tokens.push(ClientToken {
            name: "living-room".into(),
            hash: hash_secret("raw-token"),
            created_at: 1,
            last_seen_at: Some(2),
            enabled: true,
        });
        let state = state(cfg);

        let resp = handle_request(
            "GET /admin/api/v1/tokens HTTP/1.1\r\nx-admin-password: pw\r\n\r\n",
            &state,
        );

        assert!(resp.contains("200 OK"));
        assert!(resp.contains("living-room"));
        assert!(resp.contains("lastSeenAt"));
        assert!(!resp.contains("sha256:"));
        assert!(!resp.contains("raw-token"));
    }

    #[test]
    fn admin_config_save_preserves_blank_provider_password() {
        let state = state(ProxyConfig {
            admin_password_hash: hash_secret("pw"),
            provider: Some(ProviderConfig {
                user_id: "old-user".into(),
                password: "keep-me".into(),
                stb_id: "old-stb".into(),
                local_ip: "192.0.2.10".into(),
                local_mac: "00:11:22:33:44:55".into(),
                auth_server_url: "http://old-auth.example".into(),
            }),
            ..ProxyConfig::default()
        });

        let resp = handle_request(
            "POST /admin/config HTTP/1.1\r\nx-admin-password: pw\r\n\r\nprovider_user_id=new-user&provider_password=&provider_stb_id=new-stb&provider_local_ip=192.0.2.11&provider_local_mac=00:11:22:33:44:66&provider_auth_server_url=http%3A%2F%2Fnew-auth.example&channel_cache_ttl_seconds=120&epg_cache_ttl_seconds=30",
            &state,
        );

        assert!(resp.contains("200 OK"));
        let cfg = state.config.lock().unwrap();
        let provider = cfg.provider.as_ref().unwrap();
        assert_eq!("new-user", provider.user_id);
        assert_eq!("keep-me", provider.password);
        assert_eq!("new-stb", provider.stb_id);
        assert_eq!(120, cfg.channel_cache_ttl_seconds);
        assert_eq!(30, cfg.epg_cache_ttl_seconds);
    }

    #[test]
    fn admin_can_delete_token_by_name() {
        let mut cfg = ProxyConfig {
            admin_password_hash: hash_secret("pw"),
            ..ProxyConfig::default()
        };
        cfg.tokens.push(ClientToken {
            name: "living-room".into(),
            hash: hash_secret("token-1"),
            created_at: 1,
            last_seen_at: None,
            enabled: true,
        });
        cfg.tokens.push(ClientToken {
            name: "bedroom".into(),
            hash: hash_secret("token-2"),
            created_at: 1,
            last_seen_at: None,
            enabled: true,
        });
        let state = state(cfg);

        let resp = handle_request(
            "DELETE /admin/tokens?name=living-room HTTP/1.1\r\nx-admin-password: pw\r\n\r\n",
            &state,
        );

        assert!(resp.contains("200 OK"));
        assert!(resp.contains("\"deletedCount\":1"));
        let tokens = &state.config.lock().unwrap().tokens;
        assert_eq!(1, tokens.len());
        assert_eq!("bedroom", tokens[0].name);
    }

    #[test]
    fn admin_delete_token_requires_name() {
        let state = state(ProxyConfig {
            admin_password_hash: hash_secret("pw"),
            ..ProxyConfig::default()
        });

        let resp = handle_request(
            "DELETE /admin/tokens HTTP/1.1\r\nx-admin-password: pw\r\n\r\n",
            &state,
        );

        assert!(resp.contains("400 Bad Request"));
        assert!(resp.contains("missing_token_name"));
    }

    #[test]
    fn admin_delete_token_returns_not_found() {
        let state = state(ProxyConfig {
            admin_password_hash: hash_secret("pw"),
            ..ProxyConfig::default()
        });

        let resp = handle_request(
            "DELETE /admin/tokens?name=missing HTTP/1.1\r\nx-admin-password: pw\r\n\r\n",
            &state,
        );

        assert!(resp.contains("404 Not Found"));
        assert!(resp.contains("token_not_found"));
    }

    #[test]
    fn pairing_approve_by_code_creates_client_token() {
        let state = state(ProxyConfig {
            admin_password_hash: hash_secret("pw"),
            ..ProxyConfig::default()
        });

        let create = handle_request(
            "POST /api/v1/pairing/sessions HTTP/1.1\r\ncontent-type: application/json\r\n\r\n{\"deviceName\":\"Living Room ATV\",\"deviceType\":\"android_tv\",\"appId\":\"com.example.atv\",\"appVersion\":\"1\",\"clientNonce\":\"nonce\"}",
            &state,
        );
        assert!(create.contains("200 OK"));
        let created: serde_json::Value = serde_json::from_str(response_body(&create)).unwrap();
        let session_id = created["sessionId"].as_str().unwrap();
        let pairing_code = created["pairingCode"].as_str().unwrap();

        let list = handle_request(
            "GET /admin/api/v1/pairing/sessions?status=pending HTTP/1.1\r\nx-admin-password: pw\r\n\r\n",
            &state,
        );
        assert!(list.contains("200 OK"));
        assert!(list.contains(pairing_code));
        assert!(list.contains("Living Room ATV"));

        let approve = handle_request(
            &format!(
                "POST /admin/api/v1/pairing/approve HTTP/1.1\r\nx-admin-password: pw\r\ncontent-type: application/json\r\n\r\n{{\"pairingCode\":\"{}\",\"deviceLabel\":\"Den TV\"}}",
                pairing_code
            ),
            &state,
        );
        assert!(approve.contains("200 OK"));
        assert_eq!(1, state.config.lock().unwrap().tokens.len());
        assert_eq!("Den TV", state.config.lock().unwrap().tokens[0].name);

        let poll = handle_request(
            &format!(
                "GET /api/v1/pairing/sessions/{} HTTP/1.1\r\nx-client-nonce: nonce\r\n\r\n",
                session_id
            ),
            &state,
        );
        assert!(poll.contains("200 OK"));
        assert!(poll.contains("\"status\":\"approved\""));
        assert!(poll.contains("atv_dentv_"));
    }

    #[test]
    fn pairing_reject_by_code_is_visible_to_client() {
        let state = state(ProxyConfig {
            admin_password_hash: hash_secret("pw"),
            ..ProxyConfig::default()
        });
        let create = handle_request(
            "POST /api/v1/pairing/sessions HTTP/1.1\r\ncontent-type: application/json\r\n\r\n{\"clientNonce\":\"nonce\"}",
            &state,
        );
        let created: serde_json::Value = serde_json::from_str(response_body(&create)).unwrap();
        let session_id = created["sessionId"].as_str().unwrap();
        let pairing_code = created["pairingCode"].as_str().unwrap();

        let reject = handle_request(
            &format!(
                "POST /admin/api/v1/pairing/reject HTTP/1.1\r\nx-admin-password: pw\r\ncontent-type: application/json\r\n\r\n{{\"pairingCode\":\"{}\"}}",
                pairing_code
            ),
            &state,
        );
        assert!(reject.contains("200 OK"));

        let poll = handle_request(
            &format!(
                "GET /api/v1/pairing/sessions/{} HTTP/1.1\r\nx-client-nonce: nonce\r\n\r\n",
                session_id
            ),
            &state,
        );
        assert!(poll.contains("\"status\":\"rejected\""));
    }

    #[test]
    fn pairing_create_accepts_context_peer_metadata() {
        let state = state(ProxyConfig::default());
        let resp = handle_request_with_context(
            "POST /api/v1/pairing/sessions HTTP/1.1\r\ncontent-type: application/json\r\n\r\n{\"clientNonce\":\"nonce\"}",
            Some("192.0.2.55:12345".parse().unwrap()),
            &state,
        );

        assert!(resp.contains("200 OK"));
        assert!(resp.contains("pairingCode"));
    }

    #[test]
    fn fresh_cache_is_served() {
        let mut cfg = ProxyConfig::default();
        let raw = "token";
        cfg.tokens.push(ClientToken {
            name: "tv".into(),
            hash: hash_secret(raw),
            created_at: 1,
            last_seen_at: None,
            enabled: true,
        });
        let state = state(cfg);
        state.cache.lock().unwrap().update(
            vec![crate::cache::Channel {
                number: 1,
                name: "A".into(),
                stream_url: "http://x".into(),
                channel_code: None,
            }],
            now_secs(),
        );
        let resp = handle_request(
            "GET /api/v1/channels HTTP/1.1\r\nauthorization: Bearer token\r\n\r\n",
            &state,
        );
        assert!(resp.contains("200 OK"));
        assert!(resp.contains("\"name\":\"A\""));
    }

    #[test]
    fn final_channel_numbers_are_unique() {
        let channels = api::ensure_unique_channel_numbers(vec![
            crate::cache::Channel {
                number: 1,
                name: "A".into(),
                stream_url: "http://a".into(),
                channel_code: Some("a".into()),
            },
            crate::cache::Channel {
                number: 1,
                name: "B".into(),
                stream_url: "http://b".into(),
                channel_code: Some("b".into()),
            },
            crate::cache::Channel {
                number: 2,
                name: "C".into(),
                stream_url: "http://c".into(),
                channel_code: Some("c".into()),
            },
        ]);

        assert_eq!(
            vec![1, 3, 2],
            channels.iter().map(|ch| ch.number).collect::<Vec<_>>()
        );
    }

    #[test]
    fn channels_sort_by_number_before_caching() {
        let channels = api::sort_channels_by_number(vec![
            crate::cache::Channel {
                number: 3,
                name: "C".into(),
                stream_url: "http://c".into(),
                channel_code: Some("c".into()),
            },
            crate::cache::Channel {
                number: 1,
                name: "A".into(),
                stream_url: "http://a".into(),
                channel_code: Some("a".into()),
            },
            crate::cache::Channel {
                number: 2,
                name: "B".into(),
                stream_url: "http://b".into(),
                channel_code: Some("b".into()),
            },
        ]);

        assert_eq!(
            vec![1, 2, 3],
            channels.iter().map(|ch| ch.number).collect::<Vec<_>>()
        );
    }

    #[test]
    fn epg_validates_query() {
        let mut cfg = ProxyConfig::default();
        cfg.tokens.push(ClientToken {
            name: "tv".into(),
            hash: hash_secret("token"),
            created_at: 1,
            last_seen_at: None,
            enabled: true,
        });
        let state = state(cfg);

        let missing = handle_request(
            "GET /api/v1/epg/day HTTP/1.1\r\nauthorization: Bearer token\r\n\r\n",
            &state,
        );
        assert!(missing.contains("400 Bad Request"));
        assert!(missing.contains("missing_channel_code"));

        let invalid = handle_request(
            "GET /api/v1/epg/day?channelCode=ch1&dateOffset=2 HTTP/1.1\r\nauthorization: Bearer token\r\n\r\n",
            &state,
        );
        assert!(invalid.contains("400 Bad Request"));
        assert!(invalid.contains("invalid_date_offset"));
    }

    #[test]
    fn fresh_epg_cache_is_served() {
        let mut cfg = ProxyConfig::default();
        cfg.tokens.push(ClientToken {
            name: "tv".into(),
            hash: hash_secret("token"),
            created_at: 1,
            last_seen_at: None,
            enabled: true,
        });
        let state = state(cfg);
        let key = EpgCacheKey {
            channel_code: "ch1".into(),
            date_offset: 0,
        };
        state.epg_cache.lock().unwrap().update(
            key,
            vec![crate::cache::Program {
                code: "p1".into(),
                name: "News".into(),
                start: "2026-06-07T08:00:00+08:00".into(),
                end: "2026-06-07T09:00:00+08:00".into(),
                is_live: true,
                is_replayable: false,
            }],
            now_secs(),
        );

        let resp = handle_request(
            "GET /api/v1/epg/day?channelCode=ch1&dateOffset=0 HTTP/1.1\r\nauthorization: Bearer token\r\n\r\n",
            &state,
        );
        assert!(resp.contains("200 OK"));
        assert!(resp.contains("\"code\":\"p1\""));
        assert!(resp.contains("\"isLive\":true"));
    }

    #[test]
    fn cached_ctc_session_reuses_matching_provider() {
        let provider = ProviderConfig {
            user_id: "u1".into(),
            password: "pw".into(),
            stb_id: "stb".into(),
            local_ip: "192.0.2.1".into(),
            local_mac: "00:11:22:33:44:55".into(),
            auth_server_url: "http://auth".into(),
        };
        let state = state(ProxyConfig::default());
        *state.ctc_session.lock().unwrap() = Some(CachedCtcSession {
            provider: provider.clone(),
            session: crate::ctc::LoginSession {
                epg_lb_base: "http://epg/iptvepg/function/".into(),
                jsession_id: "J1".into(),
                user_token: "UT1".into(),
                user_id: provider.user_id.clone(),
            },
        });

        let session = ctc_session::cached_ctc_session(&state, &provider).unwrap();

        assert_eq!("J1", session.jsession_id);
    }

    #[test]
    fn clear_ctc_session_keeps_different_provider_session() {
        let provider = ProviderConfig {
            user_id: "u1".into(),
            password: "pw".into(),
            stb_id: "stb".into(),
            local_ip: "192.0.2.1".into(),
            local_mac: "00:11:22:33:44:55".into(),
            auth_server_url: "http://auth".into(),
        };
        let other_provider = ProviderConfig {
            user_id: "u2".into(),
            ..provider.clone()
        };
        let state = state(ProxyConfig::default());
        *state.ctc_session.lock().unwrap() = Some(CachedCtcSession {
            provider: other_provider,
            session: crate::ctc::LoginSession {
                epg_lb_base: "http://epg/iptvepg/function/".into(),
                jsession_id: "J2".into(),
                user_token: "UT2".into(),
                user_id: "u2".into(),
            },
        });

        ctc_session::clear_ctc_session(&state, &provider);

        assert_eq!(
            "J2",
            state
                .ctc_session
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .session
                .jsession_id
        );
    }
}
