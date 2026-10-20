//! Windows: a compact window from plain Win32 controls, for machines where
//! wgpu finds no graphics adapter (Windows Server 2016's Direct3D 12 can't
//! run the shaders it needs; servers often have basic display drivers). The
//! computer can still be controlled: it shows the ID, the password and the
//! link status, and installs updates. Controlling others needs the full app.

use std::{
    cell::RefCell,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc as smpsc,
    },
};

use anyhow::{Result, bail};
use slipstream_core::{directory::DirectoryClient, provision::StationLink};
use slipstream_session::{Client, ClientEvent, DirStatus, update::UpdateInfo};
use tokio::{runtime::Runtime, sync::mpsc};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM,POINT},
    Graphics::Gdi::{
        AddFontMemResourceEx, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        CreateFontW, CreateSolidBrush, DIB_RGB_COLORS, DT_CENTER, DT_LEFT, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject,
        DrawTextW, EndPaint, GetDC, GetDIBits, GetTextExtentPoint32W, HDC, HFONT,
        InvalidateRect, PAINTSTRUCT, ReleaseDC, SRCCOPY, SelectObject, ScreenToClient, SetBkColor, SetBkMode, SetDIBitsToDevice, SetTextColor,
        TRANSPARENT,
    },
    System::{
        DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
        LibraryLoader::GetModuleHandleW,
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
    },
    UI::Controls::{DRAWITEMSTRUCT, ODS_SELECTED, ODT_BUTTON},
    UI::Input::KeyboardAndMouse::EnableWindow,
    UI::WindowsAndMessaging::{
        AdjustWindowRect, BS_OWNERDRAW, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
        DestroyWindow, DispatchMessageW, GetClientRect, GetDlgCtrlID, GetMessageW, IDC_ARROW, LoadCursorW, LoadIconW, MSG,
        PostMessageW, PostQuitMessage, RegisterClassExW, SW_HIDE, SW_RESTORE, SW_SHOW, SendMessageW, SetForegroundWindow,
        SetTimer, SetWindowTextW, ShowWindow, TranslateMessage, WM_APP, WM_COMMAND, WM_CREATE, WM_CTLCOLORSTATIC, WM_DESTROY,
        WM_NCHITTEST, HTCAPTION, SW_MINIMIZE, WM_DRAWITEM, WM_PAINT, WM_SETFONT, WM_TIMER, WNDCLASSEXW, WS_POPUP, WS_CHILD, WS_MINIMIZEBOX, WS_SYSMENU, WS_VISIBLE,
    },
};

use super::{single, theme};

/// What the window shows; updated from the client's events.
#[derive(Default)]
struct View {
    title: String,
    id: String,
    password: String,
    status: String,
    online: bool,
    note: String,
    update: Option<UpdateInfo>,
    /// The update (or the software renderer) is installed: relaunch from here.
    relaunch: Option<PathBuf>,
}

const ID_TITLE: i32 = 101;
const ID_ID_LABEL: i32 = 102;
const ID_ID: i32 = 103;
const ID_PW_LABEL: i32 = 104;
const ID_PW: i32 = 105;
const ID_STATUS: i32 = 106;
const ID_NOTE: i32 = 107;
const BTN_NEW: i32 = 201;
const BTN_COPY: i32 = 202;
const BTN_UPDATE: i32 = 203;
const BTN_QUIT: i32 = 204;
const BTN_FULL: i32 = 205;
const BTN_MIN: i32 = 206;
const BTN_CLOSE: i32 = 207;

/// "Something changed": refresh the texts.
const WM_REFRESH: u32 = WM_APP + 1;
/// A later launch asked for something (see `single`).
const WM_REQUEST: u32 = WM_APP + 2;
const TIMER_SCREENSHOT: usize = 2;
const TIMER_SERVICE: usize = 3;

// The console palette (COLORREF is 0x00BBGGRR).
const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}
const BG: u32 = rgb(2, 3, 8);
const ORANGE: u32 = rgb(255, 153, 0);
const PEACH: u32 = rgb(255, 170, 120);
const LAVENDER: u32 = rgb(204, 153, 204);
const VIOLET: u32 = rgb(153, 119, 221);
const BLUE: u32 = rgb(153, 204, 255);
const TAN: u32 = rgb(255, 204, 153);
const GOLD: u32 = rgb(255, 204, 102);
const GREEN: u32 = rgb(102, 255, 170);
const RED: u32 = rgb(255, 85, 85);
const DIM: u32 = rgb(110, 120, 150);
const WHITE: u32 = rgb(255, 255, 255);

