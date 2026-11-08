---
title: "Subspace Relay: a computer's whole connection through an Android phone's mobile data"
labels: [feature, Android, Windows, Linux, macOS]
---

## Background

Phone plans often give unlimited mobile data but only a few gigabytes of
hotspot data. Subspace Relay lets a computer use the phone's connection as
the phone's own data: the phone creates a Wi-Fi network, the computer joins
it, and Slipstream on the computer sends all of its traffic to Slipstream on
the phone, which opens the real connections itself. To Android and the
carrier's accounting, that is an app's data, not tethering.

Carriers' terms often still count any data used by another device as
hotspot use, and can detect it from traffic patterns. The app says so
plainly when the relay is first turned on.

## Design

```
 Computer                                         Android phone
 ┌────────────────────────────┐   Wi-Fi Direct    ┌──────────────────────────┐
 │ apps → OS routes (0/1,128/1)│   WPA2 group      │ Subspace Relay service   │
 │   → TUN "slipstream-relay"  │ ───────────────▶  │ QUIC server, ALPN        │
 │   → ipstack (userspace TCP) │  QUIC (iroh),     │ slipstream/relay/1       │
 │   → one QUIC stream a flow  │  192.168.49.1     │ → real TCP/UDP sockets   │
 └────────────────────────────┘                   │   over mobile data       │
                                                  └──────────────────────────┘
```

**Phone.** Android doesn't let ordinary apps run a hotspot with a chosen
name, so the phone starts a Wi-Fi Direct group (`WifiP2pManager.createGroup`,
Android 10+) named `DIRECT-ss-<name>` with the user's password. A computer
joins it like any Wi-Fi network; the phone is always `192.168.49.1` on it.
A foreground service keeps the group and the relay running with the app in
the background. The relay is an iroh endpoint with relays and discovery off,
listening only on the group's address: it can't be reached from the
internet. Anyone who has joined the group may use it (as with a hotspot).
Outgoing connections use the phone's default network, which is mobile data
while the phone isn't on another Wi-Fi.

**Computer.** `slipstream relay connect` (administrator/root) creates a
TUN device (`tun` crate: `/dev/net/tun`, macOS `utun`, Windows Wintun), routes
all IPv4 traffic and DNS into it (`tproxy-config`, which restores routes and
DNS when it stops), and runs a userspace TCP/IP stack on it (`ipstack`). The
phone's own address bypasses the tunnel. Each TCP connection and each UDP
flow becomes a QUIC stream to the phone. DNS is ordinary UDP to a public
resolver, relayed like any other flow. The desktop app starts this helper
elevated (pkexec, UAC, macOS administrator prompt) and stops it through a
local control connection, which also carries its status.

**Finding the phone.** The phone answers a plain TCP request on
`192.168.49.1:47100` with its relay endpoint ID; QUIC uses UDP port 47100.

**Protocol** (`slipstream_relay::proto`, postcard, length-prefixed as in
`framing`): each QUIC bi-stream starts with `Open::Tcp(addr)` or
`Open::Udp(addr)`; the phone answers `Opened` or `Refused(reason)`. TCP
streams then carry raw bytes; UDP streams carry u16-length-prefixed
datagrams.

Licensing: `tun2proxy` was considered, but depends on GPL-3.0
`socks5-impl`; the crates used here are MIT, Apache-2.0 and WTFPL.

## Tasks

- [ ] `crates/slipstream-relay`: protocol, phone-side server, computer-side
      flow forwarding; end-to-end tests over a local QUIC link.
- [ ] Computer tunnel: TUN + ipstack + routes/DNS; `slipstream relay connect`.
- [ ] Desktop GUI: **Subspace Relay** panel (connect, status, data used),
      elevated helper and control connection on Linux, Windows and macOS.
- [ ] Windows: ship `wintun.dll` inside the one executable (pinned, checked).
- [ ] Android: foreground service, Wi-Fi Direct group with the chosen name
      and password, permissions (nearby Wi-Fi devices), relay start/stop,
      panel with name, password, connected computers and data relayed.
- [ ] IPv6 through the tunnel (blocked to fall back to IPv4 at first).
- [ ] UDP as QUIC datagrams instead of streams, if games need it.
- [ ] Real-device checks: Windows, Linux and macOS computers joined to a
      phone's group; browsing, downloads, a video call, a game; the
      computer's network restored after Disconnect, a crash and a reboot.

## Pointers

- `crates/slipstream-relay/`
- `apps/slipstream/src/cli.rs` (`relay` subcommand), `gui/relay.rs`
- `apps/slipstream-android/app/src/main/java/.../SubspaceRelayService.java`
