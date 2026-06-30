use super::http::Request;
use crate::cache::now_secs;
use std::net::SocketAddr;

#[derive(Debug)]
pub(super) struct RequestContext {
    pub(super) request: Request,
    pub(super) peer_addr: Option<SocketAddr>,
    pub(super) received_at: u64,
}

impl RequestContext {
    pub(super) fn new(request: Request, peer_addr: Option<SocketAddr>) -> Self {
        Self {
            request,
            peer_addr,
            received_at: now_secs(),
        }
    }
}
