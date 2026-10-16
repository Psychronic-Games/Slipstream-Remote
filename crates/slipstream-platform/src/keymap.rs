//! Physical key translation (W3C `KeyboardEvent.code` ⇄ native codes),
//! using Chromium's key table via the `keycode` crate.

use std::str::FromStr;

use keycode::{KeyMap, KeyMapping, KeyMappingCode};

/// Native codes for one physical key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeKey {
    /// Linux evdev code (input-event-codes.h).
    pub evdev: u16,
    /// X11 keycode (evdev + 8).
    pub xkb: u16,
    /// Windows scancode (0xE0xx for extended keys).
    pub win: u16,
    /// macOS virtual keycode.
    pub mac: u16,
}

pub fn lookup(code: &str) -> Option<NativeKey> {
    let code = KeyMappingCode::from_str(code).ok()?;
    let map = KeyMap::from_key_mapping(KeyMapping::Code(Some(code))).ok()?;
    Some(NativeKey { evdev: map.evdev, xkb: map.xkb, win: map.win, mac: map.mac })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_keys_map() {
        let a = lookup("KeyA").unwrap();
        assert_eq!(a.evdev, 30);
        assert_eq!(a.xkb, 38);
        assert_eq!(a.win, 0x1e);
        assert_eq!(a.mac, 0x00);
        let meta = lookup("MetaLeft").unwrap();
        assert_eq!(meta.evdev, 125);
        assert_eq!(meta.win, 0xe05b);
        assert!(lookup("NotAKey").is_none());
    }
}
