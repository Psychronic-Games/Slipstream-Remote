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

Back up the `slipstream-data/` folder. It holds your access key, settings,
the device list and your technicians' accounts. To move the station to another server, copy `public_html`
and `slipstream-data/`, then repoint DNS. Apps only know the domain name, so
they follow automatically.

---

## 2. Install the app on your clients' computers

Send your clients to your station's address, e.g.
`https://support.example.com/`. The page offers the right download for their
computer. The app **connects to your station by itself**:

- Apps you build yourself carry your station's settings inside the download.
- Signed apps (from a release) and the Mac app are the same file for
  everyone. They read your station from the download address the browser
  recorded on the file.
- If that didn't work (say the file was copied to a USB stick), the app
  asks for a station. The download page has a **Connect this app** button,
  and also shows your station's address and a **setup code** (like
  `K7MQ-2XPA`) to type in instead. Codes last 30 days; renewing the access
  key cancels them.

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

- **Wayland** (KDE, GNOME, …): Slipstream prepares screen access at startup,
  before anyone connects. Choose a screen and *Share/Allow* in the desktop's
  prompt. You can also use **Config → Screen permission → Approve now**;
  **Reset / approve again** lets you change the selection when no incoming
  viewer is connected. Access remains ready while Slipstream runs in the tray.
  Permission is remembered across starts when the desktop supplies a restore
  token; some desktops require approval again after a restart or reboot.
- **X11**: works without any prompt.

### macOS (12 or newer, Apple Silicon or Intel)

1. Download the zip, open it and move **Slipstream** to *Applications*.
2. Open it. The app isn't notarised by Apple yet, so macOS refuses the
   first time: go to *System Settings → Privacy & Security* and choose
   **Open Anyway**.
3. Allow **Screen Recording** and **Accessibility** when asked (also under
   *Privacy & Security*). Both are needed for someone to see and control
   the Mac. Restart the app after granting them.
4. If it asks for a station, enter the address and setup code from the
   download page (the *Connect this app* button doesn't work on Macs yet).

Until the app is notarised, macOS may ask for these permissions again after
an update.

### Android (8 or newer): preview

The Android app controls computers; being controlled from a computer comes
later (issue 09). In a session, tap to click, long-press to right-click,
drag with one finger, pinch to zoom and drag two fingers to look around
(or to scroll, when not zoomed in). Everything else is under **Console**.
Typing on the remote computer from the phone's keyboard is next.

1. Open the station's page on the phone and tap **Download** (the page
   picks Android by itself).
2. Open the downloaded file. The first time, Android asks to let your
   browser install apps: choose *Settings*, turn on **Allow from this
   source**, go back and choose **Install**. No developer options are
   needed. Play Protect may offer to scan the app; let it.
3. Back on the station's page, tap **Connect this app**: the app opens and
   links to the station. (Or type the address and setup code in it.)

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

The device's one-time code and its optional unattended password both work
with the same ID. A code changes after a session ends; changing it does not
change the unattended password or disconnect an existing controller. Windows
service hosts show their code in the signed-in user's app, with Copy and New
controls. If an older service has no code to show, update it under
**Config → Windows service → Update**. An unattended password is still
required when installing the service for access before sign-in.

Desktop links start as tabs by default. Switch workstations from the strip
below the title bar, or select **Bridge** to return to the console. Drag a tab
along the strip to reorder it; drag it outside the strip and release to open
a floating window. Drag the floating window's tab back onto the main strip
to dock it at the highlighted position. These moves keep the existing link.

