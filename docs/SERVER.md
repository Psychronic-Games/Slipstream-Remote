# Running a Slipstream server

`slipstream-server` is one small binary with two jobs:

1. **Directory** (UDP 21116) — hands out 9-digit IDs and tells viewers how to
   reach a host. It never sees passwords or session content.
2. **Relay** (TCP 21117, or 80/443 with TLS) — forwards *encrypted* traffic
   when two devices can't connect directly (strict NATs, some mobile
   networks). Most sessions go peer-to-peer and never touch it.

A tiny VPS is plenty: the directory is a few KB per device, and relayed
sessions are the exception.

## Ports

| Port | Protocol | Purpose |
|---|---|---|
| 21116 | UDP | Directory (QUIC) |
| 21117 | TCP | Relay over HTTP (default) |
| 80 + 443 | TCP | Relay over HTTPS, if `--acme-domain` or `--tls-cert` is used |

Open these in your firewall / cloud security group, or forward them on your
router if you run the server at home.

## Option A: plain binary

```sh
cargo build --release -p slipstream-server
./target/release/slipstream-server --public-host your.server.example
```

The first run creates `./slipstream-data/` with the server key (keep it — it
is the server's identity; clients pin it) and the ID database. The client
address is printed and also written to `slipstream-data/server-address.txt`.

## Option B: systemd

```sh
sudo useradd --system --home /var/lib/slipstream slipstream
sudo install -Dm755 target/release/slipstream-server /usr/local/bin/
sudo cp packaging/server/slipstream-server.service /etc/systemd/system/
sudo systemctl edit slipstream-server   # override SLIPSTREAM_PUBLIC_HOST
sudo systemctl enable --now slipstream-server
sudo cat /var/lib/slipstream/server-address.txt
```

## Option C: Docker

```sh
docker build -f packaging/server/Dockerfile -t slipstream-server .
docker run -d --name slipstream --restart unless-stopped \
  -p 21116:21116/udp -p 21117:21117 \
  -v slipstream-data:/data \
  -e SLIPSTREAM_PUBLIC_HOST=your.server.example \
  slipstream-server
docker exec slipstream cat /data/server-address.txt
```

## HTTPS for the relay (optional)

Relay traffic is already end-to-end encrypted, so TLS only hides metadata —
but some networks only allow ports 80/443. With a domain pointing at the
server:

```sh
slipstream-server --public-host relay.example.com \
  --acme-domain relay.example.com --acme-email you@example.com
```

or bring your own certificate with `--tls-cert` / `--tls-key`.

## Useful flags

```
--port <n>                 directory UDP port (default 21116)
--relay-http <addr>        relay HTTP listen address (default [::]:21117)
--no-relay                 directory only; clients use public relays
--advertise-relay <url>    also advertise other relays (other regions)
--motd <text>              message shown on clients' Bridge page
```

## Backups

Back up the data directory. Losing `server.key` means every client must paste
the new address; losing `directory.sqlite3` means devices get new IDs.