/// Client area size.
const WIDTH: i32 = 500;
const HEIGHT: i32 = 422;

/// Everything the window procedure needs (it runs on the UI thread only).
struct Ui {
    view: Arc<Mutex<View>>,
    client: Client,
    service_sender: Option<crate::service_chat::Sender>,
    rt: Arc<Runtime>,
    requests: smpsc::Receiver<single::Request>,
    controls: Vec<(i32, HWND)>,
    online: bool,
    shown: Vec<(i32, String)>,
    screenshot: Option<PathBuf>,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// The window handle, for other threads to post to (0 until it exists).
static HWND_BITS: AtomicUsize = AtomicUsize::new(0);
/// Read while painting, which Windows may ask for in the middle of our own
/// calls (so it can't wait for `UI`).
static ONLINE: AtomicBool = AtomicBool::new(false);
static BG_BRUSH: AtomicUsize = AtomicUsize::new(0);
static BUTTON_FONT: AtomicUsize = AtomicUsize::new(0);

fn wake(msg: u32) {
    let h = HWND_BITS.load(Ordering::Relaxed);
    if h != 0 {
        // SAFETY: posting to our own window; a stale handle just fails.
        unsafe { PostMessageW(h as HWND, msg, 0, 0) };
    }
}

pub fn run(
    rt: Arc<Runtime>,
    client: Client,
    mut events: mpsc::UnboundedReceiver<ClientEvent>,
    instance: single::Instance,
    screenshot: Option<(PathBuf, f64)>,
    via_service: bool,
) -> Result<()> {
    let view = Arc::new(Mutex::new(View {
        title: "SLIPSTREAM REMOTE".into(),
        id: client.settings().host.last_device_id.map(|i| i.to_string()).unwrap_or_else(|| "--- --- ---".into()),
        status: "Contacting the station…".into(),
        note: match super::softgl::problem() {
            _ if super::softgl::failed_before() => {
                "The full window didn't start on this computer last time, so this is the compact one. It can still be controlled as usual.".into()
            }
            Some(why) => format!("Compact window: {why}. The computer can still be controlled as usual."),
            None => "Compact window. The computer can still be controlled as usual.".into(),
        },
        ..Default::default()
    }));

    // Client events → view.
    {
        let view = view.clone();
        rt.spawn(async move {
            while let Some(ev) = events.recv().await {
                {
                    let mut v = view.lock().unwrap();
                    match ev {
                        ClientEvent::Password(p) if !via_service => v.password = p,
                        ClientEvent::Directory(DirStatus::Online { id }) => {
                            v.id = id.to_string();
                            v.status = "ONLINE · READY FOR LINK".into();
                            v.online = true;
                        }
                        ClientEvent::Directory(DirStatus::NotConfigured) => {
                            v.status = "NOT CONNECTED TO A STATION".into();
                            v.online = false;
                            v.note = "Download Slipstream from your support provider's page, or press its \u{201c}Connect this app\u{201d} button.".into();
                        }
                        ClientEvent::Directory(DirStatus::Error(e)) => {
                            v.status = "STATION CONNECTION FAILED".into();
                            v.online = false;
                            v.note = e;
                        }
                        ClientEvent::Directory(_) => {}
                        ClientEvent::Org(Some(o)) => v.title = format!("SLIPSTREAM REMOTE · {}", o.to_uppercase()),
                        ClientEvent::HostSessionStarted { peer, .. } => v.status = format!("{} IS CONNECTED", peer.to_uppercase()),
                        ClientEvent::HostSessionEnded { .. } => v.status = "ONLINE · READY FOR LINK".into(),
                        ClientEvent::HostStatus { text, .. } => {v.note = format!("Support: {text}");wake(WM_APP+3);},
                        ClientEvent::HostChat { text, .. } => v.note = format!("Message: {text}"),
                        ClientEvent::Notice(n) | ClientEvent::Motd(n) => v.note = n,
                        ClientEvent::UpdateAvailable(u) => {
                            v.note = format!("Version {} is available: press Install update.", u.label());
                            v.update = Some(u);
                        }
                        _ => {}
                    }
                }
                wake(WM_REFRESH);
            }
        });
    }
    let mut service_sender = None;
    if via_service {
        let mut bridge=crate::service_chat::Gui::start(&rt,eframe::egui::Context::default());
        service_sender = Some(bridge.sender());
        let view=view.clone();
        rt.spawn(async move {
            while let Some(event)=bridge.events.recv().await {
                use crate::service_chat::Event;
                {
                    let mut v=view.lock().unwrap();
                    match event {
                        Event::Password(password) => v.password = password.unwrap_or_default(),
                        Event::Status{text,..}=>{v.note=format!("Support: {text}");wake(WM_APP+3);},
                        Event::Started{peer,..}=>v.status=format!("{} IS CONNECTED",peer.to_uppercase()),
                        Event::Ended{..}=>v.status="ONLINE · READY FOR LINK".into(),
                        Event::Message{ours:false,text,..}=>{v.note=format!("Message: {text}");wake(WM_APP+3);},
                        _=>{},
                    }
                }
                wake(WM_REFRESH);
            }
        });
    }
    let requests = instance.listen(|| wake(WM_REQUEST));

    // SAFETY: plain Win32 window creation and message loop on this thread;
    // all strings outlive the calls that use them.
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        // The app's own fonts, for this process only.
        for data in [theme::FONT_ANTONIO, theme::FONT_ANTONIO_BOLD, theme::FONT_BARLOW, theme::FONT_BARLOW_SEMIBOLD, theme::FONT_MONO] {
            let n = 0u32; // written by GDI (declared const in windows-sys)
            AddFontMemResourceEx(data.as_ptr().cast(), data.len() as u32, std::ptr::null(), &n);
        }
        let class = wide("SlipstreamLite");
        let bg = CreateSolidBrush(BG);
        BG_BRUSH.store(bg as usize, Ordering::Relaxed);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            // The icon embedded by build.rs (resource 1).
            hIcon: LoadIconW(hinst, 1 as _),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: bg,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        if RegisterClassExW(&wc) == 0 {
            bail!("couldn't register the window class");
        }
        UI.with(|u| {
            *u.borrow_mut() = Some(Ui {
                view: view.clone(),
                client: client.clone(),
                service_sender,
                rt: rt.clone(),
                requests,
                controls: Vec::new(),
                online: false,
                shown: Vec::new(),
                screenshot: screenshot.as_ref().map(|(p, _)| p.clone()),
            })
        });
        let style = WS_POPUP | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut r = RECT { left: 0, top: 0, right: WIDTH, bottom: HEIGHT };
        AdjustWindowRect(&mut r, style, 0);
        let title = wide("Slipstream Remote");
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            r.right - r.left,
            r.bottom - r.top,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            bail!("couldn't create the window");
        }
        HWND_BITS.store(hwnd as usize, Ordering::Relaxed);
        ShowWindow(hwnd, SW_SHOW);
        wake(WM_REFRESH);
        if let Some((_, delay)) = screenshot {
            SetTimer(hwnd, TIMER_SCREENSHOT, (delay * 1000.0) as u32, None);
        }
        if via_service { SetTimer(hwnd, TIMER_SERVICE, 1000, None); }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    HWND_BITS.store(0, Ordering::Relaxed);
    // The new copy waits for this one to let go of the instance port, so
    // start it first: a slow shutdown then can't stop the restart.
    if let Some(exe) = view.lock().unwrap().relaunch.take() {
        std::process::Command::new(exe).env("SLIPSTREAM_RESTARTED", "1").spawn()?;
    }
    rt.block_on(client.shutdown());
    Ok(())
}

