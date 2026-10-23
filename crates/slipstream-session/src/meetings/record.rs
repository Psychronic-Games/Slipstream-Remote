//! Recording a meeting to an MP4 (or Matroska) file: H.264 video of the large picture
//! (spotlight, pin, speaker or shared screen) at the chosen size and frame
//! rate, and everyone's voices (the other participants' mix and our own
//! microphone) as Opus. The MP4 is fragmented: a second at a time, so a
//! recording cut short by a crash still plays up to that point. Plays in
//! VLC, Windows' Media Player, browsers and mpv (not macOS QuickTime, which
//! has no Opus in MP4).
//!
//! The app hands over the picture to show (`picture`), the mixed sound
//! (`speakers`) and our own encoded voice (`own_voice`); a thread encodes and
//! writes, and finishes the file (its duration) when the recorder is dropped.
use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use slipstream_audio::opus;
use slipstream_codec::{Encoder, EncoderSettings, PixelFormat, RawFrame, YuvFrame};
use std::{
    collections::VecDeque,
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// The file format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Container {
    /// Fragmented MP4 (.mp4).
    #[default]
    Mp4,
    /// Matroska (.mkv).
    Mkv,
}

impl Container {
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
        }
    }
}

/// What a recording looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub kbps: u32,
    pub container: Container,
}

impl Default for Settings {
    fn default() -> Self {
        Self { width: 1280, height: 720, fps: 30, kbps: 3000, container: Container::Mp4 }
    }
}

/// Media timescale for video (ticks a second).
const VIDEO_SCALE: u32 = 90_000;

pub struct Recorder {
    picture: Arc<Mutex<Option<Arc<YuvFrame>>>>,
    speakers: mpsc::SyncSender<Vec<i16>>,
    voice: mpsc::SyncSender<Bytes>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Result<()>>>,
    pub path: PathBuf,
    pub started: Instant,
}

