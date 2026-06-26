# Feature 003: Proxy EPG Support

## Summary

The proxy shall expose token-protected EPG endpoints so Android TV clients in Home Proxy mode can show program-guide data without authenticating directly to the IPTV backend.

## User Stories

### Story 1: Query Program Guide From LAN Client

As an authorized LAN client, I want to query the guide for a channel/day through the proxy, so the client never stores provider credentials.

Acceptance criteria:

- Given a valid client token and configured CTC provider, when the client calls `/api/v1/epg/day`, then the proxy logs in if needed, fetches the CTC guide, normalizes it, caches it, and returns JSON.
- Given a fresh cache entry, when another client requests the same channel/day, then the proxy serves the cache without calling the backend.
- Given the backend is unavailable but stale EPG data exists, then the proxy may return stale data with cache metadata.
- Given no provider is configured, then the proxy returns a structured `503` error.

### Story 2: Enforce Local Token Authorization

As the home network administrator, I want EPG queries protected by the same client tokens as channel import.

Acceptance criteria:

- Missing or invalid bearer tokens return `401`.
- EPG requests update token last-seen metadata using the existing token path.

## Functional Requirements

- **FR-001**: The proxy MUST expose `GET /api/v1/epg/day?channelCode=...&dateOffset=...`.
- **FR-002**: The proxy MUST require a valid bearer token for EPG endpoints.
- **FR-003**: The proxy MUST support CTC `dateOffset` values where `-1` means tomorrow, `0` means today, and `1` means yesterday.
- **FR-004**: The proxy MUST fetch CTC `prevue_list.jsp` using the authenticated EPG session.
- **FR-005**: The proxy MUST normalize CTC program entries into `code`, `name`, `start`, `end`, `isLive`, and `isReplayable`.
- **FR-006**: The proxy MUST cache EPG results by `(channelCode, dateOffset)` with a configurable TTL.
- **FR-007**: The proxy SHOULD return stale cached EPG data if the backend refresh fails.
- **FR-008**: The proxy MUST keep dependencies minimal and use existing `serde_json`/`ureq` plumbing.

## API Contract

Request:

```http
GET /api/v1/epg/day?channelCode=ch1&dateOffset=0
Authorization: Bearer atv_living-room_...
```

Response:

```json
{
  "data": [
    {
      "code": "p1",
      "name": "News",
      "start": "2026-06-07T08:00:00+08:00",
      "end": "2026-06-07T09:00:00+08:00",
      "isLive": true,
      "isReplayable": false
    }
  ],
  "cache": {
    "stale": false,
    "cachedAt": 1780780800,
    "ttlSeconds": 300
  }
}
```

## Non-Goals

- XMLTV generation.
- A separate admin UI for viewing guide data.
- Non-CTC provider-specific EPG integrations.

## Success Criteria

- Authorized clients can query EPG through the proxy.
- Repeated EPG requests for the same channel/day use cache.
- Unit tests cover authorization, validation, parsing, and stale-cache fallback.
