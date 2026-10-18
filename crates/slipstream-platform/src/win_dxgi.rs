//! Screen capture with DXGI Desktop Duplication, owned by the capture
//! thread: everything is released when it's dropped. (xcap's recorder kept
//! a thread holding its duplication after a session ended, so the next
//! session in the same process couldn't duplicate the screen and fell back
//! to slow GDI.) One persistent staging texture, one copy per frame out of
//! it, and frames stay BGRA for the encoder.

use anyhow::{Context, Result, bail};
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::{
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
                D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device,
                ID3D11DeviceContext, ID3D11Texture2D,
            },
            Dxgi::{
                Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB},
                CreateDXGIFactory1, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter, IDXGIFactory1,
                IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
            },
        },
    },
    core::Interface,
};

/// Duplication of the monitor whose top-left corner is at a desktop
/// position, on the calling thread's desktop.
pub struct Duplication {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    /// How long the last picture took to copy out.
    pub last_copy: std::time::Duration,
}

impl Duplication {
    pub fn new(x: i32, y: i32) -> Result<Self> {
        // SAFETY: COM calls on interfaces we hold; each returns owned refs.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().context("DXGI factory")?;
            let mut a = 0;
            while let Ok(adapter) = factory.EnumAdapters1(a) {
                a += 1;
                let mut o = 0;
                while let Ok(output) = adapter.EnumOutputs(o) {
                    o += 1;
                    let desc = output.GetDesc()?;
                    if (desc.DesktopCoordinates.left, desc.DesktopCoordinates.top) != (x, y) {
                        continue;
                    }
                    let mut device = None;
                    let mut context = None;
                    D3D11CreateDevice(
                        &adapter.cast::<IDXGIAdapter>()?,
                        D3D_DRIVER_TYPE_UNKNOWN,
                        HMODULE::default(),
                        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                        None,
                        D3D11_SDK_VERSION,
                        Some(&mut device),
                        None,
                        Some(&mut context),
                    )
                    .context("Direct3D 11 device")?;
                    let (device, context) = (device.context("no device")?, context.context("no device context")?);
                    let dup = output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device).context("DuplicateOutput")?;
                    let format = dup.GetDesc().ModeDesc.Format;
                    if format != DXGI_FORMAT_B8G8R8A8_UNORM && format != DXGI_FORMAT_B8G8R8A8_UNORM_SRGB {
                        bail!("the screen's format ({format:?}) isn't 8-bit BGRA (HDR?)");
                    }
                    return Ok(Self { device, context, dup, staging: None, last_copy: std::time::Duration::ZERO });
                }
            }
            bail!("no DXGI output at {x},{y}")
        }
    }

    /// Waits up to `timeout_ms` for the picture to change and copies it into
    /// `out` (BGRA rows of width × 4 bytes). `Ok(Some(size))` for a new
    /// picture, `Ok(None)` if nothing changed (or only the pointer moved),
    /// and an error once duplication ended (a desktop switch, a mode change).
    pub fn next(&mut self, timeout_ms: u32, out: &mut Vec<u8>) -> Result<Option<(u32, u32)>> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        // SAFETY: the duplication and the frame it hands out are live until
        // ReleaseFrame, which every path below reaches.
        unsafe {
            match self.dup.AcquireNextFrame(timeout_ms, &mut info, &mut resource) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(e) => bail!("duplication ended: {e}"),
            }
            let t = std::time::Instant::now();
            let result = self.copy_out(&info, resource, out);
            self.last_copy = t.elapsed();
            let _ = self.dup.ReleaseFrame();
            result
        }
    }

    /// The frame's picture (if it has a new one) into `out`.
    unsafe fn copy_out(&mut self, info: &DXGI_OUTDUPL_FRAME_INFO, resource: Option<IDXGIResource>, out: &mut Vec<u8>) -> Result<Option<(u32, u32)>> {
        if info.LastPresentTime == 0 {
            return Ok(None); // only the pointer changed
        }
        let texture: ID3D11Texture2D = resource.context("no frame")?.cast()?;
        // SAFETY: (all below) D3D11 calls on live objects; the mapped rows
        // are read within Map/Unmap, each RowPitch bytes apart.
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let (w, h) = (desc.Width, desc.Height);
            if self.staging.as_ref().is_none_or(|s| (s.1, s.2) != (w, h)) {
                let staging_desc = D3D11_TEXTURE2D_DESC {
                    BindFlags: 0,
                    MiscFlags: 0,
                    Usage: D3D11_USAGE_STAGING,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MipLevels: 1,
                    ArraySize: 1,
                    ..desc
                };
                let mut staging = None;
                self.device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
                self.staging = Some((staging.context("no staging texture")?, w, h));
            }
            let staging = &self.staging.as_ref().expect("made above").0;
            self.context.CopyResource(staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let row = w as usize * 4;
            out.clear();
            out.reserve(row * h as usize);
            for y in 0..h as usize {
                let src = std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize), row);
                out.extend_from_slice(src);
            }
            self.context.Unmap(staging, 0);
            Ok(Some((w, h)))
        }
    }
}
