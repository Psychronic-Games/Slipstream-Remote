//! Capturing what the host computer plays, and playing it on the viewer.
//! Both run on a thread of their own: some platforms' streams can't move
//! between threads, and PulseAudio's simple API blocks.
//!
//! * Linux: PulseAudio (see `pulse`), on PipeWire desktops too.
//! * Windows: WASAPI through cpal; capture is loopback from the default output.
//! * macOS: playback through cpal; capture isn't supported yet (needs
//!   ScreenCaptureKit audio).

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use anyhow::{Result, bail};

use crate::buffer::Jitter;
#[cfg(target_os = "linux")]
use crate::opus::{FRAME_SAMPLES, SAMPLE_RATE};

/// A running capture or playback thread; requests stop when dropped.
pub(crate) struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let start=std::time::Instant::now();
            while !t.is_finished() && start.elapsed()<std::time::Duration::from_millis(500) {
                thread::sleep(std::time::Duration::from_millis(5));
            }
            if t.is_finished() {let _=t.join();}
            else {tracing::warn!("audio device did not stop promptly; its worker will finish in the background");}
        }
    }
}

/// Starts `body` on a thread. It reports whether it got going (`ready`)
/// before looping until `stop` is set.
pub(crate) fn spawn(
    name: &str,
    body: impl FnOnce(Arc<AtomicBool>, &dyn Fn(Result<()>)) + Send + 'static,
) -> Result<Worker> {
    spawn_with_timeout(name, std::time::Duration::from_secs(5), body)
}

fn spawn_with_timeout(name:&str,timeout:std::time::Duration,body:impl FnOnce(Arc<AtomicBool>,&dyn Fn(Result<()>))+Send+'static)->Result<Worker> {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let flag = stop.clone();
    let thread = thread::Builder::new().name(name.into()).spawn(move || {
        body(flag, &|r| {
            let _ = tx.send(r);
        })
    })?;
    let worker = Worker { stop, thread: Some(thread) };
    match rx.recv_timeout(timeout) {
        Ok(Ok(())) => Ok(worker),
        Ok(Err(e)) => Err(e),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => bail!("the audio device did not respond in time"),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => bail!("the audio thread stopped"),
    }
}

/// Whether this computer can share its sound.
pub fn capture_supported() -> bool {
    #[cfg(target_os = "linux")]
    return crate::pulse::available();
    #[cfg(windows)]
    return true;
    #[allow(unreachable_code)]
    false
}

/// Receives each captured 20 ms frame (48 kHz stereo).
pub type OnFrame = Box<dyn FnMut(&[i16]) + Send + 'static>;