/// The title's current font, when [`fit_title`] made one.
static TITLE_FONT: AtomicUsize = AtomicUsize::new(0);

/// Gives the title the largest font (34 px down to 20) at which `text`
/// fits its width.
unsafe fn fit_title(ctrl: HWND, text: &str) {
    const WIDTH: i32 = 460;
    let w: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: measuring with fonts we create, on the control's own DC.
    unsafe {
        let hdc = GetDC(ctrl);
        let mut chosen = std::ptr::null_mut();
        for size in (20..=34).rev().step_by(2) {
            let f = font(size, false, "Antonio");
            let old = SelectObject(hdc, f);
            let mut extent = SIZE { cx: 0, cy: 0 };
            GetTextExtentPoint32W(hdc, w.as_ptr(), w.len() as i32, &mut extent);
            SelectObject(hdc, old);
            if extent.cx <= WIDTH || size == 20 {
                chosen = f;
                break;
            }
            DeleteObject(f);
        }
        ReleaseDC(ctrl, hdc);
        SendMessageW(ctrl, WM_SETFONT, chosen as WPARAM, 1);
        let previous = TITLE_FONT.swap(chosen as usize, Ordering::Relaxed);
        if previous != 0 {
            DeleteObject(previous as _);
        }
    }
}

unsafe fn font(height: i32, bold: bool, face: &str) -> HFONT {
    let face = wide(face);
    // SAFETY: a valid face name; the other arguments are plain values.
    unsafe { CreateFontW(-height, 0, 0, 0, if bold { 700 } else { 400 }, 0, 0, 0, 0, 0, 0, 5 /* CLEARTYPE */, 0, face.as_ptr()) }
}

