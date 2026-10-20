//! Locally hashed hardware evidence for automatic device recognition.
//! Firmware is preferred. Linux can fall back to permanent built-in PCI
//! network hardware when firmware files require root; OS/host/model IDs and
//! temporary, randomized or removable adapter addresses are not evidence.
use std::{collections::BTreeMap, sync::OnceLock};

pub(crate) type Evidence = BTreeMap<String, String>;

#[cfg(any(windows, target_os = "linux", target_os = "macos", test))]
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
        let evidence = read();
        if evidence.is_empty() {
            tracing::warn!(
                "automatic device recognition unavailable; retaining the saved identity"
            );
        }
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
    // Keep a comparable fallback if firmware permissions change later.
    read_network(&mut out, std::path::Path::new("/sys/class/net"));
    out
}

#[cfg(target_os = "linux")]
fn read_network(out: &mut Evidence, root: &std::path::Path) {
    if let Ok(interfaces) = std::fs::read_dir(root) {
        for interface in interfaces.flatten() {
            let path = interface.path();
            let bus = path.join("device/subsystem").canonicalize().ok();
            if !bus
                .as_ref()
                .is_some_and(|p| p.file_name().is_some_and(|n| n == "pci"))
            {
                continue;
            }
            // NET_ADDR_PERM=0. Never bind to a Wi-Fi privacy address, a
            // user-assigned address, a bridge/VPN or a removable USB dock.
            if std::fs::read_to_string(path.join("type")).is_ok_and(|v| v.trim() == "1")
                && std::fs::read_to_string(path.join("addr_assign_type"))
                    .is_ok_and(|v| v.trim() == "0")
                && let Ok(address) = std::fs::read_to_string(path.join("address"))
            {
                add_adapter(out, &address);
            }
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn add_adapter(out: &mut Evidence, address: &str) {
    let bytes: Option<Vec<u8>> = address
        .trim()
        .split(':')
        .map(|part| {
            if part.len() == 2 {
                u8::from_str_radix(part, 16).ok()
            } else {
                None
            }
        })
        .collect();
    let Some(bytes) = bytes.filter(|b| b.len() == 6 && b[0] & 1 == 0 && b.iter().any(|n| *n != 0))
    else {
        return;
    };
    let mut hashed = Evidence::new();
    add(
        &mut hashed,
        "network-adapter",
        &crate::directory::hex(&bytes),
    );
    if let Some(hash) = hashed.remove("network-adapter") {
        out.insert(format!("network-adapter:{hash}"), hash);
    }
}

/// Strong evidence takes precedence. Adapter sets tolerate additions/removals
/// as long as one previously recognized built-in adapter is still present.
pub(crate) fn changed(previous: &Evidence, current: &Evidence) -> bool {
    if let Some(different) = ["system-uuid", "system-serial"]
        .into_iter()
        .find_map(|kind| {
            previous
                .get(kind)
                .zip(current.get(kind))
                .map(|(old, new)| old != new)
        })
    {
        return different;
    }
    let adapters = |evidence: &Evidence| {
        evidence
            .keys()
            .filter(|kind| kind.starts_with("network-adapter:"))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
    };
    let (old, new) = (adapters(previous), adapters(current));
    !old.is_empty() && !new.is_empty() && old.is_disjoint(&new)
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
    fn adapter_fallback_is_hashed_and_tolerates_additions_and_partial_reads() {
        let mut first = Evidence::new();
        add_adapter(&mut first, "00:11:22:33:44:55");
        assert_eq!(first.len(), 1);
        assert!(!format!("{first:?}").contains("00:11:22:33:44:55"));
        let mut second = Evidence::new();
        add_adapter(&mut second, "00:11:22:33:44:66");
        assert!(changed(&first, &second));
        let mut both = first.clone();
        both.extend(second.clone());
        assert!(!changed(&first, &both));
        assert!(!changed(&both, &second));
        assert!(!changed(&first, &Evidence::new()));
        assert!(!changed(&Evidence::new(), &first));
        let mut firm_a = first.clone();
        firm_a.insert("system-uuid".into(), "uuid-a".into());
        let mut firm_b = second.clone();
        firm_b.insert("system-uuid".into(), "uuid-a".into());
        assert!(
            !changed(&firm_a, &firm_b),
            "same firmware must override an adapter replacement"
        );
        firm_b.insert("system-uuid".into(), "uuid-b".into());
        firm_b.extend(first.clone());
        assert!(changed(&firm_a, &firm_b));
        for address in [
            "00:00:00:00:00:00",
            "ff:ff:ff:ff:ff:ff",
            "01:11:22:33:44:55",
            "bad",
            "00:11:22:33:44:gg",
        ] {
            let mut invalid = Evidence::new();
            add_adapter(&mut invalid, address);
            assert!(invalid.is_empty());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_fallback_ignores_randomized_virtual_and_removable_adapters() {
        use std::{fs, os::unix::fs::symlink};
        let temp = tempfile::tempdir().unwrap();
        let network = temp.path().join("net");
        fs::create_dir(&network).unwrap();
        for (index, (name, bus, assignment)) in [
            ("built-in", "pci", "0"),
            ("randomized", "pci", "3"),
            ("usb-dock", "usb", "0"),
            ("wifi-privacy", "pci", "1"),
        ]
        .into_iter()
        .enumerate()
        {
            let path = network.join(name);
            fs::create_dir_all(path.join("device")).unwrap();
            let subsystem = temp.path().join(bus);
            fs::create_dir_all(&subsystem).unwrap();
            symlink(subsystem, path.join("device/subsystem")).unwrap();
            fs::write(path.join("type"), "1").unwrap();
            fs::write(path.join("addr_assign_type"), assignment).unwrap();
            fs::write(
                path.join("address"),
                format!("00:11:22:33:44:{:02x}", 0x50 + index),
            )
            .unwrap();
        }
        let virtual_path = network.join("vpn");
        fs::create_dir(&virtual_path).unwrap();
        fs::write(virtual_path.join("address"), "00:11:22:33:44:66").unwrap();
        let mut evidence = Evidence::new();
        read_network(&mut evidence, &network);
        assert_eq!(evidence.len(), 1);
    }

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
