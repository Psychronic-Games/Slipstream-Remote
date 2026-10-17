---
title: Verify 0.5.0 on real machines
labels: [testing, priority:high]
---

## Background

Three 0.5.0 fixes so far have only been checked on the dev machine or
under Wine:

- the host's pointer drawn into Windows video,
- the host-side chat as its own window,
- the UI animating at the display's refresh rate.

Separately, the portable Linux build has never been re-tried on the Debian
server where the first build failed to start.

## Checklist

Update both machines first: use the *Install & restart* banner, or
re-download from the station. Both must be on 0.5.0, because the protocol
changed.

**Linux viewer → Windows laptop**
- [ ] The Windows pointer is visible in the video before and after clicking
      into it, and follows the mouse on a still screen.
- [ ] Pointer shapes change: text I-beam over a text field, hand over a
      link, resize arrows on a window edge.
- [ ] The pointer is in the right place on every monitor, including one
      with display scaling (125 % / 150 %).
- [ ] The *Inbound link* banner on the laptop pulses smoothly in the video,
      and the stats show more than 2 FPS while it animates.

**Chat**
- [ ] On the controlled computer, *Chat* opens a separate window that can
      be moved outside the main window and onto another monitor.
- [ ] A message from the viewer opens it automatically and flashes the
      taskbar entry.
- [ ] It keeps working while the main window is minimized, and while the
      main window is hidden in the tray.
- [ ] Closing it and pressing *Chat* again brings it back with the history.

**Linux**
- [ ] The Linux app starts on the Debian server (no PipeWire installed).
- [ ] The animations look smooth, and CPU use stays reasonable when the
      window is in view (it measured about 6 % of one core on the dev
      machine).

**Station**
- [ ] Press *Restart relay* on the admin page. The relay log then shows the
      new binary, and a session with `SLIPSTREAM_RELAY_ONLY=1` still works.

## Done when

Every box is ticked, or a follow-up issue exists for each one that fails.

## Pointers

- Pointer drawing: `crates/slipstream-platform/src/win_cursor.rs` and the
  capture loop in `generic.rs`
- Chat window: `apps/slipstream/src/gui/home.rs` (`incoming_windows`)
- Repaint policy: end of `App::ui` in `apps/slipstream/src/gui/mod.rs`
- Logs: `slipstream.log` in the settings folder (`slipstream info` shows
  where)