/// Creates the controls (WM_CREATE).
unsafe fn build(hwnd: HWND) -> Vec<(i32, HWND)> {
    // SAFETY: creating child controls of our live window.
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let (stat, button, edit) = (wide("STATIC"), wide("BUTTON"), wide("EDIT"));
        let mut out = Vec::new();
        let mut add = |class: &[u16], id: i32, style: u32, (x, y, w, h): (i32, i32, i32, i32), f: HFONT| {
            let empty = wide("");
            let c = CreateWindowExW(0, class.as_ptr(), empty.as_ptr(), WS_CHILD | WS_VISIBLE | style, x, y+38, w, h, hwnd, id as _, hinst, std::ptr::null());
            SendMessageW(c, WM_SETFONT, f as WPARAM, 1);
            out.push((id, c));
        };
        const SS_NOPREFIX: u32 = 0x80;
        // The ID and password are read-only edit boxes so they can be
        // selected and copied; Windows colours them like the labels
        // (WM_CTLCOLORSTATIC).
        const ES_AUTOHSCROLL: u32 = 0x80;
        const ES_READONLY: u32 = 0x800;
        let text = ES_READONLY | ES_AUTOHSCROLL;
        let (title, label, big, mid, status, small, btn) = (
            font(34, false, "Antonio"),
            font(17, false, "Antonio"),
            font(52, false, "Share Tech Mono"),
            font(40, false, "Share Tech Mono"),
            font(21, false, "Antonio"),
            font(16, false, "Barlow Semi Condensed"),
            font(19, false, "Antonio"),
        );
        BUTTON_FONT.store(btn as usize, Ordering::Relaxed);
        // One line: a long organisation name shrinks the font (fit_title),
        // and only past the smallest size gets an ellipsis.
        const SS_LEFTNOWORDWRAP: u32 = 0xC;
        const SS_ENDELLIPSIS: u32 = 0x4000;
        add(&stat, ID_TITLE, SS_NOPREFIX | SS_LEFTNOWORDWRAP | SS_ENDELLIPSIS, (20, 26, 460, 46), title);
        add(&stat, ID_ID_LABEL, 0, (20, 74, 300, 26), label);
        add(&edit, ID_ID, text, (20, 98, 460, 58), big);
        add(&stat, ID_PW_LABEL, 0, (20, 154, 300, 26), label);
        add(&edit, ID_PW, text, (20, 178, 460, 46), mid);
        add(&stat, ID_STATUS, SS_NOPREFIX, (20, 226, 460, 28), status);
        add(&stat, ID_NOTE, SS_NOPREFIX, (20, 258, 460, 46), small);
        let pill = BS_OWNERDRAW as u32;
        add(&button, BTN_NEW, pill, (20, 318, 138, 34), btn);
        add(&button, BTN_COPY, pill, (166, 318, 88, 34), btn);
        add(&button, BTN_UPDATE, pill, (262, 318, 128, 34), btn);
        add(&button, BTN_FULL, pill, (262, 318, 128, 34), btn);
        add(&button, BTN_QUIT, pill, (398, 318, 82, 34), btn);
        add(&button,BTN_MIN,pill,(350,-31,62,28),btn);
        add(&button,BTN_CLOSE,pill,(418,-31,70,28),btn);
        // Edit boxes pad their text; line it up with the labels.
        const EM_SETMARGINS: u32 = 0xD3;
        for (_, c) in out.iter().filter(|(id, _)| [ID_ID, ID_PW].contains(id)) {
            SendMessageW(*c, EM_SETMARGINS, 3, 0);
        }
        out
    }
}

fn control(ui: &Ui, id: i32) -> HWND {
    ui.controls.iter().find(|(i, _)| *i == id).map_or(std::ptr::null_mut(), |(_, h)| *h)
}

fn set_text(ui: &mut Ui, id: i32, text: &str) {
    if ui.shown.iter().any(|(i, t)| *i == id && t == text) {
        return;
    }
    ui.shown.retain(|(i, _)| *i != id);
    ui.shown.push((id, text.to_string()));
    let w = wide(text);
    // SAFETY: a live control and a NUL-terminated string.
    unsafe { SetWindowTextW(control(ui, id), w.as_ptr()) };
}

