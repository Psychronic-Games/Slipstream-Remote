---
title: Hardware video encoding (NVENC, VA-API, Media Foundation, VideoToolbox)
labels: [performance, feature]
---

## Background

Video is encoded by OpenH264 on the CPU (multi-threaded, sliced). That is
fine at 1080p60. At 1440p and 4K it uses a lot of CPU and caps the frame
rate, and laptops get hot.

## Progress

**NVENC (0.28)**: `crates/slipstream-codec/src/nvenc/` drives NVIDIA's
encoder on Windows and Linux through the driver's own libraries, loaded at
run time (System32 only on Windows), with one shared CUDA primary context
(~20 ms to open an encoder). Frames are copied to the GPU as BGRA/RGBA and
NVENC converts and encodes them: Baseline H.264, CAVLC, no B-frames,
infinite GOP with IDR on request, CBR with a one-frame VBV, P1 preset,
ultra-low-latency tuning. Any failure falls back to OpenH264 with a
keyframe; `SLIPSTREAM_NO_HWENC=1` forces software. The viewer's stats name
the encoder. Measured on an RTX 5070 (release build, 32-thread desktop):

| | OpenH264 | NVENC |
|---|---|---|
| 1920×1080 | 2.8 ms/frame | 3.8 ms/frame |
| 2560×1440 | 4.5 ms/frame | 3.5 ms/frame |
| 3840×2160 | 10.7 ms/frame | 6.3 ms/frame |

NVENC's time is mostly the copy to the GPU and is constant on any CPU;
OpenH264's grows several times on laptop processors (the RTX 3060 laptop
that prompted this capped near 40 fps) and costs 8 threads. A local
144 fps session reached 130 fps with 1.7 ms NVENC encodes. Still to confirm
on real Windows hardware, including the Windows service (session 0).

## Tasks

- [ ] Add an encoder implementation per backend in
      `slipstream-codec`, chosen at runtime with OpenH264 as the fallback:
  - [x] NVENC (Windows, Linux).
  - [x] Windows: Media Foundation H.264 for Intel and AMD (`src/mf/`;
    hardware MFTs, system-memory NV12 with a Direct3D 11 fallback; tested
    only against Wine's software MFT so far: needs real Intel/AMD/NVIDIA).
  - [x] Linux: VA-API (Intel, AMD), libva loaded at run time, packed
    SPS/PPS/slice headers written in `src/vaapi/headers.rs` (radeonsi needs
    them). Verified on a Radeon 610M (Mesa 26.2); Intel iHD untested.
  - [ ] macOS: VideoToolbox.
- [ ] Keep the low-latency settings: no B-frames, infinite GOP with
      on-demand IDR, CBR or VBV-limited rate control, and slice output if
      supported.
- [ ] Zero-copy where possible (DXGI texture → MF/NVENC; DMA-BUF →
      VA-API).
- [x] Show the active encoder in the viewer's stats.
- [x] Benchmark: encode time per frame and fps at 1080p, 1440p and 4K,
      against OpenH264 (NVENC; `cargo test --release -p slipstream-codec
      bench -- --ignored --nocapture`).
- [ ] Change NVENC's size in place (reconfigure) instead of reopening when
      adaptive resolution rescales.
- [ ] Hardware decoding on the viewer for 4K at high frame rates (OpenH264
      decodes 4K in ~8–12 ms).

## Done when

4K60 desktop streaming works with low CPU on at least Windows (any GPU)
and Linux (Intel/AMD), with automatic fallback when no GPU encoder is
available.

## Pointers

- `crates/slipstream-codec/`
- `crates/slipstream-session/src/video.rs` (pacing and bitrate control)
