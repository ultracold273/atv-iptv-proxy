#!/bin/sh
set -eu

REPO="${ATV_PROXY_REPO:-ultracold273/atv-iptv-proxy}"
TAG="${ATV_PROXY_VERSION:-latest}"
ARCHIVE_NAME="${ATV_PROXY_ARCHIVE:-atv-iptv-proxy-linux-x86_64.tar.gz}"
INSTALL_DIR="${ATV_PROXY_INSTALL_DIR:-/usr/bin}"
CONFIG_DIR="${ATV_PROXY_CONFIG_DIR:-/etc/atv-iptv-proxy}"
INIT_PATH="${ATV_PROXY_INIT_PATH:-/etc/init.d/atv-iptv-proxy}"
BIN_PATH="${INSTALL_DIR}/atv-iptv-proxy"
CONFIG_PATH="${CONFIG_DIR}/config.json"
TMP_DIR="${TMPDIR:-/tmp}/atv-iptv-proxy-install.$$"

log() {
    printf '%s\n' "atv-iptv-proxy install: $*"
}

fail() {
    printf '%s\n' "atv-iptv-proxy install: ERROR: $*" >&2
    exit 1
}

need_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

download() {
    url="$1"
    output="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$output"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$output" "$url"
    else
        fail "missing curl or wget"
    fi
}

cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

[ "$(id -u)" = "0" ] || fail "run as root on OpenWrt"
need_cmd tar
need_cmd chmod
need_cmd mkdir
need_cmd mv

if [ "$TAG" = "latest" ]; then
    RELEASE_BASE="https://github.com/${REPO}/releases/latest/download"
else
    RELEASE_BASE="https://github.com/${REPO}/releases/download/${TAG}"
fi

mkdir -p "$TMP_DIR"
ARCHIVE_PATH="${TMP_DIR}/${ARCHIVE_NAME}"

log "downloading ${RELEASE_BASE}/${ARCHIVE_NAME}"
download "${RELEASE_BASE}/${ARCHIVE_NAME}" "$ARCHIVE_PATH"

if command -v sha256sum >/dev/null 2>&1; then
    if download "${RELEASE_BASE}/${ARCHIVE_NAME}.sha256" "${ARCHIVE_PATH}.sha256"; then
        (cd "$TMP_DIR" && sha256sum -c "${ARCHIVE_NAME}.sha256") || fail "checksum verification failed"
    else
        log "checksum file unavailable; continuing without checksum verification"
    fi
fi

tar -xzf "$ARCHIVE_PATH" -C "$TMP_DIR"
[ -f "${TMP_DIR}/atv-iptv-proxy-linux-x86_64" ] || fail "release archive did not contain expected binary"

mkdir -p "$INSTALL_DIR" "$CONFIG_DIR"
mv "${TMP_DIR}/atv-iptv-proxy-linux-x86_64" "$BIN_PATH"
chmod 0755 "$BIN_PATH"

cat > "$INIT_PATH" <<'INIT_EOF'
#!/bin/sh /etc/rc.common

START=95
STOP=10
USE_PROCD=1

PROG=/usr/bin/atv-iptv-proxy
CONFIG=/etc/atv-iptv-proxy/config.json

start_service() {
    procd_open_instance
    procd_set_param command "$PROG"
    procd_set_param env ATV_PROXY_CONFIG="$CONFIG"
    procd_set_param respawn 3600 5 5
    procd_set_param stdout 1
    procd_set_param stderr 1
    procd_close_instance
}

reload_service() {
    stop
    start
}
INIT_EOF
chmod 0755 "$INIT_PATH"

if [ ! -f "$CONFIG_PATH" ]; then
    cat > "$CONFIG_PATH" <<'CONFIG_EOF'
{
  "listen": "192.168.1.1:8088",
  "admin_password_hash": "sha256:replace-with-hash-from-admin-tool",
  "channel_cache_ttl_seconds": 3600,
  "backend_channels_url": null,
  "provider": null,
  "stream": {
    "udpxy_base_url": "http://192.168.1.1:4022"
  },
  "tokens": []
}
CONFIG_EOF
    chmod 0600 "$CONFIG_PATH"
    log "created config at ${CONFIG_PATH}; edit provider credentials before production use"
else
    log "keeping existing config at ${CONFIG_PATH}"
fi

if [ -x "$INIT_PATH" ]; then
    "$INIT_PATH" enable || true
    "$INIT_PATH" restart || "$INIT_PATH" start || true
fi

log "installed ${BIN_PATH}"
log "service: ${INIT_PATH}"
log "config: ${CONFIG_PATH}"
log "edit config, then run: ${INIT_PATH} restart"

