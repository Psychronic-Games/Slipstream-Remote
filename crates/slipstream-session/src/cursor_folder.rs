//! "Which folder is under the mouse pointer?" — so files dragged onto the
//! remote screen land where they were dropped.
//!
//! Windows: if the pointer is over an Explorer window, ask the shell for its
//! folder. Elsewhere (desktop, other apps, other OSes) the caller falls back
//! to the Desktop.

#[cfg(windows)]
pub fn folder_under_cursor() -> Option<std::path::PathBuf> {
    use windows_sys::Win32::{
        Foundation::POINT,
        UI::WindowsAndMessaging::{GA_ROOT, GetAncestor, GetClassNameW, GetCursorPos, WindowFromPoint},
    };
    // SAFETY: plain Win32 queries with valid out-pointers.
    let (root, class) = unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut pt) == 0 {
            return None;
        }
        let hwnd = WindowFromPoint(pt);
        if hwnd.is_null() {
            return None;
        }
        let root = GetAncestor(hwnd, GA_ROOT);
        let mut buf = [0u16; 64];
        let n = GetClassNameW(root, buf.as_mut_ptr(), buf.len() as i32);
        (root as isize, String::from_utf16_lossy(&buf[..n.max(0) as usize]))
    };
    if class != "CabinetWClass" {
        return None; // not an Explorer window (the desktop is the default anyway)
    }
    // Ask the shell which folder that Explorer window shows.
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let script = format!(
        "(New-Object -ComObject Shell.Application).Windows() | Where-Object {{ $_.HWND -eq {root} }} | \
         ForEach-Object {{ $_.Document.Folder.Self.Path }} | Select-Object -First 1"
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let p = std::path::PathBuf::from(path);
    p.is_dir().then_some(p)
}

#[cfg(not(windows))]
pub fn folder_under_cursor() -> Option<std::path::PathBuf> {
    None
}
