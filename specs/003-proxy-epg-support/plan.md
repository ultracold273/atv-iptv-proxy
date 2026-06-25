# Feature 003 Plan: Proxy EPG Support

## Architecture

Add a CTC EPG fetch path beside the existing CTC channel fetch path:

- Reuse `ctc::login` and `LoginSession`.
- Build the `prevue_list.jsp` URL from `LoginSession.epg_lb_base`.
- Parse CTC `channelPrevue` JSON into normalized proxy `Program` DTOs.
- Cache responses in memory by `(channelCode, dateOffset)`.

The HTTP server stays dependency-light and keeps the current hand-written route parser.

## Implementation Tasks

- [x] Add `Program`, `EpgResponse`, and `EpgCache` types.
- [x] Add `ctc::fetch_programs` and CTC program parser tests.
- [x] Add config field `epg_cache_ttl_seconds` with a sensible default.
- [x] Add route `GET /api/v1/epg/day` with token auth, query parsing, validation, cache hit, refresh, and stale fallback.
- [x] Add server tests for auth, validation, cache hit, and stale fallback.
- [x] Update OpenWrt example config and docs.
- [x] Run `cargo fmt`, `cargo clippy`, `cargo test`, and shell syntax checks.

## Test Plan

- Parser unit tests for compact and dotted CTC timestamp formats.
- Server unit tests for missing token and invalid query.
- Cache unit tests for freshness and response metadata.
- CTC URL-building tests for `channelcode`, `dateindex`, and cookie use where practical.

## Compatibility

Existing `/api/v1/channels` behavior is unchanged. Existing config files without `epg_cache_ttl_seconds` should load using the default value through serde defaults.
