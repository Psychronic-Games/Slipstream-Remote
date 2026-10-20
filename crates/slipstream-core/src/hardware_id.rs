//! Local firmware evidence for detecting disk images moved to other hardware.
//! Model names, MAC addresses and OS installation IDs are intentionally unused.
use std::{collections::BTreeMap, sync::OnceLock};

pub(crate) type Evidence = BTreeMap<String, String>;

#[cfg(any(windows, target_os="linux", target_os="macos", test))]
fn add(out: &mut Evidence, kind: &str, value: &str) {
    let mut value = value.trim().trim_matches('\0').to_ascii_uppercase();
    if kind == "system-uuid" {
        value.retain(|c| c != '-');
        if value.len() != 32 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return;
        }
    }
    let placeholder = value.to_ascii_lowercase();
    if value.len() < 4
        || value.len() > 128
        || value.chars().any(char::is_control)
        || [
            "unknown",
            "none",
            "default string",
            "system serial number",
            "to be filled by o.e.m.",
            "to be filled by oem",
            "not specified",
            "not applicable",
            "123456789",
            "0123456789",
        ]
        .contains(&placeholder.as_str())
        || value.chars().all(|c| matches!(c, '0' | 'F' | '-' | ' '))
    {
        return;
    }
    let digest = blake3::derive_key(
        "Slipstream physical device binding v1",
        format!("{kind}\0{value}").as_bytes(),
    );
    out.insert(kind.into(), crate::directory::hex(&digest));
}

pub(crate) fn current() -> &'static Evidence {
    static EVIDENCE: OnceLock<Evidence> = OnceLock::new();
    EVIDENCE.get_or_init(|| {
        let evidence=read();
        if evidence.is_empty() {tracing::warn!("automatic clone detection unavailable: no reliable firmware device identifier could be read; retaining the existing identity");}
        evidence
    })
}

#[cfg(target_os = "linux")]
fn read() -> Evidence {
    let mut out = Evidence::new();
    for (kind, path) in [
        ("system-uuid", "/sys/class/dmi/id/product_uuid"),
        ("system-serial", "/sys/class/dmi/id/product_serial"),
    ] {
        if let Ok(value) = std::fs::read_to_string(path) {
            add(&mut out, kind, &value);
        }
    }
    out
}

#[cfg(windows)]
fn read() -> Evidence {
    use windows_sys::Win32::System::SystemInformation::GetSystemFirmwareTable;
    // 'RSMB' returns RawSMBIOSData: four version bytes, a LE u32 length,
    // then bounded SMBIOS structures. This API also works in the service.
    const RSMB: u32 = u32::from_be_bytes(*b"RSMB");
    unsafe {
        let size = GetSystemFirmwareTable(RSMB, 0, std::ptr::null_mut(), 0);
        if !(8..=1024 * 1024).contains(&size) {
            return Evidence::new();
        }
        let mut data = vec![0; size as usize];
        if GetSystemFirmwareTable(RSMB, 0, data.as_mut_ptr(), size) != size {
            return Evidence::new();
        }
        let length = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
        if length > data.len() - 8 {
            return Evidence::new();
        }
        smbios(
            &data[8..8 + length],
            data[1] > 2 || (data[1] == 2 && data[2] >= 6),
        )
    }
}

#[cfg(any(windows, test))]
fn smbios(table: &[u8], little_uuid: bool) -> Evidence {
    let mut out = Evidence::new();
    let mut at = 0;
    while at + 4 <= table.len() {
        let kind = table[at];
        let len = usize::from(table[at + 1]);
        if len < 4 || len > table.len() - at {
            break;
        }
        let strings = at + len;
        let Some(end) = table[strings..]
            .windows(2)
            .position(|s| s == [0, 0])
            .map(|n| strings + n)
        else {
            break;
        };
        if kind == 1 {
            if len >= 24 {
                let mut uuid: [u8; 16] = table[at + 8..at + 24].try_into().unwrap();
                if little_uuid {
                    uuid[..4].reverse();
                    uuid[4..6].reverse();
                    uuid[6..8].reverse();
                }
                add(&mut out, "system-uuid", &crate::directory::hex(&uuid));
            }
            if len >= 8 {
                let index = usize::from(table[at + 7]);
                if index > 0
                    && let Some(serial) = table[strings..end].split(|b| *b == 0).nth(index - 1)
                    && let Ok(serial) = std::str::from_utf8(serial)
                {
                    add(&mut out, "system-serial", serial);
                }
            }
            break;
        }
        if kind == 127 {
            break;
        }
        at = end + 2;
    }
    out
}

#[cfg(target_os = "macos")]
fn read() -> Evidence {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let mut out = Evidence::new();
    // Fixed OS utility and arguments; stdout goes to a bounded private temp file
    // to avoid a full pipe preventing the child from reaching its deadline.
    let Ok(file) = tempfile::tempfile() else {
        return out;
    };
    let Ok(output) = file.try_clone() else {
        return out;
    };
    let Ok(mut child) = Command::new("/usr/sbin/ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(output)
        .spawn()
    else {
        return out;
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return out,
            _ if started.elapsed() > Duration::from_secs(3) => {
                let _ = child.kill();
                let _ = child.wait();
                return out;
            }
            _ => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    use std::io::{Read, Seek};
    let mut file = file;
    let _ = file.rewind();
    let mut text = String::new();
    if file.take(65536).read_to_string(&mut text).is_err() {
        return out;
    }
    for line in text.lines() {
        for (name, kind) in [
            ("IOPlatformUUID", "system-uuid"),
            ("IOPlatformSerialNumber", "system-serial"),
        ] {
            if line.contains(&format!("\"{name}\""))
                && let Some((_, value)) = line.split_once('=')
            {
                add(&mut out, kind, value.trim().trim_matches('"'));
            }
        }
    }
    out
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn read() -> Evidence {
    Evidence::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn firmware_parser_distinguishes_devices_and_ignores_placeholders() {
        let mut table = vec![0; 25];
        table[0] = 1;
        table[1] = 25;
        table[7] = 1;
        table[8..24].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        table.extend_from_slice(b"SERIAL-ONE\0\0");
        let first = smbios(&table, true);
        assert_eq!(first.len(), 2);
        let mut second = table.clone();
        second[8] = 42;
        second[25..35].copy_from_slice(b"SERIAL-TWO");
        let second = smbios(&second, true);
        assert_ne!(first["system-uuid"], second["system-uuid"]);
        assert_ne!(first["system-serial"], second["system-serial"]);
        for n in 0..table.len() {
            let _ = smbios(&table[..n], true);
        }
        let mut invalid = Evidence::new();
        for s in [
            "00000000",
            "FFFFFFFF-FFFF",
            "Default string",
            "To Be Filled By O.E.M.",
            "Unknown",
        ] {
            add(&mut invalid, "system-serial", s);
        }
        assert!(invalid.is_empty());
    }
}
