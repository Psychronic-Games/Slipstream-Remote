//! Whole-machine screen capture for the Linux service: reads the picture
//! the graphics card is sending to each display (kernel mode setting), so
//! it shows whatever is on screen: the login screen, a locked session, a
//! Wayland or X11 desktop, or a text console. Needs root: the kernel hands
//! framebuffer handles only to `CAP_SYS_ADMIN`.
//!
//! Spoken as plain ioctls on `/dev/dri/card*` (no libdrm). Framebuffers are
//! mapped as ordinary memory, which is how virtual machines' adapters and
//! simple framebuffer drivers keep them. Graphics cards that keep the
//! picture in a tiled layout only their own driver understands are refused
//! with a message rather than shown as garbage.

use std::{
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
    ptr::NonNull,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use slipstream_codec::{PixelFormat, RawFrame};
use slipstream_core::proto::DisplayInfo;

use crate::{FrameSlot, FrameStream};

const fn ioc(dir: u32, nr: u8, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | ((b'd' as u32) << 8) | nr as u32
}
const fn iow<T>(nr: u8) -> u32 {
    ioc(1, nr, size_of::<T>())
}
const fn iowr<T>(nr: u8) -> u32 {
    ioc(3, nr, size_of::<T>())
}

#[repr(C)]
#[derive(Default)]
struct GemClose {
    handle: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Default)]
struct PrimeHandle {
    handle: u32,
    flags: u32,
    fd: i32,
}

#[repr(C)]
#[derive(Default)]
struct CardRes {
    fb_id_ptr: u64,
    crtc_id_ptr: u64,
    connector_id_ptr: u64,
    encoder_id_ptr: u64,
    count_fbs: u32,
    count_crtcs: u32,
    count_connectors: u32,
    count_encoders: u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct ModeInfo {
    clock: u32,
    hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    htotal: u16,
    hskew: u16,
    vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    vtotal: u16,
    vscan: u16,
    vrefresh: u32,
    flags: u32,
    kind: u32,
    name: [u8; 32],
}

#[repr(C)]
#[derive(Default)]
struct ModeCrtc {
    set_connectors_ptr: u64,
    count_connectors: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    gamma_size: u32,
    mode_valid: u32,
    mode: ModeInfo,
}

#[repr(C)]
#[derive(Default)]
struct FbCmd {
    fb_id: u32,
    width: u32,
    height: u32,
    pitch: u32,
    bpp: u32,
    depth: u32,
    handle: u32,
}

#[repr(C)]
#[derive(Default)]
struct FbCmd2 {
    fb_id: u32,
    width: u32,
    height: u32,
    pixel_format: u32,
    flags: u32,
    handles: [u32; 4],
    pitches: [u32; 4],
    offsets: [u32; 4],
    modifier: [u64; 4],
}

#[repr(C)]
#[derive(Default)]
struct MapDumb {
    handle: u32,
    pad: u32,
    offset: u64,
}

const GEM_CLOSE: u32 = iow::<GemClose>(0x09);
const DROP_MASTER: u32 = ioc(0, 0x1f, 0);
const PRIME_HANDLE_TO_FD: u32 = iowr::<PrimeHandle>(0x2d);
const GETRESOURCES: u32 = iowr::<CardRes>(0xa0);
const GETCRTC: u32 = iowr::<ModeCrtc>(0xa1);
const GETFB: u32 = iowr::<FbCmd>(0xad);
const MAP_DUMB: u32 = iowr::<MapDumb>(0xb3);
const GETFB2: u32 = iowr::<FbCmd2>(0xce);

const FB_MODIFIERS: u32 = 1 << 1;
const MOD_LINEAR: u64 = 0;
const MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}
const XR24: u32 = fourcc(b"XR24");
const AR24: u32 = fourcc(b"AR24");
const XB24: u32 = fourcc(b"XB24");
const AB24: u32 = fourcc(b"AB24");
const RG16: u32 = fourcc(b"RG16");
const XR30: u32 = fourcc(b"XR30");
const AR30: u32 = fourcc(b"AR30");
const XB30: u32 = fourcc(b"XB30");
const AB30: u32 = fourcc(b"AB30");