fn refresh(ui: &mut Ui, hwnd: HWND) {
    if ui.service_sender.is_some() {
        let mut view = ui.view.lock().unwrap();
        view.id = crate::service::hosted_id().map(|id| id.to_string()).unwrap_or_else(|| "--- --- ---".into());
        view.online = crate::service::is_running();
        if !view.status.ends_with("IS CONNECTED") {
            view.status = if view.online {"SERVICE RUNNING".into()} else {format!("SERVICE {}", crate::service::status_text().to_uppercase())};
        }
    }
    let (title, id, pw, status, online, note, update) = {
        let v = ui.view.lock().unwrap();
        (v.title.clone(), v.id.clone(), v.password.clone(), v.status.clone(), v.online, v.note.clone(), v.update.is_some())
    };
    // In the update's slot (an update comes first): after the software
    // renderer failed to start, a way to give it another go.
    let full = !update && super::softgl::failed_before();
    if !ui.shown.iter().any(|(i, t)| *i == ID_TITLE && *t == title) {
        // SAFETY: the live title control.
        unsafe { fit_title(control(ui, ID_TITLE), &title) };
    }
    set_text(ui, ID_TITLE, &title);
    set_text(ui, ID_ID_LABEL, "YOUR ID");
    set_text(ui, ID_ID, &id);
    set_text(ui, ID_PW_LABEL, "ONE-TIME CODE");
    set_text(ui, ID_PW, &pw);
    set_text(ui, ID_STATUS, &status);
    set_text(ui, ID_NOTE, &note);
    set_text(ui, BTN_NEW, "New code");
    set_text(ui, BTN_COPY, "Copy");
    set_text(ui, BTN_UPDATE, "Install update");
    set_text(ui, BTN_FULL, "Try full window");
    set_text(ui, BTN_QUIT, "Quit");
    set_text(ui,BTN_MIN,"Minimize");
    set_text(ui,BTN_CLOSE,"Close");
    // SAFETY: live controls of our window.
    unsafe {
        EnableWindow(control(ui, BTN_NEW), i32::from(ui.service_sender.as_ref().is_none_or(|s| s.connected()) && !pw.is_empty()));
        EnableWindow(control(ui, BTN_COPY), i32::from(!pw.is_empty() && id.chars().filter(char::is_ascii_digit).count() == 9));
        ShowWindow(control(ui, BTN_UPDATE), if update { SW_SHOW } else { SW_HIDE });
        ShowWindow(control(ui, BTN_FULL), if full { SW_SHOW } else { SW_HIDE });
        if online != ui.online {
            ui.online = online;
            ONLINE.store(online, Ordering::Relaxed);
            InvalidateRect(control(ui, ID_STATUS), std::ptr::null(), 1);
        }
        let _ = hwnd;
    }
}

/// Puts `text` on the clipboard.
fn copy(hwnd: HWND, text: &str) {
    let w = wide(text);
    // SAFETY: the standard clipboard sequence; the global memory block is
    // handed to the system by SetClipboardData.
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return;
        }
        EmptyClipboard();
        let mem = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2);
        if !mem.is_null() {
            let p = GlobalLock(mem) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                GlobalUnlock(mem);
                SetClipboardData(13 /* CF_UNICODETEXT */, mem);
            }
        }
        CloseClipboard();
    }
}

/// Saves the window's client area as a PNG (the hidden --screenshot flag).
fn save_screenshot(hwnd: HWND, path: &std::path::Path) -> Result<()> {
    // SAFETY: GDI calls on our own window's DC; every object is released.
    let (w, h, bgra) = unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut r);
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let dc: HDC = GetDC(hwnd);
        let mem = CreateCompatibleDC(dc);
        let bmp = CreateCompatibleBitmap(dc, w, h);
        let old = SelectObject(mem, bmp);
        BitBlt(mem, 0, 0, w, h, dc, 0, 0, SRCCOPY);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            ..std::mem::zeroed()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        GetDIBits(mem, bmp, 0, h as u32, px.as_mut_ptr().cast(), &mut info, DIB_RGB_COLORS);
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(hwnd, dc);
        (w as u32, h as u32, px)
    };
    let rgba: Vec<u8> = bgra.chunks(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
    let mut enc = png::Encoder::new(std::fs::File::create(path)?, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgba)?;
    eprintln!("screenshot saved to {}", path.display());
    Ok(())
}

