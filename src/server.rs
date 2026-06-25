use crate::auth::{find_valid_token, generate_token, hash_secret, verify_secret, ClientToken};
use crate::backend;
use crate::cache::{now_secs, ChannelCache, EpgCache, EpgCacheKey};
use crate::config::ProxyConfig;
use crate::stream::StreamProxyConfig;
use serde::Serialize;
use std::collections::HashMap;
use std::env;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct AppState {
    config_path: PathBuf,
    config: Mutex<ProxyConfig>,
    cache: Mutex<ChannelCache>,
    epg_cache: Mutex<EpgCache>,
}

pub fn run_from_env() -> Result<(), String> {
    let config_path = env::var("ATV_PROXY_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config/local.json"));
    let config = ProxyConfig::load_or_default(&config_path).map_err(|e| e.to_string())?;
    let listen = config.listen.clone();
    let state = Arc::new(AppState {
        config_path,
        config: Mutex::new(config),
        cache: Mutex::new(ChannelCache::default()),
        epg_cache: Mutex::new(EpgCache::default()),
    });
    serve(&listen, state)
}

pub fn serve(addr: &str, state: Arc<AppState>) -> Result<(), String> {
    let listener = TcpListener::bind(addr).map_err(|e| format!("bind {addr}: {e}"))?;
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
    let mut buf = vec![0u8; 64 * 1024];
    let n = stream.read(&mut buf)?;
    if n == 0 {
        return Ok(());
    }
    let request = String::from_utf8_lossy(&buf[..n]);
    let response = handle_request(&request, state);
    stream.write_all(response.as_bytes())
}

pub fn handle_request(raw: &str, state: &AppState) -> String {
    let request = match Request::parse(raw) {
        Ok(req) => req,
        Err(msg) => return json_error(400, "bad_request", &msg),
    };
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => json(200, &serde_json::json!({"ok": true})),
        ("GET", "/admin") => admin_page(state),
        ("POST", "/admin/config") => admin_config(&request, state),
        ("POST", "/admin/tokens") => admin_create_token(&request, state),
        ("GET", "/api/v1/channels") => channels(&request, state),
        ("GET", "/api/v1/epg/day") => epg_day(&request, state),
        _ => json_error(404, "not_found", "not found"),
    }
}

fn admin_page(state: &AppState) -> String {
    let cfg = state.config.lock().unwrap();
    let html = format!(
        "<html><body><h1>ATV IPTV Proxy</h1><p>Backend: {}</p><p>udpxy: {}</p><p>Tokens: {}</p></body></html>",
        cfg.backend_channels_url.as_deref().unwrap_or("not configured"),
        cfg.stream.udpxy_base_url.as_deref().unwrap_or("not configured"),
        cfg.tokens.len()
    );
    response(200, "text/html; charset=utf-8", html)
}

fn admin_config(request: &Request, state: &AppState) -> String {
    if !authorized_admin(request, state) {
        return json_error(401, "admin_unauthorized", "admin password required");
    }
    let form = parse_form(&request.body);
    let mut cfg = state.config.lock().unwrap();
    if let Some(v) = form.get("backend_channels_url") {
        cfg.backend_channels_url = if v.trim().is_empty() {
            None
        } else {
            Some(v.trim().to_string())
        };
    }
    if let Some(v) = form.get("udpxy_base_url") {
        cfg.stream = StreamProxyConfig {
            udpxy_base_url: if v.trim().is_empty() {
                None
            } else {
                Some(v.trim().to_string())
            },
        };
    }
    if let Err(e) = cfg.save_atomic(&state.config_path) {
        return json_error(500, "config_save_failed", &e.to_string());
    }
    json(200, &serde_json::json!({"ok": true}))
}

fn admin_create_token(request: &Request, state: &AppState) -> String {
    if !authorized_admin(request, state) {
        return json_error(401, "admin_unauthorized", "admin password required");
    }
    let form = parse_form(&request.body);
    let name = form.get("name").map(String::as_str).unwrap_or("client");
    let raw = match generate_token(name) {
        Ok(token) => token,
        Err(e) => return json_error(500, "token_generation_failed", &e.to_string()),
    };
    let mut cfg = state.config.lock().unwrap();
    cfg.tokens.push(ClientToken {
        name: name.to_string(),
        hash: hash_secret(&raw),
        created_at: crate::auth::now_secs(),
        last_seen_at: None,
        enabled: true,
    });
    if let Err(e) = cfg.save_atomic(&state.config_path) {
        return json_error(500, "config_save_failed", &e.to_string());
    }
    json(200, &serde_json::json!({"token": raw}))
}

