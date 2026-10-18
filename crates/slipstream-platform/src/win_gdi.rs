//! Screen capture with GDI, for the service's host on the login screen and
//! UAC's secure desktop (DXGI duplication only follows the normal desktop).
//! One bitmap is kept and reused, and frames come out as BGRA, the way
//! Windows has them: no per-frame allocation or pixel shuffling, which made
//! xcap's screenshots the bottleneck on machines without graphics.

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GdiFlush, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
};

/// Captures one monitor's area of the calling thread's desktop. Recreate it
/// after the thread switches desktops (its screen DC belongs to the old one).
pub struct GdiCapture {
    screen: HDC,
    mem: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *const u8,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl GdiCapture {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Option<Self> {
        let (width, height) = (width as i32, height as i32);
        if width <= 0 || height <= 0 {
            return None;
        }
        // SAFETY: GDI objects created here are released in Drop (or on the
        // failure paths); the DIB's bits stay valid while it exists.
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            if screen.is_null() {
                return None;
            }
            let mem = CreateCompatibleDC(screen);
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if mem.is_null() || bitmap.is_null() || bits.is_null() {
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
                if !mem.is_null() {
                    DeleteDC(mem);
                }
                ReleaseDC(std::ptr::null_mut(), screen);
                return None;
            }
            let old = SelectObject(mem, bitmap);
            Some(Self { screen, mem, bitmap, old, bits: bits.cast(), x, y, width, height })
        }
    }

    /// Copies the screen into `out` as BGRA rows of `width * 4` bytes.
    /// False if Windows refused (typically a desktop switch).
    pub fn grab(&mut self, out: &mut Vec<u8>) -> bool {
        let len = self.width as usize * self.height as usize * 4;
        // SAFETY: both DCs are live; the DIB holds `len` bytes, complete
        // once GdiFlush returns.
        unsafe {
            if BitBlt(self.mem, 0, 0, self.width, self.height, self.screen, self.x, self.y, SRCCOPY) == 0 {
                return false;
            }
            GdiFlush();
            out.clear();
            out.extend_from_slice(std::slice::from_raw_parts(self.bits, len));
        }
        true
    }
}

impl Drop for GdiCapture {
    fn drop(&mut self) {
        // SAFETY: releasing what new() created, in reverse order.
        unsafe {
            SelectObject(self.mem, self.old);
            DeleteObject(self.bitmap);
            DeleteDC(self.mem);
            ReleaseDC(std::ptr::null_mut(), self.screen);
        }
    }
}
