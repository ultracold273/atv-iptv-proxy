# Feature Specification: Home IPTV Proxy Server

**Feature Branch**: `001-home-iptv-proxy`  
**Created**: 2026-06-24  
**Status**: Draft  
**Input**: Build a separate Rust proxy service for OpenWrt that owns IPTV backend authentication and serves LAN Android TV clients through a local authenticated API.

## Overview

The IPTV backend rejects multiple registrations for the same provider user ID/password. The home proxy solves this by making the OpenWrt router the only device that authenticates to the provider. Android TV clients authenticate only to the proxy with local tokens. The proxy caches login/session/channel/EPG data and exposes a small normalized API compatible with the ATV client domain model.

The proxy also owns the home multicast playback topology. The IPTV backend may return stream URLs such as `igmp://239.x.x.x:port` or `rtp://239.x.x.x:port`; ATV clients cannot play those directly. The proxy therefore has a configurable `udpxy` base URL and rewrites multicast stream URLs to playable HTTP URLs before returning channels to clients.

The proxy is a separate GitHub project from the Android client. It targets x86_64 OpenWrt, uses Rust, avoids web frameworks and heavyweight runtime dependencies, and includes an admin page available only from the LAN side with password protection.

## User Scenarios & Testing

### User Story 1 - Router Admin Configures Provider Login (Priority: P1)

As the router admin, I want to enter the provider IPTV credentials once on the OpenWrt proxy, so Android TV clients do not need to store the real provider password.

**Independent Test**: Start the proxy with a mock IPTV backend, configure provider credentials through the admin page, trigger a login test, and verify the proxy stores no plaintext password in logs or API responses.

**Acceptance Scenarios**:

1. **Given** the proxy is installed on OpenWrt, **When** the admin opens the LAN admin URL, **Then** the admin page requires a password before showing any configuration.
2. **Given** the admin is authenticated, **When** provider credentials are saved, **Then** the proxy can run the CTC login flow against the configured backend.
3. **Given** the login succeeds, **When** the admin views status, **Then** the page shows a redacted connected state and last refresh time, not secret values.
4. **Given** the login fails, **When** status is shown, **Then** the failure reason is visible without exposing credentials, tokens, or session IDs.

### User Story 2 - Admin Issues Local Client Tokens (Priority: P1)

As the router admin, I want to issue local tokens per Android TV client, so each client can be authorized or revoked independently without sharing provider credentials.

**Independent Test**: Create a token named `living-room-tv`, use it to call `/api/v1/channels`, revoke it, and verify subsequent calls fail with `401`.

**Acceptance Scenarios**:

1. **Given** the admin page is open, **When** the admin creates a client token, **Then** the raw token is displayed exactly once.
2. **Given** a token exists, **When** a client sends `Authorization: Bearer <token>`, **Then** the proxy validates it against stored token hashes.
3. **Given** a token is disabled or deleted, **When** that token is used, **Then** the proxy returns `401 Unauthorized`.
4. **Given** multiple tokens exist, **When** one token is revoked, **Then** other clients remain authorized.

### User Story 3 - Clients Import Channels Through Proxy (Priority: P1)

As an Android TV client, I want to fetch the normalized channel list from the proxy, so I can import provider channels without logging into the IPTV backend directly.

**Independent Test**: With a valid local token and a mock backend channel response, call `/api/v1/channels` and verify the JSON maps to the ATV `Channel` model.

**Acceptance Scenarios**:

1. **Given** the proxy has no fresh channel cache, **When** the first authorized client requests channels, **Then** the proxy logs into the IPTV backend if needed, fetches channels, normalizes them, caches them, and returns JSON.
2. **Given** the channel cache is fresh, **When** another authorized client requests channels, **Then** the proxy serves the cache without calling the backend.
3. **Given** the backend is down but a stale channel cache exists, **When** a client requests channels, **Then** the proxy may return stale data with a `stale: true` metadata flag rather than failing hard.
4. **Given** no cache exists and the backend is unreachable, **When** a client requests channels, **Then** the proxy returns a structured `503` error.

### User Story 4 - Proxy Caches Backend Work (Priority: P2)

As the household network owner, I want the proxy to avoid unnecessary backend authentication and channel/EPG requests, so the provider sees only a small number of router-originated calls.

**Independent Test**: Send concurrent channel requests and verify only one backend login/fetch happens; subsequent calls within TTL are cache hits.

**Acceptance Scenarios**:

1. **Given** several clients request channels at the same time, **When** the cache is cold, **Then** exactly one backend refresh is performed and the other requests wait for or reuse the result.
2. **Given** cached data is within TTL, **When** clients request it repeatedly, **Then** no backend request occurs.
3. **Given** cached data is expired, **When** a client requests it, **Then** the proxy refreshes it once and updates the cache atomically.
4. **Given** the admin clicks refresh, **When** the refresh succeeds, **Then** cached data is replaced and the new timestamp is visible.

### User Story 5 - Proxy Returns Playable Stream URLs (Priority: P1)

As an Android TV client, I want the proxy channel response to contain stream URLs that are already playable, so clients do not need local multicast or `udpxy` configuration in Home proxy mode.

