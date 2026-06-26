# OpenWrt Deployment

Initial target: x86_64 OpenWrt with an existing `udpxy` service.

## Quick Install

After a GitHub release exists, install the latest x86_64 build directly on the OpenWrt shell:

```sh
curl -fsSL https://raw.githubusercontent.com/ultracold273/atv-iptv-proxy/main/deploy/openwrt/install.sh | sh
```

If your shell has `bash` installed and you prefer the requested form:

```sh
curl -fsSL https://raw.githubusercontent.com/ultracold273/atv-iptv-proxy/main/deploy/openwrt/install.sh | bash
```

To pin a specific release tag:

```sh
curl -fsSL https://raw.githubusercontent.com/ultracold273/atv-iptv-proxy/main/deploy/openwrt/install.sh | ATV_PROXY_VERSION=v0.1.0 sh
```

The installer:

- downloads `atv-iptv-proxy-openwrt-x86_64.tar.gz` from GitHub Releases;
- verifies the SHA-256 file when `sha256sum` is available;
- installs `/usr/bin/atv-iptv-proxy`;
- installs `/etc/init.d/atv-iptv-proxy` from `deploy/openwrt/atv-iptv-proxy.init`;
- creates `/etc/atv-iptv-proxy/config.json` from `deploy/openwrt/config.example.json` if missing;
- creates `/etc/atv-iptv-proxy/channel-number-overrides.json` from `deploy/openwrt/channel-number-overrides.example.json` if missing;
- enables and restarts the service.

By default, `ATV_PROXY_VERSION=latest` downloads release assets from the latest release and deployment files from `main`. For a pinned release, the installer downloads deployment files from the same tag. Override `ATV_PROXY_SOURCE_REF` if you need a different branch or commit for the deployment files.

Review and edit `/etc/atv-iptv-proxy/config.json` after installation. The service will not start until `admin_password_hash` is set to a real SHA-256 hash.

## Install Binary

Copy the release binary to the router:

```sh
scp atv-iptv-proxy-openwrt-x86_64 root@192.168.1.1:/usr/bin/atv-iptv-proxy
ssh root@192.168.1.1 chmod +x /usr/bin/atv-iptv-proxy
```

## Install Config

```sh
ssh root@192.168.1.1 mkdir -p /etc/atv-iptv-proxy
scp deploy/openwrt/config.example.json root@192.168.1.1:/etc/atv-iptv-proxy/config.json
scp deploy/openwrt/channel-number-overrides.example.json root@192.168.1.1:/etc/atv-iptv-proxy/channel-number-overrides.json
```

Edit `/etc/atv-iptv-proxy/config.json` and set:

- `listen` to the LAN router address and proxy port.
- `provider` fields for the IPTV backend.
- `stream.udpxy_base_url` to the HTTP address clients can reach.
- `channel_cache_ttl_seconds` for channel-list refresh frequency.
- `epg_cache_ttl_seconds` for program-guide refresh frequency. The default is 300 seconds.
- `admin_password_hash` to a SHA-256 hash generated from your admin password. Keep the plaintext password out of the config file.

Generate the initial admin password hash on OpenWrt:

```sh
ADMIN_PASSWORD='replace-with-a-long-password'
ADMIN_HASH="sha256:$(printf '%s' "$ADMIN_PASSWORD" | sha256sum | awk '{print $1}')"
printf '%s\n' "$ADMIN_HASH"
```

Edit `/etc/atv-iptv-proxy/config.json` and replace the `admin_password_hash` value with the printed hash:

```sh
vi /etc/atv-iptv-proxy/config.json
```

Then restart the service:

```sh
/etc/init.d/atv-iptv-proxy restart
```

If the service does not start, check the log:

```sh
logread -e atv-iptv-proxy
```

The service passes the config and optional channel-number override files as startup arguments:

```sh
/usr/bin/atv-iptv-proxy \
  --config /etc/atv-iptv-proxy/config.json \
  --channel-number-overrides /etc/atv-iptv-proxy/channel-number-overrides.json
```

Optional channel-number overrides use this shape:

```json
{
  "ch00000000000000000238": {
    "name": "湖南卫视HD",
    "number": 80
  },
  "11CHANcp000011250908000239595493": {
    "name": "湖南卫视4KSDR",
    "number": 800
  }
}
```

The top-level key is `channelCode`. `name` is only for human reference and is ignored by the server. Override numbers take precedence over the IPTV backend mapping, then the proxy falls back to the next unused number. If a lower-priority source tries to reuse an already assigned number, that later channel is moved to fallback numbering so the final channel list has unique numbers.

Remove `/etc/atv-iptv-proxy/channel-number-overrides.json` or edit the init script command if you do not want overrides.

## Install Service

```sh
scp deploy/openwrt/atv-iptv-proxy.init root@192.168.1.1:/etc/init.d/atv-iptv-proxy
ssh root@192.168.1.1 chmod +x /etc/init.d/atv-iptv-proxy
ssh root@192.168.1.1 /etc/init.d/atv-iptv-proxy enable
ssh root@192.168.1.1 /etc/init.d/atv-iptv-proxy start
```

