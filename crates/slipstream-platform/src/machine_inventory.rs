//! Installed/connected devices and remote filesystem mappings. Cached briefly
//! because System information refreshes CPU/memory every three seconds.

use slipstream_core::proto::InfoSection;
use std::{
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

type CachedInventory = Option<(Instant, Vec<InfoSection>)>;

pub(crate) fn collect() -> Vec<InfoSection> {
    static CACHE: OnceLock<Mutex<CachedInventory>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(|| Mutex::new(None)).lock().expect("inventory cache");
    if let Some((at, sections)) = &*cache
        && at.elapsed() < Duration::from_secs(30)
    {
        return sections.clone();
    }
    let sections = platform();
    *cache = Some((Instant::now(), sections.clone()));
    sections
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn section(title: &str, mut rows: Vec<(String, String)>, empty: &str) -> InfoSection {
    rows.sort();
    rows.dedup();
    if rows.is_empty() {
        rows.push(("Status".into(), empty.into()));
    }
    InfoSection { title: title.into(), rows }
}

#[cfg(windows)]
fn platform() -> Vec<InfoSection> {
    use windows_sys::Win32::{Devices::DeviceAndDriverInstallation::*, System::Registry::*};
    let mut devices = Vec::new();
    let mut printers = Vec::new();
    let mut drives = Vec::new();
    let mut shares = Vec::new();
    // SetupAPI reports present devices, with the manufacturer's actual model
    // description. No PowerShell, external utility, or network RPC is needed.
    unsafe {
        let set = SetupDiGetClassDevsW(std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), DIGCF_PRESENT | DIGCF_ALLCLASSES);
        if set != -1 {
            for index in 0..4096 {
                let mut info = SP_DEVINFO_DATA {
                    cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                    ..Default::default()
                };
                if SetupDiEnumDeviceInfo(set, index, &mut info) == 0 {
                    break;
                }
                let prop = |property| {
                    let mut bytes = vec![0u8; 4096];
                    let mut size = 0;
                    if SetupDiGetDeviceRegistryPropertyW(set, &info, property, std::ptr::null_mut(), bytes.as_mut_ptr(), bytes.len() as u32, &mut size) == 0 {
                        return None;
                    }
                    bytes.truncate((size as usize).min(bytes.len()));
                    utf16(&bytes)
                };
                let class = prop(SPDRP_CLASS).unwrap_or_default();
                let classes = [
                    "USB",
                    "HIDClass",
                    "Mouse",
                    "Keyboard",
                    "Image",
                    "Camera",
                    "MEDIA",
                    "Bluetooth",
                    "Ports",
                    "Monitor",
                    "Printer",
                    "PrintQueue",
                    "WPD",
                    "Biometric",
                    "SmartCardReader",
                ];
                if !classes.iter().any(|c| class.eq_ignore_ascii_case(c)) {
                    continue;
                }
                let Some(name) = prop(SPDRP_FRIENDLYNAME).or_else(|| prop(SPDRP_DEVICEDESC)) else {
                    continue;
                };
                if class == "USB" && !prop(SPDRP_HARDWAREID).unwrap_or_default().starts_with("USB\\VID_") {
                    continue;
                }
                let maker = prop(SPDRP_MFG).filter(|s| !s.starts_with('('));
                let detail = match maker {
                    Some(m) if !name.to_lowercase().contains(&m.to_lowercase()) => format!("{class} · {m} · present"),
                    _ => format!("{class} · present"),
                };
                if ["Image", "Printer", "PrintQueue"].contains(&class.as_str()) {
                    printers.push((name, detail));
                } else {
                    devices.push((name, detail));
                }
            }
            SetupDiDestroyDeviceInfoList(set);
        }
    }
    let printer_root = r"SYSTEM\CurrentControlSet\Control\Print\Printers";
    for name in reg_children(HKEY_LOCAL_MACHINE, printer_root, false) {
        let key = format!(r"{printer_root}\{name}");
        let mut detail = Vec::new();
        for (value, label) in [
            ("Printer Driver", "Driver / model"),
            ("Port", "Port"),
            ("Location", "Location"),
            ("Share Name", "Shared as"),
        ] {
            if let Some(text) = reg_text(HKEY_LOCAL_MACHINE, &key, value) {
                detail.push(format!("{label}: {text}"));
            }
        }
        detail.push("installed queue; live availability not checked".into());
        printers.push((name, detail.join(" · ")));
    }
    for sid in reg_children(HKEY_USERS, "", false) {
        if !(sid.starts_with("S-1-5-21-") || sid.starts_with("S-1-12-1-")) || sid.ends_with("_Classes") {
            continue;
        }
        let profile = reg_text(
            HKEY_LOCAL_MACHINE,
            &format!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\{sid}"),
            "ProfileImagePath",
        )
        .and_then(|p| p.rsplit(['\\', '/']).next().map(str::to_string))
        .unwrap_or_else(|| sid.clone());
        let network = format!(r"{sid}\Network");
        for letter in reg_children(HKEY_USERS, &network, false) {
            if let Some(path) = reg_text(HKEY_USERS, &format!(r"{network}\{letter}"), "RemotePath") {
                drives.push((format!("{profile} · {letter}:"), format!("{path} · saved user mapping")));
            }
        }
        let connections = format!(r"{sid}\Printers\Connections");
        for queue in reg_children(HKEY_USERS, &connections, false) {
            let name = queue.trim_start_matches(',').replace(',', "\\");
            printers.push((
                format!(r"{profile} · \\{name}"),
                "User's network printer connection; model not reported in this profile".into(),
            ));
        }
    }
    let share_root = r"SYSTEM\CurrentControlSet\Services\LanmanServer\Shares";
    for name in reg_children(HKEY_LOCAL_MACHINE, share_root, true) {
        if let Some(raw) = crate::machine::reg_raw(HKEY_LOCAL_MACHINE, share_root, &name, RRF_RT_REG_MULTI_SZ) {
            let units = raw.as_chunks::<2>().0.iter().map(|p| u16::from_le_bytes([p[0], p[1]])).collect::<Vec<_>>();
            let values = String::from_utf16_lossy(&units);
            if let Some(path) = values.split('\0').find_map(|v| v.strip_prefix("Path=")) {
                shares.push((name, path.into()));
            }
        }
    }
    if !drives.is_empty() {
        drives.push((
            "Scope".into(),
            "Saved mappings in loaded user profiles, including signed-in users when the service runs as SYSTEM. Reachability is not checked.".into(),
        ));
    }
    vec![
        section("Peripherals", devices, "No present peripheral devices reported"),
        section("Printers and scanners", printers, "No installed printers or present scanners reported"),
        section("Network drives", drives, "No saved network drive mappings found in visible user profiles"),
        section("Shared folders", shares, "No configured shared folders reported"),
    ]
}

#[cfg(windows)]
fn utf16(bytes: &[u8]) -> Option<String> {
    let units = bytes
        .as_chunks::<2>().0.iter()
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .take_while(|&u| u != 0)
        .collect::<Vec<_>>();
    let text = String::from_utf16_lossy(&units).trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(windows)]
fn reg_text(root: windows_sys::Win32::System::Registry::HKEY, key: &str, value: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ};
    utf16(&crate::machine::reg_raw(root, key, value, RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ)?)
}