/// Captures what this computer plays, as 20 ms frames of 48 kHz stereo.
pub struct Capture(#[allow(dead_code)] Worker);

impl Capture {
    /// Captures the selected microphone, separately from system loopback.
    pub fn microphone(
        device: Option<String>,
        #[cfg_attr(not(target_os = "linux"), allow(unused_mut))] mut on_frame: OnFrame,
    ) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            spawn("meeting-microphone", move |stop, ready| {
                let mut stream = match crate::pulse::Stream::microphone(device.as_deref()) {
                    Ok(s) => s,
                    Err(e) => return ready(Err(e)),
                };
                ready(Ok(()));
                let mut frame = vec![0i16; crate::opus::FRAME_SAMPLES];
                while !stop.load(Ordering::Relaxed) {
                    if let Err(e) = stream.read(&mut frame) {
                        tracing::warn!("microphone stopped: {e:#}");
                        return;
                    }
                    on_frame(&frame);
                }
            })
            .map(Capture)
        }
        #[cfg(any(windows, target_os = "macos", target_os = "android"))]
        {
            cpal_io::microphone(device, on_frame).map(Capture)
        }
    }
    /// A steady 440 Hz tone instead of real capture (test pattern mode, tests).
    pub fn test_tone(mut on_frame: OnFrame) -> Result<Self> {
        spawn("audio-tone", move |stop, ready| {
            ready(Ok(()));
            let mut frame = vec![0i16; crate::opus::FRAME_SAMPLES];
            let mut t = 0u64;
            let mut next = std::time::Instant::now();
            while !stop.load(Ordering::Relaxed) {
                for s in frame.chunks_mut(2) {
                    let v = (t as f32 * 440.0 * std::f32::consts::TAU / crate::opus::SAMPLE_RATE as f32).sin();
                    s.fill((v * 8000.0) as i16);
                    t += 1;
                }
                on_frame(&frame);
                next += std::time::Duration::from_millis(20);
                thread::sleep(next.saturating_duration_since(std::time::Instant::now()));
            }
        })
        .map(Capture)
    }

    pub fn start(#[cfg_attr(windows, allow(unused_mut))] mut on_frame: OnFrame) -> Result<Self> {
        let mut original = on_frame;
        on_frame = Box::new(move |frame| {
            if !crate::voice::playing_call() {
                original(frame);
            }
        });
        #[cfg(target_os = "linux")]
        {
            spawn("audio-capture", move |stop, ready| {
                let mut stream = match crate::pulse::Stream::record(20) {
                    Ok(s) => s,
                    Err(e) => return ready(Err(e)),
                };
                ready(Ok(()));
                let mut frame = vec![0i16; FRAME_SAMPLES];
                while !stop.load(Ordering::Relaxed) {
                    if let Err(e) = stream.read(&mut frame) {
                        tracing::warn!("audio capture stopped: {e:#}");
                        return;
                    }
                    on_frame(&frame);
                }
            })
            .map(Capture)
        }
        #[cfg(windows)]
        {
            cpal_io::capture(on_frame).map(Capture)
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            let _ = &mut on_frame;
            bail!("sharing sound isn't supported on this system yet")
        }
    }
}

/// Plays decoded 48 kHz stereo audio through the default output.
pub struct Player {
    jitter: Arc<Jitter>,
    _worker: Option<Worker>,
    memory: Option<Arc<std::sync::Mutex<Vec<i16>>>>,
}

impl Player {
    pub fn set_render_tap(&self, tap: Option<OnFrame>) {
        self.jitter.set_render_tap(tap);
    }
    /// Collects the audio in `into` instead of playing it (tests).
    pub fn memory(into: Arc<std::sync::Mutex<Vec<i16>>>) -> Self {
        Self { jitter: Arc::default(), _worker: None, memory: Some(into) }
    }

    pub fn start() -> Result<Self> {
        Self::with_jitter(Arc::default())
    }
    pub fn meeting() -> Result<Self> {
        Self::with_jitter(Arc::new(Jitter::realtime()))
    }
    fn with_jitter(jitter: Arc<Jitter>) -> Result<Self> {
        #[cfg(target_os = "linux")]
        let worker = {
            let jitter = jitter.clone();
            spawn("audio-playback", move |stop, ready| {
                // ~40 ms queued in the server, on top of the jitter buffer.
                let mut stream = match crate::pulse::Stream::playback(40) {
                    Ok(s) => s,
                    Err(e) => return ready(Err(e)),
                };
                ready(Ok(()));
                let mut chunk = vec![0i16; SAMPLE_RATE as usize / 100 * 2]; // 10 ms
                while !stop.load(Ordering::Relaxed) {
                    jitter.pull(&mut chunk);
                    if let Err(e) = stream.write(&chunk) {
                        tracing::warn!("audio playback stopped: {e:#}");
                        return;
                    }
                }
            })?
        };
        #[cfg(any(windows, target_os = "macos", target_os = "android"))]
        let worker = cpal_io::playback(jitter.clone())?;
        Ok(Self { jitter, _worker: Some(worker), memory: None })
    }

    /// Queues one decoded frame.
    pub fn push(&self, pcm: &[i16]) {
        match &self.memory {
            Some(m) => m.lock().unwrap().extend_from_slice(pcm),
            None => self.jitter.push(pcm),
        }
    }
}

#[cfg(any(windows, target_os = "macos", target_os = "android"))]
mod cpal_io {
    use std::sync::{Arc, atomic::Ordering};

