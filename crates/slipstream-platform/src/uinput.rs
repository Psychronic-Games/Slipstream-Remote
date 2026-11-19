//! Keyboard and mouse for the Linux service, as kernel input devices
//! (`/dev/uinput`): whatever has the screen takes them like real hardware,
//! be it the login screen, a locked session, any desktop or a text console.
//!
//! Two devices: a keyboard, and a pointer with absolute position (as
//! virtual machines' mice have) plus buttons and a wheel.

use std::{
    fs::File,
    io::{self, Write},
    os::fd::AsRawFd,
};

use anyhow::{Context, Result};
use slipstream_core::proto::{InputEvent, MouseButton};

use crate::keymap;

const fn ioc(dir: u32, nr: u8, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | ((b'U' as u32) << 8) | nr as u32
}

#[repr(C)]
struct Setup {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
    name: [u8; 80],
    ff_effects_max: u32,
}

#[repr(C)]
#[derive(Default)]
struct AbsSetup {
    code: u16,
    pad: u16,
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

const DEV_CREATE: u32 = ioc(0, 1, 0);
const DEV_DESTROY: u32 = ioc(0, 2, 0);
const DEV_SETUP: u32 = ioc(1, 3, size_of::<Setup>());
const ABS_SETUP: u32 = ioc(1, 4, size_of::<AbsSetup>());
const SET_EVBIT: u32 = ioc(1, 100, size_of::<libc::c_int>());
const SET_KEYBIT: u32 = ioc(1, 101, size_of::<libc::c_int>());
const SET_RELBIT: u32 = ioc(1, 102, size_of::<libc::c_int>());
const SET_ABSBIT: u32 = ioc(1, 103, size_of::<libc::c_int>());

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
const SYN_REPORT: u16 = 0;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const REL_WHEEL_HI_RES: u16 = 11;
const REL_HWHEEL_HI_RES: u16 = 12;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const BTN_SIDE: u16 = 0x113;
const BTN_EXTRA: u16 = 0x114;
const BUS_VIRTUAL: u16 = 6;
/// The pointer's range on both axes.
const ABS_MAX: i32 = 32767;
/// One notch of a wheel in high-resolution units.
const NOTCH: f32 = 120.0;

fn check(result: libc::c_int, what: &str) -> Result<()> {
    if result < 0 {
        return Err(io::Error::last_os_error()).with_context(|| format!("setting up the remote {what}"));
    }
    Ok(())
}

struct Device(File);

impl Device {
    fn create(name: &str, product: u16, bits: &[(u32, u16)], absolute: &[u16]) -> Result<Self> {
        let file = File::options().write(true).open("/dev/uinput").context("opening /dev/uinput (the kernel's input-device maker; needs root, and the uinput module)")?;
        let fd = file.as_raw_fd();
        for (request, code) in bits {
            // SAFETY: these requests take an integer.
            check(unsafe { libc::ioctl(fd, *request as _, *code as libc::c_int) }, name)?;
        }
        for code in absolute {
            let mut axis = AbsSetup { code: *code, maximum: ABS_MAX, ..Default::default() };
            // SAFETY: the structure this request is defined with.
            check(unsafe { libc::ioctl(fd, ABS_SETUP as _, &mut axis as *mut AbsSetup) }, name)?;
        }
        let mut setup = Setup { bustype: BUS_VIRTUAL, vendor: 0x534c, product, version: 1, name: [0; 80], ff_effects_max: 0 };
        setup.name[..name.len().min(79)].copy_from_slice(&name.as_bytes()[..name.len().min(79)]);
        // SAFETY: as above; then a request without an argument.
        check(unsafe { libc::ioctl(fd, DEV_SETUP as _, &mut setup as *mut Setup) }, name)?;
        check(unsafe { libc::ioctl(fd, DEV_CREATE as _, 0) }, name)?;
        Ok(Self(file))
    }

    /// Sends events as one report.
    fn send(&mut self, events: &[(u16, u16, i32)]) -> Result<()> {
        // struct input_event: a timestamp the kernel fills in, then these.
        let mut bytes = Vec::with_capacity((events.len() + 1) * size_of::<libc::input_event>());
        for (kind, code, value) in events.iter().copied().chain([(EV_SYN, SYN_REPORT, 0)]) {
            bytes.extend_from_slice(&[0u8; size_of::<libc::timeval>()]);
            bytes.extend_from_slice(&kind.to_ne_bytes());
            bytes.extend_from_slice(&code.to_ne_bytes());
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
        self.0.write_all(&bytes).context("sending remote input")
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: a request without an argument on our own device.
        unsafe { libc::ioctl(self.0.as_raw_fd(), DEV_DESTROY as _, 0) };
    }
}

/// The service's keyboard and pointer. Dropping them unplugs both.
pub struct Devices {
    keyboard: Device,
    pointer: Device,
    /// Scrolling not yet sent: high-resolution units, and those not yet
    /// counted into whole notches (vertical, horizontal).
    wheel: [(f32, f32); 2],
}

impl Devices {
    pub fn create() -> Result<Self> {
        let keys: Vec<(u32, u16)> = [(SET_EVBIT, EV_KEY)].into_iter().chain((1..=255).map(|code| (SET_KEYBIT, code))).collect();
        let keyboard = Device::create("Slipstream Remote keyboard", 1, &keys, &[])?;
        let pointer = Device::create(
            "Slipstream Remote pointer",
            2,
            &[
                (SET_EVBIT, EV_KEY),
                (SET_KEYBIT, BTN_LEFT),
                (SET_KEYBIT, BTN_RIGHT),
                (SET_KEYBIT, BTN_MIDDLE),
                (SET_KEYBIT, BTN_SIDE),
                (SET_KEYBIT, BTN_EXTRA),
                (SET_EVBIT, EV_REL),
                (SET_RELBIT, REL_WHEEL),
                (SET_RELBIT, REL_HWHEEL),
                (SET_RELBIT, REL_WHEEL_HI_RES),
                (SET_RELBIT, REL_HWHEEL_HI_RES),
                (SET_EVBIT, EV_ABS),
                (SET_ABSBIT, ABS_X),
                (SET_ABSBIT, ABS_Y),
            ],
            &[ABS_X, ABS_Y],
        )?;
        Ok(Self { keyboard, pointer, wheel: [(0.0, 0.0); 2] })
    }