fn ioctl<T>(fd: &impl AsRawFd, request: u32, arg: &mut T) -> io::Result<()> {
    loop {
        // SAFETY: `arg` is the `repr(C)` structure this request is defined
        // with (its size is part of the request number).
        if unsafe { libc::ioctl(fd.as_raw_fd(), request as _, arg as *mut T) } == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if !matches!(e.raw_os_error(), Some(libc::EINTR | libc::EAGAIN)) {
            return Err(e);
        }
    }
}

/// One graphics card's device node.
struct Card {
    file: File,
    driver: String,
}

impl Card {
    fn open(path: &Path) -> Result<Self> {
        let file = File::options().read(true).write(true).open(path).with_context(|| format!("opening {}", path.display()))?;
        // The first program to open a card becomes its master, which would
        // keep the login screen from starting if that were us: give it up.
        // SAFETY: an ioctl without an argument.
        unsafe { libc::ioctl(file.as_raw_fd(), DROP_MASTER as _, 0) };
        let driver = path
            .file_name()
            .and_then(|name| std::fs::read_link(Path::new("/sys/class/drm").join(name).join("device/driver")).ok())
            .and_then(|link| link.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        Ok(Self { file, driver })
    }

    fn crtc_ids(&self) -> Result<Vec<u32>> {
        let mut res = CardRes::default();
        ioctl(&self.file, GETRESOURCES, &mut res).context("not a mode-setting graphics card")?;
        // Counts can change between the two calls (a display plugged in).
        for _ in 0..4 {
            let mut ids = vec![0u32; res.count_crtcs as usize];
            let mut ask = CardRes { crtc_id_ptr: ids.as_mut_ptr() as u64, count_crtcs: ids.len() as u32, ..Default::default() };
            ioctl(&self.file, GETRESOURCES, &mut ask)?;
            if ask.count_crtcs as usize <= ids.len() {
                ids.truncate(ask.count_crtcs as usize);
                return Ok(ids);
            }
            res.count_crtcs = ask.count_crtcs;
        }
        bail!("the graphics card's outputs kept changing")
    }

    fn crtc(&self, id: u32) -> Result<Option<Crtc>> {
        let mut c = ModeCrtc { crtc_id: id, ..Default::default() };
        ioctl(&self.file, GETCRTC, &mut c)?;
        if c.mode_valid == 0 || c.fb_id == 0 || c.mode.hdisplay == 0 || c.mode.vdisplay == 0 {
            return Ok(None);
        }
        Ok(Some(Crtc { fb: c.fb_id, x: c.x, y: c.y, width: c.mode.hdisplay as u32, height: c.mode.vdisplay as u32 }))
    }

    /// What framebuffer `id` looks like, with a handle to its memory that
    /// the caller closes.
    fn framebuffer(&self, id: u32) -> Result<Fb> {
        let mut fb2 = FbCmd2 { fb_id: id, ..Default::default() };
        if ioctl(&self.file, GETFB2, &mut fb2).is_ok() {
            let handle = fb2.handles[0];
            // Further planes (video formats) aren't used: let them go.
            for extra in fb2.handles[1..].iter().copied().filter(|h| *h != 0 && *h != handle) {
                self.close_handle(extra);
            }
            let modifier = (fb2.flags & FB_MODIFIERS != 0).then_some(fb2.modifier[0]);
            return Ok(Fb { width: fb2.width, height: fb2.height, pitch: fb2.pitches[0], offset: fb2.offsets[0], format: fb2.pixel_format, modifier, handle });
        }
        // Kernels before 5.7.
        let mut fb = FbCmd { fb_id: id, ..Default::default() };
        ioctl(&self.file, GETFB, &mut fb).context("reading the framebuffer's description")?;
        let format = match (fb.bpp, fb.depth) {
            (32, 30) => XR30,
            (32, _) => XR24,
            (16, _) => RG16,
            (bpp, _) => bail!("the screen uses {bpp} bits per pixel, which isn't supported"),
        };
        Ok(Fb { width: fb.width, height: fb.height, pitch: fb.pitch, offset: 0, format, modifier: None, handle: fb.handle })
    }

    fn close_handle(&self, handle: u32) {
        let _ = ioctl(&self.file, GEM_CLOSE, &mut GemClose { handle, pad: 0 });
    }

    /// Maps the framebuffer's memory for reading. The mapping outlives the
    /// handle, which is closed here.
    fn map(&self, fb: &Fb) -> Result<Mapping> {
        if fb.handle == 0 {
            bail!("the system only shows the screen's memory to root (the Slipstream service)");
        }
        let result = self.map_handle(fb);
        self.close_handle(fb.handle);
        result
    }

    fn map_handle(&self, fb: &Fb) -> Result<Mapping> {
        let bytes = bytes_per_pixel(fb.format).ok_or_else(|| anyhow!("the screen's pixel format ({}) isn't supported", fourcc_name(fb.format)))?;
        if fb.width == 0 || fb.height == 0 || (fb.pitch as u64) < fb.width as u64 * bytes as u64 {
            bail!("the framebuffer's description makes no sense ({}x{}, {} bytes a row)", fb.width, fb.height, fb.pitch);
        }
        match fb.modifier {
            Some(MOD_LINEAR | MOD_INVALID) => {}
            // No layout stated: these drivers arrange it their own way.
            None if !matches!(self.driver.as_str(), "i915" | "xe" | "amdgpu" | "radeon" | "nouveau" | "nvidia-drm" | "nvidia") => {}
            _ => bail!(
                "this computer's graphics driver ({}) keeps the screen in a layout the Slipstream service can't read yet",
                if self.driver.is_empty() { "unknown" } else { &self.driver }
            ),
        }
        let len = fb.offset as usize + fb.pitch as usize * fb.height as usize;
        let mut dumb = MapDumb { handle: fb.handle, ..Default::default() };
        let by_offset = ioctl(&self.file, MAP_DUMB, &mut dumb).map_err(anyhow::Error::from).and_then(|()| Mapping::new(&self.file, dumb.offset, len));
        let by_offset = match by_offset {
            Ok(mapping) => return Ok(mapping),
            Err(e) => e,
        };
        // Otherwise as a shareable buffer.
        let mut prime = PrimeHandle { handle: fb.handle, flags: libc::O_CLOEXEC as u32, fd: -1 };
        ioctl(&self.file, PRIME_HANDLE_TO_FD, &mut prime).with_context(|| format!("the graphics driver ({}) wouldn't share the screen's memory ({by_offset:#})", self.driver))?;
        // SAFETY: the kernel just gave us this descriptor.
        let fd = unsafe { OwnedFd::from_raw_fd(prime.fd) };
        Mapping::new(&fd, 0, len).with_context(|| format!("the graphics driver ({}) wouldn't let the screen's memory be read", self.driver))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Crtc {
    fb: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fb {
    width: u32,
    height: u32,
    pitch: u32,
    offset: u32,
    format: u32,
    modifier: Option<u64>,
    handle: u32,
}

struct Mapping {
    ptr: NonNull<u8>,
    len: usize,
}

// SAFETY: a read-only mapping this struct owns.
unsafe impl Send for Mapping {}

impl Mapping {
    fn new(fd: &impl AsRawFd, offset: u64, len: usize) -> Result<Self> {
        // SAFETY: a shared read-only mapping; the kernel checks the range
        // against the buffer's size.
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, fd.as_raw_fd(), offset as libc::off_t) };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error()).context("mapping the screen's memory");
        }
        Ok(Self { ptr: NonNull::new(ptr.cast()).expect("mmap succeeded"), len })
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: valid for `len` bytes while mapped. The compositor may
        // write while we read: a torn picture at worst, replaced next frame.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: unmapping our own mapping.
        unsafe { libc::munmap(self.ptr.as_ptr().cast(), self.len) };
    }
}

