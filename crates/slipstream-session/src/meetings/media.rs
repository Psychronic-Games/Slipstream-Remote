//! Bounded media workers owned by the meeting UI, never by a Windows service.
use super::{AudioPacket, Publisher, Video};
use anyhow::Result;
use slipstream_audio::{Player, buffer::Jitter, opus};
use slipstream_codec::{Decoder, Encoder, EncoderSettings, YuvFrame};
use slipstream_platform::FrameStream;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;
type Preview = Arc<Mutex<Option<(u64, Arc<YuvFrame>)>>>;
type DecodedFrames = Arc<Mutex<HashMap<u64, (u64, Arc<YuvFrame>)>>>;

pub struct VideoSender {
    stop: Arc<AtomicBool>,
    pub keyframe: Arc<AtomicBool>,
    pub preview: Preview,
    pub error: Arc<Mutex<Option<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for VideoSender {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let start = std::time::Instant::now();
            while !thread.is_finished() && start.elapsed() < Duration::from_millis(500) {
                std::thread::sleep(Duration::from_millis(2));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}
impl VideoSender {
    pub fn start(capture: FrameStream, publisher: Publisher) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(true));
        let preview = Arc::new(Mutex::new(None));
        let error = Arc::new(Mutex::new(None));
        let (flag, key, view, err) = (
            stop.clone(),
            keyframe.clone(),
            preview.clone(),
            error.clone(),
        );
        let thread = std::thread::Builder::new()
            .name("meeting-video-encode".into())
            .spawn(move || {
                let work = (|| {
                    let mut encoder = Encoder::new(EncoderSettings {
                        bitrate_kbps: 800,
                        max_fps: 15.0,
                    });
                    let mut decoder = Decoder::new()?;
                    let generation = rand::random::<u64>();
                    let mut seq = 0;
                    let mut last_key = std::time::Instant::now() - Duration::from_secs(3);
                    let mut next = std::time::Instant::now();
                    while !flag.load(Ordering::Acquire) {
                        let Some(frame) = capture
                            .next(Duration::from_millis(100))
                            .map_err(anyhow::Error::msg)?
                        else {
                            continue;
                        };
                        if std::time::Instant::now() < next {
                            capture.recycle(frame);
                            continue;
                        }
                        next = std::time::Instant::now() + Duration::from_millis(66);
                        let scale = (1280.0 / frame.width.max(frame.height) as f32).min(1.0);
                        encoder.set_scale(scale);
                        let force_keyframe = key.swap(false, Ordering::AcqRel)
                            || last_key.elapsed() > Duration::from_secs(2);
                        if force_keyframe {
                            last_key = std::time::Instant::now();
                        }
                        let encoded = encoder.encode(&frame, force_keyframe)?;
                        capture.recycle(frame);
                        if let Some(encoded) = encoded {
                            seq += 1;
                            if let Some(decoded) = decoder.decode(&encoded.data)? {
                                *view.lock().unwrap() =
                                    Some((generation.wrapping_add(seq), Arc::new(decoded)));
                            }
                            if !publisher.video(Video {
                                participant: 0,
                                generation,
                                seq,
                                keyframe: encoded.keyframe,
                                width: encoded.width,
                                height: encoded.height,
                                data: encoded.data,
                            }) {
                                key.store(true, Ordering::Release);
                            }
                        }
                    }
                    anyhow::Ok(())
                })();
                if let Err(e) = work {
                    *err.lock().unwrap() = Some(format!("{e:#}"));
                }
            })?;
        Ok(Self {
            stop,
            keyframe,
            preview,
            error,
            thread: Some(thread),
        })
    }
}
pub struct VideoReceiver {
    tx: mpsc::Sender<Arc<Video>>,
    pub frames: DecodedFrames,
    members: Arc<Mutex<Vec<u64>>>,
}
impl VideoReceiver {
    pub fn new(request: impl Fn(u64) + Send + 'static) -> Result<Self> {
        let (tx, mut rx) = mpsc::channel::<Arc<Video>>(4);
        let frames = Arc::new(Mutex::new(HashMap::new()));
        let views = frames.clone();
        let members = Arc::new(Mutex::new(Vec::new()));
        let active = members.clone();
        std::thread::Builder::new()
            .name("meeting-video-decode".into())
            .spawn(move || {
                let mut decoders: HashMap<u64, (u64, u64, Decoder)> = HashMap::new();
                let mut serial = 0u64;
                while let Some(frame) = rx.blocking_recv() {
                    let allowed = active.lock().unwrap().clone();
                    decoders.retain(|id, _| allowed.contains(id));
                    views.lock().unwrap().retain(|id, _| allowed.contains(id));
                    if !allowed.contains(&frame.participant) || frame.validate().is_err() {
                        continue;
                    }
                    let continuous =
                        decoders
                            .get(&frame.participant)
                            .is_some_and(|(generation, seq, _)| {
                                *generation == frame.generation
                                    && seq.checked_add(1) == Some(frame.seq)
                            });
                    if !continuous {
                        decoders.remove(&frame.participant);
                        if !frame.keyframe {
                            request(frame.participant);
                            continue;
                        }
                        let Ok(decoder) = Decoder::new() else {
                            continue;
                        };
                        decoders.insert(
                            frame.participant,
                            (frame.generation, frame.seq - 1, decoder),
                        );
                    }
                    let Some((_, seq, decoder)) = decoders.get_mut(&frame.participant) else {
                        continue;
                    };
                    *seq = frame.seq;
                    match decoder.decode(&frame.data) {
                        Ok(Some(decoded)) => {
                            serial += 1;
                            views
                                .lock()
                                .unwrap()
                                .insert(frame.participant, (serial, Arc::new(decoded)));
                        }
                        Ok(None) => {}
                        Err(_) => {
                            decoders.remove(&frame.participant);
                            request(frame.participant);
                        }
                    }
                }
            })?;
        Ok(Self {
            tx,
            frames,
            members,
        })
    }
    pub fn subscribe(&self, ids: Vec<u64>) {
        self.frames.lock().unwrap().retain(|id, _| ids.contains(id));
        *self.members.lock().unwrap() = ids;
    }
    pub fn push(&self, frame: Arc<Video>) {
        let _ = self.tx.try_send(frame);
    }
}
pub struct Sound {
    pub player: Arc<Player>,
    tx: std::sync::mpsc::SyncSender<AudioPacket>,
    members: Arc<Mutex<Vec<u64>>>,
    /// How loud each participant is right now, 0–1 (see [`Sound::levels`]).
    levels: Arc<Mutex<HashMap<u64, f32>>>,
    /// A copy of the mix for a recording.
    tap: Arc<Mutex<Option<std::sync::mpsc::SyncSender<Vec<i16>>>>>,
    stop: Arc<AtomicBool>,
    _guard: slipstream_audio::voice::PlaybackGuard,
}
impl Drop for Sound {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
impl Sound {
    pub fn start() -> Result<Self> {
        let player = Arc::new(Player::meeting()?);
        let output = player.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioPacket>(128);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let members = Arc::new(Mutex::new(Vec::new()));
        let active = members.clone();
        let levels = Arc::new(Mutex::new(HashMap::new()));
        let loudness = levels.clone();
        let tap: Arc<Mutex<Option<std::sync::mpsc::SyncSender<Vec<i16>>>>> = Arc::default();
        let recording = tap.clone();
        std::thread::Builder::new()
            .name("meeting-audio-mix".into())
            .spawn(move || {
                struct Speaker {
                    decoder: opus::Decoder,
                    jitter: Jitter,
                    seq: Option<u16>,
                }
                let mut speakers: HashMap<u64, Speaker> = HashMap::new();
                let mut next = std::time::Instant::now();
                while !flag.load(Ordering::Acquire) {
                    let allowed = active.lock().unwrap().clone();
                    speakers.retain(|id, _| allowed.contains(id));
                    for packet in rx.try_iter().take(128) {
                        if !allowed.contains(&packet.participant) {
                            continue;
                        }
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            speakers.entry(packet.participant)
                        {
                            let Ok(decoder) = opus::Decoder::new() else {
                                continue;
                            };
                            entry.insert(Speaker {
                                decoder,
                                jitter: Jitter::default(),
                                seq: None,
                            });
                        }
                        let speaker = speakers.get_mut(&packet.participant).unwrap();
                        if let Some(seq) = speaker.seq {
                            let gap = packet.sequence.wrapping_sub(seq);
                            if gap == 0 || gap > 32768 {
                                continue;
                            }
                            for _ in 1..gap.min(4) {
                                let mut pcm = [0; opus::FRAME_SAMPLES];
                                if speaker.decoder.decode(None, &mut pcm).is_ok() {
                                    speaker.jitter.push(&pcm);
                                }
                            }
                        }
                        speaker.seq = Some(packet.sequence);
                        let mut pcm = [0; opus::FRAME_SAMPLES];
                        if speaker.decoder.decode(Some(&packet.data), &mut pcm).is_ok() {
                            speaker.jitter.push(&pcm);
                        }
                    }
                    let mut sources = Vec::new();
                    let mut heard = loudness.lock().unwrap();
                    heard.retain(|id, _| speakers.contains_key(id));
                    for (id, speaker) in &speakers {
                        let mut pcm = vec![0; opus::FRAME_SAMPLES];
                        speaker.jitter.pull(&mut pcm);
                        // Quick to rise, slow to fall, so a speaker doesn't flicker between words.
                        let level = rms(&pcm);
                        let held = heard.entry(*id).or_insert(0.0);
                        *held = level.max(*held * 0.9);
                        sources.push(pcm);
                    }
                    drop(heard);
                    let mut mix = [0i16; opus::FRAME_SAMPLES];
                    let refs: Vec<_> = sources.iter().map(Vec::as_slice).collect();
                    slipstream_audio::voice::mix(&refs, &mut mix);
                    if let Some(tap) = recording.lock().unwrap().as_ref() {
                        let _ = tap.try_send(mix.to_vec());
                    }
                    output.push(&mix);
                    next += Duration::from_millis(20);
                    std::thread::sleep(next.saturating_duration_since(std::time::Instant::now()));
                    if next.elapsed() > Duration::from_millis(100) {
                        next = std::time::Instant::now();
                    }
                }
            })?;
        Ok(Self {
            player,
            tx,
            members,
            levels,
            tap,
            stop,
            _guard: Default::default(),
        })
    }
    /// How loud each participant is right now: 0 is silence, about 0.02 is
    /// quiet speech and 0.1 loud.
    pub fn levels(&self) -> HashMap<u64, f32> {
        self.levels.lock().unwrap().clone()
    }
    /// Also send each 20 ms of the mix here (None stops).
    pub fn tap(&self, tap: Option<std::sync::mpsc::SyncSender<Vec<i16>>>) {
        *self.tap.lock().unwrap() = tap;
    }
    pub fn members(&self, ids: Vec<u64>) {
        *self.members.lock().unwrap() = ids;
    }
    pub fn push(&self, packet: AudioPacket) {
        let _ = self.tx.try_send(packet);
    }
}

/// Root-mean-square loudness of 16-bit samples, 0–1.
fn rms(pcm: &[i16]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    let sum: f64 = pcm.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum();
    (sum / pcm.len() as f64).sqrt() as f32
}
