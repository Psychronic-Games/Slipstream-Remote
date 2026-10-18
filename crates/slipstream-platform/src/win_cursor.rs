//! Draws the Windows mouse pointer into captured frames. DXGI desktop
//! duplication (and xcap's WGC path) capture the screen *without* the
//! pointer, so the viewer would otherwise see none at all.

use std::cell::RefCell;

use windows_sys::Win32::{
    Foundation::POINT,
    Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HBITMAP,
        ReleaseDC,
    },
    UI::WindowsAndMessaging::{
        CURSOR_SHOWING, CURSORINFO, GetCursorInfo, GetIconInfo, GetPhysicalCursorPos, HCURSOR, ICONINFO, IDC_ARROW,
        LoadCursorW,
    },
};

/// A cursor image as straight-alpha RGBA.
struct Cursor {
    handle: HCURSOR,
    w: i32,
    h: i32,
    hot: (i32, i32),
    rgba: Vec<u8>,
}

thread_local! {
    static CACHE: RefCell<Option<Cursor>> = const { RefCell::new(None) };
}

/// Reads a bitmap as top-down 32-bit BGRA.
unsafe fn bitmap_bgra(bmp: HBITMAP) -> Option<(i32, i32, Vec<u8>)> {
    // SAFETY: caller passes a valid bitmap handle; buffers are sized from it.
    unsafe {
        let mut info: BITMAP = std::mem::zeroed();
        if GetObjectW(bmp, std::mem::size_of::<BITMAP>() as i32, (&mut info as *mut BITMAP).cast()) == 0 {
            return None;
        }
        let (w, h) = (info.bmWidth, info.bmHeight);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(std::ptr::null_mut());
        let rows = GetDIBits(dc, bmp, 0, h as u32, px.as_mut_ptr().cast(), &mut bi, DIB_RGB_COLORS);
        ReleaseDC(std::ptr::null_mut(), dc);
        (rows != 0).then_some((w, h, px))
    }
}

/// Converts a cursor handle to RGBA (colour cursors with or without alpha,
/// and old monochrome AND/XOR cursors such as the text I-beam).
unsafe fn load(handle: HCURSOR) -> Option<Cursor> {
    // SAFETY: GetIconInfo gives us bitmaps we own and must delete.
    unsafe {
        let mut ii: ICONINFO = std::mem::zeroed();
        if GetIconInfo(handle, &mut ii) == 0 {
            return None;
        }
        let mask = bitmap_bgra(ii.hbmMask);
        let color = if ii.hbmColor.is_null() { None } else { bitmap_bgra(ii.hbmColor) };
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor);
        }
        let hot = (ii.xHotspot as i32, ii.yHotspot as i32);
        let (mw, mh, mask) = mask?;
        let (w, h, mut rgba) = match color {
            Some((w, h, bgra)) => {
                let (px, _) = bgra.as_chunks::<4>();
                let has_alpha = px.iter().any(|p| p[3] != 0);
                let mut out = vec![0u8; bgra.len()];
                for (i, p) in px.iter().enumerate() {
                    // Without an alpha channel, the AND mask says what's transparent.
                    let a = if has_alpha { p[3] } else if mask.get(i * 4).copied().unwrap_or(0) > 127 { 0 } else { 255 };
                    out[i * 4..i * 4 + 4].copy_from_slice(&[p[2], p[1], p[0], a]);
                }
                (w, h, out)
            }
            None => {
                // Monochrome: mask is AND (top half) over XOR (bottom half).
                let h = mh / 2;
                let mut out = vec![0u8; (mw * h * 4) as usize];
                for y in 0..h {
                    for x in 0..mw {
                        let and = mask[((y * mw + x) * 4) as usize] > 127;
                        let xor = mask[(((y + h) * mw + x) * 4) as usize] > 127;
                        let px = match (and, xor) {
                            (false, false) => [0, 0, 0, 255],
                            (false, true) => [255, 255, 255, 255],
                            (true, false) => [0, 0, 0, 0],
                            (true, true) => [0, 0, 0, 255], // "invert": draw dark
                        };
                        out[((y * mw + x) * 4) as usize..][..4].copy_from_slice(&px);
                    }
                }
                (mw, h, out)
            }
        };
        rgba.truncate((w * h * 4) as usize);
        Some(Cursor { handle, w, h, hot, rgba })
    }
}

