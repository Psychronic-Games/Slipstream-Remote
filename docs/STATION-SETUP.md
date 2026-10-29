# Setting up your own Slipstream station

A **station** is your organisation's own private Slipstream service. It's one
folder of files on an ordinary website, and it gives you:

- a **download page** with apps preconfigured for your station (your clients
  just download and run them),
- a private **directory** of your clients' devices and their IDs,
- a **relay** for connections that can't go direct,
- an **admin page** for settings, devices and technician accounts,
- **automatic updates**: the station installs new Slipstream releases from
  GitHub by itself, and the apps then update from the station.

It runs on normal shared or VPS hosting with PHP: no Docker, no database
server and no root access. This guide takes about 15 minutes.

> Just want to help someone once? You don't need a station: use the app with
> [Slipstream Public](https://public.slipstreamremote.com/).

## What you need

| | |
|---|---|
| **A (sub)domain** | e.g. `support.example.com`. The station must be at the root of it, not in a subfolder. |
| **Linux web hosting** | Apache with `.htaccess` allowed, `mod_rewrite` and `mod_proxy_http` (Apache 2.4.47+). This is the default on Virtualmin, cPanel/WHM and most VPS panels. Nginx works too ([SERVER.md](SERVER.md#other-web-servers)). |
| **PHP 8.0 or newer** | With the standard `sodium` extension. |
| **HTTPS** | A free Let's Encrypt certificate is fine. |
| **About 400 MB of disk space** | The apps for every platform, plus room for one update. |

If your host doesn't let PHP start programs (`proc_open`), everything except
the built-in relay still works; see [SERVER.md](SERVER.md#other-web-servers).

## Step 1: Create the website

Point a DNS record (`A`/`AAAA`) for your subdomain at your server and create
a website for it with HTTPS.

**Virtualmin**

1. **Create Virtual Server**: enter the domain (e.g. `support.example.com`) and
   keep *website* and *SSL website* enabled.
2. **Server Configuration → SSL Certificate → Let's Encrypt → Request Certificate**.
3. The web root is `/home/<site-user>/public_html`. Virtualmin sites use
   PHP-FPM by default (**Server Configuration → PHP Options**).

**cPanel**

1. Add the subdomain under **Domains**.
2. Run **SSL/TLS Status → AutoSSL**.
3. The web root is shown next to the domain. Pick PHP 8 under **MultiPHP Manager**.

Open `https://support.example.com/` once to check the site and certificate work.

## Step 2: Download the station

From the [latest release](https://github.com/Psychronic-Games/Slipstream-Remote/releases/latest),
download **`slipstream-station.zip`** and unzip it on your computer.
You get a `slipstream-station` folder:

```
slipstream-station/
├── index.php        ← the download page
├── download.php
├── admin/           ← the admin page
├── api/             ← the directory the apps talk to
├── assets/
├── bin/             ← the relay program
├── clients/         ← the apps for Windows, Linux and Android, and manifest.json
└── lib/             ← station code, its version and the release key it trusts
```

## Step 3: Upload it

Upload the **contents** of `slipstream-station/` (not the folder itself) into
the web root, keeping the subfolders. Upload as the **site's own user**, so
the files belong to it. Any of these works:

- **Your panel's file manager**: upload the zip into the web root, use
  **Extract**, then move everything out of the `slipstream-station` folder
  into the web root and delete the empty folder and the zip.
- **SFTP** (FileZilla, WinSCP, Cyberduck): log in as the site user and drag
  the folder's contents into `public_html`.
- **SSH**, as the site user:

  ```sh
  cd ~/public_html
  curl -LO https://github.com/Psychronic-Games/Slipstream-Remote/releases/latest/download/slipstream-station.zip
  unzip slipstream-station.zip
  cp -r slipstream-station/. . && rm -r slipstream-station slipstream-station.zip
  chmod 755 bin/slipstream-server-linux-x86_64 clients/slipstream-linux-x86_64
  ```

Make sure `bin/slipstream-server-linux-x86_64` is executable (mode 755). File
managers and SFTP usually keep it; if the relay won't start in Step 5, set
the permission by hand.

## Step 4: Run the setup

Open `https://support.example.com/admin/` and enter:

- **Organisation name**: shown to your clients in the app and on the
  download page, and used in download file names ("Slipstream Remote - Acme IT.exe").
- **Password for admin**: at least 10 characters. Setup creates the first technician account, **admin**, with station administrator permission. Store its credentials in your password manager.

The station creates its settings and private **access key**, writes its relay
routing into `.htaccess`, and starts the relay.

## Step 5: Check the status page

The **Status** tab lists checks, each with a hint when it fails. All of them
should be green:

| Check | If it's not green |
|---|---|
| PHP 8.x | Switch the site to PHP 8.0 or newer in your panel. |
| Signature checking (sodium) | Ask your host to enable the PHP `sodium` extension. |
| Device storage / outside the web root | The station keeps its data in `slipstream-data/` *next to* `public_html`. If it can't write there it falls back to `public_html/data`, protected only by `.htaccess`. Make the folder next to it writable for the site user, or set `data_dir` in `config.php`. |
| HTTPS | Install a certificate (Step 1). |
| Relay program present | Upload `bin/` again. |
| PHP may start the relay | `proc_open` is disabled on this host: run the relay as a service instead ([SERVER.md](SERVER.md#other-web-servers)). |
| Relay running | Make `bin/slipstream-server-linux-x86_64` executable, press **Restart relay** and read the relay log at the bottom of the page. |
| Web server forwards /relay | Enable `mod_rewrite` and `mod_proxy_http` and allow `.htaccess` overrides (`AllowOverride FileInfo`). On Nginx, see [SERVER.md](SERVER.md#other-web-servers). |

**Station software** shows the installed version and whether a newer release
is available. **Client downloads** shows the apps the station offers.

## Step 6: Choose your settings

On the **Settings** tab:

- **Private** (on by default, recommended): only apps downloaded from *your*
  station can use it. Strangers can't register devices or look up your IDs.
- **Public backup relay**: lets your apps fall back to Slipstream Public's
  relay. Both the station and the app must allow it.
- **Ask the person at the computer to allow links**: links made with a
  computer's one-time password wait until someone there presses **Allow**,
  next to a warning about phone scams. Unattended passwords never ask. Turn it
  on if your clients are often talked through connecting by phone.
- **Install new Slipstream releases automatically** (on by default): see
  [Updates](#updates) below.
- **Message shown in every app**: e.g. "Call us on 555-0100 before connecting".
- **Download page password**: stops random visitors from downloading your
  preconfigured app. Updates from inside the app still work without it.
- **Access key → Make a new access key**: if an app falls into the wrong
  hands. Every client then has to download the app again.

Add your **technicians** on the **Technicians** tab: they sign in from the
app (**Config → Stations**) to keep their saved devices on the station.

## Step 7: Hand out the app

Send your clients to `https://support.example.com/`. The apps they download
there connect to your station by themselves. When someone needs help, they
read you their ID and one-time password from the **BRIDGE** page. For
computers nobody sits at, set an unattended password in the app (and on
Windows, install the Slipstream service from **Config**). The
[getting started guide](GETTING-STARTED.md#2-install-the-app-on-your-clients-computers)
covers each platform.

On private stations, the first registration automatically adds the workstation
to the station's device list, with its computer name in
**Ungrouped**. Assign the group to selected technicians to show it in their apps;
unassigned groups are hidden. Technicians still need the device password to
connect. Lead technicians can rename and group shared devices from the app;
the station's **Devices** tab also offers **Edit shared device** and a last
reported hardware summary (model, serial, OS, processor and RAM). Click the device ID or
**View full system report** for the latest complete System information snapshot,
including storage, graphics, network, peripherals, printers and shared folders.
Updated host apps refresh it at startup and every 15 minutes. The report remains
available offline and shows when it was collected. Custom PCs use the motherboard
model when no system model is supplied; motherboard and system serials are labeled
separately. Firmware placeholders are ignored; unavailable or unreadable serials
cannot be recovered by the station. Public stations skip automatic enrollment
and hardware collection.

From **Devices → Assign to technicians**, search and check one or more accounts, then press
**Assign** to prepare their personal panels before sign-in. No device password is
assigned, and existing personal details or saved passwords are kept. Use
**Edit workstation** on the same page to set its name, notes and groups. The
Groups picker searches existing groups and can create a new one. Select several
groups when the workstation belongs in more than one; no selections means
Ungrouped. Drag the themed bar below Notes to resize it, or use its arrow keys.

The **Groups** tab shows station and personal technician groups with their
source. Search a row's **Workstations** dropdown, check the devices that belong
there and press **Save workstations**. Removing a device from one group keeps
its other memberships and saved passwords. Newly added personal entries have
no saved password. Ungrouped is automatic for devices with no named groups.

**Visible to these technicians** is also a searchable checkbox dropdown. Check
the technicians and press **Done** in that picker to save immediately. It shows
**Saving…**, then **Saved**, without reloading the page or discarding edits in
another group. A failed save keeps your selections and shows an error; press
Done to retry. Closing a picker without Done leaves a clearly marked pending
edit, and the browser warns before leaving with unsaved changes. Filtering
never clears selections hidden by the search. Multiple technicians can receive a group. New station
groups start with no recipients, including Ungrouped. Personal groups always
remain visible to their owner. Clearing recipients revokes shared visibility,
while independently saved personal entries and passwords remain intact. The
same tab creates and renames station groups and searches the group list.

The Status page's relay activity report has search and severity filters, wrapped
event rows and expandable technical details. It records connections and failures rather than only
upstream actor messages. Connection records include a connection ID, endpoint,
peer address, duration and transport bytes received/sent. Private stations using
the file registry also resolve the workstation ID and station alias (falling
back to its computer name). Public relays keep endpoint/connection diagnostics
without collecting workstation inventory or loading a private device catalog.

`client_stream_ended` is an INFO event: the relay observed an end of stream. It
does not establish whether the user closed the app, the computer slept or a
network path changed. `close_reason_not_supplied` means upstream supplied no
more precise cause. Transport errors and access-key/capacity denials are WARN
and include the actual error or policy reason. Byte counts cover the relayed
transport, including protocol traffic; they are not file-transfer totals. Failed
copies omit unavailable final byte counts rather than reporting zero.

Process events report listener configuration, PID, shutdown signal and supervisor
restart reason, including admin requests, settings/key changes and station
updates. Launch failures identify a missing binary, unavailable process execution,
missing port or startup timeout. Logs omit access keys, passwords and request
query strings. Behind the station's loopback proxy, the relay uses the last
proxy-added forwarded address; direct remote connections use their socket IP.
A relay binary update needs a relay restart before these events are available.

## Updates

The station checks GitHub for a new release every 6 hours. It verifies the
release's `station-update.json` against the release key in
`lib/release-keys.txt`, then downloads each file and checks it against the
signed list before installing. A compromised download path can't push files
to your station: it would need the project's private release key.

- **Automatic** (the default): the station installs new releases in the
  background after answering an app. It needs PHP-FPM, or a host where PHP
  may start programs and PHP's command-line program is installed (the usual
  case; the status page says if neither works).
- **One click**: **Status → Station software → Install update**. Use **Check
  now** to look straight away. This works on any PHP setup.

An update replaces the station's PHP files, the relay program and the apps,
and restarts the relay if it changed. Your settings, devices, accounts,
`config.php` and `.htaccess` are left alone. The files it replaced are kept
in `slipstream-data/update/previous/` until the next update.

Running desktop apps notice the new version within 6 hours (or at their next
start) and offer **Install & restart**, keeping their settings, ID and
passwords. Android users install the new APK from the download page.

Stations set up before version 0.27 can't update themselves yet: upload a
current station package once, as in Step 3 (your settings and devices are
kept). From then on they update by themselves.

To install updates from a fork or a mirror instead, see `update_repo` and
`update_releases_url` in `config.sample.php`.

## Back up

Back up the `slipstream-data/` folder next to your web root. It holds your
access key, settings, the device list and your technicians' accounts.

To move the station to another server, copy `public_html` and
`slipstream-data/`, then point DNS at the new server. Apps only know the
domain name, so they follow by themselves.

## Troubleshooting

- **The admin page shows a blank page or error 500**: the site is probably on
  PHP 7. Switch it to PHP 8 in your panel.
- **"Relay: NOT ROUTED"**: the relay runs but Apache doesn't forward
  `/relay` to it. Check `mod_proxy_http` is enabled and `.htaccess` overrides
  are allowed (`AllowOverride FileInfo`).
- **Update problems** show under **Status → Station software**. "Can't
  write" means the PHP user can't replace the station's files: upload them as
  the site user, not as root. "PHP can't download" means the host blocks
  outgoing HTTPS: enable the curl extension or `allow_url_fopen`, or update
  by hand as in Step 3.
- More in [SERVER.md](SERVER.md#troubleshooting) and
  [GETTING-STARTED.md](GETTING-STARTED.md#troubleshooting).

## Administrator sign-in

Station administrators sign in to `/admin/` with their technician username and
password. Grant **Station admin** on the Technicians page to allow another
technician to manage the web console, settings, accounts and shared devices.
Ordinary technicians use their accounts in the app and cannot open the console.
The console retains at least one administrator.

Existing stations automatically move their former shared password to an **admin**
account on the first visit after upgrading. If that username is already taken,
the console shows **station-admin** (or a numbered alternative) for the migrated
account; existing technician credentials and permissions are preserved. Old
password-only browser sessions must sign in again. Resetting a password, removing
an administrator, or using **Sign out everywhere** also revokes browser access.
Password resets still clear the technician's saved encrypted device passwords.
