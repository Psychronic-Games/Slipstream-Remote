---
title: ARM builds (Linux aarch64, Windows on ARM)
labels: [infra]
---

## Background

Linux and Windows desktop apps and the station relay are currently distributed
for x86_64 only. This issue adds native ARM Linux and Windows-on-ARM builds,
including the relay for ARM VPSes. Android release APKs already target arm64;
the macOS packaging script targets Apple Silicon and Intel but still needs
native verification (issue 03).

## Tasks

- [ ] `build-station.sh`: `cargo zigbuild --target
      aarch64-unknown-linux-gnu.2.31` for the app and `slipstream-server`.
      This needs an aarch64 stub `libpipewire-0.3.so`.
- [ ] `cargo xwin build --target aarch64-pc-windows-msvc`.
- [ ] Manifest platforms `linux-aarch64` and `windows-aarch64`. Check
      `slipstream_core::platform()` and the station's architecture
      detection (download page, `slip_relay_binary`).
- [ ] OpenH264 performance on ARM (NEON) — maybe hardware encoding first.

## Pointers

- `scripts/build-station.sh`
- `server/station/lib/slipstream.php` (relay binary per `php_uname('m')`)
- `server/station/index.php`, `download.php`
