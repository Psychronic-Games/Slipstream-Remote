---
title: Hardware video encoding (NVENC, VA-API, Media Foundation, VideoToolbox)
labels: [performance, feature]
---

## Background

Video is encoded by OpenH264 on the CPU (multi-threaded, sliced). That is
fine at 1080p60. At 1440p and 4K it uses a lot of CPU and caps the frame
rate, and laptops get hot.

## Tasks

- [ ] Add an encoder trait implementation per backend in
      `slipstream-codec`, chosen at runtime with OpenH264 as the fallback:
  - Windows: Media Foundation H.264 (works on Intel, AMD and NVIDIA), or
    NVENC / AMF directly.
  - Linux: VA-API (Intel, AMD) and NVENC. Load the libraries with `dlopen`
    like PipeWire, to keep the binary portable.
  - macOS: VideoToolbox.
- [ ] Keep the low-latency settings: no B-frames, infinite GOP with
      on-demand IDR, CBR or VBV-limited rate control, and slice output if
      supported.
- [ ] Zero-copy where possible (DXGI texture → MF/NVENC; DMA-BUF →
      VA-API).
- [ ] Show the active encoder in the viewer's stats.
- [ ] Benchmark: encode time per frame and fps at 1080p, 1440p and 4K,
      against OpenH264.

## Done when

4K60 desktop streaming works with low CPU on at least Windows (any GPU)
and Linux (Intel/AMD), with automatic fallback when no GPU encoder is
available.

## Pointers

- `crates/slipstream-codec/`
- `crates/slipstream-session/src/video.rs` (pacing and bitrate control)
