---
title: Stream the host's audio to the viewer
labels: [feature]
---

## Background

Sound works from Windows and Linux hosts (`crates/slipstream-audio`):
Opus in QUIC datagrams, a jitter buffer on the viewer, a *Sound* toggle and
a Config default. What's left:

## Tasks

- [x] Capture: Windows (WASAPI loopback via cpal), Linux (PulseAudio via
      dlopen, which covers PipeWire desktops).
- [x] Opus 20 ms frames as datagrams; jitter buffer; toolbar toggle; host
      permission (introduced with protocol 4; current protocol is 6).
- [ ] macOS capture: ScreenCaptureKit audio (`SCStreamConfiguration.
      capturesAudio`), needs the Screen Recording permission it already has.
- [ ] Listen on real machines: Linux → Windows and back; check latency
      (should feel in sync with the picture) and that a lost packet is
      inaudible.
- [ ] A volume slider, if people want one.

## Pointers

- `crates/slipstream-audio/src/io.rs`, `pulse.rs` (audio capture/playback)
- `crates/slipstream-session/src/host.rs`, `viewer.rs` (session audio)
- `crates/slipstream-core/src/proto.rs` (`Capabilities`)