fn bytes_per_pixel(format: u32) -> Option<usize> {
    match format {
        XR24 | AR24 | XB24 | AB24 | XR30 | AR30 | XB30 | AB30 => Some(4),
        RG16 => Some(2),
        _ => None,
    }
}

fn fourcc_name(format: u32) -> String {
    format.to_le_bytes().iter().map(|b| if b.is_ascii_graphic() { *b as char } else { '?' }).collect()
}

/// Copies the `width`×`height` part of a framebuffer at (`x`, `y`) into
/// `out` as tightly packed 32-bit pixels, and says which order they're in.
fn copy_pixels(src: &[u8], fb: &Fb, (x, y, width, height): (u32, u32, u32, u32), out: &mut Vec<u8>) -> Result<PixelFormat> {
    let bytes = bytes_per_pixel(fb.format).context("unsupported pixel format")?;
    let (x, y, width, height) = (x as usize, y as usize, width as usize, height as usize);
    let (pitch, base) = (fb.pitch as usize, fb.offset as usize);
    let last = base + (y + height - 1) * pitch + (x + width) * bytes;
    if x + width > fb.width as usize || y + height > fb.height as usize || last > src.len() {
        bail!("the display's picture lies outside its framebuffer");
    }
    out.clear();
    out.reserve(width * height * 4);
    let rows = (0..height).map(|row| {
        let start = base + (y + row) * pitch + x * bytes;
        &src[start..start + width * bytes]
    });
    let ten_bit = |out: &mut Vec<u8>, row: &[u8]| {
        // 2 bits unused, then three 10-bit channels: keep each one's top 8.
        out.extend(row.as_chunks::<4>().0.iter().flat_map(|p| {
            let v = u32::from_le_bytes(*p);
            [(v >> 2) as u8, (v >> 12) as u8, (v >> 22) as u8, 255]
        }));
    };
    Ok(match fb.format {
        XR24 | AR24 => {
            rows.for_each(|row| out.extend_from_slice(row));
            PixelFormat::Bgra
        }
        XB24 | AB24 => {
            rows.for_each(|row| out.extend_from_slice(row));
            PixelFormat::Rgba
        }
        XR30 | AR30 => {
            rows.for_each(|row| ten_bit(out, row));
            PixelFormat::Bgra
        }
        XB30 | AB30 => {
            rows.for_each(|row| ten_bit(out, row));
            PixelFormat::Rgba
        }
        _ => {
            for row in rows {
                out.extend(row.as_chunks::<2>().0.iter().flat_map(|p| {
                    let v = u16::from_le_bytes(*p);
                    let (r, g, b) = ((v >> 11) as u8, ((v >> 5) & 0x3f) as u8, (v & 0x1f) as u8);
                    [(b << 3) | (b >> 2), (g << 2) | (g >> 4), (r << 3) | (r >> 2), 255]
                }));
            }
            PixelFormat::Bgra
        }
    })
}

