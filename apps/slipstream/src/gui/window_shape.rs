/// Native regions work with both Direct3D and the Server 2016 software
/// renderer; Windows does not need an alpha-capable swapchain for this.
#[cfg(windows)]
pub(super) fn update() {
    use std::{cell::RefCell, collections::BTreeMap};
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, SetWindowRgn},
        System::Threading::GetCurrentThreadId,
        UI::{
            HiDpi::GetDpiForWindow,
            Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::{EnumThreadWindows, GetWindowLongPtrW, GetWindowRect, GetWindowTextW, IsWindow, IsWindowVisible, IsZoomed,
                SetWindowPos, GWL_STYLE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
                WM_NCCALCSIZE, WM_NCDESTROY, WS_CAPTION},
        },
    };
    const FRAME_SUBCLASS: usize = 0x534c4341;
    // egui-winit enables winit's undecorated shadow. Its NCCALCSIZE handler
    // leaves a one-pixel native strip at the top, white on older Windows.
    // Cover the whole non-maximized window with our client surface instead.
    // Let winit handle maximized work-area bounds and every other message.
    unsafe extern "system" fn client_frame(window: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, id: usize, _data: usize) -> LRESULT {
        unsafe {
            if msg == WM_NCCALCSIZE && wparam != 0 && IsZoomed(window) == 0 {
                return 0;
            }
            if msg == WM_NCDESTROY {
                RemoveWindowSubclass(window, Some(client_frame), id);
            }
            DefSubclassProc(window, msg, wparam, lparam)
        }
    }
    thread_local! {static LAST:RefCell<BTreeMap<usize,(i32,i32,i32)>>=const { RefCell::new(BTreeMap::new()) };}
    unsafe extern "system" fn visit(window: HWND, _data: LPARAM) -> i32 {
        unsafe {
            if IsWindowVisible(window) == 0 {
                return 1;
            }
            let mut title = [0u16; 512];
            let len = GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32);
            if len <= 0 || !String::from_utf16_lossy(&title[..len as usize]).contains("Slipstream Remote") {
                return 1;
            }
            let mut data = 0;
            if GetWindowLongPtrW(window, GWL_STYLE) as u32 & WS_CAPTION != 0
                && GetWindowSubclass(window, Some(client_frame), FRAME_SUBCLASS, &mut data) == 0
                && SetWindowSubclass(window, Some(client_frame), FRAME_SUBCLASS, 0) != 0
            {
                // No RefCell borrow is held while Windows recalculates the frame.
                SetWindowPos(window, std::ptr::null_mut(), 0, 0, 0, 0,
                    SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
            }
            let mut rect: RECT = std::mem::zeroed();
            if GetWindowRect(window, &mut rect) == 0 {
                return 1;
            }
            let mut monitor: MONITORINFO = std::mem::zeroed();
            monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            let full = GetMonitorInfoW(MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST), &mut monitor) != 0
                && rect.left == monitor.rcMonitor.left
                && rect.top == monitor.rcMonitor.top
                && rect.right == monitor.rcMonitor.right
                && rect.bottom == monitor.rcMonitor.bottom;
            let radius = if full || IsZoomed(window) != 0 {
                0
            } else {
                (18 * GetDpiForWindow(window).max(96) / 96) as i32
            };
            let key = (rect.right - rect.left, rect.bottom - rect.top, radius);
            if LAST.with(|last| last.borrow().get(&(window as usize)) == Some(&key)) {
                return 1;
            }
            let region = if radius == 0 {
                std::ptr::null_mut()
            } else {
                CreateRoundRectRgn(0, 0, key.0 + 1, key.1 + 1, radius * 2, radius * 2)
            };
            if radius > 0 && region.is_null() {
                return 1;
            }
            // SetWindowRgn can send nested native window messages. Cache first,
            // and hold no RefCell borrow across that call.
            LAST.with(|last| {
                last.borrow_mut().insert(window as usize, key);
            });
            if SetWindowRgn(window, region, 1) == 0 {
                LAST.with(|last| {
                    last.borrow_mut().remove(&(window as usize));
                });
                if !region.is_null() {
                    DeleteObject(region);
                }
            }
        }
        1
    }
    unsafe {
        LAST.with(|last| last.borrow_mut().retain(|&window, _| IsWindow(window as HWND) != 0));
        EnumThreadWindows(GetCurrentThreadId(), Some(visit), 0);
    }
}

