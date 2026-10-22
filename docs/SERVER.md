# Running your own Slipstream Station

Slipstream isn't a central service. Anyone — an IT firm, a company, a family
— can run their own **station**: one self-contained web folder that gives
them a private remote-support service:

- **Download page** — clients get apps that connect to your
  station by themselves: unsigned builds carry its address, access key and
  your organisation's name; signed builds and the Mac app pick them up from
  the download address, a *Connect this app* button or a setup code shown on
  the page. Usually nothing to type.
- **Private directory** — the 9-digit IDs of *your* clients' devices, listed
  in your admin page with their online status. Apps from other stations
  can't use yours.
- **Relay** — when two computers can't reach each other directly (strict
  firewalls), traffic goes through your station instead of someone else's.
  It's end-to-end encrypted; the station can't see screens or passwords.
- **Technician accounts** — your technicians sign in from the app (any
  app, even one from another organisation's station) and keep their own
  list of devices here, plus a shared list you manage.
- **Admin page** — setup, health checks, devices, technicians, settings,
  access key.

Move it to another server any time: copy the folder and the data folder,
repoint DNS, done. Apps only know your domain.

New to Slipstream? [GETTING-STARTED.md](GETTING-STARTED.md) walks through
the whole setup step by step. This page is the reference.

## Requirements

- A domain or subdomain with HTTPS, e.g. `support.example.com`. The station
  must live at the **root** of it (not in a subfolder).
- PHP 8.0+ with the `sodium` extension (standard).
- Linux web host where PHP may start programs (`proc_open`), Apache with
  `mod_rewrite` + `mod_proxy_http` (2.4.47+) and `.htaccess` allowed. That's
  the default on Virtualmin, cPanel/WHM and most VPS setups.

Without the last point the directory and download page still work. Relay-only
connections need public fallback enabled by both the station and client, or a
separately managed relay. (See *Other web servers* below.)

## Install

1. Build or download the station package: `scripts/build-station.sh` creates
   `dist/slipstream-station/` with the PHP site, the relay program (`bin/`)
   and the client apps (`clients/`).
2. Upload the *contents* of that folder to your site's web root
   (`public_html`), keeping the folder structure.
3. Open `https://support.example.com/admin/`, enter your organisation's name
   and an admin password. The station creates its settings, its access key,
   its relay routing (in `.htaccess`) and starts the relay.
4. Check the **Status** tab: every line should be green.
5. Send your clients to `https://support.example.com/`.

### Virtualmin example

Create the virtual server (e.g. `relay.example.com`), request a Let's
Encrypt certificate (*Server Configuration → SSL Certificate*), upload the
files to `/home/<site>/public_html` as the site's user, open `/admin/`.
No Apache or root changes are needed.

## Updating

Build a new package (`scripts/build-station.sh`) and upload the files
again, replacing `bin/`, `clients/` and the PHP files. Upload the apps
before `clients/manifest.json`, so no app is offered a file that isn't there
yet. Your settings and device list live in the data folder and are
untouched. The full release checklist is in [BUILDING.md](BUILDING.md).

`clients/manifest.json` (written by the build script) records the version,
build number, checksums and release signatures. Desktop apps check your station about 20 seconds after
they start and every 6 hours; when a newer build is there they offer
**Install & restart**. The update is downloaded from your station
(preconfigured, like the download page), checked against the release key
built into the app, swapped in, and the app restarts — keeping its settings, ID and passwords.
Headless machines can run `slipstream update`.

Android APKs use their own release certificate and are outside the desktop
update manifest. Users install the newer APK from the station over the existing
app; Android self-update is not implemented.

The release key stays on the machine that builds the package. Someone who
breaks into the web server can't push their own app as an update: the apps
refuse anything not signed with that key. See
[BUILDING.md](BUILDING.md#the-release-signing-key).

Apps installed in a protected folder (e.g. by a system package) can't
replace themselves and point users to the download page instead.

To restart the relay with a new `bin/` program, use **Restart relay** on
the admin page (otherwise it switches on its next restart).

## Where data lives

Device records and station settings go in `slipstream-data/` **next to**
the web root (`/home/<site>/slipstream-data`), outside public reach. If that
isn't writable, `./data` inside the site is used, protected by `.htaccess`.
Copy `config.sample.php` to `config.php` to choose another folder or to use
MariaDB/MySQL instead of files.

Back up `slipstream-data/`: it holds the access key, the device IDs, and the
technicians (`accounts/`, `tokens/`, `shared.json`).

## The relay

The relay is the `slipstream-server` program in `bin/`. The PHP starts it as
your site's own user on a private local port, restarts it automatically if
it stops (e.g. after a reboot, the next app check-in revives it), and
`.htaccess` forwards `/relay` to it. Start, stop and its log are on the
admin page.

## Meetings

Private-station meeting guests need the scoped relay-lease support introduced
in 0.23.0. Upgrade both the station API and managed relay binary, then restart
the relay when upgrading a pre-meetings station. A client-only update does not
add this server support. The production station received that runtime upgrade
with 0.23.0; 0.23.1, 0.23.2 and 0.24.0 were client-only publications.

The authenticated `meeting_relay` operation issues expiring relay-only guest
credentials. These do not authorize directory/account access and keep the
station-wide access key out of invitations. Admission remains the organizer's
responsibility. Media is forwarded by the organizer's client; the station
provides directory/relay services rather than a meeting media server. See
[meeting usage](MEETINGS.md) and the
[deployment handoff](DEVELOPMENT-2026-10-21.md) for limits and rollback details.

## Access control

Stations are **private** by default: every request must carry the station's
access key, which is built into the apps from its download page. If an app
gets into the wrong hands, make a new key on the Settings tab — everyone then
needs to download the app again. You can also put a password on the
download page.

## Technicians

The **Technicians** tab of the admin page adds technician accounts
(username, name, password of 10+ characters) and makes some of them station
admins. A technician signs in from the app under *Config → Stations* with
the station's address, username and password. That gives their app the
station's access key, so it works even from an app downloaded from another
station, and a sign-in token (kept on the station only as a hash).

Each technician keeps a list of devices on the station. Passwords they save
for devices arrive encrypted by the app with a key made from their sign-in
password (Argon2id, then XChaCha20-Poly1305, bound to the device's ID): the
data folder alone can't reveal them. Setting a new password for a technician
signs them out everywhere and clears the passwords they had saved, which
can't be opened any more. *Sign out everywhere* ends every sign-in at once.

The **Shared devices** list on the same tab is seen by every technician,
without passwords. Station admins can also change it from the app.

Sign-ins are rate-limited per address and per account. The standalone
`slipstream-server` station doesn't have accounts.

## Other web servers

**Nginx (+ PHP-FPM):** `.htaccess` isn't read, so add the relay routing and
the folder protections to the server block — see
[`packaging/server/nginx-relay.conf`](../packaging/server/nginx-relay.conf),
using the port shown on the admin page.

**No PHP at all:** run the `slipstream-server` binary on its own — it serves
the same directory API, the relay and a status page on one port:

```sh
slipstream-server --listen [::]:8080 --access-key <random-string> --org-name "Acme IT"
```

and put HTTPS in front of it (e.g. Caddy: `reverse_proxy 127.0.0.1:8080`).
Configure apps with `slipstream set-server your.host --key <random-string>`.
`packaging/server/slipstream-server.service` keeps it running under systemd.

## Troubleshooting

- `SLIPSTREAM_RELAY_ONLY=1 slipstream connect <ID> --password …` forces a
  connection through the relay — the stats line then says `RELAY`.
- Relay shows **NOT ROUTED**: Apache isn't forwarding `/relay` — check that
  `mod_proxy_http` and `mod_rewrite` are enabled and `.htaccess` is allowed.