**Independent Test**: Configure `udpxy` as `http://openwrt:4022`, mock a backend channel URL `igmp://239.49.0.1:8000`, fetch `/api/v1/channels`, and verify the response contains `http://openwrt:4022/udp/239.49.0.1:8000`.

**Acceptance Scenarios**:

1. **Given** a backend channel has an `igmp://` URL and `udpxy` is configured, **When** the channel is returned by the proxy API, **Then** `streamUrl` is rewritten to the configured `udpxy` HTTP URL.
2. **Given** a backend channel has an `rtp://` URL and `udpxy` is configured, **When** the channel is returned by the proxy API, **Then** `streamUrl` is rewritten to the configured `udpxy` HTTP URL.
3. **Given** a backend channel already has an HTTP(S) stream URL, **When** the channel is returned by the proxy API, **Then** `streamUrl` is returned unchanged.
4. **Given** a backend channel has a multicast URL and no `udpxy` is configured, **When** the channel response is generated, **Then** the proxy returns a structured configuration error rather than silently returning an unplayable multicast URL by default.

## Functional Requirements

### Project & Platform

- **FR-001**: The proxy MUST live in a separate project directory and Git repository from the Android client.
- **FR-002**: The repository remote MUST be different from the Android client remote.
- **FR-003**: The proxy MUST target x86_64 OpenWrt as the primary deployment platform.
- **FR-004**: The proxy MUST be implemented in Rust.
- **FR-005**: The proxy SHOULD avoid runtime dependencies unless they remove meaningful security or protocol risk.
- **FR-006**: The proxy MUST NOT use a full web framework, database server, JavaScript frontend build pipeline, or async runtime unless a later plan explicitly justifies it.

### Listener & Network Boundaries

- **FR-007**: The public client API MUST listen only on a configured LAN address/port or be firewall-restricted to LAN clients by the OpenWrt service setup.
- **FR-008**: The admin page MUST be reachable only from the LAN side.
- **FR-009**: The proxy MUST reject admin requests from non-LAN source addresses when source address information is available.
- **FR-010**: The proxy MUST expose `/health` without secrets for local monitoring.

### Admin Page

- **FR-011**: The admin page MUST require password authentication before displaying configuration or token management.
- **FR-012**: The admin password MUST NOT be stored in plaintext.
- **FR-013**: The admin page MUST allow editing provider settings: User ID, Password, STB ID, Local IP, Local MAC, Auth server URL, and `udpxy` base URL.
- **FR-014**: The admin page MUST redact provider password and session values after save.
- **FR-015**: The admin page MUST show status: backend login state, last successful refresh, last failed refresh, cache age, and client token list.
- **FR-016**: The admin page MUST support manual backend refresh.
- **FR-017**: The admin page MUST use plain server-rendered HTML and MUST NOT require a frontend bundler.

### Local Client Tokens

- **FR-018**: The proxy MUST issue high-entropy local bearer tokens only through authenticated admin actions or local CLI actions.
- **FR-019**: Raw client tokens MUST be displayed exactly once at creation time.
- **FR-020**: The proxy MUST store only token hashes and metadata, not raw tokens.
- **FR-021**: Token validation MUST use constant-time comparison for hashes.
- **FR-022**: Tokens MUST be individually named, disabled, deleted, and audited with `created_at` and `last_seen_at` timestamps.
- **FR-023**: The client API MUST require `Authorization: Bearer <token>` for all data endpoints except `/health`.

### Backend Authentication

- **FR-024**: The proxy MUST implement the CTC IPTV login flow currently used by the Android client: login page, authenticator build, `uploadAuthInfo`, `getServiceList`, EPG redirect, and portal auth.
- **FR-025**: The proxy MUST generate the CTC 3DES authenticator locally and MUST NOT delegate provider password handling to clients.
- **FR-026**: The proxy MUST keep backend session state internal and MUST NOT expose `JSESSIONID`, `UserToken`, authenticator hex, or provider password through API responses or logs.
- **FR-027**: The proxy MUST coalesce concurrent backend login/refresh attempts so only one refresh runs for the same cache key at a time.

### Client API

- **FR-028**: The proxy MUST expose `GET /api/v1/channels` returning normalized channel JSON consumable by the ATV client.
- **FR-029**: The proxy SHOULD expose `GET /api/v1/epg/current?channelCode=...` for now/next program lookup.
- **FR-030**: The proxy SHOULD expose `GET /api/v1/epg/day?channelCode=...&date=YYYY-MM-DD` for guide panel data.
- **FR-031**: Error responses MUST be structured JSON with stable error codes.
- **FR-032**: The API MUST include enough cache metadata for clients to distinguish fresh and stale proxy responses.
- **FR-033**: `GET /api/v1/channels` MUST return `streamUrl` values that are directly playable by ATV clients.
- **FR-034**: The proxy MUST rewrite backend `igmp://` and `rtp://` stream URLs through the configured `udpxy` base URL before returning channel data.
- **FR-035**: The proxy MUST pass through HTTP(S) stream URLs unchanged.
- **FR-036**: The proxy MUST normalize `udpxy` base URLs with or without `http://`, `https://`, and trailing slash.
- **FR-037**: If a multicast stream URL is encountered while `udpxy` is not configured, the proxy MUST fail the refresh or response with a structured configuration error unless an explicit raw-multicast compatibility mode is added later.