/// Draws an anti-aliased bar filling `at`: a capsule with rounded ends where
/// asked (GDI's own shapes are jagged), plus 2 px black seams at `seams`
/// (x offsets), blended over the background.
unsafe fn bar(hdc: HDC, at: RECT, color: u32, round: (bool, bool), seams: &[i32]) {
    let (w, h) = (at.right - at.left, at.bottom - at.top);
    if w <= 0 || h <= 0 {
        return;
    }
    let r = h as f32 / 2.0;
    let channels = |c: u32| [(c & 0xff) as f32, ((c >> 8) & 0xff) as f32, ((c >> 16) & 0xff) as f32];
    let (fg, bg) = (channels(color), channels(BG));
    let mut px = vec![0u32; (w * h) as usize];
    for py in 0..h {
        for qx in 0..w {
            let (fx, fy) = (qx as f32 + 0.5, py as f32 + 0.5);
            // Distance outside the shape: rounded ends are circles of radius r.
            let cx = if round.0 && fx < r {
                r
            } else if round.1 && fx > w as f32 - r {
                w as f32 - r
            } else {
                fx
            };
            let d = ((fx - cx).powi(2) + (fy - r).powi(2)).sqrt() - r;
            let cover = (0.5 - d).clamp(0.0, 1.0);
            let ink = if seams.iter().any(|&sx| qx == sx || qx == sx - 1) { [0.0; 3] } else { fg };
            let mix = |i: usize| (bg[i] + (ink[i] - bg[i]) * cover) as u32;
            // DIB pixels are 0x00RRGGBB.
            px[(py * w + qx) as usize] = mix(0) << 16 | mix(1) << 8 | mix(2);
        }
    }
    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h, // top-down
        biPlanes: 1,
        biBitCount: 32,
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: `px` holds w*h 32-bit pixels as described by `info`.
    unsafe { SetDIBitsToDevice(hdc, at.left, at.top, w as u32, h as u32, 0, 0, 0, h as u32, px.as_ptr().cast(), &info, DIB_RGB_COLORS) };
}

/// A button drawn as the app's pill: colour, black capitals, seams near the
/// ends (white while pressed).
unsafe fn draw_pill(d: &DRAWITEMSTRUCT) {
    let (label, color) = match d.CtlID as i32 {
        BTN_NEW => ("NEW PASSWORD", VIOLET),
        BTN_COPY => ("COPY", LAVENDER),
        BTN_UPDATE => ("INSTALL UPDATE", GREEN),
        BTN_FULL => ("FULL WINDOW", BLUE),
        BTN_QUIT => ("QUIT", RED),
        BTN_MIN => ("", BLUE),
        BTN_CLOSE => ("", PEACH),
        _ => ("", DIM),
    };
    let (hdc, mut r) = (d.hDC, d.rcItem);
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let text = wide(label);
    // SAFETY: drawing into the DC Windows handed us for this button.
    unsafe {
        let old_font = SelectObject(hdc, BUTTON_FONT.load(Ordering::Relaxed) as HFONT);
        let mut size = std::mem::zeroed();
        GetTextExtentPoint32W(hdc, text.as_ptr(), label.len() as i32, &mut size);
        // Seams like the main app's pills, where the label clears them.
        let cap = h * 55 / 100;
        let seams: &[i32] = if w - 2 * cap > size.cx + 12 { &[cap, w - cap] } else { &[] };
        let fill = if d.itemState & ODS_SELECTED != 0 { WHITE } else { color };
        bar(hdc, r, fill, (true, true), seams);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, 0);
        if [BTN_MIN,BTN_CLOSE].contains(&(d.CtlID as i32)) {draw_window_symbol(hdc,r,d.CtlID as i32,fill);} else {
            DrawTextW(hdc, text.as_ptr(), -1, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }
        SelectObject(hdc, old_font);
    }
}