impl Recorder {
    /// Starts recording into `path` (created or replaced).
    pub fn start(path: &Path, settings: Settings) -> Result<Self> {
        ensure!(settings.width >= 16 && settings.height >= 16 && settings.fps >= 1, "bad recording settings");
        let settings = Settings { width: settings.width & !1, height: settings.height & !1, fps: settings.fps.min(60), ..settings };
        let file = File::create(path).with_context(|| format!("can't write {}", path.display()))?;
        let picture: Arc<Mutex<Option<Arc<YuvFrame>>>> = Arc::default();
        let (speakers, speakers_rx) = mpsc::sync_channel(64);
        let (voice, voice_rx) = mpsc::sync_channel(64);
        let stop = Arc::new(AtomicBool::new(false));
        let (shown, stopping) = (picture.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("meeting-recorder".into())
            .spawn(move || record(file, settings, shown, speakers_rx, voice_rx, stopping))?;
        Ok(Self { picture, speakers, voice, stop, thread: Some(thread), path: path.into(), started: Instant::now() })
    }

    /// The picture to record from now on (None: black).
    pub fn picture(&self, frame: Option<Arc<YuvFrame>>) {
        *self.picture.lock().unwrap() = frame;
    }

    /// 20 ms of the other participants' mixed sound (48 kHz stereo).
    pub fn speakers(&self, pcm: &[i16]) {
        let _ = self.speakers.try_send(pcm.to_vec());
    }

    /// Where the mixed sound goes (for `media::Sound::tap`).
    pub fn speakers_input(&self) -> mpsc::SyncSender<Vec<i16>> {
        self.speakers.clone()
    }

    /// Where our own Opus voice packets go (for `Publisher::tap_audio`).
    pub fn voice_input(&self) -> mpsc::SyncSender<Bytes> {
        self.voice.clone()
    }

    /// Finishes the file. Returns its path, or why it couldn't be completed.
    pub fn finish(mut self) -> Result<PathBuf> {
        self.stop.store(true, Ordering::Release);
        match self.thread.take().map(|t| t.join()) {
            Some(Ok(result)) => result.map(|()| self.path.clone()),
            Some(Err(_)) => anyhow::bail!("the recorder stopped unexpectedly"),
            None => Ok(self.path.clone()),
        }
    }

    /// Stops after an error the writer met (e.g. the disk filled up).
    pub fn failed(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| t.is_finished())
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn record(
    file: File,
    settings: Settings,
    picture: Arc<Mutex<Option<Arc<YuvFrame>>>>,
    speakers: mpsc::Receiver<Vec<i16>>,
    voice: mpsc::Receiver<Bytes>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let Settings { width, height, fps, kbps, .. } = settings;
    let mut encoder = Encoder::new(EncoderSettings { bitrate_kbps: kbps, max_fps: fps as f32 });
    let mut opus_out = opus::Encoder::new(96_000)?;
    let mut own = opus::Decoder::new()?;
    let mut canvas = vec![0u8; (width * height * 4) as usize];
    // A keyframe every two seconds, so players can seek.
    let keyframe_every = fps as u64 * 2;

    // The first frame is a keyframe: its SPS/PPS go in the file's header.
    paint(picture.lock().unwrap().as_deref(), &mut canvas, width, height);
    let first = encode(&mut encoder, &canvas, width, height, true)?;
    let mut out = Writer::create(file, settings, &first.data)?;
    out.video(annexb_to_avcc(&first.data), 0, true)?;

    let start = Instant::now();
    let (mut frames, mut audio_frames) = (1u64, 0u64);
    let mut mine: VecDeque<[i16; opus::FRAME_SAMPLES]> = VecDeque::new();
    let mut heard: VecDeque<Vec<i16>> = VecDeque::new();
    while !stop.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(5));
        for packet in voice.try_iter() {
            let mut pcm = [0i16; opus::FRAME_SAMPLES];
            if own.decode(Some(&packet), &mut pcm).is_ok() {
                mine.push_back(pcm);
            }
        }
        heard.extend(speakers.try_iter());
        let now = start.elapsed().as_millis() as u64;
        // Sound: one 20 ms frame per 20 ms of recording, whatever arrived.
        while (audio_frames + 1) * 20 <= now {
            let mut mix = heard.pop_front().unwrap_or_else(|| vec![0; opus::FRAME_SAMPLES]);
            mix.resize(opus::FRAME_SAMPLES, 0);
            if let Some(voice) = mine.pop_front() {
                for (m, v) in mix.iter_mut().zip(voice) {
                    *m = m.saturating_add(v);
                }
            }
            out.audio(opus_out.encode(&mix)?.to_vec(), audio_frames)?;
            audio_frames += 1;
            // Don't let late arrivals build up a delay.
            heard.truncate(4);
            mine.truncate(4);
        }
        // Picture: at the chosen frame rate.
        if frames * 1000 / fps as u64 <= now {
            paint(picture.lock().unwrap().as_deref(), &mut canvas, width, height);
            let encoded = encode(&mut encoder, &canvas, width, height, frames.is_multiple_of(keyframe_every))?;
            out.video(annexb_to_avcc(&encoded.data), frames, encoded.keyframe)?;
            frames += 1;
        }
        out.tick()?;
    }
    out.finish(start.elapsed().as_millis() as f64)
}

/// The chosen container.
enum Writer {
    Mp4(Mp4),
    Mkv(Mkv),
}

impl Writer {
    fn create(file: File, settings: Settings, first_keyframe: &[u8]) -> Result<Self> {
        Ok(match settings.container {
            Container::Mp4 => Writer::Mp4(Mp4::create(file, settings, first_keyframe)?),
            Container::Mkv => Writer::Mkv(Mkv::create(file, settings, first_keyframe)?),
        })
    }

    /// Video frame number `n`.
    fn video(&mut self, data: Vec<u8>, n: u64, keyframe: bool) -> Result<()> {
        match self {
            Writer::Mp4(m) => {
                let ticks = VIDEO_SCALE / m.fps;
                m.video(data, ticks, keyframe);
                Ok(())
            }
            Writer::Mkv(m) => {
                let ms = n * 1000 / m.fps as u64;
                m.block(1, ms, keyframe, &data)
            }
        }
    }

    /// 20 ms sound frame number `n`.
    fn audio(&mut self, data: Vec<u8>, n: u64) -> Result<()> {
        match self {
            Writer::Mp4(m) => {
                m.audio(data);
                Ok(())
            }
            Writer::Mkv(m) => m.block(2, n * 20, true, &data),
        }
    }