/// A display that is showing something: which output of which card.
#[derive(Debug, Clone)]
pub struct Screen {
    card: PathBuf,
    crtc: u32,
    pub info: DisplayInfo,
}

fn card_paths() -> Vec<PathBuf> {
    let mut cards: Vec<(u32, PathBuf)> = std::fs::read_dir("/dev/dri")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let number = entry.file_name().to_str()?.strip_prefix("card")?.parse().ok()?;
            Some((number, entry.path()))
        })
        .collect();
    cards.sort();
    cards.into_iter().map(|(_, path)| path).collect()
}

/// The displays that are on, in card and output order. Where several
/// outputs show parts of one framebuffer (X11) their real positions are
/// known; otherwise (Wayland: one framebuffer each) they're taken to sit
/// side by side, left to right.
pub fn screens() -> Result<Vec<Screen>> {
    let cards = card_paths();
    if cards.is_empty() {
        bail!("this computer has no graphics card the system drives (/dev/dri is empty)");
    }
    let mut out = Vec::new();
    let mut problem = None;
    for path in cards {
        let found = Card::open(&path).and_then(|card| {
            let mut found = Vec::new();
            for id in card.crtc_ids()? {
                if let Some(crtc) = card.crtc(id)? {
                    found.push((id, crtc));
                }
            }
            Ok(found)
        });
        match found {
            Ok(found) => out.extend(found.into_iter().map(|(id, crtc)| (path.clone(), id, crtc))),
            Err(e) => problem = Some(e.context(format!("{}", path.display()))),
        }
    }
    if out.is_empty()
        && let Some(e) = problem
    {
        return Err(e);
    }
    let placed = out.iter().any(|(_, _, c)| c.x != 0 || c.y != 0);
    let mut next_x = 0;
    Ok(out
        .into_iter()
        .enumerate()
        .map(|(i, (card, crtc, c))| {
            let x = if placed { c.x as i32 } else { next_x };
            next_x += c.width as i32;
            Screen {
                card,
                crtc,
                info: DisplayInfo {
                    index: i as u32,
                    name: format!("Screen {}", i + 1),
                    width: c.width,
                    height: c.height,
                    x,
                    y: if placed { c.y as i32 } else { 0 },
                    primary: i == 0,
                },
            }
        })
        .collect())
}

