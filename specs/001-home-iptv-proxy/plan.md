# Implementation Plan: Home IPTV Proxy Server

**Branch**: `001-home-iptv-proxy` | **Date**: 2026-06-24 | **Spec**: [spec.md](spec.md)

## Summary

Create a small Rust proxy service for x86_64 OpenWrt. The proxy owns CTC IPTV backend authentication, caches channel/EPG data, rewrites backend multicast stream URLs through configured `udpxy`, serves a normalized LAN API to ATV clients, and provides a password-protected LAN admin page for provider config and local token management.

## Technical Context

**Language/Version**: Rust 2021 edition, stable toolchain  
**Primary Platform**: x86_64 OpenWrt  
**Primary Runtime Model**: Blocking `std::net::TcpListener` with a small internal HTTP router unless load testing proves a need for async  
**Initial Dependencies**: none in the scaffold  
**Planned Minimal Dependencies**: to be added only where justified by implementation tasks  
**Storage**: file-backed config/state under OpenWrt service directory, atomic writes  
**Testing**: Rust unit tests plus integration tests using in-process mock backend listeners  
**Security Constraints**: no plaintext provider password in logs/API; no raw local token persistence; admin page LAN-only and password protected; raw backend stream URLs are not exposed to clients by default

## Dependency Policy

The proxy should stay dependency-light, but not at the cost of security-critical correctness.

| Area | Preferred Approach | Dependency Stance |
|---|---|---|
| HTTP server | Small internal HTTP/1.1 parser/router over `TcpListener` | Avoid web framework |
| JSON API | `serde`/`serde_json` if hand-written JSON becomes risky | Likely justified |
| Config | Simple line/TOML-like parser first, or `toml` if complexity grows | Defer |
| Random tokens | Read from `/dev/urandom` on OpenWrt/Linux | Avoid `rand` initially |
| Hashing | Use a small audited hash crate if needed | Likely justified |
| 3DES CTC auth | Use RustCrypto DES/cipher crates rather than custom crypto | Justified |
| Stream URL rewrite | Small local parser for `igmp://` and `rtp://` URLs | Avoid URL crate initially |
| HTML admin | Server-rendered static strings | No template engine |
| Async runtime | Blocking threads and request limits | Avoid initially |
| Database | Atomic JSON/state files | Avoid SQLite |

## Project Structure