#[cfg(windows)]
fn reg_children(root: windows_sys::Win32::System::Registry::HKEY, key: &str, values: bool) -> Vec<String> {
    use windows_sys::Win32::System::Registry::*;
    let key = key.encode_utf16().chain([0]).collect::<Vec<_>>();
    let mut handle = std::ptr::null_mut();
    let mut names = Vec::new();
    // SAFETY: read-only registry handle and fixed, correctly sized name buffer;
    // it is closed on every path after a successful open.
    unsafe {
        if RegOpenKeyExW(root, key.as_ptr(), 0, KEY_READ, &mut handle) != 0 {
            return names;
        }
        for index in 0..4096 {
            let mut text = [0u16; 512];
            let mut len = text.len() as u32;
            let status = if values {
                RegEnumValueW(
                    handle,
                    index,
                    text.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            } else {
                RegEnumKeyExW(
                    handle,
                    index,
                    text.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if status == 259 {
                break;
            } // ERROR_NO_MORE_ITEMS
            if status == 0 {
                names.push(String::from_utf16_lossy(&text[..len as usize]));
            }
        }
        RegCloseKey(handle);
    }
    names
}

#[cfg(target_os = "linux")]
fn platform() -> Vec<InfoSection> {
    let mut devices = Vec::new();
    let mut printers = Vec::new();
    for entry in std::fs::read_dir("/sys/bus/usb/devices").into_iter().flatten().flatten().take(1024) {
        let read = |name| {
            std::fs::read_to_string(entry.path().join(name))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        if read("idVendor").as_deref() == Some("1d6b") {
            continue;
        }
        let Some(product) = read("product") else {
            continue;
        };
        let maker = read("manufacturer").unwrap_or_default();
        let model = if product.to_lowercase().contains(&maker.to_lowercase()) {
            product
        } else {
            format!("{maker} {product}").trim().to_string()
        };
        let detail = format!("USB · {}:{}", read("idVendor").unwrap_or_default(), read("idProduct").unwrap_or_default());
        // The USB interface class identifies printer and imaging functions.
        let path = entry.path();
        let imaging = std::fs::read_dir(&path)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|p| std::fs::read_to_string(p.path().join("bInterfaceClass")).ok())
            .any(|v| matches!(v.trim(), "06" | "07"));
        if imaging {
            printers.push((model, detail));
        } else {
            devices.push((model, detail));
        }
    }
    // CUPS's queue/PPD files contain the configured printer's model. No
    // discovery broadcasts, SANE probing or potentially blocking device opens.
    if let Ok(config) = std::fs::read_to_string("/etc/cups/printers.conf") {
        let mut queue = None;
        for line in config.lines().map(str::trim) {
            if let Some(name) = line
                .strip_prefix("<Printer ")
                .or_else(|| line.strip_prefix("<DefaultPrinter "))
                .and_then(|s| s.strip_suffix('>'))
            {
                queue = Some(name.to_string());
            }
            if let Some(name) = &queue
                && let Some(uri) = line.strip_prefix("DeviceURI ")
            {
                let ppd = std::path::Path::new("/etc/cups/ppd").join(format!("{name}.ppd"));
                let model = std::fs::read_to_string(ppd)
                    .unwrap_or_default()
                    .lines()
                    .find_map(|l| l.strip_prefix("*NickName: "))
                    .map(|v| v.trim_matches('"').to_string());
                printers.push((
                    name.clone(),
                    format!("{} · {}", model.unwrap_or_else(|| "Model not reported".into()), redact_uri(uri)),
                ));
            }
        }
    }
    let mounts = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let drives = parse_mounts(&mounts);
    vec![
        section("Peripherals", devices, "No USB peripherals reported"),
        section(
            "Printers and scanners",
            printers,
            "No USB printer/scanner or readable CUPS queue configuration reported",
        ),
        section("Network drives", drives, "No network filesystem mounts visible to this process"),
    ]
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn redact_uri(uri: &str) -> String {
    // Device URIs can embed SMB/IPP credentials; those never belong in a ticket.
    if let Some((scheme, rest)) = uri.split_once("://")
        && let Some((_, host)) = rest.rsplit_once('@')
    {
        return format!("{scheme}://{host}");
    }
    uri.to_string()
}

#[cfg(any(target_os = "linux", test))]
fn parse_mounts(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (before, after) = line.split_once(" - ")?;
            let fields = before.split_whitespace().collect::<Vec<_>>();
            let fs = after.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 5 || fs.len() < 2 || !["cifs", "smb3", "nfs", "nfs4", "fuse.sshfs", "fuse.rclone", "davfs", "fuse.davfs"].contains(&fs[0]) {
                return None;
            }
            Some((mount_unescape(fields[4]), format!("{} ({})", redact_uri(&mount_unescape(fs[1])), fs[0])))
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
fn mount_unescape(s: &str) -> String {
    s.replace("\\040", " ").replace("\\011", "\t").replace("\\134", "\\")
}

#[cfg(target_os = "macos")]
fn platform() -> Vec<InfoSection> {
    // system_profiler is already used for hardware on macOS. Its inventory is
    // textual here so model/manufacturer details survive OS version changes.
    use std::process::Command;
    let mut devices = Vec::new();
    let mut printers = Vec::new();
    if let Ok(out) = Command::new("system_profiler")
        .args(["SPUSBDataType", "SPPrintersDataType", "-detailLevel", "mini", "-timeout", "3"])
        .output()
    {
        let mut printing = false;
        for line in String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|l| !l.is_empty()) {
            if line == "Printers:" {
                printing = true;
                continue;
            }
            let target = if printing { &mut printers } else { &mut devices };
            if let Some((label, value)) = line.split_once(':') {
                target.push((label.to_string(), value.trim().to_string()));
            }
        }
    }
    let mut drives = Vec::new();
    if let Ok(out) = Command::new("mount").output() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if (line.contains("smbfs") || line.contains("nfs") || line.contains("webdav") || line.contains("osxfuse"))
                && let Some((source, mount)) = line.split_once(" on ")
            {
                drives.push((mount.into(), redact_uri(source)));
            }
        }
    }
    vec![
        section("Peripherals", devices, "No USB inventory reported"),
        section("Printers and scanners", printers, "No configured printer inventory reported"),
        section("Network drives", drives, "No network filesystem mounts reported"),
    ]
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn platform() -> Vec<InfoSection> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn network_mounts_preserve_paths_and_hide_uri_credentials() {
        let input = "45 1 0:33 / /mnt/Shared\\040Files rw - cifs //server/share rw\n46 1 0:34 / /home/x/Web rw - fuse.sshfs user@host:/path rw\n47 1 8:1 / / rw - ext4 /dev/sda1 rw";
        let rows = parse_mounts(input);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "/mnt/Shared Files");
        assert_eq!(redact_uri("smb://user:secret@server/printer"), "smb://server/printer");
    }
}
