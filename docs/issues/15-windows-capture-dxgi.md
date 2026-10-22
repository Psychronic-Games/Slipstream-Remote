---
title: "Windows: measure direct DXGI capture on real hardware"
labels: [platform:windows, performance]
---

## Background

Before 0.7.1, Windows capture went through xcap's DXGI recorder. Each frame:

- created a new staging texture,
- copied to it and mapped it at once (a CPU/GPU stall),
- copied it row by row into a `Vec`,
- cloned that `Vec`,
- swapped BGRA → RGBA in scalar code.

Then `generic.rs` copied it once more to draw the pointer. That was about
four full-frame copies plus a swizzle: roughly 60 MB per frame at 1440p and
130 MB at 4K. Direct duplication replaced that path in 0.7.1. The remaining
work is measuring current capture/encoding on real Windows hardware and
deciding from those measurements whether dirty-rectangle copies are useful.
Wine does not exercise real DXGI duplication.

## Progress

- Since 0.6.1 the service's helper uses `win_gdi.rs` on the login screen
  and UAC prompts: one persistent DIB section, one copy per frame, BGRA
  kept as is.
- The host reports per-stage timings (`screen capture (DXGI)` in the
  viewer's stats tooltip and log), which gives the before and after
  numbers this issue asks for. On the Server 2016 VM at 1280×800, DXGI
  capture took 1.8 ms a frame and encoding was the bottleneck.
- 0.7.1 replaced xcap's recorder with `win_dxgi.rs`, in the app and the
  service. Its recorder kept a thread holding the duplication after a
  session ended, so every later session fell back to GDI.

## Tasks

- [x] Use Desktop Duplication directly: one persistent staging texture,
      `AcquireNextFrame` with a short timeout, created and released by
      each session.
- [x] One copy per frame out of the mapped rows (`RowPitch` as the
      stride), keeping BGRA for the encoder; the frame is released before
      encoding.
- [x] Draw the pointer into that buffer, as before.
- [ ] Copy only the dirty rectangles (`GetFrameDirtyRects`) into the
      reused buffer, if measurements show the full copy matters at 4K.
- [ ] Measure on the laptop at 1080p/1440p/4K: fps and capture time in
      the stats tooltip.

## Pointers

- `crates/slipstream-platform/src/win_dxgi.rs` (duplication)
- `crates/slipstream-platform/src/generic.rs` (capture loop)
- `crates/slipstream-platform/src/win_cursor.rs`
- `crates/slipstream-platform/src/slot.rs` (`buffer`/`reuse`)