fn channels(request: &Request, state: &AppState) -> String {
    if !authorized_client(request, state) {
        return json_error(401, "unauthorized", "valid bearer token required");
    }
    let (ttl, provider, backend_url, stream_cfg) = {
        let cfg = state.config.lock().unwrap();
        (
            cfg.channel_cache_ttl_seconds,
            cfg.provider.clone(),
            cfg.backend_channels_url.clone(),
            cfg.stream.clone(),
        )
    };
    let now = now_secs();
    {
        let cache = state.cache.lock().unwrap();
        if cache.is_fresh(ttl, now) {
            if let Some(resp) = cache.response(ttl, false) {
                return json(200, &resp);
            }
        }
    }
    let fetched = if let Some(provider) = provider {
        crate::ctc::fetch_channels(&provider, &stream_cfg)
    } else if let Some(url) = backend_url {
        backend::fetch_channels(&url, &stream_cfg)
    } else {
        return json_error(
            503,
            "backend_not_configured",
            "provider or backend channel URL is not configured",
        );
    };

    match fetched {
        Ok(channels) => {
            let mut cache = state.cache.lock().unwrap();
            cache.update(channels, now);
            json(200, &cache.response(ttl, false).unwrap())
        }
        Err(e) => {
            if let Some(stale) = state.cache.lock().unwrap().response(ttl, true) {
                json(200, &stale)
            } else {
                json_error(503, "backend_unavailable", &e)
            }
        }
    }
}

fn epg_day(request: &Request, state: &AppState) -> String {
    if !authorized_client(request, state) {
        return json_error(401, "unauthorized", "valid bearer token required");
    }
    let Some(channel_code) = request.query.get("channelCode").filter(|v| !v.is_empty()) else {
        return json_error(
            400,
            "missing_channel_code",
            "channelCode query parameter is required",
        );
    };
    let date_offset = match request
        .query
        .get("dateOffset")
        .map(String::as_str)
        .unwrap_or("0")
        .parse::<i32>()
    {
        Ok(value @ -1..=1) => value,
        _ => return json_error(400, "invalid_date_offset", "dateOffset must be -1, 0, or 1"),
    };
    let key = EpgCacheKey {
        channel_code: channel_code.clone(),
        date_offset,
    };
    let (ttl, provider) = {
        let cfg = state.config.lock().unwrap();
        (cfg.epg_cache_ttl_seconds, cfg.provider.clone())
    };
    let now = now_secs();
    {
        let cache = state.epg_cache.lock().unwrap();
        if cache.is_fresh(&key, ttl, now) {
            if let Some(resp) = cache.response(&key, ttl, false) {
                return json(200, &resp);
            }
        }
    }
    let Some(provider) = provider else {
        return json_error(503, "backend_not_configured", "provider is not configured");
    };

    match crate::ctc::fetch_programs(&provider, channel_code, date_offset) {
        Ok(programs) => {
            let mut cache = state.epg_cache.lock().unwrap();
            cache.update(key.clone(), programs, now);
            json(200, &cache.response(&key, ttl, false).unwrap())
        }
        Err(e) => {
            if let Some(stale) = state.epg_cache.lock().unwrap().response(&key, ttl, true) {
                json(200, &stale)
            } else {
                json_error(503, "backend_unavailable", &e)
            }
        }
    }
}

fn authorized_admin(request: &Request, state: &AppState) -> bool {
    let Some(value) = request.headers.get("x-admin-password") else {
        return false;
    };
    let cfg = state.config.lock().unwrap();
    verify_secret(value, &cfg.admin_password_hash)
}

fn authorized_client(request: &Request, state: &AppState) -> bool {
    let Some(header) = request.headers.get("authorization") else {
        return false;
    };
    let Some(raw) = header.strip_prefix("Bearer ") else {
        return false;
    };
    let mut cfg = state.config.lock().unwrap();
    let ok = find_valid_token(&mut cfg.tokens, raw).is_some();
    if ok {
        let _ = cfg.save_atomic(&state.config_path);
    }
    ok
}

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: String,
}

impl Request {
    fn parse(raw: &str) -> Result<Self, String> {
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
        let mut lines = head.lines();
        let first = lines
            .next()
            .ok_or_else(|| "missing request line".to_string())?;
        let mut parts = first.split_whitespace();
        let method = parts
            .next()
            .ok_or_else(|| "missing method".to_string())?
            .to_string();
        let target = parts.next().ok_or_else(|| "missing target".to_string())?;
        let (path_raw, query_raw) = target.split_once('?').unwrap_or((target, ""));
        let path = path_raw.to_string();
        let mut headers = HashMap::new();
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        Ok(Self {
            method,
            path,
            query: parse_form(query_raw),
            headers,
            body: body.to_string(),
        })
    }
}

fn parse_form(body: &str) -> HashMap<String, String> {
    body.split('&')
        .filter_map(|part| part.split_once('='))
        .map(|(k, v)| (percent_decode(k), percent_decode(v)))
        .collect()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &value[i + 1..i + 3];
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn json<T: Serialize>(status: u16, value: &T) -> String {
    response(
        status,
        "application/json",
        serde_json::to_string(value).unwrap(),
    )
}

fn json_error(status: u16, code: &str, message: &str) -> String {
    json(
        status,
        &serde_json::json!({"error": {"code": code, "message": message}}),
    )
}

fn response(status: u16, content_type: &str, body: String) -> String {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::hash_secret;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    fn state(config: ProxyConfig) -> AppState {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("atv-proxy-test-{}-{id}.json", now_secs()));
        let _ = fs::remove_file(&path);
        AppState {
            config_path: path,
            config: Mutex::new(config),
            cache: Mutex::new(ChannelCache::default()),
            epg_cache: Mutex::new(EpgCache::default()),
        }
    }

    #[test]
    fn health_is_public() {
        let state = state(ProxyConfig::default());
        assert!(handle_request("GET /health HTTP/1.1\r\n\r\n", &state).contains("200 OK"));
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
}