```text
atv-iptv-proxy/
├── Cargo.toml
├── README.md
├── src/
│   ├── main.rs
│   ├── config.rs              # service config and redaction
│   ├── http.rs                # minimal HTTP parsing/response helpers
│   ├── admin.rs               # admin routes and HTML
│   ├── auth.rs                # local token/admin password validation
│   ├── ctc/
│   │   ├── mod.rs
│   │   ├── authenticator.rs   # CTC 3DES authenticator
│   │   ├── client.rs          # backend HTTP flow
│   │   └── parsers.rs         # response parsing
│   ├── cache.rs               # TTL, stale fallback, atomic writes
│   ├── stream.rs              # multicast -> udpxy playable URL resolver
│   └── api.rs                 # `/api/v1/*` routes
├── tests/
│   ├── admin_flow.rs
│   ├── api_token_flow.rs
│   ├── backend_login_flow.rs
│   └── cache_flow.rs
└── specs/
    └── 001-home-iptv-proxy/
        ├── spec.md
        └── plan.md
```

## Phase 0: Repository Scaffold

- [x] Create sibling project directory beside `atv`.
- [x] Add Rust `Cargo.toml` with no runtime dependencies.
- [x] Add `.gitignore` for Rust build outputs, local config, runtime state, and packaging artifacts.
- [x] Add README pointing to spec and plan.
- [x] Add placeholder `src/main.rs` so `cargo test` has a starting point.
- [x] Initialize git repository and add remote `https://github.com/ultracold273/atv-iptv-proxy.git`.
- [x] Run `cargo test` to verify the scaffold.

## Phase 1: Configuration & State Foundation

### Goals

Create file-backed config/state primitives with secret redaction and atomic writes.

### Tasks

- [ ] Define `ProxyConfig`: listen addresses, LAN CIDRs, cache TTLs, admin password hash, provider config, stream proxy config, token metadata.
- [ ] Define `ProviderConfig`: User ID, password secret, STB ID, IP, MAC, auth server URL.
- [ ] Define `StreamProxyConfig`: `udpxy` base URL, rewrite policy, and optional raw-stream diagnostics flag.
- [ ] Define redacted status views so admin/API output cannot include secrets.
- [ ] Implement atomic write helper: write temp file, fsync where practical, rename.
- [ ] Add tests for config load/save, redaction, malformed config, and atomic write failure handling.

## Phase 2: Local Authentication

### Goals

Implement admin password verification and per-client bearer token issuance/validation.

### Tasks

- [ ] Implement high-entropy token generation from `/dev/urandom`.
- [ ] Implement token hash storage and constant-time validation.
- [ ] Implement token create/list/disable/delete state transitions.
- [ ] Implement admin password hash format with salt and constant-time comparison.
- [ ] Add tests for token one-time display, revocation, disabled tokens, last-seen updates, and timing-safe comparison behavior.

## Phase 3: Minimal HTTP & Admin Page

### Goals

Serve the LAN admin page and protected client API without pulling in a web framework.

### Tasks

- [ ] Implement limited HTTP/1.1 parser with method, path, query, headers, and bounded body size.
- [ ] Implement response helpers for HTML, JSON, redirects, and errors.
- [ ] Add LAN source-address check for admin routes.
- [ ] Build server-rendered admin pages: login, status, provider config, `udpxy` config, token management, manual refresh.
- [ ] Add admin session handling or password challenge flow.
- [ ] Add tests for unauthorized admin access, LAN restriction, password failure, config save, token create/revoke, and CSRF-resistant form handling.

## Phase 4: CTC Backend Client

### Goals

Port the CTC auth/channel fetch flow from the Android implementation into Rust.

### Tasks

- [ ] Implement CTC authenticator: password-derived 24-byte key, 8-digit random, DESede/ECB/PKCS5Padding equivalent.
- [ ] Port response parsers for EncryToken, `CTCSetConfig`, document redirects, hidden form inputs, channel blocks, and channel mapping JSON.
- [ ] Implement backend HTTP client with cookie/session handling and redirect awareness.
- [ ] Implement login flow: `/auth`, `/uploadAuthInfo`, `/getServiceList`, EPG redirect, portal auth.
- [ ] Implement channel fetch flow: `frameset_builder.jsp` and `get_channel_info_mapping.jsp`.
- [ ] Add mock-backend integration tests for success, login failure, parse failure, missing channel mapping, and backend HTTP errors.

## Phase 5: Cache & API

### Goals

Expose the normalized proxy API and ensure cache behavior prevents constant backend calls.

### Tasks

- [ ] Implement `stream.rs` resolver: HTTP(S) passthrough, `igmp://` rewrite, `rtp://` rewrite, blank `udpxy` configuration error.
- [ ] Normalize `udpxy` base URL values with or without scheme and trailing slash.
- [ ] Apply stream URL resolution during channel normalization before writing channel cache or returning API data.
- [ ] Implement channel cache with TTL, stale fallback, refresh metadata, and atomic persistence.
- [ ] Implement refresh coalescing so concurrent cold-cache requests share one backend refresh.
- [ ] Implement `GET /health`.
- [ ] Implement `GET /api/v1/channels` with token authentication and cache metadata.
- [ ] Ensure `/api/v1/channels.data[].streamUrl` is directly playable by ATV clients.
- [ ] Implement structured JSON errors with stable codes.
- [ ] Add EPG endpoint skeletons or first implementation depending on client needs.
- [ ] Add tests for stream URL rewrite, missing `udpxy` failure, cache hit, cache miss, concurrent refresh, stale fallback, no-cache backend failure, and unauthorized API calls.

## Phase 6: OpenWrt Deployment

### Goals

Make the proxy easy to run as an OpenWrt service on x86_64.

### Tasks

- [ ] Add cross-build instructions for x86_64 OpenWrt target.
- [ ] Add sample config path and directory layout.
- [ ] Add `procd` init script template.
- [ ] Add firewall/UCI notes for LAN-only binding.
- [ ] Add release packaging notes and checksum generation.
- [ ] Verify service start/stop/restart behavior on target or equivalent Linux environment.

## Phase 7: Regression Suite & Hardening

### Goals

Close the loop with repeatable tests and security checks.

### Tasks

- [ ] Add a fixture-driven integration suite for full admin-create-token-to-client-fetch flow.
- [ ] Add grep-style secret leak checks for logs/fixtures where practical.
- [ ] Add malformed request tests: oversize body, missing headers, invalid token, invalid query, broken backend response.
- [ ] Add cache corruption recovery tests.
- [ ] Add regression fixtures where backend channels include `igmp://`, `rtp://`, HTTP(S), and malformed stream URLs.
- [ ] Add a CI workflow for `cargo fmt --check`, `cargo clippy`, and `cargo test` after the GitHub remote exists.

## Open Questions

- Should the first release support EPG endpoints, or only channel import while Android keeps direct EPG behavior disabled in proxy mode?
- Should admin auth use a short-lived session cookie or a password challenge on every form submission?
- Should OpenWrt deployment use native package feeds later, or is copying a binary plus init script enough for early releases?
- Should raw backend stream URLs ever be retained in cache for diagnostics, or should cache store only resolved playable URLs?

## Complexity Tracking

No complexity violations yet. The only likely dependency exceptions are crypto, JSON, and possibly config parsing. Each should be introduced in the implementation task where it becomes necessary.