    /// MP4 writes a fragment about every second.
    fn tick(&mut self) -> Result<()> {
        match self {
            Writer::Mp4(m) => m.maybe_flush(false),
            Writer::Mkv(_) => Ok(()),
        }
    }

    fn finish(self, duration_ms: f64) -> Result<()> {
        match self {
            Writer::Mp4(m) => m.finish(),
            Writer::Mkv(m) => m.finish(duration_ms),
        }
    }
}

fn encode(encoder: &mut Encoder, canvas: &[u8], width: u32, height: u32, keyframe: bool) -> Result<slipstream_codec::EncodedFrame> {
    let frame = RawFrame { width, height, stride: width * 4, format: PixelFormat::Rgba, data: canvas.to_vec() };
    encoder.encode(&frame, keyframe)?.context("the encoder skipped a recording frame")
}

/// Draws `frame` fitted (letterboxed) into the RGBA canvas, black around it.
fn paint(frame: Option<&YuvFrame>, canvas: &mut [u8], width: u32, height: u32) {
    canvas.as_chunks_mut::<4>().0.iter_mut().for_each(|p| *p = [0, 0, 0, 255]);
    let Some(f) = frame.filter(|f| f.width > 1 && f.height > 1) else { return };
    let scale = (width as f32 / f.width as f32).min(height as f32 / f.height as f32);
    let (w, h) = ((f.width as f32 * scale) as u32, (f.height as f32 * scale) as u32);
    let (x0, y0) = ((width - w) / 2, (height - h) / 2);
    for y in 0..h {
        let sy = ((y as f32 + 0.5) / scale) as usize;
        let sy = sy.min(f.height as usize - 1);
        let row = ((y0 + y) * width * 4) as usize;
        for x in 0..w {
            let sx = (((x as f32 + 0.5) / scale) as usize).min(f.width as usize - 1);
            // The viewer's conversion: BT.601 limited range.
            let yy = f.y[sy * f.y_stride as usize + sx] as f32 - 16.0;
            let ci = (sy / 2) * f.uv_stride as usize + sx / 2;
            let (u, v) = (f.u[ci] as f32 - 128.0, f.v[ci] as f32 - 128.0);
            let o = row + ((x0 + x) * 4) as usize;
            canvas[o] = (1.164 * yy + 1.596 * v).clamp(0.0, 255.0) as u8;
            canvas[o + 1] = (1.164 * yy - 0.392 * u - 0.813 * v).clamp(0.0, 255.0) as u8;
            canvas[o + 2] = (1.164 * yy + 2.017 * u).clamp(0.0, 255.0) as u8;
        }
    }
}

/// The NAL units of an Annex-B access unit.
fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut units = Vec::new();
    for (n, &s) in starts.iter().enumerate() {
        let mut end = starts.get(n + 1).map_or(data.len(), |next| next - 3);
        // A four-byte start code leaves one more zero behind.
        while end > s && data[end - 1] == 0 {
            end -= 1;
        }
        if end > s {
            units.push(&data[s..end]);
        }
    }
    units
}

/// Matroska stores H.264 as length-prefixed NAL units.
fn annexb_to_avcc(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    for unit in nal_units(data) {
        out.extend_from_slice(&(unit.len() as u32).to_be_bytes());
        out.extend_from_slice(unit);
    }
    out
}

/// AVCDecoderConfigurationRecord from the first keyframe's SPS and PPS.
fn avc_config(keyframe: &[u8]) -> Result<Vec<u8>> {
    let units = nal_units(keyframe);
    let sps = units.iter().find(|u| u[0] & 31 == 7).context("the first frame has no SPS")?;
    let pps = units.iter().find(|u| u[0] & 31 == 8).context("the first frame has no PPS")?;
    ensure!(sps.len() >= 4, "short SPS");
    let mut c = vec![1, sps[1], sps[2], sps[3], 0xff, 0xe1];
    c.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    c.extend_from_slice(sps);
    c.push(1);
    c.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    c.extend_from_slice(pps);
    Ok(c)
}

/// An ISO BMFF box.
fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

/// A "full box": version and flags first.
fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version];
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(kind, &b)
}

const MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

struct Sample {
    data: Vec<u8>,
    duration: u32,
    keyframe: bool,
}

