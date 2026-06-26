use crate::auth::{find_valid_token, generate_token, hash_secret, verify_secret, ClientToken};
use crate::backend;
use crate::cache::{now_secs, ChannelCache, EpgCache, EpgCacheKey};
use crate::config::{load_channel_number_overrides, ProxyConfig};
use crate::stream::StreamProxyConfig;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct AppState {
    config_path: PathBuf,
    channel_number_overrides_path: Option<PathBuf>,
    config: Mutex<ProxyConfig>,
    cache: Mutex<ChannelCache>,
    epg_cache: Mutex<EpgCache>,
}

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
    let state = Arc::new(AppState {
        config_path,
        channel_number_overrides_path: override_path,
        config: Mutex::new(config),
        cache: Mutex::new(ChannelCache::default()),
        epg_cache: Mutex::new(EpgCache::default()),
    });
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
    let peer = stream
        .peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let mut buf = vec![0u8; 64 * 1024];
    let n = stream.read(&mut buf)?;
    if n == 0 {
        eprintln!("request: peer={peer} empty_read=true");
        return Ok(());
    }
    let request = String::from_utf8_lossy(&buf[..n]);
    let response = handle_request(&request, state);
    eprintln!(
        "response: peer={} status={} bytes={}",
        peer,
        response_status(&response),
        response.len()
    );
    stream.write_all(response.as_bytes())
}

pub fn handle_request(raw: &str, state: &AppState) -> String {
    let started = Instant::now();
    let request = match Request::parse(raw) {
        Ok(req) => req,
        Err(msg) => {
            eprintln!("request: parse_failed error={msg}");
            return json_error(400, "bad_request", &msg);
        }
    };
    eprintln!("request: method={} path={}", request.method, request.path);
    let response = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => json(200, &serde_json::json!({"ok": true})),
        ("GET", "/admin") => admin_page(state),
        ("POST", "/admin/config") => admin_config(&request, state),
        ("POST", "/admin/tokens") => admin_create_token(&request, state),
        ("GET", "/api/v1/channels") => channels(&request, state),
        ("GET", "/api/v1/epg/day") => epg_day(&request, state),
        _ => json_error(404, "not_found", "not found"),
    };
    eprintln!(
        "request: method={} path={} status={} elapsed_ms={}",
        request.method,
        request.path,
        response_status(&response),
        started.elapsed().as_millis()
    );
    response
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
        eprintln!("admin_config: unauthorized");
        return json_error(401, "admin_unauthorized", "admin password required");
    }
    let form = parse_form(&request.body);
    let mut cfg = state.config.lock().unwrap();
    let mut updated = Vec::new();
    if let Some(v) = form.get("backend_channels_url") {
        cfg.backend_channels_url = if v.trim().is_empty() {
            None
        } else {
            Some(v.trim().to_string())
        };
        updated.push("backend_channels_url");
    }
    if let Some(v) = form.get("udpxy_base_url") {
        cfg.stream = StreamProxyConfig {
            udpxy_base_url: if v.trim().is_empty() {
                None
            } else {
                Some(v.trim().to_string())
            },
        };
        updated.push("udpxy_base_url");
    }
    if let Err(e) = cfg.save_atomic(&state.config_path) {
        eprintln!("admin_config: save_failed error={e}");
        return json_error(500, "config_save_failed", &e.to_string());
    }
    eprintln!("admin_config: saved fields={}", updated.join(","));
    json(200, &serde_json::json!({"ok": true}))
}

fn admin_create_token(request: &Request, state: &AppState) -> String {
    if !authorized_admin(request, state) {
        eprintln!("admin_token: unauthorized");
        return json_error(401, "admin_unauthorized", "admin password required");
    }
    let form = parse_form(&request.body);
    let name = form.get("name").map(String::as_str).unwrap_or("client");
    let raw = match generate_token(name) {
        Ok(token) => token,
        Err(e) => {
            eprintln!("admin_token: generation_failed name={name} error={e}");
            return json_error(500, "token_generation_failed", &e.to_string());
        }
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
        eprintln!("admin_token: save_failed name={name} error={e}");
        return json_error(500, "config_save_failed", &e.to_string());
    }
    eprintln!(
        "admin_token: created name={name} total_tokens={}",
        cfg.tokens.len()
    );
    json(200, &serde_json::json!({"token": raw}))
}

fn channels(request: &Request, state: &AppState) -> String {
    let Some(client_name) = authorized_client_name(request, state) else {
        eprintln!("channels: unauthorized");
        return json_error(401, "unauthorized", "valid bearer token required");
    };
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
                eprintln!(
                    "channels: cache_hit client={} count={} ttl_seconds={}",
                    client_name,
                    resp.data.len(),
                    ttl
                );
                return json(200, &resp);
            }
        }
    }
    let fetched = if let Some(provider) = provider {
        eprintln!(
            "channels: cache_miss client={} source=provider",
            client_name
        );
        match load_channel_number_overrides(state.channel_number_overrides_path.as_deref()) {
            Ok(overrides) => crate::ctc::fetch_channels(&provider, &stream_cfg, &overrides),
            Err(e) => Err(format!("channel number overrides load failed: {e}")),
        }
    } else if let Some(url) = backend_url {
        eprintln!(
            "channels: cache_miss client={} source=backend_url",
            client_name
        );
        backend::fetch_channels(&url, &stream_cfg)
    } else {
        eprintln!("channels: backend_not_configured client={client_name}");
        return json_error(
            503,
            "backend_not_configured",
            "provider or backend channel URL is not configured",
        );
    };

    match fetched {
        Ok(channels) => {
            let channels = sort_channels_by_number(ensure_unique_channel_numbers(channels));
            let count = channels.len();
            let mut cache = state.cache.lock().unwrap();
            cache.update(channels, now);
            eprintln!(
                "channels: refresh_ok client={} count={}",
                client_name, count
            );
            json(200, &cache.response(ttl, false).unwrap())
        }
        Err(e) => {
            if let Some(stale) = state.cache.lock().unwrap().response(ttl, true) {
                eprintln!(
                    "channels: refresh_failed_serving_stale client={} count={} error={}",
                    client_name,
                    stale.data.len(),
                    e
                );
                json(200, &stale)
            } else {
                eprintln!(
                    "channels: refresh_failed_no_cache client={} error={}",
                    client_name, e
                );
                json_error(503, "backend_unavailable", &e)
            }
        }
    }
}

