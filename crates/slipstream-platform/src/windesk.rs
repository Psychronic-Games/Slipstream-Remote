//! Windows: following the input desktop, for a host started by the
//! Slipstream service.
//!
//! Windows shows the login screen and UAC prompts on separate desktops
//! (Winlogon, the secure desktop). A thread captures and injects input only
//! on the desktop it's attached to, so in service mode the capture and input
//! threads re-attach to whichever desktop has input before their work. Only
//! a SYSTEM process may open the Winlogon desktop, which is why this runs
//! inside the service's helper.
//!
//! Ctrl+Alt+Del can't be typed with SendInput: the service generates it
//! (`SendSAS`) when the helper signals [`SAS_EVENT`].

use std::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, GENERIC_ALL},
    System::{
        StationsAndDesktops::{CloseDesktop, GetUserObjectInformationW, HDESK, OpenInputDesktop, SetThreadDesktop, UOI_NAME},
        Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent},
    },
};

/// Set by the service when Ctrl+Alt+Del should be generated.
pub const SAS_EVENT: &str = "Global\\SlipstreamRemoteSAS";

static FOLLOW: AtomicBool = AtomicBool::new(false);

/// Turns desktop following on (the service's helper does this at start).
pub fn set_follow_input_desktop(on: bool) {
    FOLLOW.store(on, Ordering::Relaxed);
}

pub fn following() -> bool {
    FOLLOW.load(Ordering::Relaxed)
}

thread_local! {
    /// The desktop this thread is attached to, and its name.
    static CURRENT: RefCell<Option<(HDESK, String)>> = const { RefCell::new(None) };
}

fn desktop_name(desk: HDESK) -> String {
    let mut buf = [0u16; 128];
    let mut needed = 0u32;
    // SAFETY: a live desktop handle and a buffer of the size given.
    let ok = unsafe { GetUserObjectInformationW(desk, UOI_NAME, buf.as_mut_ptr().cast(), size_of_val(&buf) as u32, &mut needed) };
    if ok == 0 {
        return String::new();
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Attaches the calling thread to the desktop that currently has input, if
/// that changed. Returns true when it switched (callers then restart
/// whatever they hold for the old desktop). Does nothing unless following
/// is on. The thread must not own windows.
pub fn follow() -> bool {
    if !following() {
        return false;
    }
    // SAFETY: OpenInputDesktop/SetThreadDesktop/CloseDesktop on handles we
    // own; a thread without windows may switch desktops freely.
    unsafe {
        let desk = OpenInputDesktop(0, 0, GENERIC_ALL);
        if desk.is_null() {
            return false;
        }
        let name = desktop_name(desk);
        CURRENT.with(|c| {
            let mut c = c.borrow_mut();
            if c.as_ref().is_some_and(|(_, n)| *n == name) {
                CloseDesktop(desk);
                return false;
            }
            if SetThreadDesktop(desk) == 0 {
                CloseDesktop(desk);
                return false;
            }
            tracing::info!(desktop = %name, "following the input desktop");
            if let Some((old, _)) = c.replace((desk, name)) {
                CloseDesktop(old);
            }
            true
        })
    }
}

/// Asks the service to generate Ctrl+Alt+Del. False if no service listens.
pub fn request_sas() -> bool {
    let name: Vec<u16> = SAS_EVENT.encode_utf16().chain([0]).collect();
    // SAFETY: opening and signalling a named event, then closing it.
    unsafe {
        let ev = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if ev.is_null() {
            return false;
        }
        let ok = SetEvent(ev) != 0;
        CloseHandle(ev);
        ok
    }
}