/// Fragmented MP4: the header (no samples) first, then a movie fragment
/// (moof + mdat) about every second.
struct Mp4 {
    out: BufWriter<File>,
    video: Vec<Sample>,
    audio: Vec<Sample>,
    /// Decode time of the next fragment's first sample, per track.
    video_time: u64,
    audio_time: u64,
    sequence: u32,
    fps: u32,
}

impl Mp4 {
    fn create(file: File, s: Settings, first_keyframe: &[u8]) -> Result<Self> {
        let mut out = BufWriter::new(file);
        let mut ftyp = b"isom".to_vec();
        ftyp.extend_from_slice(&0x200u32.to_be_bytes());
        for brand in [b"isom", b"iso6", b"avc1", b"mp41"] {
            ftyp.extend_from_slice(brand);
        }
        out.write_all(&bx(b"ftyp", &ftyp))?;

        let mut mvhd = vec![0u8; 8]; // creation, modification
        mvhd.extend_from_slice(&1000u32.to_be_bytes()); // timescale
        mvhd.extend_from_slice(&0u32.to_be_bytes()); // duration: in the fragments
        mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate
        mvhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
        mvhd.extend_from_slice(&[0; 10]);
        MATRIX.iter().for_each(|m| mvhd.extend_from_slice(&m.to_be_bytes()));
        mvhd.extend_from_slice(&[0; 24]);
        mvhd.extend_from_slice(&3u32.to_be_bytes()); // next track ID

        // Video sample entry: avc1 with the SPS/PPS.
        let mut avc1 = vec![0; 6];
        avc1.extend_from_slice(&1u16.to_be_bytes()); // data reference index
        avc1.extend_from_slice(&[0; 16]);
        avc1.extend_from_slice(&(s.width as u16).to_be_bytes());
        avc1.extend_from_slice(&(s.height as u16).to_be_bytes());
        avc1.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        avc1.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        avc1.extend_from_slice(&0u32.to_be_bytes());
        avc1.extend_from_slice(&1u16.to_be_bytes()); // frame count
        avc1.extend_from_slice(&[0; 32]); // compressor name
        avc1.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
        avc1.extend_from_slice(&(-1i16).to_be_bytes());
        avc1.extend(bx(b"avcC", &avc_config(first_keyframe)?));
        // Audio sample entry: Opus with its dOps.
        let mut opus = vec![0; 6];
        opus.extend_from_slice(&1u16.to_be_bytes());
        opus.extend_from_slice(&[0; 8]);
        opus.extend_from_slice(&2u16.to_be_bytes()); // channels
        opus.extend_from_slice(&16u16.to_be_bytes()); // sample size
        opus.extend_from_slice(&[0; 4]);
        opus.extend_from_slice(&(48_000u32 << 16).to_be_bytes());
        let mut dops = vec![0, 2];
        dops.extend_from_slice(&312u16.to_be_bytes()); // pre-skip
        dops.extend_from_slice(&48_000u32.to_be_bytes());
        dops.extend_from_slice(&[0, 0, 0]); // gain, mapping family 0
        opus.extend(bx(b"dOps", &dops));

        let trak = |id: u32, scale: u32, video: bool, entry: Vec<u8>| -> Vec<u8> {
            let mut tkhd = vec![0u8; 8];
            tkhd.extend_from_slice(&id.to_be_bytes());
            tkhd.extend_from_slice(&[0; 4]);
            tkhd.extend_from_slice(&0u32.to_be_bytes()); // duration
            tkhd.extend_from_slice(&[0; 8]);
            tkhd.extend_from_slice(&[0; 4]); // layer, alternate group
            tkhd.extend_from_slice(&(if video { 0u16 } else { 0x0100 }).to_be_bytes());
            tkhd.extend_from_slice(&[0; 2]);
            MATRIX.iter().for_each(|m| tkhd.extend_from_slice(&m.to_be_bytes()));
            let (w, h) = if video { (s.width << 16, s.height << 16) } else { (0, 0) };
            tkhd.extend_from_slice(&w.to_be_bytes());
            tkhd.extend_from_slice(&h.to_be_bytes());
            let mut mdhd = vec![0u8; 8];
            mdhd.extend_from_slice(&scale.to_be_bytes());
            mdhd.extend_from_slice(&0u32.to_be_bytes());
            mdhd.extend_from_slice(&0x55C4u16.to_be_bytes()); // language: und
            mdhd.extend_from_slice(&[0; 2]);
            let mut hdlr = vec![0u8; 4];
            hdlr.extend_from_slice(if video { b"vide" } else { b"soun" });
            hdlr.extend_from_slice(&[0; 12]);
            hdlr.extend_from_slice(if video { b"Video\0" } else { b"Sound\0" });
            let header = if video { full(b"vmhd", 0, 1, &[0; 8]) } else { full(b"smhd", 0, 0, &[0; 4]) };
            let dinf = bx(b"dinf", &full(b"dref", 0, 0, &[&1u32.to_be_bytes()[..], &full(b"url ", 0, 1, &[])].concat()));
            let mut stsd = 1u32.to_be_bytes().to_vec();
            stsd.extend(bx(if video { b"avc1" } else { b"Opus" }, &entry));
            let empty = 0u32.to_be_bytes();
            let stbl = bx(
                b"stbl",
                &[
                    full(b"stsd", 0, 0, &stsd),
                    full(b"stts", 0, 0, &empty),
                    full(b"stsc", 0, 0, &empty),
                    full(b"stsz", 0, 0, &[0u8; 8]),
                    full(b"stco", 0, 0, &empty),
                ]
                .concat(),
            );
            let minf = bx(b"minf", &[header, dinf, stbl].concat());
            let mdia = bx(b"mdia", &[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf].concat());
            bx(b"trak", &[full(b"tkhd", 0, 3, &tkhd), mdia].concat())
        };
        let trex = |id: u32| {
            let mut t = id.to_be_bytes().to_vec();
            t.extend_from_slice(&1u32.to_be_bytes()); // sample description index
            t.extend_from_slice(&[0; 12]);
            full(b"trex", 0, 0, &t)
        };
        let moov = bx(
            b"moov",
            &[
                full(b"mvhd", 0, 0, &mvhd),
                trak(1, VIDEO_SCALE, true, avc1),
                trak(2, 48_000, false, opus),
                bx(b"mvex", &[trex(1), trex(2)].concat()),
            ]
            .concat(),
        );
        out.write_all(&moov)?;
        out.flush()?;
        Ok(Self { out, video: Vec::new(), audio: Vec::new(), video_time: 0, audio_time: 0, sequence: 0, fps: s.fps })
    }

