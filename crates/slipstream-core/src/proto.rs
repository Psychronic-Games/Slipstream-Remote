//! Wire protocol for viewer ⇄ host sessions.
//!
//! Every message is `postcard`-encoded and length-prefixed (see [`crate::framing`]).

use serde::{Deserialize, Serialize};

/// Bumped whenever the session protocol changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 1;

/// ALPN for remote-control sessions (viewer → host).
pub const ALPN_REMOTE: &[u8] = b"slipstream/remote/1";

// Stream layout: the viewer's first bi-stream is the control stream. The
// host's first uni-stream carries video. Every later bi-stream, from either
// side, is a file transfer.

/// Messages sent by the viewer on the control stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ViewerMsg {
    /// First message on a new session.
    Hello { version: u32, viewer_name: String },
    /// Answer to [`HostMsg::AuthChallenge`]: one SPAKE2 message and one key
    /// confirmation per challenge, in the same order.
    AuthResponse {
        pakes: Vec<Vec<u8>>,
        confirms: Vec<[u8; 32]>,
    },
    /// Start (or switch) streaming the given display.
    StartStream { display: u32, settings: StreamSettings },
    /// Change quality settings mid-stream.
    SetSettings(StreamSettings),
    /// The viewer finished decoding frame `seq`.
    FrameAck { seq: u64 },
    /// The decoder lost sync; send an IDR frame.
    RequestKeyframe,
    Input(InputEvent),
    Clipboard(ClipboardData),
    Chat(String),
    Bye,
}

/// Messages sent by the host on the control stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostMsg {
    /// One SPAKE2 message per password the host accepts.
    AuthChallenge { pakes: Vec<Vec<u8>> },
    /// Mutual-authentication confirmation from the host.
    AuthOk { confirm: [u8; 32], session: SessionInfo },
    AuthFailed { reason: String, retry_after_secs: u32 },
    /// Displays changed (monitor plugged in, resolution change, …).
    Displays(Vec<DisplayInfo>),
    StreamStarted { display: u32, width: u32, height: u32 },
    Clipboard(ClipboardData),
    Chat(String),
    /// A non-fatal problem the viewer should show to the user.
    Notice(String),
    Bye { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub host_name: String,
    pub os: String,
    pub displays: Vec<DisplayInfo>,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    pub input: bool,
    pub clipboard: bool,
    pub file_transfer: bool,
    pub chat: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub index: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StreamSettings {
    /// Target bitrate in kilobits per second; 0 = automatic.
    pub bitrate_kbps: u32,
    pub max_fps: u32,
    /// Maximum frames in flight (sent but not yet acked).
    pub max_in_flight: u32,
}

impl Default for StreamSettings {
    fn default() -> Self {
        Self {
            bitrate_kbps: 0,
            max_fps: 60,
            max_in_flight: 2,
        }
    }
}

/// Header + payload of one encoded video frame on the video stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFrame {
    pub seq: u64,
    pub keyframe: bool,
    pub width: u32,
    pub height: u32,
    /// Host capture time, microseconds since stream start.
    pub timestamp_us: u64,
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    /// Absolute pointer position, normalized to the streamed display (0..1).
    MouseMove { x: f32, y: f32 },
    MouseButton { button: MouseButton, down: bool },
    /// Scroll, in "lines" (one notch of a classic mouse wheel = 1.0).
    Wheel { dx: f32, dy: f32 },
    /// Physical key by W3C `KeyboardEvent.code` name ("KeyA", "ShiftLeft", …).
    Key { code: String, down: bool },
    /// Release every key and button the viewer may have held (focus lost).
    ReleaseAll,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClipboardData {
    Text(String),
}

/// First message on a file-transfer bi-stream; raw file bytes follow.
/// Either side may open one at any time after authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHeader {
    pub name: String,
    pub size: u64,
}

/// Receiver's verdict, sent back on the same bi-stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileResult {
    Saved { name: String },
    Rejected(String),
}