/// Rasterize the shared delta mark or close cross on a compact title button.
unsafe fn draw_window_symbol(hdc:HDC,button:RECT,kind:i32,fill:u32) {
    const N:usize=20;
    let mut pixels=vec![0u32;N*N];
    let channels=|color:u32|[(color&255) as f32,((color>>8)&255) as f32,((color>>16)&255) as f32];
    let (fg,bg)=(channels(BG),channels(fill));
    for y in 0..N {for x in 0..N {
        let mut coverage=0.0;
        for sy in 0..4 {for sx in 0..4 {
            let u=(x as f32+(sx as f32+0.5)/4.0)/N as f32;
            let v=(y as f32+(sy as f32+0.5)/4.0)/N as f32;
            let ink=if kind==BTN_MIN {theme::logo_shade(1.0-u,1.0-v).is_some()} else {
                (0.15..0.85).contains(&u)&&(0.15..0.85).contains(&v)&&((u-v).abs()<0.07||(u+v-1.0).abs()<0.07)
            };
            if ink {coverage+=1.0/16.0;}
        }}
        let c=|i:usize|(bg[i]+(fg[i]-bg[i])*coverage) as u32;
        pixels[y*N+x]=c(0)<<16|c(1)<<8|c(2);
    }}
    unsafe {
        let mut info:BITMAPINFO=std::mem::zeroed();
        info.bmiHeader=BITMAPINFOHEADER{biSize:size_of::<BITMAPINFOHEADER>() as u32,biWidth:N as i32,biHeight:-(N as i32),biPlanes:1,biBitCount:32,..std::mem::zeroed()};
        let x=(button.left+button.right-N as i32)/2;let y=(button.top+button.bottom-N as i32)/2;
        SetDIBitsToDevice(hdc,x,y,N as u32,N as u32,0,0,0,N as u32,pixels.as_ptr().cast(),&info,DIB_RGB_COLORS);
    }
}

/// The console strips along the top and bottom: (left, right, colour).
const TOP: [(i32, i32, u32); 4] = [(12, 96, ORANGE), (102, 300, PEACH), (306, 360, VIOLET), (366, 488, TAN)];
const BOTTOM: [(i32, i32, u32); 3] = [(12, 96, LAVENDER), (102, 340, BLUE), (346, 488, GOLD)];