Choose **Config → Viewing → New links → Tabs / Separate windows**, then
**Save configuration**, to set the default for future connections. Existing
links keep their current layout. You can mix tabbed and floating sessions.
**Pop out**, **Dock**, and the tab's right-click menu provide the same moves
without dragging; at compact widths, Pop out / Dock are under Console.
**Links → Open window / Focus window** remains available. Closing a tab or
session window ends only that workstation's link. Phones keep embedded tabs;
separate native windows require desktop support. On systems that withhold
cross-window pointer positions, use the Dock action if dragging back is
unavailable. The title bar identifies the workstation; picture quality and
FPS/latency sit together on the session toolbar.

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
| **Bridge** (the delta pointing back) | Back to your home page. The session keeps running and is listed under *Links*. |
| **SCREEN 1 2 3…** | Switch between the remote computer's screens (shown when it has several). |
| **Full** | Full screen. Move the mouse to the top edge to bring the toolbar back; **Ctrl+Alt+Enter** leaves full screen too. |
| **Keys** | Send keys your own computer would catch: Ctrl+Alt+Del, Alt+Tab, the Windows key, Alt+F4, Print Screen, Ctrl+Esc. |
| **Chat** | Text chat with the person at the remote computer. On their side it opens in its own window. |
| **Files** | The two-pane file manager: your computer on the left, theirs on the right. Select files or folders and send them either way, or make folders. |
| **Sound** | Play the remote computer's sound here, or stop. The default is under *Config → Remote sound*. |
| **Tools ▾** | **Privacy mode** blacks out the remote computer's own screen while you work (Windows 10 2004 and later). **Port forwarding…** opens tunnels, see below. **System information…** shows its hardware, OS, drives, temperatures and network, updating live, with *Copy all* for tickets. |
| **Balanced ▾** | Quality preset: *Balanced*, *Speed* (lowest delay), *Quality* (sharpest picture) and *Low bandwidth* (slow connections). |
| Stats | Frames per second, bandwidth (kbit/s below 1 Mbit/s), round-trip time, and **DIRECT** or **RELAY** (whether traffic goes peer-to-peer or through your station). Hover for decode time and, from 0.6.2 hosts, the remote computer's own figures: capture, encoding, and waiting for a new screen frame or for you. Every 30 s the same figures go to this app's log. The Speed preset can reduce moving-picture resolution on a slow host and sharpen it once motion stops. Balanced and Quality retain full picture resolution. |
| **End link** | Disconnect. |

**Control an Android phone from a PC:** update both apps, open Slipstream on
the phone and configure/link its station. Its ID and one-time password appear
on Bridge. Tap **Enable remote control** and enable **Slipstream Remote
control** in Android Accessibility settings. If Android blocks the switch,
open **App info**, use its menu to **Allow restricted settings**, then return
to Accessibility. This permission is optional for viewing only.

Tap **Start sharing**, approve Android's capture prompt, then connect from
your PC using the phone's ID and password. After approval Slipstream moves
to the background; tap its sharing notification to return. Mouse clicks tap; held left-button
drags swipe/hold, the wheel swipes, and keyboard typing edits a focused field.
**Keys → Android Back / Home / Recents** navigates the phone. You can switch
phone apps while sharing. **Stop sharing** in Slipstream or its notification
ends access and disconnects viewers. A new capture session requires approval
on the phone; a saved password does not skip that prompt. Protected apps may
hide their content. Android host audio, clipboard and files are not available
in this initial host implementation.

Drops onto a remote desktop or folder automatically open a small LCARS
**Transfers** window. It shows the destination, bytes and progress, then keeps
**Complete**, **Failed** or **Cancelled** receipts. **Confirming** means the
bytes have been sent and Slipstream is waiting for the host's save receipt.
Close hides this window while work continues; **Cancel** stops a job, **Clear
finished** removes old receipts, and **Files** opens the full browser.

**File Transfer** opens in its own resizable LCARS window. Each pane has
**Name** and **Modified** sorting; click the active choice again to reverse it.
Folders stay at the top. Images show mini thumbnails; hover for a larger view.
PNG, JPEG, GIF (first frame), WebP, BMP, ICO and TIFF are supported, along with
passive text previews for common notes, logs and source files. Large,
unsupported or unreadable files keep their console icon. Remote thumbnails
require an updated host as well as an updated viewer.

You can also:

- **Drag and drop** files from your desktop onto the remote screen. They are
  saved into the folder under the mouse pointer: an open Explorer window, or
  the desktop.
- **Copy and paste** text between the two computers. The clipboard syncs both
  ways while connected, unless switched off in Settings.
- **Copy and paste files.** Copy files or folders on your computer, click
  into the remote screen and press Ctrl+V (Cmd+V on a Mac): they're
  uploaded and pasted into whatever is in front over there, such as an
  Explorer window or the desktop. Files copied on the remote computer
  arrive on your clipboard, ready to paste (up to 64 MB or 500 files at a
  time; use *Files* for more).
- **Run several sessions** at once. Each one is listed on the *Links* page.

Files sent without choosing a folder (for example dropped on the remote
window when no folder is under the pointer) land on the receiving computer's
Desktop, or in *Downloads* if there is no Desktop folder.

### Port forwarding

