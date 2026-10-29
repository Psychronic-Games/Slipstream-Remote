//! System and motherboard manufacturer serials from Windows RawSMBIOSData. Keep this
//! parser independent of Windows so malformed firmware can be tested anywhere.

#[cfg(windows)]
pub(super) fn read(kind: u8) -> Option<String> {
    use windows_sys::Win32::System::SystemInformation::GetSystemFirmwareTable;
    const RSMB: u32 = u32::from_be_bytes(*b"RSMB");
    // SAFETY: a sizing query followed by a read into a buffer of that size.
    // https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-getsystemfirmwaretable
    unsafe {
        let size = GetSystemFirmwareTable(RSMB, 0, std::ptr::null_mut(), 0);
        if !(8..=1024 * 1024).contains(&size) {
            return None;
        }
        let mut data = vec![0; size as usize];
        if GetSystemFirmwareTable(RSMB, 0, data.as_mut_ptr(), size) != size {
            return None;
        }
        parse_serial(&data, kind)
    }
}

fn parse_serial(data: &[u8], wanted: u8) -> Option<String> {
    // The eight-byte RawSMBIOSData header is not itself an SMBIOS structure.
    let length = u32::from_le_bytes(data.get(4..8)?.try_into().ok()?) as usize;
    let mut table = data.get(8..)?.get(..length)?;
    while table.len() >= 4 {
        let kind = table[0];
        let length = usize::from(table[1]);
        if length < 4 || kind == 127 {
            return None;
        }
        let strings = table.get(length..)?;
        let end = strings.windows(2).position(|s| s == [0, 0])?;
        if kind == wanted {
            if length < 8 {
                return None;
            }
            let index = usize::from(table[7]).checked_sub(1)?;
            let serial = strings[..end].split(|b| *b == 0).nth(index)?;
            return std::str::from_utf8(serial).ok().map(str::to_string);
        }
        table = &strings[end + 2..];
    }
    None
}

#[cfg(test)]
mod tests {
    fn parse(data: &[u8]) -> Option<String> { super::parse_serial(data, 1) }

    fn raw(table: &[u8]) -> Vec<u8> {
        let mut data = vec![0, 3, 6, 0];
        data.extend_from_slice(&(table.len() as u32).to_le_bytes());
        data.extend_from_slice(table);
        data
    }

    #[test]
    fn motherboard_serial_is_read_separately_from_the_system_serial() {
        let data = raw(b"\x01\x08\x00\x00\x00\x00\x00\x01Default string\0\0\x02\x08\x00\x00\x00\x00\x00\x01BOARD-123\0\0");
        assert_eq!(parse(&data).as_deref(), Some("Default string"));
        assert_eq!(super::parse_serial(&data, 2).as_deref(), Some("BOARD-123"));
        for end in 0..data.len() {
            assert!(super::parse_serial(&data[..end], 2).is_none());
        }
    }

    #[test]
    fn reads_system_serial_after_header_and_other_records() {
        let table =
            b"\x00\x04\x00\x00BIOS\0\0\x01\x08\x01\x00\x01\x02\x00\x03Maker\0Model\0aBc1234\0\0";
        assert_eq!(parse(&raw(table)).as_deref(), Some("aBc1234"));
        // Accept older Type 1 structures; a UUID/SKU/family is not required.
        assert_eq!(
            parse(&raw(b"\x01\x08\x00\x00\x00\x00\x00\x01OLD123\0\0")).as_deref(),
            Some("OLD123")
        );
    }

    #[test]
    fn rejects_truncated_headers_records_and_string_sets() {
        let data = raw(b"\x01\x08\x00\x00\x00\x00\x00\x01ABC123\0\0");
        for end in 0..data.len() {
            assert!(
                parse(&data[..end]).is_none(),
                "accepted prefix of length {end}"
            );
        }
        for table in [
            &b"\x01\x00\x00\x00\0\0"[..],
            &b"\x01\xff\x00\x00\0\0"[..],
            &b"\x01\x04\x00\x00\0\0"[..],
            &b"\x01\x08\x00\x00\x00\x00\x00\x01ABC123\0"[..],
            &b"\x00\x04\x00\x00unterminated"[..],
        ] {
            assert!(parse(&raw(table)).is_none());
        }
    }

    #[test]
    fn never_substitutes_another_records_serial_or_string() {
        for index in [0, 2, 255] {
            let mut table = vec![1, 8, 0, 0, 0, 0, 0, index];
            table.extend_from_slice(b"Maker\0\0\x02\x08\x00\x00\x00\x00\x00\x01BOARD123\0\0");
            assert!(parse(&raw(&table)).is_none());
        }
        assert!(
            parse(&raw(
                b"\x7f\x04\x00\x00\0\0\x01\x08\x00\x00\x00\x00\x00\x01ABC123\0\0"
            ))
            .is_none()
        );
        assert!(parse(&raw(b"\x01\x08\x00\x00\x00\x00\x00\x01\xff\0\0")).is_none());
    }
}
