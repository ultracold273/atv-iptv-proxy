use super::authz::authorized_admin;
use super::http::{json, json_error, parse_form, response, Request};
use super::pairing_error;
use super::state::AppState;
use crate::auth::{generate_token, hash_secret, ClientToken};
use crate::cache::now_secs;
use crate::config::{ProviderConfig, ProxyConfig};
use crate::pairing::{ApprovePairingRequest, PairingDecisionResponse, RejectPairingRequest};
use crate::stream::StreamProxyConfig;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminStatusResponse {
    listen: String,
    backend_mode: String,
    provider_configured: bool,
    backend_configured: bool,
    udpxy_configured: bool,
    token_count: usize,
    pending_pairing_count: usize,
    channel_cache: Option<AdminChannelCacheStatus>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminChannelCacheStatus {
    cached_at: u64,
    ttl_seconds: u64,
    channel_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminConfigResponse {
    listen: String,
    channel_cache_ttl_seconds: u64,
    epg_cache_ttl_seconds: u64,
    backend_channels_url: Option<String>,
    provider: Option<AdminProviderConfig>,
    stream: StreamProxyConfig,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminProviderConfig {
    user_id: String,
    password_configured: bool,
    stb_id: String,
    local_ip: String,
    local_mac: String,
    auth_server_url: String,
}

impl From<&ProviderConfig> for AdminProviderConfig {
    fn from(provider: &ProviderConfig) -> Self {
        Self {
            user_id: provider.user_id.clone(),
            password_configured: !provider.password.is_empty(),
            stb_id: provider.stb_id.clone(),
            local_ip: provider.local_ip.clone(),
            local_mac: provider.local_mac.clone(),
            auth_server_url: provider.auth_server_url.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
struct AdminTokensResponse {
    data: Vec<AdminTokenView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminTokenView {
    name: String,
    created_at: u64,
    last_seen_at: Option<u64>,
    enabled: bool,
}

pub(super) fn page() -> String {
    response(
        200,
        "text/html; charset=utf-8",
        include_str!("admin.html").to_string(),
    )
}

pub(super) fn status(request: &Request, state: &AppState) -> String {
    if !authorized_admin(request, state) {
        return json_error(401, "admin_unauthorized", "admin password required");
    }
    let (
        listen,
        backend_mode,
        provider_configured,
        backend_configured,
        udpxy_configured,
        token_count,
        channel_ttl,
    ) = {
        let cfg = state.config.lock().unwrap();
        let backend_mode = if cfg.provider.is_some() {
            "provider"
        } else if cfg.backend_channels_url.is_some() {
            "http_backend"
        } else {
            "not_configured"
        };
        (
            cfg.listen.clone(),
            backend_mode.to_string(),
            cfg.provider.is_some(),
            cfg.backend_channels_url.is_some(),
            cfg.stream.udpxy_base_url.is_some(),
            cfg.tokens.len(),
            cfg.channel_cache_ttl_seconds,
        )
    };
    let pending_pairing_count = state
        .pairing
        .lock()
        .unwrap()
        .pending_sessions(now_secs())
        .data
        .len();
    let channel_cache = state
        .cache
        .lock()
        .unwrap()
        .response(channel_ttl, false)
        .map(|response| AdminChannelCacheStatus {
            cached_at: response.cache.cached_at,
            ttl_seconds: response.cache.ttl_seconds,
            channel_count: response.data.len(),
        });

    json(
        200,
        &AdminStatusResponse {
            listen,
            backend_mode,
            provider_configured,
            backend_configured,
            udpxy_configured,
            token_count,
            pending_pairing_count,
            channel_cache,
        },
    )
}

pub(super) fn config_get(request: &Request, state: &AppState) -> String {
    if !authorized_admin(request, state) {
        return json_error(401, "admin_unauthorized", "admin password required");
    }
    let cfg = state.config.lock().unwrap();
    json(
        200,
        &AdminConfigResponse {
            listen: cfg.listen.clone(),
            channel_cache_ttl_seconds: cfg.channel_cache_ttl_seconds,
            epg_cache_ttl_seconds: cfg.epg_cache_ttl_seconds,
            backend_channels_url: cfg.backend_channels_url.clone(),
            provider: cfg.provider.as_ref().map(AdminProviderConfig::from),
            stream: cfg.stream.clone(),
        },
    )
}

pub(super) fn tokens_get(request: &Request, state: &AppState) -> String {
    if !authorized_admin(request, state) {
        return json_error(401, "admin_unauthorized", "admin password required");
    }
    let cfg = state.config.lock().unwrap();
    let data = cfg
        .tokens
        .iter()
        .map(|token| AdminTokenView {
            name: token.name.clone(),
            created_at: token.created_at,
            last_seen_at: token.last_seen_at,
            enabled: token.enabled,
        })
        .collect::<Vec<_>>();
    json(200, &AdminTokensResponse { data })
}

pub(super) fn config_update(request: &Request, state: &AppState) -> String {
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
    if let Some(v) = form.get("channel_cache_ttl_seconds") {
        cfg.channel_cache_ttl_seconds = match parse_ttl(v) {
            Ok(value) => value,
            Err(message) => return json_error(400, "invalid_channel_cache_ttl", message),
        };
        updated.push("channel_cache_ttl_seconds");
    }
    if let Some(v) = form.get("epg_cache_ttl_seconds") {
        cfg.epg_cache_ttl_seconds = match parse_ttl(v) {
            Ok(value) => value,
            Err(message) => return json_error(400, "invalid_epg_cache_ttl", message),
        };
        updated.push("epg_cache_ttl_seconds");
    }
    if form.keys().any(|key| key.starts_with("provider_")) {
        let has_provider_value = form
            .iter()
            .any(|(key, value)| key.starts_with("provider_") && !value.trim().is_empty());
        if cfg.provider.is_none() && !has_provider_value {
            return save_config(&cfg, state, &updated);
        }
        let mut provider = cfg.provider.clone().unwrap_or_else(|| ProviderConfig {
            user_id: String::new(),
            password: String::new(),
            stb_id: String::new(),
            local_ip: String::new(),
            local_mac: String::new(),
            auth_server_url: String::new(),
        });
        if let Some(v) = form.get("provider_user_id") {
            provider.user_id = v.trim().to_string();
        }
        if let Some(v) = form.get("provider_password") {
            if !v.is_empty() {
                provider.password = v.to_string();
            }
        }
        if let Some(v) = form.get("provider_stb_id") {
            provider.stb_id = v.trim().to_string();
        }
        if let Some(v) = form.get("provider_local_ip") {
            provider.local_ip = v.trim().to_string();
        }
        if let Some(v) = form.get("provider_local_mac") {
            provider.local_mac = v.trim().to_string();
        }
        if let Some(v) = form.get("provider_auth_server_url") {
            provider.auth_server_url = v.trim().to_string();
        }
        cfg.provider = Some(provider);
        updated.push("provider");
    }
    save_config(&cfg, state, &updated)
}

fn save_config(cfg: &ProxyConfig, state: &AppState, updated: &[&str]) -> String {
    if let Err(e) = cfg.save_atomic(&state.config_path) {
        eprintln!("admin_config: save_failed error={e}");
        return json_error(500, "config_save_failed", &e.to_string());
    }
    eprintln!("admin_config: saved fields={}", updated.join(","));
    json(200, &serde_json::json!({"ok": true}))
}

pub(super) fn create_token(request: &Request, state: &AppState) -> String {
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

pub(super) fn delete_token(request: &Request, state: &AppState) -> String {
    if !authorized_admin(request, state) {
        eprintln!("admin_token_delete: unauthorized");
        return json_error(401, "admin_unauthorized", "admin password required");
    }
    let form = parse_form(&request.body);
    let Some(name) = request.query.get("name").or_else(|| form.get("name")) else {
        return json_error(400, "missing_token_name", "name is required");
    };
    let name = name.trim();
    if name.is_empty() {
        return json_error(400, "missing_token_name", "name is required");
    }

    let mut cfg = state.config.lock().unwrap();
    let before = cfg.tokens.len();
    cfg.tokens.retain(|token| token.name != name);
    let deleted_count = before.saturating_sub(cfg.tokens.len());
    if deleted_count == 0 {
        return json_error(404, "token_not_found", "token name not found");
    }
    if let Err(e) = cfg.save_atomic(&state.config_path) {
        eprintln!("admin_token_delete: save_failed name={name} error={e}");
        return json_error(500, "config_save_failed", &e.to_string());
    }
    eprintln!(
        "admin_token_delete: deleted name={name} deleted_count={} remaining_tokens={}",
        deleted_count,
        cfg.tokens.len()
    );
    json(
        200,
        &serde_json::json!({"ok": true, "deletedCount": deleted_count}),
    )
}

pub(super) fn pairing_sessions(request: &Request, state: &AppState) -> String {
    if !authorized_admin(request, state) {
        eprintln!("admin_pairing_sessions: unauthorized");
        return json_error(401, "admin_unauthorized", "admin password required");
    }
    let status = request
        .query
        .get("status")
        .map(String::as_str)
        .unwrap_or("pending");
    if status != "pending" {
        return json_error(
            400,
            "unsupported_status",
            "only pending pairing sessions can be listed",
        );
    }
    let response = state.pairing.lock().unwrap().pending_sessions(now_secs());
    json(200, &response)
}

pub(super) fn pairing_approve(request: &Request, state: &AppState) -> String {
    if !authorized_admin(request, state) {
        eprintln!("admin_pairing_approve: unauthorized");
        return json_error(401, "admin_unauthorized", "admin password required");
    }
    let parsed = match serde_json::from_str::<ApprovePairingRequest>(&request.body) {
        Ok(parsed) => parsed,
        Err(e) => return json_error(400, "bad_request", &e.to_string()),
    };
    let candidate = match state.pairing.lock().unwrap().approval_candidate(
        &parsed.pairing_code,
        parsed.device_label.as_deref(),
        now_secs(),
    ) {
        Ok(candidate) => candidate,
        Err(e) => return pairing_error(e),
    };
    let raw = match generate_token(&candidate.token_name) {
        Ok(token) => token,
        Err(e) => return json_error(500, "token_generation_failed", &e.to_string()),
    };
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.tokens.push(ClientToken {
            name: candidate.token_name.clone(),
            hash: hash_secret(&raw),
            created_at: crate::auth::now_secs(),
            last_seen_at: None,
            enabled: true,
        });
        if let Err(e) = cfg.save_atomic(&state.config_path) {
            return json_error(500, "config_save_failed", &e.to_string());
        }
    }
    if let Err(e) = state
        .pairing
        .lock()
        .unwrap()
        .approve(&candidate.session_id, raw)
    {
        return pairing_error(e);
    }
    json(
        200,
        &PairingDecisionResponse {
            status: "approved",
            client_id: Some(candidate.session_id),
        },
    )
}

pub(super) fn pairing_reject(request: &Request, state: &AppState) -> String {
    if !authorized_admin(request, state) {
        eprintln!("admin_pairing_reject: unauthorized");
        return json_error(401, "admin_unauthorized", "admin password required");
    }
    let parsed = match serde_json::from_str::<RejectPairingRequest>(&request.body) {
        Ok(parsed) => parsed,
        Err(e) => return json_error(400, "bad_request", &e.to_string()),
    };
    match state
        .pairing
        .lock()
        .unwrap()
        .reject(&parsed.pairing_code, now_secs())
    {
        Ok(()) => json(
            200,
            &PairingDecisionResponse {
                status: "rejected",
                client_id: None,
            },
        ),
        Err(e) => pairing_error(e),
    }
}

fn parse_ttl(value: &str) -> Result<u64, &'static str> {
    let parsed = value
        .trim()
        .parse::<u64>()
        .map_err(|_| "TTL must be a positive integer")?;
    if parsed == 0 {
        return Err("TTL must be greater than zero");
    }
    Ok(parsed)
}