/// Starts capturing `screen`; frames go to the returned stream.
pub fn capture(screen: Screen, max_fps: u32) -> Result<FrameStream> {
    let slot = FrameSlot::new();
    let producer = slot.clone();
    let interval = Duration::from_secs_f64(1.0 / max_fps.clamp(1, 240) as f64);
    let card = Card::open(&screen.card)?;
    thread::Builder::new().name("kms-capture".into()).spawn(move || {
        if let Err(e) = capture_loop(&card, screen.crtc, interval, &producer) {
            producer.fail(format!("{e:#}"));
        }
    })?;
    Ok(FrameStream::new(slot))
}

/// Framebuffers stay mapped between frames (compositors flip between two
/// or three), but not for long: a number can be given to a new framebuffer.
const KEEP_MAPPINGS: Duration = Duration::from_secs(2);
/// A display that stops showing anything (a mode change, the login screen
/// handing over to a desktop) gets this long to come back.
const GONE_FOR: Duration = Duration::from_secs(10);

struct Grabber<'a> {
    card: &'a Card,
    crtc: u32,
    mapped: Vec<(u32, Fb, Mapping)>,
    mapped_at: Instant,
}

impl Grabber<'_> {
    /// Copies the display's current picture into `out`: its size and pixel
    /// order, or `None` while the display shows nothing.
    fn grab(&mut self, out: &mut Vec<u8>) -> Result<Option<(u32, u32, PixelFormat)>> {
        let Some(crtc) = self.card.crtc(self.crtc)? else { return Ok(None) };
        if self.mapped_at.elapsed() > KEEP_MAPPINGS {
            self.mapped.clear();
        }
        if !self.mapped.iter().any(|(id, ..)| *id == crtc.fb) {
            let fb = self.card.framebuffer(crtc.fb)?;
            let mapping = self.card.map(&fb)?;
            if self.mapped.is_empty() {
                self.mapped_at = Instant::now();
            }
            self.mapped.push((crtc.fb, fb, mapping));
        }
        let (_, fb, mapping) = self.mapped.iter().find(|(id, ..)| *id == crtc.fb).expect("just mapped");
        // The mode can be larger than the framebuffer for a moment.
        let (width, height) = (crtc.width.min(fb.width.saturating_sub(crtc.x)), crtc.height.min(fb.height.saturating_sub(crtc.y)));
        if width == 0 || height == 0 {
            return Ok(None);
        }
        let format = copy_pixels(mapping.bytes(), fb, (crtc.x, crtc.y, width, height), out)?;
        Ok(Some((width, height, format)))
    }
}