    fn video(&mut self, data: Vec<u8>, duration: u32, keyframe: bool) {
        self.video.push(Sample { data, duration, keyframe });
    }

    fn audio(&mut self, data: Vec<u8>) {
        self.audio.push(Sample { data, duration: opus::FRAME as u32, keyframe: true });
    }

    /// Writes a fragment once there's a second of video (or `now`).
    fn maybe_flush(&mut self, now: bool) -> Result<()> {
        if self.video.is_empty() && self.audio.is_empty() {
            return Ok(());
        }
        if !now && (self.video.len() as u32) < self.fps {
            return Ok(());
        }
        self.sequence += 1;
        let tracks: [(u32, Vec<Sample>, u64); 2] = [
            (1, std::mem::take(&mut self.video), self.video_time),
            (2, std::mem::take(&mut self.audio), self.audio_time),
        ];
        // trun data offsets point into the mdat after this moof; build the
        // moof once to measure it, then again with the real offsets.
        let build = |base: u32| -> Vec<u8> {
            let mut trafs = Vec::new();
            let mut offset = base;
            for (id, samples, time) in &tracks {
                if samples.is_empty() {
                    continue;
                }
                let tfhd = full(b"tfhd", 0, 0x02_0000, &id.to_be_bytes()); // default-base-is-moof
                let tfdt = full(b"tfdt", 1, 0, &time.to_be_bytes());
                let mut trun = (samples.len() as u32).to_be_bytes().to_vec();
                trun.extend_from_slice(&offset.to_be_bytes());
                for s in samples {
                    trun.extend_from_slice(&s.duration.to_be_bytes());
                    trun.extend_from_slice(&(s.data.len() as u32).to_be_bytes());
                    // Non-keyframes depend on others and aren't sync samples.
                    let flags: u32 = if s.keyframe { 0x0200_0000 } else { 0x0101_0000 };
                    trun.extend_from_slice(&flags.to_be_bytes());
                    offset += s.data.len() as u32;
                }
                // data offset, duration, size and flags per sample.
                let trun = full(b"trun", 0, 0x0001 | 0x0100 | 0x0200 | 0x0400, &trun);
                trafs.extend(bx(b"traf", &[tfhd, tfdt, trun].concat()));
            }
            bx(b"moof", &[full(b"mfhd", 0, 0, &self.sequence.to_be_bytes()), trafs].concat())
        };
        let moof_len = build(0).len() as u32;
        let moof = build(moof_len + 8);
        let payload: usize = tracks.iter().flat_map(|t| &t.1).map(|s| s.data.len()).sum();
        self.out.write_all(&moof)?;
        self.out.write_all(&((payload + 8) as u32).to_be_bytes())?;
        self.out.write_all(b"mdat")?;
        for (_, samples, _) in &tracks {
            for s in samples {
                self.out.write_all(&s.data)?;
            }
        }
        self.out.flush()?;
        self.video_time += tracks[0].1.iter().map(|s| s.duration as u64).sum::<u64>();
        self.audio_time += tracks[1].1.iter().map(|s| s.duration as u64).sum::<u64>();
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        self.maybe_flush(true)?;
        self.out.flush()?;
        self.out.get_ref().sync_all()?;
        Ok(())
    }
}

