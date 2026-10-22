//! Small local-only snapshots for the explicit meeting screen chooser.
use super::Client;
use anyhow::{Context, Result, ensure};
use slipstream_platform::{PixelFormat, RawFrame};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

impl Client {
    pub async fn meeting_screen_preview(
        &self,
        index: u32,
        cancelled: Arc<AtomicBool>,
    ) -> Result<RawFrame> {
        ensure!(!cancelled.load(Ordering::Acquire), "Preview cancelled");
        let stream = self.meeting_capture(index, 1).await?;
        tokio::task::spawn_blocking(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !cancelled.load(Ordering::Acquire) {
                ensure!(
                    Instant::now() < deadline,
                    "Preview unavailable: no screen frame arrived"
                );
                if let Some(frame) = stream
                    .next(Duration::from_millis(100))
                    .map_err(anyhow::Error::msg)?
                {
                    return thumbnail(&frame);
                }
            }
            anyhow::bail!("Preview cancelled")
        })
        .await
        .context("Screen preview worker stopped")?
    }
}

fn thumbnail(frame: &RawFrame) -> Result<RawFrame> {
    frame.validate()?;
    let scale = (320.0 / frame.width as f64)
        .min(180.0 / frame.height as f64)
        .min(1.0);
    let width = ((frame.width as f64 * scale) as u32).max(1);
    let height = ((frame.height as f64 * scale) as u32).max(1);
    let mut data = vec![0; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let source = ((u64::from(y) * u64::from(frame.height) / u64::from(height))
                * u64::from(frame.stride)
                + (u64::from(x) * u64::from(frame.width) / u64::from(width)) * 4)
                as usize;
            let target = ((y * width + x) * 4) as usize;
            let p = &frame.data[source..source + 4];
            let pixel = if frame.format == PixelFormat::Bgra {
                [p[2], p[1], p[0], 255]
            } else {
                [p[0], p[1], p[2], 255]
            };
            data[target..target + 4].copy_from_slice(&pixel);
        }
    }
    Ok(RawFrame {
        width,
        height,
        stride: width * 4,
        format: PixelFormat::Rgba,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_checks_stride_channels_and_keeps_portrait_aspect_bounded() {
        let frame = RawFrame {
            width: 2,
            height: 4,
            stride: 12,
            format: PixelFormat::Bgra,
            data: [1, 2, 3, 0, 4, 5, 6, 0, 99, 99, 99, 99].repeat(4),
        };
        let thumb = thumbnail(&frame).unwrap();
        assert_eq!((thumb.width, thumb.height, thumb.data.len()), (2, 4, 32));
        assert!(
            thumb
                .data
                .as_chunks::<8>().0.iter()
                .all(|row| *row == [3, 2, 1, 255, 6, 5, 4, 255])
        );
        let large = RawFrame {
            width: 400,
            height: 800,
            stride: 1600,
            format: PixelFormat::Rgba,
            data: vec![7; 400 * 800 * 4],
        };
        let thumb = thumbnail(&large).unwrap();
        assert_eq!(
            (thumb.width, thumb.height, thumb.data.len()),
            (90, 180, 90 * 180 * 4)
        );
        assert!(
            thumbnail(&RawFrame {
                data: vec![],
                ..frame
            })
            .is_err()
        );
    }
}