fn ensure_unique_channel_numbers(
    mut channels: Vec<crate::cache::Channel>,
) -> Vec<crate::cache::Channel> {
    let mut counts = HashMap::new();
    for channel in &channels {
        *counts.entry(channel.number).or_insert(0) += 1;
    }

    let mut used = HashSet::new();
    for channel in &channels {
        if counts.get(&channel.number) == Some(&1) {
            used.insert(channel.number);
        }
    }

    let mut next = 1;
    let mut duplicate_heads = HashSet::new();

    for channel in &mut channels {
        if counts.get(&channel.number) == Some(&1) {
            continue;
        }

        if duplicate_heads.insert(channel.number) && used.insert(channel.number) {
            continue;
        }

        let original = channel.number;
        while used.contains(&next) {
            next += 1;
        }
        channel.number = next;
        used.insert(next);
        eprintln!(
            "channels: channel_number collision source=final_response number={} channel={} channel_code={} action=fallback fallback_number={}",
            original,
            channel.name,
            channel.channel_code.as_deref().unwrap_or(""),
            next
        );
    }

    channels
}

fn sort_channels_by_number(mut channels: Vec<crate::cache::Channel>) -> Vec<crate::cache::Channel> {
    channels.sort_by(|a, b| {
        a.number
            .cmp(&b.number)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.channel_code.cmp(&b.channel_code))
    });
    channels
}

fn epg_day(request: &Request, state: &AppState) -> String {
    let Some(client_name) = authorized_client_name(request, state) else {
        eprintln!("epg_day: unauthorized");
        return json_error(401, "unauthorized", "valid bearer token required");
    };
    let Some(channel_code) = request.query.get("channelCode").filter(|v| !v.is_empty()) else {
        eprintln!("epg_day: missing_channel_code client={client_name}");
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
        _ => {
            eprintln!("epg_day: invalid_date_offset client={client_name}");
            return json_error(400, "invalid_date_offset", "dateOffset must be -1, 0, or 1");
        }
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
                eprintln!(
                    "epg_day: cache_hit client={} channel={} date_offset={} count={} ttl_seconds={}",
                    client_name,
                    channel_code,
                    date_offset,
                    resp.data.len(),
                    ttl
                );
                return json(200, &resp);
            }
        }
    }
    let Some(provider) = provider else {
        eprintln!(
            "epg_day: provider_not_configured client={} channel={} date_offset={}",
            client_name, channel_code, date_offset
        );
        return json_error(503, "backend_not_configured", "provider is not configured");
    };

    eprintln!(
        "epg_day: cache_miss client={} channel={} date_offset={} source=provider",
        client_name, channel_code, date_offset
    );
    match crate::ctc::fetch_programs(&provider, channel_code, date_offset) {
        Ok(programs) => {
            let count = programs.len();
            let mut cache = state.epg_cache.lock().unwrap();
            cache.update(key.clone(), programs, now);
            eprintln!(
                "epg_day: refresh_ok client={} channel={} date_offset={} count={}",
                client_name, channel_code, date_offset, count
            );
            json(200, &cache.response(&key, ttl, false).unwrap())
        }
        Err(e) => {
            if let Some(stale) = state.epg_cache.lock().unwrap().response(&key, ttl, true) {
                eprintln!(
                    "epg_day: refresh_failed_serving_stale client={} channel={} date_offset={} count={} error={}",
                    client_name,
                    channel_code,
                    date_offset,
                    stale.data.len(),
                    e
                );
                json(200, &stale)
            } else {
                eprintln!(
                    "epg_day: refresh_failed_no_cache client={} channel={} date_offset={} error={}",
                    client_name, channel_code, date_offset, e
                );
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

fn authorized_client_name(request: &Request, state: &AppState) -> Option<String> {
    let header = request.headers.get("authorization")?;
    let raw = header.strip_prefix("Bearer ")?;
    let mut cfg = state.config.lock().unwrap();
    let client_name = find_valid_token(&mut cfg.tokens, raw).map(|token| token.name.clone());
    if client_name.is_some() {
        let _ = cfg.save_atomic(&state.config_path);
    }
    client_name
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

fn response_status(response: &str) -> &str {
    response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("unknown")
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
            channel_number_overrides_path: None,
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
        let channels = ensure_unique_channel_numbers(vec![
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
        let channels = sort_channels_by_number(vec![
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
}
