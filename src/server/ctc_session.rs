use super::state::{AppState, CachedCtcSession};
use crate::config::ProviderConfig;
use crate::stream::StreamProxyConfig;

pub(super) fn cached_ctc_session(
    state: &AppState,
    provider: &ProviderConfig,
) -> Result<crate::ctc::LoginSession, String> {
    let mut cached = state.ctc_session.lock().unwrap();
    if let Some(entry) = cached.as_ref().filter(|entry| entry.provider == *provider) {
        eprintln!("ctc_session: cache_hit user_id={}", provider.user_id);
        return Ok(entry.session.clone());
    }

    if cached.is_some() {
        eprintln!(
            "ctc_session: provider_changed user_id={} action=relogin",
            provider.user_id
        );
    } else {
        eprintln!(
            "ctc_session: cache_miss user_id={} action=login",
            provider.user_id
        );
    }
    let session = crate::ctc::login(provider)?;
    *cached = Some(CachedCtcSession {
        provider: provider.clone(),
        session: session.clone(),
    });
    Ok(session)
}

fn refresh_ctc_session(
    state: &AppState,
    provider: &ProviderConfig,
) -> Result<crate::ctc::LoginSession, String> {
    eprintln!(
        "ctc_session: refresh user_id={} action=relogin",
        provider.user_id
    );
    let session = crate::ctc::login(provider)?;
    *state.ctc_session.lock().unwrap() = Some(CachedCtcSession {
        provider: provider.clone(),
        session: session.clone(),
    });
    Ok(session)
}

pub(super) fn clear_ctc_session(state: &AppState, provider: &ProviderConfig) {
    let mut cached = state.ctc_session.lock().unwrap();
    if cached
        .as_ref()
        .is_some_and(|entry| entry.provider == *provider)
    {
        eprintln!("ctc_session: invalidate user_id={}", provider.user_id);
        *cached = None;
    }
}

pub(super) fn fetch_ctc_channels_with_cached_session(
    state: &AppState,
    provider: &ProviderConfig,
    stream_cfg: &StreamProxyConfig,
    overrides: &crate::config::ChannelNumberOverrides,
) -> Result<Vec<crate::cache::Channel>, String> {
    let session = cached_ctc_session(state, provider)?;
    match crate::ctc::fetch_channels_with_session(&session, stream_cfg, overrides) {
        Ok(channels) => Ok(channels),
        Err(first_error) => {
            clear_ctc_session(state, provider);
            eprintln!(
                "ctc_session: retry_after_failure operation=fetch_channels user_id={} error={}",
                provider.user_id, first_error
            );
            let fresh = refresh_ctc_session(state, provider)?;
            crate::ctc::fetch_channels_with_session(&fresh, stream_cfg, overrides)
        }
    }
}

pub(super) fn fetch_ctc_programs_with_cached_session(
    state: &AppState,
    provider: &ProviderConfig,
    channel_code: &str,
    date_offset: i32,
) -> Result<Vec<crate::cache::Program>, String> {
    let session = cached_ctc_session(state, provider)?;
    match crate::ctc::fetch_programs_with_session(&session, channel_code, date_offset) {
        Ok(programs) => Ok(programs),
        Err(first_error) => {
            clear_ctc_session(state, provider);
            eprintln!(
                "ctc_session: retry_after_failure operation=fetch_programs user_id={} channel={} date_offset={} error={}",
                provider.user_id, channel_code, date_offset, first_error
            );
            let fresh = refresh_ctc_session(state, provider)?;
            crate::ctc::fetch_programs_with_session(&fresh, channel_code, date_offset)
        }
    }
}