    /// Presses and releases a key that does nothing by itself, which wakes
    /// a blanked screen as touching a real keyboard would.
    pub fn nudge(&mut self) -> Result<()> {
        const KEY_LEFTSHIFT: u16 = 42;
        self.keyboard.send(&[(EV_KEY, KEY_LEFTSHIFT, 1)])?;
        self.keyboard.send(&[(EV_KEY, KEY_LEFTSHIFT, 0)])
    }

    /// Injects one event. `at` places a pointer position (0..1 on its
    /// display) on the whole desktop (0..1 across all displays), which is
    /// what an absolute pointer addresses.
    pub fn inject(&mut self, event: InputEvent, at: impl Fn(f32, f32) -> (f32, f32)) -> Result<()> {
        match event {
            InputEvent::MouseMove { x, y } => {
                let (x, y) = at(x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
                let scale = |v: f32| (v.clamp(0.0, 1.0) * ABS_MAX as f32).round() as i32;
                self.pointer.send(&[(EV_ABS, ABS_X, scale(x)), (EV_ABS, ABS_Y, scale(y))])
            }
            InputEvent::MouseButton { button, down } => {
                let code = match button {
                    MouseButton::Left => BTN_LEFT,
                    MouseButton::Right => BTN_RIGHT,
                    MouseButton::Middle => BTN_MIDDLE,
                    MouseButton::Back => BTN_SIDE,
                    MouseButton::Forward => BTN_EXTRA,
                };
                self.pointer.send(&[(EV_KEY, code, down as i32)])
            }
            InputEvent::Wheel { dx, dy } => {
                // Protocol: positive = up/left. Kernel: positive = up/right.
                let mut events = Vec::new();
                for (axis, amount, hi_res, notches) in [(0, dy, REL_WHEEL_HI_RES, REL_WHEEL), (1, -dx, REL_HWHEEL_HI_RES, REL_HWHEEL)] {
                    let (fine, coarse) = &mut self.wheel[axis];
                    *fine += amount * NOTCH;
                    let units = fine.trunc();
                    *fine -= units;
                    if units == 0.0 {
                        continue;
                    }
                    events.push((EV_REL, hi_res, units as i32));
                    *coarse += units / NOTCH;
                    let whole = coarse.trunc();
                    *coarse -= whole;
                    if whole != 0.0 {
                        events.push((EV_REL, notches, whole as i32));
                    }
                }
                if events.is_empty() { Ok(()) } else { self.pointer.send(&events) }
            }
            InputEvent::Key { code, down } => match keymap::lookup(&code) {
                Some(key) if (1..=255).contains(&key.evdev) => self.keyboard.send(&[(EV_KEY, key.evdev, down as i32)]),
                _ => Ok(()),
            },
            InputEvent::ReleaseAll => Ok(()),
            // Kernel devices type keys, not characters: those of a US keyboard.
            InputEvent::Text(text) => {
                for key in slipstream_core::proto::us_keys_for_text(&text) {
                    if let InputEvent::Key { code, down } = key
                        && let Some(key) = keymap::lookup(&code).filter(|k| (1..=255).contains(&k.evdev))
                    {
                        self.keyboard.send(&[(EV_KEY, key.evdev, down as i32)])?;
                    }
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_numbers_match_the_kernel() {
        // From linux/uinput.h.
        assert_eq!(DEV_CREATE, 0x5501);
        assert_eq!(DEV_DESTROY, 0x5502);
        assert_eq!(DEV_SETUP, 0x405c_5503);
        assert_eq!(ABS_SETUP, 0x401c_5504);
        assert_eq!(SET_EVBIT, 0x4004_5564);
        assert_eq!(SET_ABSBIT, 0x4004_5567);
        assert_eq!(size_of::<libc::input_event>(), size_of::<libc::timeval>() + 8);
    }

    /// Makes real devices: needs access to /dev/uinput (root, or a desktop
    /// user the system grants it to). `cargo test -- --ignored uinput`.
    #[test]
    #[ignore]
    fn devices_appear_and_accept_events() {
        let mut devices = Devices::create().unwrap();
        let listed = std::fs::read_to_string("/proc/bus/input/devices").unwrap();
        assert!(listed.contains("Slipstream Remote keyboard") && listed.contains("Slipstream Remote pointer"));
        std::thread::sleep(std::time::Duration::from_millis(300));
        devices.nudge().unwrap();
        devices.inject(InputEvent::Wheel { dx: 0.0, dy: 0.004 }, |x, y| (x, y)).unwrap();
        drop(devices);
        let listed = std::fs::read_to_string("/proc/bus/input/devices").unwrap();
        assert!(!listed.contains("Slipstream Remote keyboard"));
    }
}
