---
title: Stream the host's audio to the viewer
labels: [feature]
---

## Background

There's no sound. Support work often needs it ("I hear an error beep"),
and so does any media use.

## Tasks

- [ ] Capture system output:
  - Windows: WASAPI loopback.
  - Linux: PipeWire monitor stream, via the existing dlopen loader, with
    PulseAudio as a fallback.
  - macOS: ScreenCaptureKit audio.
- [ ] Encode with Opus (20 ms frames) and send it on its own
      uni-directional stream or as QUIC datagrams.
- [ ] Viewer playback with a small jitter buffer, plus a mute toggle in the
      toolbar.
- [ ] Host setting *Allow sound* and a new capability. This bumps
      `PROTOCOL_VERSION`.

## Pointers

- `crates/slipstream-platform/src/pw.rs` (PipeWire loader)
- `crates/slipstream-core/src/proto.rs` (`Capabilities`)
