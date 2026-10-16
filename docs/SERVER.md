# Running a Slipstream server

A Slipstream server does two jobs:

1. **Directory** — hands out 9-digit IDs and tells a viewer how to reach a
   host. It's a small JSON API over HTTPS. It never sees passwords or screen
   content, and every registration is signed by the device's own key.
2. **Relay** (optional) — forwards *encrypted* traffic when two devices can't
   connect directly. Most sessions are peer-to-peer and never touch it. Without
   your own relay, apps use the free public relays.

Apps only need your server's **web address** (e.g. `relay.example.com`), so
you can move it to another machine later by repointing DNS — nothing breaks.

Pick one:

| | Needs | Relay |
|---|---|---|
| **A. PHP folder** | any web host with PHP 8+ | public, or add the binary (A2) |
| **B. Binary** | a Linux/Windows/macOS machine | built in |

---

## A. PHP folder (simplest — works on shared hosting)

1. Create a site/subdomain, e.g. `relay.example.com`, with **HTTPS** (Let's
   Encrypt). In Virtualmin: *Create Virtual Server*, then *Server
   Configuration → SSL Certificate → Let's Encrypt → Request Certificate*.
2. Upload the contents of [`server/web/`](../server/web) into its web root
   (`public_html`): `index.php`, `slipstream.php`, `config.sample.php` and
   the `api/` folder.
3. Open `https://relay.example.com/` — the status page should say
   **ONLINE — READY FOR CONNECTIONS** with all checks green.
4. In each Slipstream app: **CONFIG → Server address → `relay.example.com`**.

That's it. No database is needed: device records are small files, kept
*outside* the web root when possible (`../slipstream-data`), otherwise in a
`data/` folder protected by `.htaccess`. For large installs, copy
`config.sample.php` to `config.php` and point `db` at MariaDB/MySQL.

PHP requirements: 8.0+, with the `sodium` extension (standard since PHP 7.2).

### A2. Add your own relay to the PHP site (optional)

Your own relay means relayed sessions don't depend on third-party servers.
It runs as a small background service on the same machine; Apache forwards
three URLs to it, so it shares the domain with the PHP folder.

```sh
# on the server, as root
install -Dm755 slipstream-server /usr/local/bin/slipstream-server
useradd --system --home /var/lib/slipstream slipstream
cp packaging/server/slipstream-server.service /etc/systemd/system/
systemctl edit slipstream-server     # add the two lines below under [Service]
#   Environment=SLIPSTREAM_LISTEN=127.0.0.1:21117
#   Environment=SLIPSTREAM_ARGS=--relay-only
systemctl enable --now slipstream-server
```

Then add [`packaging/server/apache-relay.conf`](../packaging/server/apache-relay.conf)
to the site's HTTPS virtual host (Virtualmin: *Web Configuration → Edit
Directives*) and reload Apache. Nginx users: see
[`nginx-relay.conf`](../packaging/server/nginx-relay.conf).

The status page notices the relay within 10 minutes and starts advertising it
to apps automatically.

---

## B. Standalone binary

One executable serves the directory, the relay and a status page on a single
port:

```sh
cargo build --release -p slipstream-server
./target/release/slipstream-server                    # listens on [::]:8080
```

Apps then use `http://your-server:8080`. For HTTPS, put it behind any
reverse proxy (Caddy, Nginx, Apache) — e.g. with Caddy:

```
relay.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

and run `slipstream-server --listen 127.0.0.1:8080`. Use the systemd unit in
[`packaging/server/`](../packaging/server) to keep it running.

| Flag / env | Meaning |
|---|---|
| `--listen` / `SLIPSTREAM_LISTEN` | address to listen on (default `[::]:8080`) |
| `--data-dir` / `SLIPSTREAM_DATA` | where the device database lives |
| `--public-url` / `SLIPSTREAM_PUBLIC_URL` | public URL to advertise for the relay (default: taken from each request) |
| `--relay-only` | relay only (next to the PHP directory) |
| `--no-relay` | directory only |
| `--advertise-relay <url>` | also advertise other relays (regions) |
| `--motd` / `SLIPSTREAM_MOTD` | message shown on apps' Bridge page |

Linux builds need glibc 2.34+ (Debian 12, Ubuntu 22.04, RHEL 9 or newer).

---

## Ports and firewalls

Everything runs over normal web ports. The apps reach the directory and relay
through HTTPS (443) or whatever port you serve them on; device-to-device
links use UDP and need no server ports at all.

## Backups

Back up the data folder (`slipstream-data/`, or the database). Losing it
means devices get new IDs the next time they connect.
