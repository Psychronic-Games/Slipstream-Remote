//! Wire protocol for viewer ⇄ host sessions.
//!
//! Every message is `postcard`-encoded and length-prefixed (see [`crate::framing`]).

use serde::{Deserialize, Serialize};

/// Bumped whenever the session protocol changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 5;

/// Audio travels as QUIC datagrams (a lost one is concealed, never waited
/// for): `[AUDIO_DATAGRAM][u16 LE sequence][Opus packet]`, 20 ms each.
pub const AUDIO_DATAGRAM: u8 = 0xA1;

/// QUIC send priorities (higher goes first). Input, acks and control beat
/// video, and video beats file transfers, so a big copy can't freeze the
/// picture or the mouse. File streams keep the default, 0.
pub const PRIORITY_CONTROL: i32 = 20;
pub const PRIORITY_VIDEO: i32 = 10;

/// ALPN for remote-control sessions (viewer → host).
pub const ALPN_REMOTE: &[u8] = b"slipstream/remote/1";

// Stream layout: the viewer's first bi-stream is the control stream. The
// host's first uni-stream carries video. Every later bi-stream starts with a
// [`StreamOpen`] request (file browsing and transfer).

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
    /// Start or stop sending the host's sound.
    SetAudio(bool),
    /// Black out the host's own screen (privacy mode), or stop.
    SetPrivacy(bool),
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
    /// The video shows the host's mouse pointer, so the viewer can hide its own.
    pub cursor_in_video: bool,
    /// The host can share its sound (and allows it).
    pub audio: bool,
    /// The host can black out its own screen during the session.
    pub privacy: bool,
    /// The host allows port forwarding (`StreamOpen::Tunnel`).
    pub tunnels: bool,
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
    /// Files copied on the host: what they are, so the viewer can decide
    /// whether to fetch them for pasting.
    Files {
        items: Vec<ClipFile>,
        total_bytes: u64,
        file_count: u64,
    },
}

/// One top-level item of a copied file list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipFile {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Where an uploaded file should go on the receiving side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UploadTarget {
    /// The receiver's default folder (its Desktop).
    Default,
    /// A specific folder (absolute path on the receiver).
    Folder(String),
    /// Whatever folder is under the receiver's mouse pointer (e.g. an
    /// Explorer window), falling back to the Desktop.
    UnderCursor,
    /// A temporary folder for files being pasted (see
    /// [`StreamOpen::ClipboardFiles`]); one per paste.
    Clipboard { batch: u64 },
}

/// First message on every bi-stream after the control stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamOpen {
    /// Send a file. `rel_path` may contain `/`-separated subfolders, which
    /// are created inside the target. After [`StreamReply::Ready`], `size`
    /// raw bytes follow; the receiver answers [`StreamReply::Saved`].
    Upload { target: UploadTarget, rel_path: String, size: u64 },
    /// Read a file: [`StreamReply::File`] then its bytes.
    Download { path: String },
    /// List a folder (`None` = home): [`StreamReply::Listing`].
    List { path: Option<String> },
    /// Shortcut folders and drives: [`StreamReply::Places`].
    Places,
    /// Create a folder: [`StreamReply::Done`].
    MakeDir { path: String },
    /// Port forwarding: the host connects to `host:port` (as seen from the
    /// host) and, after [`StreamReply::Ready`], the stream carries that TCP
    /// connection's bytes both ways.
    Tunnel { host: String, port: u16 },
    /// Put the files uploaded to `UploadTarget::Clipboard { batch }` on the
    /// receiver's clipboard, ready to paste: [`StreamReply::Done`].
    ClipboardFiles { batch: u64 },
    /// The host's hardware and system: [`StreamReply::SystemInfo`]. (New
    /// variants go last: older hosts then just fail that one stream.)
    SystemInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamReply {
    Ready,
    Saved { path: String },
    Listing(DirListing),
    Places(Vec<Place>),
    File { size: u64 },
    Done,
    Error(String),
    SystemInfo(Vec<InfoSection>),
}

/// One group of facts about a computer ("Processor", "Storage"…), already
/// worded for display, so new facts need no protocol change.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InfoSection {
    pub title: String,
    /// (label, value) rows.
    pub rows: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirListing {
    /// Absolute, canonical path of the folder.
    pub path: String,
    pub parent: Option<String>,
    /// Path separator on that machine ("/" or "\\").
    pub sep: String,
    pub entries: Vec<DirEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Unix seconds, 0 if unknown.
    pub modified: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Place {
    pub label: String,
    pub path: String,
}
