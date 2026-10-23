//! The graphics-card encoders behind one interface, tried in order of
//! preference: NVIDIA's NVENC, then VA-API (Linux: AMD, Intel) or Media
//! Foundation (Windows: Intel, AMD and others). `SLIPSTREAM_HWENC=nvenc`,
//! `vaapi` or `mf` tries only that one (to test it on a machine that has
//! several); `SLIPSTREAM_NO_HWENC=1` turns them all off.
use anyhow::Result;

use crate::{EncodedFrame, PixelFormat, RawFrame, nvenc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Nvenc,
    #[cfg(target_os = "linux")]
    Vaapi,
    #[cfg(windows)]
    Mf,
}

impl Kind {
    /// The encoders to try here, best first.
    pub fn all() -> Vec<Kind> {
        let every = vec![
            Kind::Nvenc,
            #[cfg(target_os = "linux")]
            Kind::Vaapi,
            #[cfg(windows)]
            Kind::Mf,
        ];
        match std::env::var("SLIPSTREAM_HWENC") {
            Ok(only) => every.into_iter().filter(|k| k.name().eq_ignore_ascii_case(only.trim())).collect(),
            Err(_) => every,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Nvenc => "nvenc",
            #[cfg(target_os = "linux")]
            Kind::Vaapi => "vaapi",
            #[cfg(windows)]
            Kind::Mf => "mf",
        }
    }

    /// How the viewer's statistics name it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Nvenc => "video encoding (NVIDIA hardware)",
            #[cfg(target_os = "linux")]
            Kind::Vaapi => "video encoding (graphics hardware, VA-API)",
            #[cfg(windows)]
            Kind::Mf => "video encoding (graphics hardware, Media Foundation)",
        }
    }
}

pub(crate) enum Hardware {
    Nvenc(nvenc::NvEncoder),
    #[cfg(target_os = "linux")]
    Vaapi(crate::vaapi::VaEncoder),
    #[cfg(windows)]
    Mf(crate::mf::MfEncoder),
}

/// Calls the same method on whichever encoder this is.
macro_rules! each {
    ($self:expr, $hw:ident => $call:expr) => {
        match $self {
            Hardware::Nvenc($hw) => $call,
            #[cfg(target_os = "linux")]
            Hardware::Vaapi($hw) => $call,
            #[cfg(windows)]
            Hardware::Mf($hw) => $call,
        }
    };
}

impl Hardware {
    pub fn open(kind: Kind, width: u32, height: u32, fps: f32, kbps: u32, format: PixelFormat) -> Result<Self> {
        Ok(match kind {
            Kind::Nvenc => Hardware::Nvenc(nvenc::NvEncoder::new(width, height, fps, kbps, format)?),
            #[cfg(target_os = "linux")]
            Kind::Vaapi => Hardware::Vaapi(crate::vaapi::VaEncoder::new(width, height, fps, kbps, format)?),
            #[cfg(windows)]
            Kind::Mf => Hardware::Mf(crate::mf::MfEncoder::new(width, height, fps, kbps, format)?),
        })
    }

    pub fn kind(&self) -> Kind {
        match self {
            Hardware::Nvenc(_) => Kind::Nvenc,
            #[cfg(target_os = "linux")]
            Hardware::Vaapi(_) => Kind::Vaapi,
            #[cfg(windows)]
            Hardware::Mf(_) => Kind::Mf,
        }
    }

    pub fn device(&self) -> &str {
        each!(self, hw => hw.device())
    }

    pub fn size(&self) -> (u32, u32) {
        each!(self, hw => hw.size())
    }

    pub fn format(&self) -> PixelFormat {
        each!(self, hw => hw.format())
    }

    pub fn set_bitrate(&mut self, fps: f32, kbps: u32) -> Result<()> {
        each!(self, hw => hw.set_bitrate(fps, kbps))
    }

    pub fn encode(&mut self, frame: &RawFrame, force_keyframe: bool) -> Result<EncodedFrame> {
        each!(self, hw => hw.encode(frame, force_keyframe))
    }
}