/// Where the pointer is and what it looks like, in physical screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pointer {
    handle: usize,
    pos: (i32, i32),
}

/// The standard arrow (a shared system cursor).
fn arrow() -> HCURSOR {
    // SAFETY: loads a predefined cursor; no instance, nothing to free.
    unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) }
}

/// The pointer right now. Windows hides it on a computer with no mouse
/// plugged in (a server or VM reached only remotely: it shows while another
/// remote tool's virtual mouse is there and vanishes when that one leaves),
/// and while a touch screen is in use. The person controlling it remotely
/// still needs to see where they are, so it's then drawn as the arrow.
pub fn current() -> Option<Pointer> {
    // SAFETY: both calls fill properly sized structs.
    unsafe {
        let mut ci: CURSORINFO = std::mem::zeroed();
        ci.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
        if GetCursorInfo(&mut ci) == 0 {
            return None;
        }
        if ci.flags & CURSOR_SHOWING == 0 || ci.hCursor.is_null() {
            ci.hCursor = arrow();
        }
        // Physical coordinates whatever this process's DPI awareness, to
        // match the monitor positions (which are physical too).
        let mut p = POINT { x: 0, y: 0 };
        if GetPhysicalCursorPos(&mut p) == 0 {
            p = ci.ptScreenPos;
        }
        Some(Pointer { handle: ci.hCursor as usize, pos: (p.x, p.y) })
    }
}

/// Composites `pointer` into an RGBA frame showing the monitor whose
/// top-left corner is at `origin` (screen coordinates).
pub fn draw(frame: &mut [u8], fw: u32, fh: u32, stride: u32, origin: (i32, i32), pointer: Pointer) {
    let handle = pointer.handle as HCURSOR;
    let pos = pointer.pos;
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.as_ref().is_none_or(|c| c.handle != handle) {
            // SAFETY: cursor handles are shared system objects; a stale one
            // just makes GetIconInfo fail.
            // A cursor that can't be read (another session's, say) is
            // drawn as the arrow rather than not at all.
            *cache = unsafe { load(handle).or_else(|| load(arrow()).map(|c| Cursor { handle, ..c })) };
        }
        let Some(c) = cache.as_ref() else { return };
        let (x0, y0) = (pos.0 - c.hot.0 - origin.0, pos.1 - c.hot.1 - origin.1);
        for cy in 0..c.h {
            let y = y0 + cy;
            if y < 0 || y >= fh as i32 {
                continue;
            }
            for cx in 0..c.w {
                let x = x0 + cx;
                if x < 0 || x >= fw as i32 {
                    continue;
                }
                let s = ((cy * c.w + cx) * 4) as usize;
                let a = c.rgba[s + 3] as u32;
                if a == 0 {
                    continue;
                }
                let d = (y as u32 * stride + x as u32 * 4) as usize;
                if d + 3 >= frame.len() {
                    continue;
                }
                for i in 0..3 {
                    let src = c.rgba[s + i] as u32;
                    frame[d + i] = ((src * a + frame[d + i] as u32 * (255 - a)) / 255) as u8;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDC_ARROW, IDC_HAND, IDC_IBEAM, LoadCursorW};

    use super::*;

    #[test]
    fn system_cursors_decode_and_draw() {
        for id in [IDC_ARROW, IDC_IBEAM, IDC_HAND] {
            // SAFETY: loading a shared system cursor.
            let handle = unsafe { LoadCursorW(std::ptr::null_mut(), id) };
            let c = unsafe { load(handle) }.expect("cursor loads");
            let alphas: Vec<u8> = c.rgba.chunks(4).map(|p| p[3]).collect();
            assert!(alphas.iter().any(|&a| a == 255), "cursor has visible pixels");
            assert!(alphas.iter().any(|&a| a == 0), "cursor has a transparent background");

            // Drawn at the hotspot of a grey 64x64 frame placed at (100, 100).
            let mut frame = vec![128u8; 64 * 64 * 4];
            let p = Pointer { handle: handle as usize, pos: (100 + 10 + c.hot.0, 100 + 10 + c.hot.1) };
            CACHE.with(|cache| *cache.borrow_mut() = None);
            draw(&mut frame, 64, 64, 64 * 4, (100, 100), p);
            assert!(frame.chunks(4).any(|px| px[..3] != [128, 128, 128]), "pointer was drawn");
        }
    }
}