/// Matroska element: ID bytes, size, contents.
fn element(id: u32, body: &[u8]) -> Vec<u8> {
    let bytes = id.to_be_bytes();
    let mut out: Vec<u8> = bytes.iter().skip_while(|b| **b == 0).copied().collect();
    // Sizes are always 8 bytes: simple, and room for anything.
    out.extend_from_slice(&(body.len() as u64 | 0x0100_0000_0000_0000).to_be_bytes());
    out.extend_from_slice(body);
    out
}

fn uint(id: u32, n: u64) -> Vec<u8> {
    element(id, &n.to_be_bytes())
}

fn float(id: u32, f: f64) -> Vec<u8> {
    element(id, &f.to_be_bytes())
}

fn text(id: u32, s: &str) -> Vec<u8> {
    element(id, s.as_bytes())
}

/// Matroska: clusters written as they fill; the duration patched at the end.
struct Mkv {
    out: BufWriter<File>,
    /// File offset of the Duration float, filled in by `finish`.
    duration_at: u64,
    cluster: Vec<u8>,
    cluster_start: u64,
    fps: u32,
}

impl Mkv {
    fn create(file: File, s: Settings, first_keyframe: &[u8]) -> Result<Self> {
        use std::io::Seek;
        let mut out = BufWriter::new(file);
        let header = [uint(0x4286, 1), uint(0x42F7, 1), uint(0x42F2, 4), uint(0x42F3, 8), text(0x4282, "matroska"), uint(0x4287, 4), uint(0x4285, 2)].concat();
        out.write_all(&element(0x1A45_DFA3, &header))?;
        // A segment of unknown size: players read to the end.
        out.write_all(&[0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])?;
        let app = concat!("Slipstream Remote ", env!("CARGO_PKG_VERSION"));
        let info = element(0x1549_A966, &[uint(0x2AD7B1, 1_000_000), text(0x4D80, app), text(0x5741, app), float(0x4489, 0.0)].concat());
        let info_at = out.stream_position()?;
        out.write_all(&info)?;
        // The Duration float is the info's last 8 bytes.
        let duration_at = info_at + info.len() as u64 - 8;
        let video = [
            uint(0xD7, 1),
            uint(0x73C5, 1),
            uint(0x83, 1),
            text(0x86, "V_MPEG4/ISO/AVC"),
            element(0x63A2, &avc_config(first_keyframe)?),
            element(0xE0, &[uint(0xB0, s.width as u64), uint(0xBA, s.height as u64)].concat()),
        ]
        .concat();
        // OpusHead: version 1, 2 channels, 312 samples pre-skip, 48 kHz, no gain, family 0.
        let mut opus_head = b"OpusHead".to_vec();
        opus_head.extend_from_slice(&[1, 2]);
        opus_head.extend_from_slice(&312u16.to_le_bytes());
        opus_head.extend_from_slice(&48_000u32.to_le_bytes());
        opus_head.extend_from_slice(&[0, 0, 0]);
        let audio = [
            uint(0xD7, 2),
            uint(0x73C5, 2),
            uint(0x83, 2),
            text(0x86, "A_OPUS"),
            element(0x63A2, &opus_head),
            uint(0x56AA, 6_500_000),
            uint(0x56BB, 80_000_000),
            element(0xE1, &[float(0xB5, 48_000.0), uint(0x9F, 2)].concat()),
        ]
        .concat();
        out.write_all(&element(0x1654_AE6B, &[element(0xAE, &video), element(0xAE, &audio)].concat()))?;
        Ok(Self { out, duration_at, cluster: Vec::new(), cluster_start: 0, fps: s.fps })
    }

