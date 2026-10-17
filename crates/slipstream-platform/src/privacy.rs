//! Windows privacy mode: a black curtain over the computer's own screens
//! while someone works on it remotely.
//!
//! The curtain is excluded from screen capture (`WDA_EXCLUDEFROMCAPTURE`,
//! Windows 10 2004 and later), so the viewer still sees the desktop, and it
//! is transparent to the mouse (`WS_EX_TRANSPARENT`), so injected clicks
//! reach the windows underneath. Local keyboard and mouse are blocked
//! separately by the input thread where Windows allows it (see `generic`).

use std::{
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};

use anyhow::{Result, anyhow, bail};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, CreateFontW, DT_CENTER, DT_NOPREFIX, DT_WORDBREAK, DeleteObject, DrawTextW, EndPaint,
        FillRect, GetStockObject, BLACK_BRUSH, PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
    },
    System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW},
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW, GetSystemMetrics,
        HWND_TOPMOST, LWA_ALPHA, MSG, PostMessageW, PostQuitMessage, RegisterClassW, SM_CXVIRTUALSCREEN,
        SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SW_SHOWNOACTIVATE, SetLayeredWindowAttributes, SetTimer, SetWindowDisplayAffinity, SetWindowPos, ShowWindow,
        TranslateMessage, WM_CLOSE, WM_DESTROY, WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    },
};

const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;

/// The text on the curtain.
static MESSAGE: Mutex<String> = Mutex::new(String::new());

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Windows build number (RtlGetVersion: not subject to version lies).
fn build_number() -> u32 {
    #[repr(C)]
    struct OsVersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }
    let ntdll = wide("ntdll.dll");
    // SAFETY: RtlGetVersion fills an OSVERSIONINFOW whose size field is set.
    unsafe {
        let module = LoadLibraryW(ntdll.as_ptr());
        let Some(f) = GetProcAddress(module, c"RtlGetVersion".as_ptr().cast()) else { return 0 };
        let get: unsafe extern "system" fn(*mut OsVersionInfo) -> i32 = std::mem::transmute(f);
        let mut v: OsVersionInfo = std::mem::zeroed();
        v.size = size_of::<OsVersionInfo>() as u32;
        if get(&mut v) == 0 { v.build } else { 0 }
    }
}

/// Privacy mode needs capture exclusion: Windows 10 2004 (build 19041) on.
pub fn supported() -> bool {
    build_number() >= 19041
}

/// The curtain, up until dropped.
pub struct Curtain {
    hwnd: usize,
    thread: Option<JoinHandle<()>>,
}

impl Curtain {
    pub fn show(who: &str) -> Result<Self> {
        if !supported() {
            bail!("needs Windows 10 version 2004 or later");
        }
        *MESSAGE.lock().unwrap() = format!(
            "SLIPSTREAM REMOTE · PRIVACY MODE\n\n{} is working on this computer.\nThe screen comes back when they're done.",
            if who.is_empty() { "Your support technician" } else { who }
        );
        let (tx, rx) = mpsc::channel();
        let thread = thread::Builder::new().name("privacy".into()).spawn(move || run(tx))?;
        match rx.recv() {
            Ok(Ok(hwnd)) => Ok(Self { hwnd, thread: Some(thread) }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(anyhow!("the privacy screen stopped")),
        }
    }
}

impl Drop for Curtain {
    fn drop(&mut self) {
        // SAFETY: posting to the curtain's window; its thread destroys it.
        unsafe { PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

static CLASS: AtomicUsize = AtomicUsize::new(0);

fn run(ready: mpsc::Sender<Result<usize>>) {
    // Service mode: put the curtain on the desktop that has input.
    crate::windesk::follow();
    // SAFETY: window creation and a message loop on this thread.
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let class = wide("SlipstreamPrivacy");
        if CLASS.load(Ordering::Relaxed) == 0 {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                hbrBackground: GetStockObject(BLACK_BRUSH),
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            CLASS.store(RegisterClassW(&wc) as usize, Ordering::Relaxed);
        }
        let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (w, h) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        let title = wide("Slipstream privacy");
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            x,
            y,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            let _ = ready.send(Err(anyhow!("couldn't create the privacy screen")));
            return;
        }
        if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) == 0 {
            DestroyWindow(hwnd);
            let _ = ready.send(Err(anyhow!("Windows refused to hide the privacy screen from capture")));
            return;
        }
        SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        // Stay above windows that come to the front later.
        SetTimer(hwnd, 1, 1000, None);
        let _ = ready.send(Ok(hwnd as usize));
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: standard handling of our own window's messages.
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut r: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut r);
                FillRect(hdc, &r, GetStockObject(BLACK_BRUSH));
                let face = wide("Segoe UI");
                let font = CreateFontW(-34, 0, 0, 0, 600, 0, 0, 0, 0, 0, 0, 5, 0, face.as_ptr());
                let old = SelectObject(hdc, font);
                SetBkMode(hdc, TRANSPARENT as i32);
                SetTextColor(hdc, 0x0000_99ff); // the console orange (0x00BBGGRR)
                let text = wide(&MESSAGE.lock().unwrap());
                let mut box_ = RECT { left: r.left, top: r.top + (r.bottom - r.top) / 2 - 90, right: r.right, bottom: r.bottom };
                DrawTextW(hdc, text.as_ptr(), -1, &mut box_, DT_CENTER | DT_WORDBREAK | DT_NOPREFIX);
                SelectObject(hdc, old);
                DeleteObject(font);
                EndPaint(hwnd, &ps);
                0
            }
            WM_TIMER => {
                SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                0
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
