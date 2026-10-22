use anyhow::{Context, Result, bail};
use nokhwa::{
    Camera,
    pixel_format::RgbFormat,
    utils::{CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType},
};

pub(super) fn devices() -> Result<Vec<(u32, String)>> {
    let mut devices = Vec::new();
    for (ordinal, info) in nokhwa::query(nokhwa::utils::ApiBackend::Auto)?
        .into_iter()
        .enumerate()
    {
        let index = info.index().as_index().unwrap_or(ordinal as u32);
        // UVC webcams expose a second /dev/videoN for metadata. Nokhwa lists
        // it before the real camera on some boots; it cannot provide pictures.
        #[cfg(target_os = "linux")]
        if !v4l::Device::new(index as usize)
            .and_then(|d| d.query_caps())
            .is_ok_and(|caps| {
                caps.capabilities
                    .contains(v4l::capability::Flags::VIDEO_CAPTURE)
            })
        {
            continue;
        }
        devices.push((index, info.human_name()));
    }
    devices.sort_by_key(|(index, _)| *index);
    Ok(devices)
}

pub(super) fn open(index: u32) -> Result<Camera> {
    #[cfg(target_os = "linux")]
    {
        let device = v4l::Device::new(index as usize).context("Opening the selected camera")?;
        anyhow::ensure!(
            device
                .query_caps()?
                .capabilities
                .contains(v4l::capability::Flags::VIDEO_CAPTURE),
            "The saved camera is a metadata-only device. Select your camera in Devices."
        );
    }
    // Closest requires the exact pixel format and (in this Nokhwa version)
    // the requested resolution's FPS list. Bootstrap with a decodable native
    // mode, then rank advertised modes ourselves, trying safe alternatives.
    let request = RequestedFormat::new::<RgbFormat>(RequestedFormatType::None);
    let mut camera = Camera::new(CameraIndex::Index(index), request)
        .context("Finding a supported camera format")?;
    let formats = ranked(camera.compatible_camera_formats()?);
    let mut errors = Vec::new();
    for format in formats.into_iter().take(16) {
        let opened = camera
            .set_camera_requset(RequestedFormat::new::<RgbFormat>(
                RequestedFormatType::Exact(format),
            ))
            .and_then(|_| camera.open_stream());
        match opened {
            Ok(()) => {
                tracing::info!(camera=index,format=%camera.camera_format(),"meeting camera started");
                return Ok(camera);
            }
            Err(error) => {
                errors.push(format!("{format}: {error}"));
                let _ = camera.stop_stream();
            }
        }
    }
    bail!(
        "No supported meeting camera mode could be opened. {}",
        if errors.is_empty() {
            "The camera reported no usable picture formats.".into()
        } else {
            errors.join("; ")
        }
    )
}

fn ranked(mut formats: Vec<CameraFormat>) -> Vec<CameraFormat> {
    formats.retain(|f| {
        f.width() > 0
            && f.height() > 0
            && f.width() <= 1920
            && f.height() <= 1080
            && (1..=60).contains(&f.frame_rate())
    });
    formats.sort_by_key(|f| {
        let x = i64::from(f.width()) - 640;
        let y = i64::from(f.height()) - 480;
        (
            f.frame_rate()<10,
            x * x + y * y,
            f.frame_rate().abs_diff(15),
            match f.format() {
                FrameFormat::YUYV => 0,
                FrameFormat::MJPEG => 1,
                _ => 2,
            },
        )
    });
    formats.dedup();
    formats
}

#[cfg(test)]
mod tests {
    use super::*;
    use nokhwa::utils::Resolution;
    #[test]
    fn native_modes_allow_mjpeg_only_and_non_vga_cameras_without_synthesizing_a_mode() {
        let mode = |w, h, format, fps| CameraFormat::new(Resolution::new(w, h), format, fps);
        let mjpeg = mode(1280, 720, FrameFormat::MJPEG, 30);
        assert_eq!(ranked(vec![mjpeg]), vec![mjpeg]);
        let near = mode(640, 360, FrameFormat::NV12, 30);
        let slow = mode(640, 360, FrameFormat::NV12, 5);
        let large = mode(3840, 2160, FrameFormat::MJPEG, 30);
        assert_eq!(ranked(vec![large, slow, near]), vec![near, slow]);
        let target = mode(640, 480, FrameFormat::YUYV, 15);
        assert_eq!(ranked(vec![near, target, target]), vec![target, near]);
    }
}