    /// A SimpleBlock for `track` at `ms` from the start.
    fn block(&mut self, track: u8, ms: u64, keyframe: bool, data: &[u8]) -> Result<()> {
        // A new cluster at each video keyframe (every two seconds).
        if self.cluster.is_empty() || (track == 1 && keyframe) || ms >= self.cluster_start + 30_000 {
            self.flush()?;
            self.cluster_start = ms;
            self.cluster = uint(0xE7, ms);
        }
        let relative = ms.saturating_sub(self.cluster_start).min(i16::MAX as u64) as i16;
        let mut body = vec![0x80 | track];
        body.extend_from_slice(&relative.to_be_bytes());
        body.push(if keyframe { 0x80 } else { 0 });
        body.extend_from_slice(data);
        self.cluster.extend(element(0xA3, &body));
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if !self.cluster.is_empty() {
            self.out.write_all(&element(0x1F43_B675, &self.cluster))?;
            self.out.flush()?;
            self.cluster.clear();
        }
        Ok(())
    }

    fn finish(mut self, duration_ms: f64) -> Result<()> {
        use std::io::{Seek, SeekFrom};
        self.flush()?;
        self.out.seek(SeekFrom::Start(self.duration_at))?;
        self.out.write_all(&duration_ms.to_be_bytes())?;
        self.out.flush()?;
        self.out.get_ref().sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_for(container: Container) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("meeting.{}", container.extension()));
        let rec = Recorder::start(&path, Settings { width: 640, height: 360, fps: 30, kbps: 1500, container }).unwrap();
        let (w, h) = (320u32, 240u32);
        let frame = YuvFrame {
            width: w,
            height: h,
            y: vec![120; (w * h) as usize],
            u: vec![90; (w * h / 4) as usize],
            v: vec![200; (w * h / 4) as usize],
            y_stride: w,
            uv_stride: w / 2,
        };
        rec.picture(Some(Arc::new(frame)));
        for i in 0..60 {
            let tone: Vec<i16> = (0..opus::FRAME_SAMPLES).map(|n| (((n + i * 1920) as f32 * 0.05).sin() * 8000.0) as i16).collect();
            rec.speakers(&tone);
            std::thread::sleep(Duration::from_millis(20));
        }
        let path = rec.finish().unwrap();
        // When ffprobe is installed, it must read both streams and their frames.
        if let Ok(out) = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-count_frames", "-show_entries", "stream=codec_name,nb_read_frames", "-of", "csv=p=0"])
            .arg(&path)
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            assert!(out.status.success(), "ffprobe: {}", String::from_utf8_lossy(&out.stderr));
            assert!(text.contains("h264") && text.contains("opus"), "{text}");
            let frames: Vec<u64> = text.lines().filter_map(|l| l.split(',').nth(1)?.trim().parse().ok()).collect();
            // About 36 pictures and 60 sound frames; fewer when the machine is busy.
            assert!(frames.len() == 2 && frames.iter().all(|n| *n > 10), "{text}");
            eprintln!("{container:?}: {}", text.replace('\n', " "));
        }
        std::fs::read(&path).unwrap()
    }

    #[test]
    fn records_playable_mp4_and_mkv() {
        let mp4 = record_for(Container::Mp4);
        assert_eq!(&mp4[4..8], b"ftyp");
        assert!(mp4.windows(4).any(|w| w == b"avcC") && mp4.windows(4).any(|w| w == b"dOps") && mp4.windows(4).any(|w| w == b"moof"));
        let mkv = record_for(Container::Mkv);
        assert_eq!(&mkv[..4], &[0x1A, 0x45, 0xDF, 0xA3]);
        assert!(mkv.windows(15).any(|w| w == b"V_MPEG4/ISO/AVC") && mkv.windows(6).any(|w| w == b"A_OPUS"));
    }
}
