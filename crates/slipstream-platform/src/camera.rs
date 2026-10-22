//! Explicit camera capture, separate from desktop capture and control.
use crate::FrameStream;
use anyhow::{Context, Result};

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub fn devices() -> Result<Vec<(u32, String)>> {
    #[cfg(windows)]
    media_foundation()?;
    Ok(nokhwa::query(nokhwa::utils::ApiBackend::Auto)?
        .into_iter()
        .enumerate()
        .map(|(ordinal, info)| (info.index().as_index().unwrap_or(ordinal as u32), info.human_name()))
        .collect())
}
#[cfg(target_os = "android")]
pub fn devices() -> Result<Vec<(u32, String)>> {
    Ok(vec![(0, "Rear camera".into()), (1, "Front camera".into())])
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub fn start(index: u32) -> Result<FrameStream> {
    #[cfg(windows)]
    media_foundation()?;
    use nokhwa::{
        Camera,
        pixel_format::RgbFormat,
        utils::{
            CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
            Resolution,
        },
    };
    let slot = crate::FrameSlot::new();
    let producer = slot.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("meeting-camera".into())
        .spawn(move || {
            let opened = (|| {
                let wanted = CameraFormat::new(Resolution::new(640, 480), FrameFormat::YUYV, 15);
                let request =
                    RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(wanted));
                let mut camera = Camera::new(CameraIndex::Index(index), request)?;
                camera.open_stream()?;
                anyhow::Ok(camera)
            })();
            let mut camera = match opened {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Err(format!("Camera: {e:#}")));
                    return;
                }
            };
            if tx.send(Ok(())).is_err() {
                return;
            }
            while !producer.is_closed() {
                let next = (|| {
                    let buffer = camera.frame()?;
                    let size = buffer.resolution();
                    anyhow::ensure!(
                        size.width() <= 3840
                            && size.height() <= 3840
                            && u64::from(size.width()) * u64::from(size.height()) <= 3840 * 2160,
                        "Camera resolution is too large"
                    );
                    let rgba = if buffer.source_frame_format() == FrameFormat::MJPEG {
                        image::load_from_memory_with_format(
                            buffer.buffer(),
                            image::ImageFormat::Jpeg,
                        )?
                        .to_rgba8()
                    } else {
                        let rgb = buffer.decode_image::<RgbFormat>()?;
                        image::DynamicImage::ImageRgb8(rgb).into_rgba8()
                    };
                    let (width, height) = rgba.dimensions();
                    Ok(crate::RawFrame {
                        width,
                        height,
                        stride: width * 4,
                        format: crate::PixelFormat::Rgba,
                        data: rgba.into_raw(),
                    })
                })();
                match next {
                    Ok(frame) => producer.put(frame),
                    Err(e) => {
                        producer.fail(format!("Camera stopped: {e:#}"));
                        break;
                    }
                }
            }
            let _ = camera.stop_stream();
        })?;
    match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(FrameStream::new(slot)),
        Ok(Err(e)) => {
            slot.close();
            anyhow::bail!(e)
        }
        Err(_) => {
            slot.close();
            anyhow::bail!("Camera did not respond in time")
        }
    }
}
#[cfg(target_os = "android")]
pub fn start(index: u32) -> Result<FrameStream> {
    start_cancellable(index, std::sync::Arc::default())
}
pub fn start_cancellable(
    index: u32,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<FrameStream> {
    use std::sync::atomic::Ordering;
    anyhow::ensure!(
        !cancelled.load(Ordering::Acquire),
        "Camera sharing cancelled"
    );
    #[cfg(not(target_os = "android"))]
    {
        let capture = start(index)?;
        anyhow::ensure!(
            !cancelled.load(Ordering::Acquire),
            "Camera sharing cancelled"
        );
        Ok(capture)
    }
    #[cfg(target_os = "android")]
    {
        let epoch = crate::android::camera_start(index)?;
        let started = std::time::Instant::now();
        while crate::android::camera_dimensions().is_none() {
            if cancelled.load(Ordering::Acquire)
                || started.elapsed() > std::time::Duration::from_secs(60)
            {
                crate::android::camera_stop(epoch);
                anyhow::bail!("Camera permission was cancelled or did not arrive in time");
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        if cancelled.load(Ordering::Acquire) {
            crate::android::camera_stop(epoch);
            anyhow::bail!("Camera sharing cancelled");
        }
        Ok(crate::android::camera_stream()?
            .with_cleanup(move || crate::android::camera_stop(epoch)))
    }
}
pub fn default_camera() -> Result<FrameStream> {
    start(devices()?.first().context("No camera is available")?.0)
}

/// The executable delay-loads these optional components. Pin their system
/// copies before entering nokhwa, so a missing Server/N media feature is a
/// camera error instead of a loader exception that prevents using the app.
#[cfg(windows)]
fn media_foundation() -> Result<()> {
    use windows_sys::Win32::{
        Foundation::FreeLibrary,
        System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
    };
    static AVAILABLE: std::sync::OnceLock<std::result::Result<(), String>> =
        std::sync::OnceLock::new();
    AVAILABLE.get_or_init(|| {
        let mut loaded = Vec::new();
        for name in ["mf.dll", "mfplat.dll", "mfreadwrite.dll"] {
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            // SAFETY: terminated UTF-16 and a system-directory-only search.
            let module = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
            if module.is_null() {
                for previous in loaded { unsafe { FreeLibrary(previous); } }
                return Err("Camera sharing needs the Windows Media Foundation feature (Media Feature Pack on N editions). Remote support and meetings remain available.".into());
            }
            loaded.push(module);
        }
        // Keep these library references until process exit: delay-import
        // pointers may be used by any later camera worker.
        Ok(())
    }).as_ref().map_err(|message| anyhow::anyhow!(message.clone())).copied()
}

/// Convert checked Android YUV planes and rotate into the phone's current view.
#[allow(clippy::too_many_arguments)]
pub fn yuv420(
    y: &[u8],
    u: &[u8],
    v: &[u8],
    width: usize,
    height: usize,
    y_row: usize,
    uv_row: usize,
    uv_pixel: usize,
    rotation: u32,
) -> Result<(u32, u32, Vec<u8>)> {
    anyhow::ensure!(
        width >= 2
            && height >= 2
            && width <= 1280
            && height <= 1280
            && width * height <= 1280 * 720
            && y_row >= width
            && y_row <= 16384
            && uv_row <= 16384
            && (1..=2).contains(&uv_pixel)
            && [0, 90, 180, 270].contains(&rotation),
        "Invalid camera frame"
    );
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let chroma_last = (chroma_width - 1) * uv_pixel + 1;
    anyhow::ensure!(
        uv_row >= chroma_last
            && y.len() >= y_row * (height - 1) + width
            && u.len() >= uv_row * (chroma_height - 1) + chroma_last
            && v.len() >= uv_row * (chroma_height - 1) + chroma_last,
        "Camera planes are shorter than their strides"
    );
    let (out_w, out_h) = if rotation == 90 || rotation == 270 {
        (height, width)
    } else {
        (width, height)
    };
    let mut output = vec![0; out_w * out_h * 4];
    for row in 0..height {
        for col in 0..width {
            let yy = i32::from(y[row * y_row + col]) - 16;
            let ci = (row / 2) * uv_row + (col / 2) * uv_pixel;
            let uu = i32::from(u[ci]) - 128;
            let vv = i32::from(v[ci]) - 128;
            let (x, z) = match rotation {
                90 => (height - 1 - row, col),
                180 => (width - 1 - col, height - 1 - row),
                270 => (row, width - 1 - col),
                _ => (col, row),
            };
            let at = (z * out_w + x) * 4;
            output[at] = ((298 * yy + 409 * vv + 128) >> 8).clamp(0, 255) as u8;
            output[at + 1] = ((298 * yy - 100 * uu - 208 * vv + 128) >> 8).clamp(0, 255) as u8;
            output[at + 2] = ((298 * yy + 516 * uu + 128) >> 8).clamp(0, 255) as u8;
            output[at + 3] = 255;
        }
    }
    Ok((out_w as u32, out_h as u32, output))
}
#[cfg(test)]
mod tests {
    #[test]
    fn camera_planes_check_bounds_and_rotate() {
        let y = [16, 40, 80, 120, 160, 235];
        let u = [128, 128];
        let v = [128, 128];
        let (w, h, pixels) = super::yuv420(&y, &u, &v, 2, 3, 2, 1, 1, 90).unwrap();
        assert_eq!((w, h, pixels.len()), (3, 2, 24));
        assert_eq!(pixels[3], 255);
        assert!(super::yuv420(&y[..2], &u, &v, 2, 3, 2, 1, 1, 90).is_err());
        assert!(super::yuv420(&y, &u, &v, usize::MAX, 3, 2, 1, 1, 90).is_err());
    }
}
