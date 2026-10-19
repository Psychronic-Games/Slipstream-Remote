//! Small, passive previews shared by local browsing and authenticated hosts.
use std::{
    io::{Cursor, Read},
    path::Path,
    sync::{Arc, OnceLock},
};

use anyhow::{Result, bail};
use image::{DynamicImage, ImageDecoder, ImageReader};
use slipstream_core::proto::FilePreview;

const IMAGE_BYTES: u64 = 32 * 1024 * 1024;
const PIXELS: u64 = 16_000_000;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Image,
    Text,
}

fn kind(name: &str) -> Option<Kind> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff" => {
            Some(Kind::Image)
        }
        "txt" | "md" | "log" | "csv" | "json" | "toml" | "yaml" | "yml" | "xml" | "html"
        | "htm" | "css" | "js" | "ts" | "rs" | "py" | "sh" | "c" | "h" | "cpp" | "ini" | "conf"
        | "svg" => Some(Kind::Text),
        _ => None,
    }
}

pub fn supported(name: &str) -> bool {
    kind(name).is_some()
}

/// The permit lives with the blocking decoder even if its caller cancels.
/// Across folders and sessions at most two decoders run in this process.
pub async fn load(path: String) -> Result<Option<FilePreview>> {
    static WORKERS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    let permit = WORKERS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
        .clone()
        .acquire_owned()
        .await?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        read(Path::new(&path))
    })
    .await?
}

fn read(path: &Path) -> Result<Option<FilePreview>> {
    let Some(kind) = path.to_str().and_then(kind) else {
        return Ok(None);
    };
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        bail!("not a regular file");
    }
    if kind == Kind::Image && meta.len() > IMAGE_BYTES {
        bail!("image exceeds preview size limit");
    }
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        bail!("not a regular file");
    }
    let max = if kind == Kind::Image {
        IMAGE_BYTES + 1
    } else {
        FilePreview::TEXT_BYTES as u64
    };
    let mut bytes = Vec::new();
    file.take(max).read_to_end(&mut bytes)?;
    if kind == Kind::Text {
        // A partial final UTF-8 character is harmless; invalid/binary text is not previewed.
        let text = match std::str::from_utf8(&bytes) {
            Ok(t) => t,
            Err(e) if e.error_len().is_none() => std::str::from_utf8(&bytes[..e.valid_up_to()])?,
            Err(_) => return Ok(None),
        }
        .to_owned();
        let preview = FilePreview::Text(text);
        return Ok(preview.valid().then_some(preview));
    }
    if bytes.len() as u64 > IMAGE_BYTES {
        bail!("image exceeds preview size limit");
    }
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(96 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > PIXELS {
        bail!("image exceeds preview pixel limit");
    }
    let orientation = decoder.orientation()?;
    let mut image =
        DynamicImage::from_decoder(decoder)?.thumbnail(FilePreview::EDGE, FilePreview::EDGE);
    image.apply_orientation(orientation);
    let image = image.to_rgba8();
    Ok(Some(FilePreview::Image {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn bounded_previews_and_fallbacks() {
        let dir =
            std::env::temp_dir().join(format!("slipstream-previews-{}", rand::random::<u64>()));
        std::fs::create_dir(&dir).unwrap();
        let image = image::RgbaImage::from_pixel(640, 320, image::Rgba([20, 80, 240, 255]));
        image.save(dir.join("photo.png")).unwrap();
        let p = load(dir.join("photo.png").display().to_string())
            .await
            .unwrap()
            .unwrap();
        assert!(p.valid());
        assert!(
            matches!(p, FilePreview::Image { width:128, height:64, ref rgba } if rgba[..4] == [20,80,240,255])
        );
        for ext in ["jpg", "gif", "webp", "bmp", "ico", "tiff"] {
            let image = image::RgbImage::from_pixel(64, 32, image::Rgb([20, 80, 240]));
            if ext == "ico" {
                image::DynamicImage::ImageRgb8(image).to_rgba8().save(dir.join(format!("format.{ext}"))).unwrap();
            } else { image.save(dir.join(format!("format.{ext}"))).unwrap(); }
            let preview = read(&dir.join(format!("format.{ext}"))).unwrap().unwrap();
            assert!(preview.valid(), "{ext}");
        }
        // Phone-photo EXIF orientation must survive thumbnailing.
        use image::ImageEncoder;
        let mut jpeg = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut jpeg);
        let mut exif =
            b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0".to_vec();
        encoder
            .set_exif_metadata(std::mem::take(&mut exif))
            .unwrap();
        let image = image::RgbImage::from_pixel(640, 320, image::Rgb([20, 80, 240]));
        encoder
            .write_image(image.as_raw(), 640, 320, image::ExtendedColorType::Rgb8)
            .unwrap();
        std::fs::write(dir.join("rotated.jpg"), jpeg).unwrap();
        assert!(matches!(
            read(&dir.join("rotated.jpg")).unwrap(),
            Some(FilePreview::Image {
                width: 64,
                height: 128,
                ..
            })
        ));
        std::fs::write(dir.join("notes.txt"), "é".repeat(8192)).unwrap();
        assert!(
            matches!(read(&dir.join("notes.txt")).unwrap(), Some(FilePreview::Text(t)) if t.len() == 8192)
        );
        std::fs::write(dir.join("broken.jpg"), b"not an image").unwrap();
        assert!(read(&dir.join("broken.jpg")).is_err());
        std::fs::write(dir.join("binary.txt"), [0, 1, 2]).unwrap();
        assert!(read(&dir.join("binary.txt")).unwrap().is_none());
        assert!(read(&dir.join("unknown.exe")).unwrap().is_none());
        std::fs::create_dir(dir.join("folder.png")).unwrap();
        assert!(read(&dir.join("folder.png")).is_err());
        let f = std::fs::File::create(dir.join("large.png")).unwrap();
        f.set_len(IMAGE_BYTES + 1).unwrap();
        assert!(read(&dir.join("large.png")).is_err());
        // The header is valid, but its decoded allocation would be excessive.
        let mut bmp = vec![0u8; 54];
        bmp[..2].copy_from_slice(b"BM");
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&8192u32.to_le_bytes());
        bmp[22..26].copy_from_slice(&8192u32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        std::fs::write(dir.join("huge.bmp"), bmp).unwrap();
        assert!(read(&dir.join("huge.bmp")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
