//! Local camera probe: device metadata by default; --capture INDEX reads and
//! discards two frames, without saving or transmitting camera images.
fn main() -> anyhow::Result<()> {
    println!("Cameras: {:?}", slipstream_platform::camera::devices()?);
    if std::env::args().nth(1).as_deref() == Some("--capture") {
        let index: u32 = std::env::args().nth(2).expect("camera index").parse()?;
        let stream = slipstream_platform::camera::start(index)?;
        for _ in 0..2 {
            let frame = stream.next(std::time::Duration::from_secs(3))
                .map_err(anyhow::Error::msg)?.ok_or_else(|| anyhow::anyhow!("No camera frame"))?;
            frame.validate()?;
            println!("Received {} × {} {:?}", frame.width, frame.height, frame.format);
        }
    }
    Ok(())
}
