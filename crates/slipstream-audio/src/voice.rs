//! Conversational capture with an echo reference taken from actual playback.
use crate::{
    Capture, Player,
    io::{Worker, spawn},
    opus,
};
use anyhow::Result;
use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering},
    mpsc,
};
static PLAYING: AtomicUsize = AtomicUsize::new(0);
static MICROPHONE_EPOCH: AtomicU64 = AtomicU64::new(0);
pub fn stop_all_microphones() {
    MICROPHONE_EPOCH.fetch_add(1, Ordering::AcqRel);
}
pub fn playing_call() -> bool {
    PLAYING.load(Ordering::Acquire) > 0
}
pub struct PlaybackGuard;
impl Default for PlaybackGuard {
    fn default() -> Self {
        PLAYING.fetch_add(1, Ordering::AcqRel);
        Self
    }
}
impl Drop for PlaybackGuard {
    fn drop(&mut self) {
        PLAYING.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct Microphone {
    _capture: Capture,
    _worker: Worker,
    player: Arc<Player>,
}
impl Drop for Microphone {
    fn drop(&mut self) {
        self.player.set_render_tap(None);
    }
}
impl Microphone {
    pub fn start(
        player: Arc<Player>,
        device: Option<String>,
        mut send: impl FnMut(Vec<u8>) + Send + 'static,
    ) -> Result<Self> {
        let (capture_tx, capture_rx) = mpsc::sync_channel::<Vec<i16>>(4);
        let (render_tx, render_rx) = mpsc::sync_channel::<Vec<i16>>(12);
        let epoch = MICROPHONE_EPOCH.load(Ordering::Acquire);
        let worker = spawn("meeting-voice", move |stop, ready| {
            let mut encoder = match opus::Encoder::voice() {
                Ok(e) => e,
                Err(e) => return ready(Err(e)),
            };
            let mut processor = sonora::AudioProcessing::builder()
                .config(sonora::Config {
                    echo_canceller: Some(Default::default()),
                    noise_suppression: Some(Default::default()),
                    gain_controller2: Some(Default::default()),
                    ..Default::default()
                })
                .capture_config(sonora::StreamConfig::new(48_000, 1))
                .render_config(sonora::StreamConfig::new(48_000, 1))
                .build();
            let mut cleaned = [0i16; opus::FRAME_SAMPLES];
            let mut mono = [0i16; 480];
            let mut output = [0i16; 480];
            let mut reference = [0i16; 480];
            let mut render_output = [0i16; 480];
            let mut sequence = 0u16;
            ready(Ok(()));
            while !stop.load(Ordering::Relaxed) && MICROPHONE_EPOCH.load(Ordering::Acquire) == epoch
            {
                let Ok(frame) = capture_rx.recv_timeout(std::time::Duration::from_millis(30))
                else {
                    continue;
                };
                for rendered in render_rx.try_iter() {
                    for chunk in rendered.as_chunks::<960>().0 {
                        for (sample, stereo) in reference.iter_mut().zip(chunk.as_chunks::<2>().0) {
                            *sample = ((i32::from(stereo[0]) + i32::from(stereo[1])) / 2) as i16;
                        }
                        let _ = processor.process_render_i16(&reference, &mut render_output);
                    }
                }
                for (part, chunk) in frame.as_chunks::<960>().0.iter().enumerate() {
                    for (sample, stereo) in mono.iter_mut().zip(chunk.as_chunks::<2>().0) {
                        *sample = ((i32::from(stereo[0]) + i32::from(stereo[1])) / 2) as i16;
                    }
                    let _ = processor.set_stream_delay_ms(80);
                    if processor.process_capture_i16(&mono, &mut output).is_err() {
                        output.copy_from_slice(&mono);
                    }
                    for (stereo, sample) in cleaned[part * 960..(part + 1) * 960]
                        .as_chunks_mut::<2>()
                        .0
                        .iter_mut()
                        .zip(output)
                    {
                        stereo.fill(sample);
                    }
                }
                if let Ok(encoded) = encoder.encode(&cleaned) {
                    let mut data = Vec::with_capacity(encoded.len() + 2);
                    data.extend_from_slice(&sequence.to_le_bytes());
                    data.extend_from_slice(encoded);
                    sequence = sequence.wrapping_add(1);
                    send(data);
                }
            }
        })?;
        let capture = Capture::microphone(
            device,
            Box::new(move |pcm| {
                let _ = capture_tx.try_send(pcm.to_vec());
            }),
        )?;
        player.set_render_tap(Some(Box::new(move |pcm| {
            let _ = render_tx.try_send(pcm.to_vec());
        })));
        Ok(Self {
            _capture: capture,
            _worker: worker,
            player,
        })
    }
}

/// Mix simultaneous speakers with headroom, preserving quiet single speakers.
pub fn mix(sources: &[&[i16]], out: &mut [i16]) {
    out.fill(0);
    for (i, sample) in out.iter_mut().enumerate() {
        let sum: i32 = sources
            .iter()
            .map(|s| i32::from(s.get(i).copied().unwrap_or(0)))
            .sum();
        *sample = sum.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn simultaneous_speakers_mix_without_wrapping() {
        let mut out = [0; 3];
        super::mix(&[&[30000, -30000, 100], &[10000, -10000, 200]], &mut out);
        assert_eq!(out, [32767, -32768, 300]);
    }
}
