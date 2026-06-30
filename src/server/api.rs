use super::authz::authorized_client_name;
use super::context::RequestContext;
use super::ctc_session::{
    fetch_ctc_channels_with_cached_session, fetch_ctc_programs_with_cached_session,
};
use super::http::{json, json_error, Request};
use super::pairing_error;
use super::policies::{check_pairing_create, PolicyDecision};
use super::state::AppState;
use crate::backend;
use crate::cache::{now_secs, EpgCacheKey};
use crate::config::load_channel_number_overrides;
use crate::pairing::CreatePairingRequest;
use std::collections::{HashMap, HashSet};

pub(super) fn pairing_create(ctx: &RequestContext, state: &AppState) -> String {
    match check_pairing_create(ctx, state) {
        PolicyDecision::Allow => {}
        PolicyDecision::Deny {
            status,
            code,
            message,
        } => return json_error(status, code, message),
    }

    let parsed = match serde_json::from_str::<CreatePairingRequest>(&ctx.request.body) {
        Ok(parsed) => parsed,
        Err(e) => return json_error(400, "bad_request", &e.to_string()),
    };
    match state.pairing.lock().unwrap().create(parsed, now_secs()) {
        Ok(response) => json(200, &response),
        Err(e) => pairing_error(e),
    }
}

pub(super) fn pairing_poll(request: &Request, state: &AppState) -> String {
    let Some(session_id) = request.path.strip_prefix("/api/v1/pairing/sessions/") else {
        return json_error(404, "not_found", "not found");
    };
    let Some(client_nonce) = request.headers.get("x-client-nonce") else {
        return json_error(401, "invalid_client_nonce", "valid client nonce required");
    };
    match state
        .pairing
        .lock()
        .unwrap()
        .poll(session_id, client_nonce, now_secs())
    {
        Ok(response) => json(200, &response),
        Err(e) => pairing_error(e),
    }
}

pub(super) fn channels(request: &Request, state: &AppState) -> String {
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
            Ok(overrides) => {
                fetch_ctc_channels_with_cached_session(state, &provider, &stream_cfg, &overrides)
            }
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

pub(super) fn ensure_unique_channel_numbers(
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

pub(super) fn sort_channels_by_number(
    mut channels: Vec<crate::cache::Channel>,
) -> Vec<crate::cache::Channel> {
    channels.sort_by(|a, b| {
        a.number
            .cmp(&b.number)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.channel_code.cmp(&b.channel_code))
    });
    channels
}

pub(super) fn epg_day(request: &Request, state: &AppState) -> String {
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
    match fetch_ctc_programs_with_cached_session(state, &provider, channel_code, date_offset) {
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