fn capture_loop(card: &Card, crtc: u32, interval: Duration, slot: &FrameSlot) -> Result<()> {
    let mut grabber = Grabber { card, crtc, mapped: Vec::new(), mapped_at: Instant::now() };
    let mut next = Instant::now();
    // No "picture changed" signal here either: compare with the last frame
    // sent, so a still screen costs a copy and a compare, not an encode.
    let mut prev: Vec<u8> = Vec::new();
    let mut prev_shape = None;
    let mut shown = false;
    let mut gone_since: Option<Instant> = None;
    while !slot.is_closed() {
        let mut data = slot.buffer(prev.len());
        match grabber.grab(&mut data) {
            Ok(Some(shape)) => {
                gone_since = None;
                shown = true;
                if prev_shape == Some(shape) && data == prev {
                    slot.reuse(data);
                } else {
                    prev.clear();
                    prev.extend_from_slice(&data);
                    prev_shape = Some(shape);
                    let (width, height, format) = shape;
                    slot.put(RawFrame { width, height, stride: width * 4, format, data });
                }
            }
            // Nothing readable right now. If there never was, say why; a
            // picture that was there usually returns after a mode change.
            other => {
                slot.reuse(data);
                grabber.mapped.clear();
                let why = match other {
                    Err(e) if !shown => return Err(e),
                    Err(e) => format!("{e:#}"),
                    Ok(_) if !shown => bail!("that display isn't showing anything"),
                    Ok(_) => "the display stopped showing anything".to_string(),
                };
                if gone_since.get_or_insert_with(Instant::now).elapsed() > GONE_FOR {
                    bail!("{why}");
                }
            }
        }
        next += interval;
        match next.checked_duration_since(Instant::now()) {
            Some(wait) => thread::sleep(wait),
            None => next = Instant::now(),
        }
    }
    Ok(())
}

/// One picture of `screen`, for checking a machine: (width, height, RGBA).
pub fn snapshot(screen: &Screen) -> Result<(u32, u32, Vec<u8>)> {
    let card = Card::open(&screen.card)?;
    let mut grabber = Grabber { card: &card, crtc: screen.crtc, mapped: Vec::new(), mapped_at: Instant::now() };
    let mut data = Vec::new();
    let (width, height, format) = grabber.grab(&mut data)?.context("that display isn't showing anything")?;
    for pixel in data.as_chunks_mut::<4>().0 {
        if format == PixelFormat::Bgra {
            pixel.swap(0, 2);
        }
        pixel[3] = 255;
    }
    Ok((width, height, data))
}

