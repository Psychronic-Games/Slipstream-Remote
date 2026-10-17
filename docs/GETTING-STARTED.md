# Getting started with Slipstream Remote

This guide takes you from nothing to your first remote-support session:

1. [Set up your station](#1-set-up-your-station) — once, on your web server.
2. [Get the app onto your clients' computers](#2-install-the-app-on-your-clients-computers).
3. [Connect and take control](#3-connect-to-a-computer).
4. [Keep everything up to date](#4-keeping-up-to-date).

Plus [unattended access](#unattended-access), [settings](#app-settings) and
[troubleshooting](#troubleshooting).

**Who does what?** The *station* is your own small website (for example
`support.yourcompany.com`). It hands out the app, keeps a private list of
your clients' computers, and relays connections that can't go direct.
Whoever runs IT sets it up once. Everyone else just downloads the app from
it.

---

## 1. Set up your station

### What you need

- **A domain or subdomain** for the station, e.g. `support.example.com`. The
  station must sit at the root of it, not in a subfolder.
- **Ordinary web hosting** on a Linux server with:
  - PHP 8.0 or newer (with the standard `sodium` extension),
  - HTTPS (a free Let's Encrypt certificate is fine),
  - Apache with `.htaccess` allowed, plus `mod_rewrite` and `mod_proxy_http`
    (Apache 2.4.47+) for the built-in relay.

  That is the default on Virtualmin, cPanel/WHM and most VPS setups. No
  Docker, no database server and no root access are needed.
- **The station package**: the `slipstream-station` folder. Either take it
  from a release, or build it yourself with `scripts/build-station.sh` (see
  [BUILDING.md](BUILDING.md)).

> If PHP isn't allowed to start programs on your host, everything still works
> except your own relay; apps then use the free public relays for
> connections that can't go direct. You can also skip PHP entirely and run
> the server binary. See [SERVER.md](SERVER.md#other-web-servers).

### Step 1: Create the site

Point a DNS record (`A`/`AAAA`) for your subdomain at your server and create
a website for it with HTTPS.

**On Virtualmin:**

1. *Create Virtual Server*, enter the domain (e.g. `support.example.com`),
   and keep *website* and *SSL website* enabled.
2. *Server Configuration → SSL Certificate → Let's Encrypt*, then
   *Request Certificate*.
3. The web root is `/home/<site-user>/public_html`.

**On cPanel:** add the subdomain under *Domains*, then run *SSL/TLS Status →
AutoSSL*. The web root is shown next to the domain.

### Step 2: Upload the station

Upload the **contents** of `slipstream-station/` (not the folder itself)
into the web root, keeping the subfolders:

```
public_html/
├── index.php        ← download page
├── download.php
├── admin/           ← admin page
├── api/             ← the directory the apps talk to
├── assets/
├── bin/             ← the relay program
├── clients/         ← the apps for Windows, Linux, macOS + manifest.json
└── lib/
```

Use SFTP, your panel's file manager or `rsync`. Upload as the **site's own
user** so the files belong to it (Virtualmin and cPanel do this
automatically). Make sure `bin/slipstream-server-linux-x86_64` keeps its
"executable" permission (mode 755).

### Step 3: Run the setup

Open `https://support.example.com/admin/` and fill in:

- **Organisation name** — shown to your clients in the app and on the
  download page, and used in download file names
  ("Slipstream Remote - Acme IT.exe").
- **Admin password** — at least 10 characters.

The station then creates its settings and private **access key**, writes its
relay routing into `.htaccess`, and starts the relay.

### Step 4: Check the status page

The **Status** tab shows the station at a glance (devices, devices online,
the relay as **ACTIVE**, **NOT ROUTED** or **DOWN**, and the access mode). It
also lists checks, each with a hint when it fails. Every check should be
green:

| Check | If it's not green |
|---|---|
| PHP 8.x | Switch the site to PHP 8.0 or newer in your panel. |
| Signature checking (sodium) | Ask your host to enable the PHP `sodium` extension. |
| Device storage / outside the web root | The station keeps its data in `slipstream-data/` *next to* `public_html`. If that can't be written, it falls back to `public_html/data`, which is protected only by `.htaccess`. Make the folder next to it writable for the site user. |
| HTTPS | Install a certificate (Step 1). |
| Relay program present | Upload `bin/` again. |
| PHP may start the relay | `proc_open` is disabled on this host. Run the relay as a service instead ([SERVER.md](SERVER.md#other-web-servers)). |
| Relay running | Make sure `bin/` is executable, press **Restart relay** and read the relay log at the bottom of the page. |
| Web server forwards /relay | Enable `mod_rewrite` and `mod_proxy_http`, and allow `.htaccess` overrides (`AllowOverride FileInfo`). On Nginx, see [SERVER.md](SERVER.md#other-web-servers). |

The **Client downloads** list underneath shows which apps the station
offers, and the build number they are.

The relay restarts itself after a server reboot: the first app that checks
in brings it back up.

### Step 5: Choose your settings

On the **Settings** tab:

- **Private** (on by default, recommended): only apps downloaded from *your*
  station can use it. Strangers can't register devices or look up your IDs.
- **Public relay fallback**: lets apps use the free public relays if your own
  relay is unreachable. Turn it off if all traffic must stay on your server.
- **Message shown in every app**: e.g. "Call us on 555-0100 before
  connecting".
- **Download page password**: stops random visitors downloading your
  preconfigured app. Updates from inside the app still work without it.
- **Access key → Make a new access key**: if an app has fallen into the
  wrong hands. Every client then has to download the app again.

### Step 6: Back up

Back up the `slipstream-data/` folder. It holds your access key, settings and
the device list. To move the station to another server, copy `public_html`
and `slipstream-data/`, then repoint DNS. Apps only know the domain name, so
they follow automatically.

---

## 2. Install the app on your clients' computers

Send your clients to your station's address, e.g.
`https://support.example.com/`. The page offers the right download for their
computer. Every download is **already configured for your station**: there
is nothing to type and no setup.

### Windows 10/11

1. Download and open `Slipstream Remote - <your org>.exe`. It is a single
   file and doesn't need installing.
2. The app is not code-signed yet, so Windows SmartScreen may warn about it.
   Choose **More info → Run anyway**.

### Linux

1. Download `Slipstream Remote - <your org>`.
2. Make it executable: right-click → *Properties* → *Allow executing as
   program*, or run `chmod +x "Slipstream Remote - "*`.
3. Run it. The first start adds Slipstream to the application menu.

It runs on any 64-bit distribution from 2020 on (glibc 2.31+: Debian 11,
Ubuntu 20.04, Fedora, Arch…) and needs no extra packages.

- **Wayland** (KDE, GNOME, …): the first time someone connects, the desktop
  asks which screen to share and whether to allow remote control. Choose
  *Share/Allow*. The choice is remembered for later sessions.
- **X11**: works without any prompt.

### macOS

macOS builds are not packaged by the build script yet. They have to be
built on a Mac and dropped into the station's `clients/` folder as
`slipstream-macos.zip`. The first time, macOS asks for **Screen Recording**
and **Accessibility** permission (*System Settings → Privacy & Security*);
both are needed for someone to see and control the Mac.

### What your client sees

The **Bridge** page shows:

- **Your ID**: a 9-digit number, e.g. `829 918 930`. It stays the same for
  this computer.
- **One-time password**: changes after every session, so an old password
  can't be reused.
- A status light: **Online — ready for link** when the station has
  registered the computer.

Your client reads both to you over the phone. While someone is connected, a
red **Inbound link** banner shows who it is and for how long. It also has
**Chat** and **Terminate** buttons, so the client can end the session at any
time.

---

## 3. Connect to a computer

1. Open Slipstream on your own computer. You download it from the same
   station page.
2. On the **Bridge** page, under **Establish link**, enter the client's
   **Partner ID** and **Password**, then press **Engage**.
3. Their screen appears. **Click into it to take control.** Your mouse and
   keyboard now go to the remote computer.
4. To get your keyboard back, click the toolbar or switch to another window.

The toolbar along the top:

| Button | What it does |
|---|---|
| **◀ Bridge** | Back to your home page. The session keeps running and is listed under *Links*. |
| **1 2 3…** | Switch between the remote computer's screens (shown when it has several). |
| **Full** | Full screen. Move the mouse to the top edge to bring the toolbar back. |
| **Keys** | Send keys your own computer would catch: Ctrl+Alt+Del, Alt+Tab, the Windows key, Alt+F4, Print Screen, Ctrl+Esc. |
| **Chat** | Text chat with the person at the remote computer. On their side it opens in its own window. |
| **Files** | The two-pane file manager: your computer on the left, theirs on the right. Select files or folders and send them either way, or make folders. |
| **Balanced ▾** | Quality preset: *Balanced*, *Speed* (lowest delay), *Quality* (sharpest picture) and *Low bandwidth* (slow connections). |
| Stats | Frames per second, bandwidth, round-trip time, and **DIRECT** or **RELAY** (whether traffic goes peer-to-peer or through your station). |
| **End link** | Disconnect. |

You can also:

- **Drag and drop** files from your desktop onto the remote screen. They are
  saved into the folder under the mouse pointer: an open Explorer window, or
  the desktop.
- **Copy and paste** text between the two computers. The clipboard syncs both
  ways while connected, unless switched off in Settings.
- **Run several sessions** at once. Each one is listed on the *Links* page.

Files sent without choosing a folder (for example dropped on the remote
window when no folder is under the pointer) land on the receiving computer's
Desktop, or in *Downloads* if there is no Desktop folder.

---

## Unattended access

To reach a computer when nobody is in front of it (servers, your own PCs,
regular clients), set this up on that computer, under **Config**:

1. **Unattended password**: a permanent password of 8 characters or more. It
   works next to the one-time password and doesn't change.
2. **Start with system**: launches Slipstream hidden in the tray at login.
3. **Close button → keep running in the tray**: closing the window then
   keeps the computer reachable. This is switched on automatically together
   with *Start with system*.

Then connect with the ID and the unattended password.

For headless machines without a desktop session, `slipstream host
--password …` runs a host from the command line. Note that this needs a
logged-in graphical session to capture.

---

## App settings

The **Config** page on each computer:

| Setting | Meaning |
|---|---|
| Device name | How this computer is labelled in your station's device list and to people connecting. |
| Server address / Access key | Filled in by the download; change only to move to another station. |
| Public fallback relays | Use the free public relays if your station's relay can't be reached (if your station allows it). |
| Accept connections | Turn off to make this computer unreachable. It can still connect out. |
| Permissions | What connected people may do: **Control** (mouse and keyboard), **Clipboard**, **Files**. Untick any to allow view-only, no clipboard, or no file access. |
| One-time password length | Longer means harder to guess. |
| Unattended password, Start with system, Close button | See [Unattended access](#unattended-access). |
| Screen permission | Wayland: forget the remembered screen-sharing approval, so it's asked again. |
| Max frame rate | Upper limit for the video you *receive*. |
| Clipboard sync | Share clipboard text with remote computers. |

Settings, the computer's identity (its ID) and passwords are stored in the
user's settings folder: `~/.config/slipstream` on Linux,
`%APPDATA%\Slipstream\Slipstream\config` on Windows and
`~/Library/Application Support/dev.Slipstream.Slipstream` on macOS.
`slipstream info` prints the exact location. Settings survive
updates and re-downloads.

---

## 4. Keeping up to date

**Updating the station.** Build a new package and upload `clients/` again.
Upload the apps first and `clients/manifest.json` last, so no app sees the
new version before its file is there. Also upload `bin/` and the PHP files
if they changed. Your settings and devices live in `slipstream-data/` and
are never touched. If `bin/` changed, press **Restart relay** on the admin
page.

**Updating the apps.** Apps check your station about 20 seconds after they
start and then every 6 hours. When a newer build is there, a banner offers
**Install & restart**. The app then:

- downloads the update from your station,
- checks it against the station's checksums,
- swaps it in and restarts,
- keeps its ID, passwords and settings.

*About → Check for updates* checks right away, and `slipstream
update` does the same on the command line.

When an update changes the connection protocol, both computers need the new
version. The older side gets a clear "please update" message instead of a
broken session.

---

## Troubleshooting

| Problem | What to do |
|---|---|
| App says **Server unreachable** | Check the computer's internet connection and that the station's address opens in a browser. A computer clock that is wrong does *not* matter: Slipstream corrects for it. |
| **"This is a private Slipstream station"** | The app wasn't downloaded from this station, or the access key was renewed. Download the app again from the station page. |
| **Partner not found / offline** | The other computer's app isn't running, has *Accept connections* off, or has no internet. Its status light should say *Online*. |
| **Wrong password** | One-time passwords change after every session. Ask for the current one. |
| Connection works but is slow, stats say **RELAY** | Both sides are behind strict firewalls, so traffic goes through your station. Try the *Speed* or *Low bandwidth* preset. |
| Black or frozen screen from a Wayland computer | The screen-sharing prompt on that computer wasn't approved. Approve it, or reset it under *Config → Screen permission*. |
| Mac shows a black screen or ignores the mouse | Grant **Screen Recording** and **Accessibility** to Slipstream in *System Settings → Privacy & Security*, then restart the app. |
| Update fails: "installed in a protected folder" | The app can't replace itself there. Download the new version from the station page instead. |
| Station relay is red | See [Step 4](#step-4-check-the-status-page). The relay log on the admin page usually says why. |

Each app writes a log file, `slipstream.log`, in its settings folder.
`slipstream info` shows where. Send it along with any bug report.
