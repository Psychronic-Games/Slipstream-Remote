//! Remote audio: the Opus codec, capturing what the host computer plays,
//! and playing it on the viewer.

pub mod buffer;
mod io;
pub mod opus;
#[cfg(target_os = "linux")]
mod pulse;
pub mod voice;

pub use io::{Capture, Player, capture_supported};