unsafe fn paint_frame(hwnd: HWND) {
    super::window_shape::update();
    // SAFETY: a standard BeginPaint/EndPaint pair on our window.
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        bar(hdc,RECT{left:10,top:6,right:344,bottom:36},LAVENDER,(true,true),&[]);
        let font=SelectObject(hdc,BUTTON_FONT.load(Ordering::Relaxed) as HFONT);
        SetTextColor(hdc,BG);SetBkMode(hdc,TRANSPARENT as i32);
        let text=wide("SLIPSTREAM REMOTE");let mut title=RECT{left:24,top:6,right:336,bottom:36};
        DrawTextW(hdc,text.as_ptr(),-1,&mut title,DT_LEFT|DT_VCENTER|DT_SINGLELINE);SelectObject(hdc,font);
        for (strip, y) in [(&TOP[..], 48), (&BOTTOM[..], HEIGHT - 22)] {
            let n = strip.len();
            for (i, &(l, r, color)) in strip.iter().enumerate() {
                // Rounded at the strip's two outer ends, square between segments.
                bar(hdc, RECT { left: l, top: y, right: r, bottom: y + 12 }, color, (i == 0, i == n - 1), &[]);
            }
        }
        EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg==WM_NCHITTEST {
        let mut point=POINT{x:(lparam as i16) as i32,y:((lparam>>16) as i16) as i32};
        unsafe{ScreenToClient(hwnd,&mut point);}
        if point.y>=0&&point.y<40&&point.x<350{return HTCAPTION as LRESULT;}
    }
    if msg==WM_APP+3 {unsafe{ShowWindow(hwnd,SW_RESTORE);SetForegroundWindow(hwnd);};return 0;}
    if msg == WM_DRAWITEM {
        // SAFETY: WM_DRAWITEM's lparam is a DRAWITEMSTRUCT.
        let d = unsafe { &*(lparam as *const DRAWITEMSTRUCT) };
        if d.CtlType == ODT_BUTTON {
            unsafe { draw_pill(d) };
            return 1;
        }
    }
    // Ending the message loop can't wait for `UI`: the window is usually
    // destroyed from inside one of our own handlers (Quit, a relaunch, the
    // screenshot timer), which still holds it, so the nested WM_DESTROY
    // would get the default handling and the app would never exit.
    if msg == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return 0;
    }
    if msg == WM_PAINT {
        unsafe { paint_frame(hwnd) };
        return 0;
    }
    if msg == WM_CTLCOLORSTATIC {
        // SAFETY: wparam is the control's DC, lparam its window.
        let color = match unsafe { GetDlgCtrlID(lparam as HWND) } {
            ID_TITLE => ORANGE,
            ID_ID_LABEL | ID_PW_LABEL => LAVENDER,
            ID_ID => TAN,
            ID_PW => GOLD,
            ID_STATUS if ONLINE.load(Ordering::Relaxed) => GREEN,
            ID_STATUS => RED,
            _ => DIM,
        };
        unsafe {
            SetTextColor(wparam as HDC, color);
            SetBkColor(wparam as HDC, BG);
        }
        return BG_BRUSH.load(Ordering::Relaxed) as LRESULT;
    }
    // Our own calls (creating controls, setting text) make Windows call in
    // again; those nested messages get the default handling.
    let handled = UI.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        let ui = guard.as_mut()?;
        match msg {
            WM_CREATE => {
                // SAFETY: WM_CREATE's lparam is a CREATESTRUCTW; we only need the window.
                let _ = unsafe { &*(lparam as *const CREATESTRUCTW) };
                ui.controls = unsafe { build(hwnd) };
                Some(0)
            }
            WM_REFRESH => {
                refresh(ui, hwnd);
                // An installed update: close, and `run` relaunches it.
                if ui.view.lock().unwrap().relaunch.is_some() {
                    unsafe { DestroyWindow(hwnd) };
                }
                Some(0)
            }
            WM_REQUEST => {
                while let Ok(req) = ui.requests.try_recv() {
                    unsafe {
                        ShowWindow(hwnd, SW_RESTORE);
                        SetForegroundWindow(hwnd);
                    }
                    if let single::Request::Link(url) = req {
                        link(ui, &url);
                    }
                }
                Some(0)
            }
            WM_COMMAND => {
                match (wparam & 0xffff) as i32 {
                    BTN_MIN=>unsafe{ShowWindow(hwnd,SW_MINIMIZE);},
                    BTN_CLOSE=>unsafe{DestroyWindow(hwnd);},
                    BTN_NEW => {
                        if let Some(sender) = &ui.service_sender {
                            sender.regenerate_password();
                        } else {
                            let pw = ui.client.regenerate_password();
                            ui.view.lock().unwrap().password = pw;
                        }
                        refresh(ui, hwnd);
                    }
                    BTN_COPY => {
                        let v = ui.view.lock().unwrap();
                        copy(hwnd, &format!("ID: {}\r\nPassword: {}", v.id, v.password));
                    }
                    BTN_UPDATE => install_update(ui),
                    BTN_FULL => retry_full_window(ui),
                    BTN_QUIT => unsafe {
                        DestroyWindow(hwnd);
                    },
                    _ => return None,
                }
                Some(0)
            }
            WM_TIMER if wparam == TIMER_SERVICE => { refresh(ui, hwnd); Some(0) }
            WM_TIMER if wparam == TIMER_SCREENSHOT => {
                if let Some(path) = ui.screenshot.take()
                    && let Err(e) = save_screenshot(hwnd, &path)
                {
                    eprintln!("screenshot failed: {e:#}");
                }
                unsafe { DestroyWindow(hwnd) };
                Some(0)
            }
            _ => None,
        }
    });
    match handled {
        Some(r) => r,
        // SAFETY: default handling for everything else.
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Exchanges a station link's setup code and switches to that station.
fn link(ui: &Ui, url: &str) {
    let Some(link) = StationLink::parse(url) else { return };
    let (client, view) = (ui.client.clone(), ui.view.clone());
    ui.rt.spawn(async move {
        let result = async { DirectoryClient::new(&link.station, None)?.link(&link.code).await }.await;
        let note = match result {
            Ok(cfg) => {
                client.update_settings(|s| {
                    s.server.provisioned = true;
                    slipstream_core::provision::apply(s, &cfg);
                });
                format!("Connected to {}.", cfg.org.unwrap_or(cfg.server))
            }
            Err(e) => format!("Couldn't connect to the station: {e:#}"),
        };
        view.lock().unwrap().note = note;
        wake(WM_REFRESH);
    });
}

/// Clears the software renderer's failed start and relaunches, so the app
/// tries the full window again.
fn retry_full_window(ui: &Ui) {
    super::softgl::retry();
    if let Ok(exe) = std::env::current_exe() {
        ui.view.lock().unwrap().relaunch = Some(exe);
    }
    wake(WM_REFRESH);
}

/// Downloads and installs the offered update, then relaunches.
fn install_update(ui: &Ui) {
    let Some(info) = ui.view.lock().unwrap().update.clone() else { return };
    let (client, view) = (ui.client.clone(), ui.view.clone());
    view.lock().unwrap().note = format!("Installing {}…", info.label());
    wake(WM_REFRESH);
    ui.rt.spawn(async move {
        let result = client.install_update(&info, |_, _| {}).await;
        {
            let mut v = view.lock().unwrap();
            match result {
                Ok(exe) => v.relaunch = Some(exe),
                Err(e) => v.note = format!("The update failed: {e:#}"),
            }
        }
        wake(WM_REFRESH);
    });
}
