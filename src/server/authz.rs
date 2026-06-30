use super::http::Request;
use super::state::AppState;
use crate::auth::{find_valid_token, verify_secret};

pub(super) fn authorized_admin(request: &Request, state: &AppState) -> bool {
    let Some(value) = request.headers.get("x-admin-password") else {
        return false;
    };
    let cfg = state.config.lock().unwrap();
    verify_secret(value, &cfg.admin_password_hash)
}

pub(super) fn authorized_client_name(request: &Request, state: &AppState) -> Option<String> {
    let header = request.headers.get("authorization")?;
    let raw = header.strip_prefix("Bearer ")?;
    let mut cfg = state.config.lock().unwrap();
    let client_name = find_valid_token(&mut cfg.tokens, raw).map(|token| token.name.clone());
    if client_name.is_some() {
        let _ = cfg.save_atomic(&state.config_path);
    }
    client_name
}
