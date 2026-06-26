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
OVERRIDES_PATH="${CONFIG_DIR}/channel-number-overrides.json"
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
    SOURCE_REF="${ATV_PROXY_SOURCE_REF:-main}"
else
    RELEASE_BASE="https://github.com/${REPO}/releases/download/${TAG}"
    SOURCE_REF="${ATV_PROXY_SOURCE_REF:-${TAG}}"
fi
RAW_BASE="https://raw.githubusercontent.com/${REPO}/${SOURCE_REF}"

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

log "installing init script from ${RAW_BASE}/deploy/openwrt/atv-iptv-proxy.init"
download "${RAW_BASE}/deploy/openwrt/atv-iptv-proxy.init" "$INIT_PATH"
chmod 0755 "$INIT_PATH"

CREATED_CONFIG=0
if [ ! -f "$CONFIG_PATH" ]; then
    log "installing config template from ${RAW_BASE}/deploy/openwrt/config.example.json"
    download "${RAW_BASE}/deploy/openwrt/config.example.json" "$CONFIG_PATH"
    chmod 0600 "$CONFIG_PATH"
    log "created config at ${CONFIG_PATH}; edit provider credentials before production use"
    CREATED_CONFIG=1
else
    log "keeping existing config at ${CONFIG_PATH}"
fi

if [ ! -f "$OVERRIDES_PATH" ]; then
    log "installing channel number override template from ${RAW_BASE}/deploy/openwrt/channel-number-overrides.example.json"
    download "${RAW_BASE}/deploy/openwrt/channel-number-overrides.example.json" "$OVERRIDES_PATH"
    chmod 0644 "$OVERRIDES_PATH"
    log "created channel number overrides at ${OVERRIDES_PATH}; edit or remove it as needed"
else
    log "keeping existing channel number overrides at ${OVERRIDES_PATH}"
fi

if [ -x "$INIT_PATH" ]; then
    "$INIT_PATH" enable || true
    if [ "$CREATED_CONFIG" = "0" ]; then
        "$INIT_PATH" restart || "$INIT_PATH" start || true
    else
        log "service enabled but not started; set admin_password_hash in ${CONFIG_PATH}, then run: ${INIT_PATH} restart"
    fi
fi

log "installed ${BIN_PATH}"
log "service: ${INIT_PATH}"
log "config: ${CONFIG_PATH}"
log "channel number overrides: ${OVERRIDES_PATH}"
log "edit config, then run: ${INIT_PATH} restart"