    use anyhow::{Context, Result, anyhow};
    use cpal::{
        FromSample, SampleFormat, SizedSample,
        traits::{DeviceTrait, HostTrait, StreamTrait},
    };

    use super::{Worker, spawn};
    use crate::{
        buffer::{to_i16, to_stereo},
        opus::FRAME_SAMPLES,
    };
    use crate::{
        buffer::{Jitter, Resampler, from_i16, from_stereo},
        opus::SAMPLE_RATE,
    };

    fn output_device() -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
        let device = cpal::default_host().default_output_device().context("no audio output device")?;
        let config = device.default_output_config().map_err(|e| anyhow!("audio output: {e}"))?;
        Ok((device, config))
    }

    /// Keeps `stream` playing until the worker is stopped.
    fn run(stream: cpal::Stream, stop: &std::sync::atomic::AtomicBool) {
        if let Err(e) = stream.play() {
            tracing::warn!("audio stream: {e}");
        }
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// Windows: records the default output device (WASAPI loopback: an
    /// output device opened for input).
    #[cfg(windows)]
    pub fn capture(on_frame: super::OnFrame) -> Result<Worker> {
        spawn("audio-capture", move |stop, ready| {
            let built = output_device().and_then(|(device, config)| {
                let format = config.sample_format();
                let config = config.config();
                match format {
                    SampleFormat::F32 => input::<f32>(&device, config, on_frame),
                    SampleFormat::I16 => input::<i16>(&device, config, on_frame),
                    SampleFormat::I32 => input::<i32>(&device, config, on_frame),
                    other => Err(anyhow!("unsupported audio format {other}")),
                }
            });
            match built {
                Ok(stream) => {
                    ready(Ok(()));
                    run(stream, &stop);
                }
                Err(e) => ready(Err(e)),
            }
        })
    }

    fn input<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        mut on_frame: super::OnFrame,
    ) -> Result<cpal::Stream>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let channels = config.channels as usize;
        let mut resampler = Resampler::new(config.sample_rate, SAMPLE_RATE);
        let mut frame = Vec::with_capacity(FRAME_SAMPLES);
        let mut pending: std::collections::VecDeque<[f32; 2]> = Default::default();
        device
            .build_input_stream(
                config,
                move |data: &[T], _| {
                    let mut scratch = [0f32; 8];
                    for chunk in data.chunks(channels) {
                        let n = chunk.len().min(8);
                        for (s, v) in scratch.iter_mut().zip(chunk) {
                            *s = v.to_sample::<f32>();
                        }
                        pending.push_back(to_stereo(&scratch[..n]));
                    }
                    // Resample to 48 kHz while there's input left for it.
                    while pending.len() > 2 {
                        let [l, r] = resampler.next_frame(|| pending.pop_front().unwrap_or_default());
                        frame.push(to_i16(l));
                        frame.push(to_i16(r));
                        if frame.len() == FRAME_SAMPLES {
                            on_frame(&frame);
                            frame.clear();
                        }
                    }
                },
                |e| tracing::warn!("audio capture: {e}"),
                None,
            )
            .map_err(|e| anyhow!("audio capture: {e}"))
    }

    pub fn microphone(name: Option<String>, on_frame: super::OnFrame) -> Result<Worker> {
        spawn("meeting-microphone", move |stop, ready| {
            let built = (|| {
                let host = cpal::default_host();
                let device = if let Some(name) = name {
                    host.input_devices()?
                        .find(|d| d.id().is_ok_and(|id| id.to_string() == name) || d.description().is_ok_and(|d| d.name() == name))
                        .context("Microphone was not found")?
                } else {
                    host.default_input_device()
                        .context("No microphone is available")?
                };
                let config = device.default_input_config()?;
                let format = config.sample_format();
                let config = config.config();
                match format {
                    SampleFormat::F32 => input::<f32>(&device, config, on_frame),
                    SampleFormat::I16 => input::<i16>(&device, config, on_frame),
                    SampleFormat::I32 => input::<i32>(&device, config, on_frame),
                    _ => Err(anyhow!("Unsupported microphone sample format")),
                }
            })();
            match built {
                Ok(stream) => {
                    ready(Ok(()));
                    run(stream, &stop);
                }
                Err(e) => ready(Err(e)),
            }
        })
    }

    pub fn playback(jitter: Arc<Jitter>) -> Result<Worker> {
        spawn("audio-playback", move |stop, ready| {
            let built = output_device().and_then(|(device, config)| {
                let format = config.sample_format();
                let config = config.config();
                let jitter = jitter.clone();
                match format {
                    SampleFormat::F32 => output::<f32>(&device, config, jitter),
                    SampleFormat::I16 => output::<i16>(&device, config, jitter),
                    SampleFormat::I32 => output::<i32>(&device, config, jitter),
                    other => Err(anyhow!("unsupported audio format {other}")),
                }
            });
            match built {
                Ok(stream) => {
                    ready(Ok(()));
                    run(stream, &stop);
                }
                Err(e) => ready(Err(e)),
            }
        })
    }

    fn output<T>(device: &cpal::Device, config: cpal::StreamConfig, jitter: Arc<Jitter>) -> Result<cpal::Stream>
    where
        T: SizedSample + FromSample<f32>,
    {
        let channels = config.channels as usize;
        let mut resampler = Resampler::new(SAMPLE_RATE, config.sample_rate);
        // 48 kHz audio taken from the jitter buffer 10 ms at a time.
        let mut chunk = vec![0i16; SAMPLE_RATE as usize / 100 * 2];
        let mut at = chunk.len();
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _| {
                    let mut scratch = [0f32; 8];
                    for out in data.chunks_mut(channels) {
                        let stereo = resampler.next_frame(|| {
                            if at >= chunk.len() {
                                jitter.pull(&mut chunk);
                                at = 0;
                            }
                            at += 2;
                            [from_i16(chunk[at - 2]), from_i16(chunk[at - 1])]
                        });
                        let n = out.len().min(8);
                        from_stereo(stereo, &mut scratch[..n]);
                        for (o, v) in out.iter_mut().zip(scratch) {
                            *o = T::from_sample(v);
                        }
                    }
                },
                |e| tracing::warn!("audio playback: {e}"),
                None,
            )
            .map_err(|e| anyhow!("audio playback: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_audio_io_does_not_block_shutdown_or_initialization() {
        use std::time::{Duration,Instant};
        for initialized in [true,false] {
            let (release,wait)=std::sync::mpsc::channel();let (done,finished)=std::sync::mpsc::channel();
            let start=Instant::now();
            let worker=spawn_with_timeout("stalled-audio-test",Duration::from_millis(30),move |stop,ready| {
                if initialized {ready(Ok(()));}
                wait.recv().unwrap();
                assert!(stop.load(Ordering::Relaxed));done.send(()).unwrap();
            });
            if initialized {drop(worker.unwrap());}else{assert!(worker.is_err());}
            assert!(start.elapsed()<Duration::from_secs(2));
            release.send(()).unwrap();finished.recv_timeout(Duration::from_secs(2)).unwrap();
        }
    }

    /// Needs a real sound system: `cargo test -p slipstream-audio -- --ignored`.
    /// Records what the computer plays for half a second, and plays silence.
    #[test]
    #[ignore]
    fn real_devices_open() {
        let frames = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n = frames.clone();
        let capture = Capture::start(Box::new(move |f: &[i16]| {
            assert_eq!(f.len(), crate::opus::FRAME_SAMPLES);
            n.fetch_add(1, Ordering::Relaxed);
        }))
        .expect("capture");
        let player = Player::start().expect("playback");
        for _ in 0..25 {
            player.push(&[0; crate::opus::FRAME_SAMPLES]);
            thread::sleep(std::time::Duration::from_millis(20));
        }
        drop(capture);
        let got = frames.load(Ordering::Relaxed);
        println!("captured {got} frames in 0.5 s");
        assert!((15..=30).contains(&got), "{got} frames");
    }
}
