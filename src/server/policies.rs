use super::context::RequestContext;
use super::state::AppState;

pub(super) enum PolicyDecision {
    Allow,
    #[allow(dead_code)]
    Deny {
        status: u16,
        code: &'static str,
        message: &'static str,
    },
}

pub(super) fn check_pairing_create(_ctx: &RequestContext, _state: &AppState) -> PolicyDecision {
    let _ = (_ctx.peer_addr, _ctx.received_at);
    PolicyDecision::Allow
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProxyConfig;
    use crate::server::http::Request;
    use crate::server::state::AppState;
    use std::net::SocketAddr;
    use std::path::PathBuf;

    #[test]
    fn pairing_create_policy_allows_by_default() {
        let request = Request::parse("POST /api/v1/pairing/sessions HTTP/1.1\r\n\r\n{}").unwrap();
        let ctx = RequestContext::new(request, None);
        let state = AppState::new(PathBuf::new(), None, ProxyConfig::default());

        assert!(matches!(
            check_pairing_create(&ctx, &state),
            PolicyDecision::Allow
        ));
    }

    #[test]
    fn request_context_carries_peer_for_future_policy_checks() {
        let request = Request::parse("POST /api/v1/pairing/sessions HTTP/1.1\r\n\r\n{}").unwrap();
        let peer = "192.0.2.55:12345".parse::<SocketAddr>().unwrap();
        let ctx = RequestContext::new(request, Some(peer));

        assert_eq!(Some(peer), ctx.peer_addr);
        assert!(ctx.received_at > 0);
    }
}