## Firewall

Bind `listen` to the LAN IP where possible. If the service listens on all interfaces, add firewall rules so only LAN clients can reach it.

The proxy does not relay media itself. It returns `udpxy` URLs such as:

```text
http://192.168.1.1:4022/udp/239.0.0.1:8000
```

Make sure Android TV clients can reach both the proxy port and the `udpxy` port on the LAN.

## EPG API

Authorized clients can query program-guide data through the proxy:

```http
GET /api/v1/epg/day?channelCode=ch1&dateOffset=0
Authorization: Bearer atv_living-room_...
```

`dateOffset` follows the CTC backend `dateindex` convention: `-1` means tomorrow, `0` means today, and `1` means yesterday. Responses use normalized program JSON with `start` and `end` as ISO-8601 strings. The proxy caches EPG data by channel and date offset, and may return stale cached guide data if the IPTV backend is temporarily unavailable.

## Admin API Examples

The admin API uses the plaintext admin password in the `x-admin-password` header. The service verifies it against `admin_password_hash` in the config file. These examples assume the proxy is listening on the OpenWrt LAN address and that the admin password has already been configured.

```sh
PROXY_URL="http://192.168.1.1:8088"
ADMIN_PASSWORD="replace-with-admin-password"
```

Check that the proxy is reachable:

```sh
curl -fsS "$PROXY_URL/health"
```

Save the HTTP channel backend URL and the `udpxy` base URL:

```sh
curl -fsS -X POST "$PROXY_URL/admin/config" \
  -H "x-admin-password: $ADMIN_PASSWORD" \
  --data-urlencode "backend_channels_url=http://192.168.1.100:9000/channels" \
  --data-urlencode "udpxy_base_url=http://192.168.1.1:4022"
```

The response should be:

```json
{"ok":true}
```

To clear the HTTP channel backend and rely on the configured CTC provider instead, send an empty `backend_channels_url`:

```sh
curl -fsS -X POST "$PROXY_URL/admin/config" \
  -H "x-admin-password: $ADMIN_PASSWORD" \
  --data-urlencode "backend_channels_url=" \
  --data-urlencode "udpxy_base_url=http://192.168.1.1:4022"
```

Create a token for one Android TV client:

```sh
curl -fsS -X POST "$PROXY_URL/admin/tokens" \
  -H "x-admin-password: $ADMIN_PASSWORD" \
  --data-urlencode "name=living-room-tv"
```

The raw token is returned only in this response:

```json
{"token":"atv_living-room-tv_..."}
```

Android TV clients can also pair without typing the token on the TV. First, start pairing from the Android app's Home Proxy tab. Then list pending sessions from an admin shell:

```sh
curl -fsS "$PROXY_URL/admin/api/v1/pairing/sessions?status=pending" \
  -H "x-admin-password: $ADMIN_PASSWORD"
```

The response includes the short code shown on the TV:

```json
{
  "data": [
    {
      "sessionId": "ps_...",
      "pairingCode": "482913",
      "deviceName": "Living Room ATV",
      "deviceType": "android_tv",
      "appId": "com.example.atv",
      "appVersion": "1.0.0",
      "createdAt": 1782543000,
      "expiresAt": 1782543300
    }
  ]
}
```

Approve the code and optionally choose the token label shown in the proxy config:

```sh
curl -fsS -X POST "$PROXY_URL/admin/api/v1/pairing/approve" \
  -H "x-admin-password: $ADMIN_PASSWORD" \
  -H "Content-Type: application/json" \
  -d '{"pairingCode":"482913","deviceLabel":"living-room-tv"}'
```

The Android client receives the generated token through its pending pairing session. To reject a pending code instead:

```sh
curl -fsS -X POST "$PROXY_URL/admin/api/v1/pairing/reject" \
  -H "x-admin-password: $ADMIN_PASSWORD" \
  -H "Content-Type: application/json" \
  -d '{"pairingCode":"482913"}'
```

If `jq` is available, capture the token for testing:

```sh
TOKEN="$(curl -fsS -X POST "$PROXY_URL/admin/tokens" \
  -H "x-admin-password: $ADMIN_PASSWORD" \
  --data-urlencode "name=bedroom-tv" | jq -r '.token')"
```

Use the token with the client APIs:

```sh
curl -fsS "$PROXY_URL/api/v1/channels" \
  -H "Authorization: Bearer $TOKEN"
```

```sh
curl -fsS "$PROXY_URL/api/v1/epg/day?channelCode=ch1&dateOffset=0" \
  -H "Authorization: Bearer $TOKEN"
```

Current admin config posts update `backend_channels_url` and `stream.udpxy_base_url`. Configure CTC provider credentials, cache TTLs, listen address, and `admin_password_hash` in `/etc/atv-iptv-proxy/config.json`, then restart the service.