#[cfg(target_os = "linux")]
pub(super) fn update() {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_none() {
        return;
    }
    thread_local! {static SHAPE:std::cell::RefCell<Option<X11Shape>>=std::cell::RefCell::new(X11Shape::connect().ok());}
    SHAPE.with(|shape| {
        if let Some(shape) = shape.borrow_mut().as_mut() {
            let _ = shape.update();
        }
    });
}

#[cfg(target_os = "linux")]
struct X11Shape {
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
    atoms: [u32; 6],
    windows: Vec<u32>,
    discovered: Option<std::time::Instant>,
    last: std::collections::BTreeMap<u32, (u16, u16, u16)>,
}

#[cfg(target_os = "linux")]
impl X11Shape {
    fn connect() -> anyhow::Result<Self> {
        use x11rb::{connection::Connection, protocol::xproto::ConnectionExt};
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let mut atoms = [0; 6];
        for (slot, name) in atoms.iter_mut().zip([
            "_NET_CLIENT_LIST",
            "_NET_WM_PID",
            "_NET_WM_STATE",
            "_NET_WM_STATE_FULLSCREEN",
            "_NET_WM_STATE_MAXIMIZED_HORZ",
            "_NET_WM_STATE_MAXIMIZED_VERT",
        ]) {
            *slot = conn.intern_atom(false, name.as_bytes())?.reply()?.atom;
        }
        Ok(Self {
            conn,
            root,
            atoms,
            windows: Vec::new(),
            discovered: None,
            last: Default::default(),
        })
    }
    fn update(&mut self) -> anyhow::Result<()> {
        use x11rb::{
            connection::Connection,
            protocol::xproto::{AtomEnum, ConnectionExt},
        };
        if self.discovered.is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(250)) {
            let clients = self.conn.get_property(false, self.root, self.atoms[0], AtomEnum::WINDOW, 0, 4096)?.reply()?;
            let mut owned = Vec::new();
            for window in clients.value32().into_iter().flatten() {
                if let Ok(reply) = self.conn.get_property(false, window, self.atoms[1], AtomEnum::CARDINAL, 0, 1)?.reply()
                    && reply.value32().and_then(|mut v| v.next()) == Some(std::process::id())
                {
                    owned.push(window);
                }
            }
            self.windows = owned;
            self.last.retain(|w, _| self.windows.contains(w));
            self.discovered = Some(std::time::Instant::now());
        }
        for window in self.windows.clone() {
            let _ = self.round(window);
        }
        self.conn.flush()?;
        Ok(())
    }
    fn round(&mut self, window: u32) -> anyhow::Result<()> {
        use x11rb::protocol::{
            shape::{ConnectionExt as _, SK, SO},
            xproto::{AtomEnum, ClipOrdering, ConnectionExt as _, Rectangle},
        };
        let geometry = self.conn.get_geometry(window)?;
        let state = self.conn.get_property(false, window, self.atoms[2], AtomEnum::ATOM, 0, 64)?;
        let geometry = geometry.reply()?;
        let state = state.reply()?.value32().into_iter().flatten().collect::<Vec<_>>();
        let square = state.contains(&self.atoms[3]) || (state.contains(&self.atoms[4]) && state.contains(&self.atoms[5]));
        let (w, h) = (geometry.width, geometry.height);
        let r = if square { 0 } else { 18.min(w / 2).min(h / 2) };
        let key = (w, h, r);
        if self.last.get(&window) == Some(&key) {
            return Ok(());
        }
        let mut bands = Vec::new();
        if h > 2 * r {
            bands.push(Rectangle {
                x: 0,
                y: r as i16,
                width: w,
                height: h - 2 * r,
            });
        }
        for y in 0..r {
            let dy = f64::from(r) - f64::from(y) - 0.5;
            let inset = (f64::from(r) - (f64::from(r).powi(2) - dy * dy).sqrt()).ceil() as u16;
            for row in [y, h - y - 1] {
                bands.push(Rectangle {
                    x: inset as i16,
                    y: row as i16,
                    width: w - 2 * inset,
                    height: 1,
                });
            }
        }
        self.conn
            .shape_rectangles(SO::SET, SK::BOUNDING, ClipOrdering::UNSORTED, window, 0, 0, &bands)?
            .check()?;
        self.last.insert(window, key);
        Ok(())
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(super) fn update() {}