A tunnel makes a service on the remote computer, or on its network,
reachable from yours: its Remote Desktop, a web admin page, a printer or
router. In *Tools ▾ → Port forwarding…*, enter a local port (or leave it
empty for any free one), the host as the remote computer sees it
(`localhost` for the computer itself, or an address on its network) and
the port. Then point your program at `127.0.0.1:<local port>`; for
Remote Desktop, `mstsc /v:127.0.0.1:13389`. Tunnels close with the session.

From the command line: `slipstream connect <ID> --forward
13389:localhost:3389` (repeat `--forward` for more).

The remote computer has to allow it: *Config → Permissions → Port
forwarding*, which is off by default.

---

## Technicians: saved devices and other stations

Your station's admin page has a **Technicians** tab: add an account for each
technician there. In the app, under **Config → Stations**, each technician
signs in with the station's address, their username and password. You can
be signed in to several stations at once, including other organisations'
(you don't need their app or access key, only an account there).

The Bridge then shows **Saved devices**, per station:

- **+ Add device**, or **★** on a recent link, saves a device with a name
  and notes. It's kept on the station, so it follows you to any computer.
- Optionally save its **unattended password** too: **Connect** then works
  in one click (🔑 marks those). The password is encrypted on your computer
  with a key made from your sign-in password, so the station can't read it.
  If an admin resets your sign-in password, saved passwords are cleared.
- The lamp shows which devices are online right now.
- **Shared by …** lists devices the station's admins share with everyone.

From the command line: `slipstream sign-in relay.example.com jsmith`,
`slipstream saved`, `slipstream save 482913077 --station relay.example.com
--name "Front desk" --password …` and `slipstream connect 482913077
--station relay.example.com`.

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

### Windows: the Slipstream service

Without the service, Slipstream runs as the logged-in user, so Windows
keeps it away from UAC prompts, windows running as administrator, and the
login screen. For servers and machines you look after unattended, install
the service: **Config → Incoming access → Windows service → Install**
(Windows asks for administrator rights). It uses this computer's station
and unattended password, and keeps the same ID.

With the service:

- you can approve UAC prompts, type into administrator windows, and log in
  at the login screen; *Keys → Ctrl + Alt + Del* sends the real thing;
- it runs from boot, before anyone logs in, and survives logging off;
- it installs updates by itself;
- the service is the computer's one host, under the app's ID (install
  hands the ID over). The app on that computer stops hosting but can
  still control others. Its Bridge shows the ID and whether the service
  is running. If the service is stopped, the app starts it (no
  administrator prompt needed) and offers *Start the service*;
- the app starts hidden in the tray at every sign-in, so the service's
  presence shows there (*Config → Tray icon* turns that off).

The service keeps its own copy of the settings: after changing the
unattended password or permissions, press **Update** next to it. From an
administrator command prompt: `slipstream service install [--password …]`,
`slipstream service uninstall`, `slipstream service status`, and (as any
user) `slipstream service start`. The service's log is under *Config →
Service log* (`C:\ProgramData\Slipstream Remote\public\slipstream.log`).

For headless Linux machines, `slipstream host --password …` runs a host
from the command line (it needs a logged-in graphical session to
capture).

---

## App settings

The **Config** page on each computer:

| Setting | Meaning |
|---|---|
| Device name | How this computer is labelled in your station's device list and to people connecting. |
| Server address / Access key | Filled in by the download; change only to move to another station. |
| Public fallback relays | Use the free public relays if your station's relay can't be reached (if your station allows it). |
| Accept connections | Turn off to make this computer unreachable. It can still connect out. |
| Permissions | What connected people may do: **Control** (mouse and keyboard), **Clipboard**, **Files**, **Sound** (hear this computer) and **Port forwarding** (off by default). Untick any to allow view-only, no clipboard, or no file access. |
| One-time password length | Longer means harder to guess. |
| Unattended password, Start with system, Close button | See [Unattended access](#unattended-access). |
| Screen permission / access | Wayland: Approve now, or Reset / approve again to change the selection. Prepared at startup and kept ready in the tray; persistence depends on the desktop. Windows: no screen-sharing approval is needed, including for the background service. |
| Max frame rate | Upper limit for the video you *receive*. |
| Clipboard sync | Share copied text and files with remote computers. |
| Remote sound | Whether sessions start with the remote computer's sound on. |

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
- checks its signature against the release key the app was built with,
  and its checksum,
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
| Remote screen is **black** after the other person logs off, locks the computer or closes another remote tool | Windows is showing its lock screen, which normal apps can't see (the session bar says so). Install the [Slipstream service](#windows-the-slipstream-service) on that computer to reach the lock and login screens. |
| **Ctrl+Alt+Del does nothing** | Windows only accepts it from a service: install the [Slipstream service](#windows-the-slipstream-service) on the remote computer. |
| Service shows **Installed, not running** | Press **Start** (on the Bridge or in Config; no administrator prompt needed). If it won't start, Config shows Windows' own explanation with a *Copy* button, and *Config → Service log* opens the service's log (`C:\ProgramData\Slipstream Remote\public\slipstream.log`). |
| **Two Slipstream processes** in Task Manager with the service installed | Expected. The service runs in the background where it can't see the screen, and starts a helper in the signed-in session that captures and controls it. The tray icon is a third, unless *Config → Tray icon* is off. |
| A **small window** instead of the full app (Windows Server 2016, some VMs) | Slipstream draws the full window in software on machines without usable graphics, by itself. You get the small window only if that failed on this computer last time; press **Full window** in it to try again. The small window still lets others control the computer. |
| **Wrong password** | One-time passwords change after every session. Ask for the current one. |
| **"Too many wrong passwords"** | After three wrong tries the computer waits before it checks another password, longer each time (up to 5 minutes). Wait the time shown, then try the right one. |
| Update not offered, log says **"signature doesn't match"** | The station's apps were built with a different release key from yours. Download the app from the station page again. |
| Connection works but is slow, stats say **RELAY** | Both sides are behind strict firewalls, so traffic goes through your station. Try the *Speed* or *Low bandwidth* preset. |
| A direct link authenticates but stalls as video or a large transfer starts | Update both ends. Android uses individual UDP datagrams for compatibility. On other platforms, starting the affected app with `SLIPSTREAM_DISABLE_GSO=1` disables UDP segmentation offload for drivers that mishandle it. |
| Low frame rate, stats say **DIRECT** | Hover over the stats. Long *video encoding* times mean the remote computer's processor is the limit. Long *screen capture* times point at capture. Long *waiting for the viewer* times mean the network. The 30-second lines in the log show the same, over time. |
| Black or frozen screen from a Wayland computer | The screen-sharing prompt on that computer wasn't approved. Approve it, or reset it under *Config → Screen permission*. |
| Mac shows a black screen or ignores the mouse | Grant **Screen Recording** and **Accessibility** to Slipstream in *System Settings → Privacy & Security*, then restart the app. |
| Update fails: "installed in a protected folder" | The app can't replace itself there. Download the new version from the station page instead. |
| Station relay is red | See [Step 4](#step-4-check-the-status-page). The relay log on the admin page usually says why. |

Each app writes a log file, `slipstream.log`, in its settings folder.
`slipstream info` shows where. Send it along with any bug report.


## Scan the remote network

During a session, open **Tools → Network scan…** (on a phone, **Console →
Tools**). The adapters come from the remote computer. Choose its connected
IPv4 adapter; the app fills in its subnet and shows its subnet mask and
gateway. Edit the subnet to narrow a large network, choose common TCP ports
or enter ports/ranges (e.g. `22,80,443,3389` or `1-1024`), then press
**Scan network**. Scans support at most 4096 addresses, 1024 ports and
262144 probes; larger ranges need narrowing.

**Table** lists devices, MAC addresses when available, discovery evidence,
and open TCP ports. **Topology** charts the reported gateway and devices
on the subnet. It shows logical membership, not physical switch links.
Select a device for its ports; filter by address, MAC, port or service, or
**Copy results** for a ticket. Service names describe conventional ports,
not verified software. A device listed only by the neighbor cache is marked
as unconfirmed. Firewalls can hide devices, and only selected TCP ports are
checked. IPv6 and UDP scans are not included.

**Stop scan**, closing the window, or ending the session stops the scan and
keeps partial results while the session tab exists. The remote computer's
**Config → Incoming access → Permissions → Network scan** controls access.

For sharp text during movement, choose **Balanced** or **Quality**. These
preserve full picture resolution; on a slow VM the frame rate may be lower.
**Speed** and **Low bandwidth** can reduce resolution while moving and
sharpen the picture when still. Hover the session statistics to see the
encoded picture dimensions. Both computers need session protocol 6.


## Support tools

Under a live session's **Tools → Support report…**, you can collect system
inventory/public IP, add technician notes, review the text and copy it or
save a new local report folder. Pick an existing destination folder. Each
export has its own folder and `report.txt`; the screenshot checkbox adds
`screen.png` from the latest received frame. Screenshots are off by default
and may be stale after an interruption. Reports omit credentials, chat and
raw logs. Network scan/filter results and completed diagnostics are retained
snapshots, not an automatic fresh scan. Closing the report cancels pending
remote collection/status requests; a local export already started finishes
in its own report folder.

The same panel has **Message shown to the person being helped**. Choose a
message or type up to 160 characters, then Show status / Clear status. The
host app shows a small support window with Dismiss and Stop support. Stop
ends that session without automatic reconnect. Remote control permission is
required. The host app must be running in the signed-in desktop; updated
Windows service hosts can launch it through the authenticated local bridge.
A headless CLI host cannot display a desktop banner. The compact Windows app
shows the message in its note area with Dismiss / Stop support for that
session; when several technicians send statuses, it shows the newest one.
Clearing or ending one message retains another session's status. Closing a
service session clears its status. A message neither grants consent nor
executes a change.

In **Tools → Network scan…**, select a discovered device and expand
**Connect through / Wake-on-LAN / Diagnostics**:

- **Connect from my computer** checks the selected TCP service, then creates
  a loopback endpoint on your computer through the workstation. Port forwarding
  must be enabled on the host. TCP 80/443 open in your local browser; for other
  ports, Copy endpoint into the appropriate client. Web interfaces using their
  original hostnames, certificates, absolute redirects or multiple ports may
  need additional configuration. Closing the scanner retains tunnels; closing
  a tunnel or ending/interruption of its link closes its forwarded connections.
- **Send wake packet** broadcasts three magic packets through the selected
  remote adapter. Enter a MAC, or use a discovered device's MAC. The remote
  host must allow Wake-on-LAN, Network scan and Control. Wake-on-LAN is off
  by default. Configure firmware/adapter wake support on the sleeping device;
  a sent packet is not confirmation it woke. /31 and /32 interfaces have no
  supported broadcast, and requests are limited to one every two seconds.
  This first version uses the connected workstation; it does not automatically
  find another awake peer or save wake targets to the station.
- **Run diagnostics** checks gateway ICMP, IPv4/A answers from configured DNS
  resolvers, an optional numeric comparison resolver, and a verified station
  API hello from the host. Your query name is sent to those resolvers. ICMP
  silence may be filtering, and DNS/CDN answers may differ legitimately.
  Gateway/DNS probes use the selected adapter where supported; station HTTPS
  uses OS routing. This is not a relay throughput or video-relay health test.
  Copy diagnostics exports the results. Closing/stopping a request cancels
  pending checks; an already-sent packet or launch cannot be recalled.

Save host permissions and update an installed Windows service, then reconnect
so the new session uses those permissions. Older hosts can continue ordinary
sessions; new optional support requests may require updating the host.

## Imaging and automatic device IDs

Update Slipstream on the master laptop, **update/start its installed Windows
service**, and run the updated app/service once **before capturing the image**.
This records a hardware binding. Config → Device identity shows whether the
app can read firmware identifiers. Both the service's machine profile and
any user profile included in the image need their baseline established.

At startup, a profile compares locally hashed firmware evidence. Windows uses
the SMBIOS system UUID, with a system serial fallback; Linux uses readable
DMI UUID/serial files and macOS uses platform firmware identifiers. The model,
hostname, MAC address and cloned Windows installation ID are not the binding.
On different hardware, Slipstream creates a fresh random private key, clears
the cached station ID and stale public service ID/pin, and registers a new ID.
The original laptop retains its identity. Repeated starts on the clone keep
its new identity. Station settings, unattended password and service installation
are preserved. Custom device labels remain; a label matching the old system
hostname follows the new host name. Ordinary service updates/reinstalls retain
the service's identity even if its user profile has a separate key.

Missing/unreadable identifiers retain the current ID instead of guessing.
Duplicated firmware identifiers, VMs cloned with the same hardware UUID, and
old images with no recorded binding cannot reliably establish a different
device. Some Linux systems restrict DMI identifiers to privileged processes.
Changing/replacing firmware identity (including a motherboard) can trigger a
new ID. Android has no desktop firmware binding in this implementation. Real
Windows clone/service behavior still needs confirmation on the intended laptops.
