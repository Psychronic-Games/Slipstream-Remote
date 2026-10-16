//! X11 screen capture in pure Rust (x11rb): MIT-SHM when available for
//! speed, plain GetImage otherwise, with the mouse cursor composited in via
//! XFixes. Needs no system libraries beyond a running X server.

use std::{
    os::fd::{FromRawFd, OwnedFd},
    ptr::NonNull,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use slipstream_codec::{PixelFormat, RawFrame};
use slipstream_core::proto::DisplayInfo;
use x11rb::{
    connection::Connection,
    protocol::{
        randr::ConnectionExt as _,
        shm::ConnectionExt as _,
        xfixes::ConnectionExt as _,
        xproto::{ConnectionExt as _, ImageFormat, Window},
    },
    rust_connection::RustConnection,
};

use crate::{FrameSlot, FrameStream};

fn connect() -> Result<(RustConnection, Window)> {
    let (conn, screen) = RustConnection::connect(None).context("cannot connect to the X server")?;
    let root = conn.setup().roots[screen].root;
    Ok((conn, root))
}

pub fn list_displays() -> Result<Vec<DisplayInfo>> {
    let (conn, root) = connect()?;
    let mut out = Vec::new();
    if let Ok(Ok(reply)) = conn.randr_get_monitors(root, true).map(|c| c.reply()) {
        {
            for (i, m) in reply.monitors.iter().enumerate() {
                let name = conn
                    .get_atom_name(m.name)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                    .unwrap_or_else(|| format!("Display {}", i + 1));
                out.push(DisplayInfo {
                    index: i as u32,
                    name,
                    width: m.width as u32,
                    height: m.height as u32,
                    x: m.x as i32,
                    y: m.y as i32,
                    primary: m.primary,
                });
            }
        }
    }
    if out.is_empty() {
        let s = &conn.setup().roots[0];
        out.push(DisplayInfo {
            index: 0,
            name: "Screen".into(),
            width: s.width_in_pixels as u32,
            height: s.height_in_pixels as u32,
            x: 0,
            y: 0,
            primary: true,
        });
    }
    if !out.iter().any(|d| d.primary) {
        out[0].primary = true;
    }
    Ok(out)
}

/// A shared-memory segment the X server writes screenshots into.
struct Shm {
    seg: u32,
    ptr: NonNull<u8>,
    len: usize,
}

impl Shm {
    fn new(conn: &RustConnection, len: usize) -> Result<Self> {
        conn.shm_query_version()?.reply().map_err(|_| anyhow!("no MIT-SHM"))?;
        // SAFETY: plain libc calls; results are checked.
        let fd = unsafe { libc::memfd_create(c"slipstream-shm".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            bail!("memfd_create failed");
        }
        // SAFETY: we own the freshly created fd.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        use std::os::fd::AsRawFd;
        // SAFETY: valid fd; size checked by the kernel.
        if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } != 0 {
            bail!("ftruncate failed");
        }
        // SAFETY: shared read mapping of our own memfd.
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        if ptr == libc::MAP_FAILED {
            bail!("mmap failed");
        }
        let seg = conn.generate_id()?;
        conn.shm_attach_fd(seg, fd, false)?.check()?;
        Ok(Self { seg, ptr: NonNull::new(ptr.cast()).expect("mmap ok"), len })
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: mapping is valid for `len` bytes for our lifetime.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        // SAFETY: unmapping our own mapping.
        unsafe { libc::munmap(self.ptr.as_ptr().cast(), self.len) };
    }
}

/// Starts capturing `display`; frames go to the returned stream.
pub fn capture(display: DisplayInfo, max_fps: u32) -> Result<FrameStream> {
    let slot = FrameSlot::new();
    let producer = slot.clone();
    let interval = Duration::from_secs_f64(1.0 / max_fps.clamp(1, 240) as f64);
    // Fail fast (before spawning) if there's no X server.
    connect()?;
    thread::Builder::new().name("x11-capture".into()).spawn(move || {
        if let Err(e) = capture_loop(&display, interval, &producer) {
            producer.fail(format!("X11 capture: {e:#}"));
        }
    })?;
    Ok(FrameStream::new(slot))
}

fn capture_loop(d: &DisplayInfo, interval: Duration, slot: &FrameSlot) -> Result<()> {
    let (conn, root) = connect()?;
    let (w, h) = (d.width as u16, d.height as u16);
    let len = w as usize * h as usize * 4;
    let shm = Shm::new(&conn, len).map_err(|e| tracing::info!("X11: {e}; using GetImage")).ok();
    let cursor = conn.xfixes_query_version(5, 0).ok().and_then(|c| c.reply().ok()).is_some();
    let mut next = Instant::now();
    while !slot.is_closed() {
        let mut data = match &shm {
            Some(shm) => {
                let reply = conn
                    .shm_get_image(root, d.x as i16, d.y as i16, w, h, !0, ImageFormat::Z_PIXMAP.into(), shm.seg, 0)?
                    .reply()?;
                if reply.depth < 24 {
                    bail!("unsupported colour depth {}", reply.depth);
                }
                shm.bytes().to_vec()
            }
            None => {
                let reply = conn.get_image(ImageFormat::Z_PIXMAP, root, d.x as i16, d.y as i16, w, h, !0)?.reply()?;
                if reply.depth < 24 || reply.data.len() < len {
                    bail!("unsupported image format (depth {})", reply.depth);
                }
                reply.data
            }
        };
        if cursor && let Ok(c) = conn.xfixes_get_cursor_image()?.reply() {
            draw_cursor(&mut data, w as i32, h as i32, c.x as i32 - c.xhot as i32 - d.x, c.y as i32 - c.yhot as i32 - d.y, c.width as i32, c.height as i32, &c.cursor_image);
        }
        // Z_PIXMAP 24/32-bit on little-endian is B, G, R, X.
        slot.put(RawFrame { width: w as u32, height: h as u32, stride: w as u32 * 4, format: PixelFormat::Bgra, data });
        next += interval;
        match next.checked_duration_since(Instant::now()) {
            Some(wait) => thread::sleep(wait),
            None => next = Instant::now(),
        }
    }
    Ok(())
}

/// Alpha-blends the (premultiplied ARGB) cursor image into a BGRX frame.
#[allow(clippy::too_many_arguments)]
fn draw_cursor(frame: &mut [u8], fw: i32, fh: i32, x0: i32, y0: i32, cw: i32, ch: i32, pixels: &[u32]) {
    for cy in 0..ch {
        let y = y0 + cy;
        if !(0..fh).contains(&y) {
            continue;
        }
        for cx in 0..cw {
            let x = x0 + cx;
            if !(0..fw).contains(&x) {
                continue;
            }
            let p = pixels[(cy * cw + cx) as usize];
            let a = p >> 24;
            if a == 0 {
                continue;
            }
            let o = ((y * fw + x) * 4) as usize;
            let inv = 255 - a;
            for (i, shift) in [(0usize, 0u32), (1, 8), (2, 16)] {
                let src = (p >> shift) & 0xff; // premultiplied
                frame[o + i] = (src + (frame[o + i] as u32 * inv) / 255).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_blends_and_clips() {
        let mut frame = vec![0u8; 4 * 4 * 4];
        // 2x2 opaque white cursor at (3,3): only one pixel is inside.
        draw_cursor(&mut frame, 4, 4, 3, 3, 2, 2, &[0xffff_ffff; 4]);
        let o = (3 * 4 + 3) * 4;
        assert_eq!(&frame[o..o + 3], &[255, 255, 255]);
        assert_eq!(frame[0], 0);
    }
}
