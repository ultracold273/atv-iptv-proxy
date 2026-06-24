# ATV IPTV Proxy

LAN-only IPTV authentication and channel proxy for the ATV Android TV client.

This project is intentionally **spec-first**. The initial scaffold contains the Rust package, repository metadata, and the first feature specification/plan. Implementation should follow [`specs/001-home-iptv-proxy/spec.md`](specs/001-home-iptv-proxy/spec.md) and [`specs/001-home-iptv-proxy/plan.md`](specs/001-home-iptv-proxy/plan.md).

## Goals

- Keep the real IPTV credentials on the OpenWrt router.
- Let Android TV clients use local proxy tokens instead of provider credentials.
- Cache session, channel, and EPG data so clients do not repeatedly hit the IPTV backend.
- Run on x86_64 OpenWrt with a small Rust binary and minimal dependencies.

## Repository

Planned remote:

```text
https://github.com/ultracold273/atv-iptv-proxy.git
```
