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

## CI/CD

Pull requests and pushes run:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

Releases use the Rust crate version in `Cargo.toml` as the source of truth. To cut a new release, bump `[package].version`, merge to `main`, and the release workflow creates the matching tag and GitHub Release:

```toml
[package]
version = "0.2.1"
```

If the matching tag already exists, the workflow skips release creation on push. Manual workflow dispatch rebuilds the release for the current `Cargo.toml` version.

The release artifact is intended as the initial OpenWrt x86_64 deployment binary. Native OpenWrt package/feed publishing can be added once the service layout stabilizes.

See [docs/openwrt.md](docs/openwrt.md) for the current OpenWrt service setup.

Quick install on OpenWrt after a release is available:

```sh
curl -fsSL https://raw.githubusercontent.com/ultracold273/atv-iptv-proxy/main/deploy/openwrt/install.sh | sh
```

After installation, set `admin_password_hash` in `/etc/atv-iptv-proxy/config.json` and restart the service. The proxy refuses to start with the example placeholder or the built-in default password hash. See [docs/openwrt.md](docs/openwrt.md) for the exact OpenWrt commands.
