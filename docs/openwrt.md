# OpenWrt Deployment

Initial target: x86_64 OpenWrt with an existing `udpxy` service.

## Install Binary

Copy the release binary to the router:

```sh
scp atv-iptv-proxy-linux-x86_64 root@192.168.1.1:/usr/bin/atv-iptv-proxy
ssh root@192.168.1.1 chmod +x /usr/bin/atv-iptv-proxy
```

## Install Config

```sh
ssh root@192.168.1.1 mkdir -p /etc/atv-iptv-proxy
scp deploy/openwrt/config.example.json root@192.168.1.1:/etc/atv-iptv-proxy/config.json
```

Edit `/etc/atv-iptv-proxy/config.json` and set:

- `listen` to the LAN router address and proxy port.
- `provider` fields for the IPTV backend.
- `stream.udpxy_base_url` to the HTTP address clients can reach.
- `admin_password_hash` to a SHA-256 hash created by the proxy token/hash tooling once that CLI is added. Until then, generate it from a trusted machine and keep the plaintext out of the config.

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