/// What the capture would work with, for the log and for checking a
/// machine: each card's driver and each lit output's framebuffer.
pub fn describe() -> Vec<String> {
    let mut lines = Vec::new();
    for path in card_paths() {
        let card = match Card::open(&path) {
            Ok(card) => card,
            Err(e) => {
                lines.push(format!("{}: {e:#}", path.display()));
                continue;
            }
        };
        let ids = match card.crtc_ids() {
            Ok(ids) => ids,
            Err(e) => {
                lines.push(format!("{} ({}): {e:#}", path.display(), card.driver));
                continue;
            }
        };
        lines.push(format!("{} ({}): {} output(s)", path.display(), card.driver, ids.len()));
        for id in ids {
            let Ok(Some(crtc)) = card.crtc(id) else { continue };
            let detail = match card.framebuffer(crtc.fb) {
                Ok(fb) => {
                    let readable = match card.map(&fb) {
                        Ok(_) => "readable".to_string(),
                        Err(e) => format!("not readable: {e:#}"),
                    };
                    let layout = match fb.modifier {
                        Some(MOD_LINEAR) => "linear".to_string(),
                        Some(MOD_INVALID) | None => "layout not stated".to_string(),
                        Some(other) => format!("layout {other:#x}"),
                    };
                    format!("framebuffer {}x{} {} ({layout}), {readable}", fb.width, fb.height, fourcc_name(fb.format))
                }
                Err(e) => format!("{e:#}"),
            };
            lines.push(format!("  output {id}: {}x{} at {},{}; {detail}", crtc.width, crtc.height, crtc.x, crtc.y));
        }
    }
    if lines.is_empty() {
        lines.push("no graphics cards under /dev/dri".into());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_numbers_match_the_kernel() {
        // From drm.h / drm_mode.h on x86-64 and arm64.
        assert_eq!(GEM_CLOSE, 0x4008_6409);
        assert_eq!(DROP_MASTER, 0x0000_641f);
        assert_eq!(PRIME_HANDLE_TO_FD, 0xc00c_642d);
        assert_eq!(GETRESOURCES, 0xc040_64a0);
        assert_eq!(GETCRTC, 0xc068_64a1);
        assert_eq!(GETFB, 0xc01c_64ad);
        assert_eq!(MAP_DUMB, 0xc010_64b3);
        assert_eq!(GETFB2, 0xc068_64ce);
    }

    fn fb(width: u32, height: u32, pitch: u32, format: u32) -> Fb {
        Fb { width, height, pitch, offset: 0, format, modifier: None, handle: 1 }
    }

    #[test]
    fn copies_the_displays_part_without_row_padding() {
        // 3x2 framebuffer, 16 bytes a row (4 of padding); the display shows
        // the 2x2 on the right.
        let mut src = vec![0u8; 32];
        for (row, pixels) in [[1u8, 2, 3], [4, 5, 6]].iter().enumerate() {
            for (column, pixel) in pixels.iter().enumerate() {
                src[row * 16 + column * 4] = *pixel;
            }
        }
        let mut out = Vec::new();
        let format = copy_pixels(&src, &fb(3, 2, 16, XR24), (1, 0, 2, 2), &mut out).unwrap();
        assert_eq!(format, PixelFormat::Bgra);
        assert_eq!(out.len(), 16);
        assert_eq!([out[0], out[4], out[8], out[12]], [2, 3, 5, 6]);
        assert_eq!(copy_pixels(&src, &fb(3, 2, 16, XB24), (0, 0, 3, 2), &mut out).unwrap(), PixelFormat::Rgba);
    }

    #[test]
    fn refuses_a_picture_outside_the_framebuffer() {
        let src = vec![0u8; 32];
        let mut out = Vec::new();
        assert!(copy_pixels(&src, &fb(3, 2, 16, XR24), (2, 0, 2, 2), &mut out).is_err());
        // A description larger than the memory behind it.
        assert!(copy_pixels(&src, &fb(3, 4, 16, XR24), (0, 0, 3, 4), &mut out).is_err());
    }

    #[test]
    fn converts_16_and_30_bit_pixels() {
        let mut out = Vec::new();
        // Pure red, then pure blue, in 5-6-5.
        let src = [0x00, 0xf8, 0x1f, 0x00];
        copy_pixels(&src, &fb(2, 1, 4, RG16), (0, 0, 2, 1), &mut out).unwrap();
        assert_eq!(out, [0, 0, 255, 255, 255, 0, 0, 255]);
        // Full red in 2-10-10-10 (red in the top channel).
        let src = (0x3ffu32 << 20).to_le_bytes();
        copy_pixels(&src, &fb(1, 1, 4, XR30), (0, 0, 1, 1), &mut out).unwrap();
        assert_eq!(out, [0, 0, 255, 255]);
    }
}
