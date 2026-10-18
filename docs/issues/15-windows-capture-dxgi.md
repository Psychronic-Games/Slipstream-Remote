---
title: "Windows: capture straight from DXGI (fewer full-frame copies)"
labels: [platform:windows, performance]
---

## Background

Windows capture goes through xcap's DXGI recorder. Each frame:

- creates a new staging texture,
- copies to it and maps it at once (a CPU/GPU stall),
- copies it row by row into a `Vec`,
- clones that `Vec`,
- swaps BGRA → RGBA in scalar code.

Then `generic.rs` copies it once more to draw the pointer. That is about
four full-frame copies plus a swizzle: roughly 60 MB per frame at 1440p and
130 MB at 4K. It likely keeps 4K below 60 fps on Windows. Under Wine the
recorder isn't available, so this path can only be measured on real
Windows.

## Progress

- Since 0.6.1 the service's helper uses this same xcap recorder on the
  normal desktop, so it has the same copies. On the login screen and UAC
  prompts it uses `win_gdi.rs`: one persistent DIB section, one copy per
  frame, BGRA kept as is.
- The host now reports per-stage timings (`screen capture (DXGI)` in the
  viewer's stats tooltip and log), which gives the before and after
  numbers this issue asks for. On the Server 2016 VM at 1280×800, DXGI
  capture took 1.8 ms a frame and encoding was the bottleneck. So this
  matters most at 1440p and 4K.

## Tasks

- [ ] Use Desktop Duplication directly (the `windows` crate is already a
      dependency): one persistent staging texture, `AcquireNextFrame` with
      a short timeout, and the dirty rectangles.
- [ ] Hand the mapped rows (`RowPitch` as the stride) to the frame slot's
      reused buffer with a single copy, keeping BGRA. The encoder converts
      BGRA directly.
- [ ] Draw the pointer into that buffer (as now), and release the DXGI
      frame before encoding.
- [ ] Measure on the laptop at 1080p/1440p/4K, before and after: fps and
      CPU in the stats overlay.

## Pointers

- `crates/slipstream-platform/src/generic.rs` (capture loop)
- `crates/slipstream-platform/src/win_cursor.rs`
- `crates/slipstream-platform/src/slot.rs` (`buffer`/`reuse`)