### Cache

- **FR-038**: The proxy MUST cache the channel list with a configurable TTL.
- **FR-039**: The proxy SHOULD cache EPG responses with a shorter configurable TTL.
- **FR-040**: The proxy MUST serve fresh cache without calling the backend.
- **FR-041**: The proxy MAY serve stale cache when backend refresh fails, but MUST mark the response as stale.
- **FR-042**: The proxy MUST write cache updates atomically so interrupted refreshes do not corrupt existing cache.
- **FR-043**: Persistent cache files MUST NOT contain provider password, raw client tokens, authenticator hex, or session cookies unless a later design explicitly encrypts them.
- **FR-044**: Persistent channel cache SHOULD store resolved playable stream URLs. If raw backend URLs are also retained for diagnostics or future re-resolution, they MUST NOT be exposed to clients by default.

### Testing & Observability

- **FR-045**: The proxy MUST include unit tests for token generation/validation, config redaction, cache TTL decisions, CTC response parsing, and stream URL rewriting.
- **FR-046**: The proxy MUST include integration tests with a mock IPTV backend covering login, channel fetch, stream URL rewrite, missing `udpxy` failure, cache hit, cache miss, stale fallback, and token rejection.
- **FR-047**: Logs MUST include request outcome, cache hit/miss, refresh outcome, and token client name when known.
- **FR-048**: Logs MUST NOT include provider password, raw local tokens, backend session IDs, or authenticator values.

## Key Entities

- **ProviderConfig**: Provider credentials and endpoint fields needed for CTC login. Password is write-only in admin UI and redacted in status.
- **StreamProxyConfig**: `udpxy` base URL and stream URL rewrite policy used to convert backend multicast URLs into playable HTTP URLs.
- **ClientToken**: Named local API credential represented by hash, created timestamp, optional last-seen timestamp, and enabled flag.
- **BackendSession**: Internal runtime state containing EPG base URL, session cookie, user token, and expiry/refresh metadata.
- **ChannelCache**: Normalized channel list plus refresh timestamp, TTL, stale flag, and backend source metadata.
- **ProxyConfig**: File-backed service configuration: listen addresses, LAN CIDRs, cache TTLs, admin hash, provider config, and token metadata.

## Success Criteria

- **SC-001**: A fresh OpenWrt x86_64 deployment can start the proxy, open the LAN admin page, save provider credentials, create a client token, and fetch channels from an Android client.
- **SC-002**: Two Android clients can import channels through the proxy without either client knowing the provider password.
- **SC-003**: Ten channel-list requests within the configured TTL produce one backend fetch at most.
- **SC-004**: Revoking one client token blocks that client while other tokens continue working.
- **SC-005**: A backend multicast URL is returned to ATV clients as a playable `udpxy` HTTP URL.
- **SC-006**: Test coverage includes backend mock flows, stream URL rewrite behavior, and cache behavior sufficient to prevent regressions in auth/session/channel import.
- **SC-007**: A recursive search of logs and persisted config/cache shows no plaintext provider password, raw local token, backend session ID, or authenticator hex.

## Assumptions

- The router runs x86_64 OpenWrt and has enough storage for one Rust binary plus small JSON/TOML state files.
- The proxy is for personal home-network use with the user's own IPTV subscription.
- LAN firewalling is controlled by the router admin.
- The first implementation can use HTTP on the LAN; HTTPS can be added later if OpenWrt certificate management is desired.
- Provider protocol behavior matches the CTC flow already ported in the ATV Android client.
- `udpxy` runs on the router or another LAN host reachable by Android TV clients.

## Out of Scope

- Sharing the proxy outside the home network.
- Multi-provider support.
- A mobile pairing app.
- A frontend JavaScript application for admin UI.
- Full transparent emulation of every provider endpoint.
- Streaming/transcoding media data through the proxy. The proxy rewrites stream URLs to an existing `udpxy` service but does not relay media itself.

## Technical Decisions

- **Rust over Go**: The user prefers Rust and the target OpenWrt environment does not already contain Go tooling. Rust also gives a small single-binary deployment model.
- **Normalized API over transparent provider clone**: The proxy serves the shape the ATV client needs instead of pretending to be the CTC backend. This keeps the proxy contract stable even if backend quirks change.
- **Admin-issued bearer tokens**: The proxy cannot know an arbitrary LAN client is legitimate. Tokens are created by an authenticated admin action and copied to the Android client out of band.
- **Cache-first backend access**: The router should appear as one well-behaved client to the IPTV backend, so channel/EPG refreshes are TTL-driven and coalesced.
- **Proxy-owned `udpxy` rewrite**: In Home proxy mode, Android clients should receive playable stream URLs and should not need local multicast or `udpxy` settings. The router owns that topology.
