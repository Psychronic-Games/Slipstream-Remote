//! Read-only input-device discovery, using capture's device identifiers.
fn main() -> anyhow::Result<()> {
    for device in slipstream_audio::devices::microphones()? {
        println!(
            "{}{} ({})",
            device.name,
            if device.is_default { " · default" } else { "" },
            device.id
        );
    }
    Ok(())
}
