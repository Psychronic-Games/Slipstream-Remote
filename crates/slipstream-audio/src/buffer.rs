//! The viewer's jitter buffer, and the conversions between the codec's
//! 48 kHz stereo and whatever an audio device uses.

use std::{collections::VecDeque, sync::Mutex};

use crate::opus::SAMPLE_RATE;

/// Play once this much is buffered (absorbs network jitter)…
const START_MS: usize = 60;
/// …and drop the oldest audio beyond this, so delay can't build up.
const MAX_MS: usize = 200;

const fn samples(ms: usize) -> usize {
    ms * SAMPLE_RATE as usize / 1000 * 2
}

/// Decoded audio waiting to be played: interleaved 48 kHz stereo.
#[derive(Default)]
pub struct Jitter {
    low_latency: bool,
    state: Mutex<JitterState>,
    render_tap: Mutex<Option<crate::io::OnFrame>>,
}

#[derive(Default)]
struct JitterState {
    samples: VecDeque<i16>,
    playing: bool,
}

impl Jitter {
    pub fn realtime() -> Self {
        Self {
            low_latency: true,
            ..Default::default()
        }
    }
    pub fn set_render_tap(&self, tap: Option<crate::io::OnFrame>) {
        *self.render_tap.lock().unwrap() = tap;
    }
    pub fn push(&self, pcm: &[i16]) {
        let mut s = self.state.lock().unwrap();
        s.samples.extend(pcm);
        if s.samples.len() > samples(if self.low_latency { 80 } else { MAX_MS }) {
            let excess = s.samples.len() - samples(if self.low_latency { 20 } else { START_MS });
            s.samples.drain(..excess);
        }
    }

    /// Fills `out` (interleaved stereo frames) with audio, or silence while
    /// (re)filling.
    pub fn pull(&self, out: &mut [i16]) {
        let mut s = self.state.lock().unwrap();
        if !s.playing && s.samples.len() >= samples(if self.low_latency { 20 } else { START_MS }) {
            s.playing = true;
        }
        let n = if s.playing { out.len().min(s.samples.len()) } else { 0 };
        for (o, v) in out.iter_mut().zip(s.samples.drain(..n)) {
            *o = v;
        }
        out[n..].fill(0);
        if s.playing && s.samples.is_empty() {
            s.playing = false; // ran dry: build up a cushion again
        }
        drop(s);
        if let Some(tap) = self.render_tap.lock().unwrap().as_mut() {
            tap(out);
        }
    }
}

/// Mixes a device frame of `channels` samples down to stereo.
pub fn to_stereo(frame: &[f32]) -> [f32; 2] {
    match frame {
        [] => [0.0; 2],
        [mono] => [*mono; 2],
        [l, r] => [*l, *r],
        // Surround (L, R, C, …): fold the centre in, where voices usually are.
        [l, r, c, ..] => [l + 0.707 * c, r + 0.707 * c],
    }
}

/// Spreads a stereo frame over a device frame of `out.len()` channels.
pub fn from_stereo([l, r]: [f32; 2], out: &mut [f32]) {
    match out {
        [] => {}
        [mono] => *mono = (l + r) / 2.0,
        [a, b, rest @ ..] => {
            *a = l;
            *b = r;
            rest.fill(0.0);
        }
    }
}

pub fn to_i16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 32767.0) as i16
}

pub fn from_i16(v: i16) -> f32 {
    v as f32 / 32768.0
}

/// Linear-interpolation resampler for stereo frames. Exact (a copy) when
/// the rates match, which is the usual case (48 kHz devices).
pub struct Resampler {
    /// Input frames per output frame.
    step: f64,
    pos: f64,
    prev: [f32; 2],
    next: [f32; 2],
}

impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        Self { step: from_rate as f64 / to_rate as f64, pos: 1.0, prev: [0.0; 2], next: [0.0; 2] }
    }

    /// Produces one output frame, pulling input frames from `input` as needed.
    pub fn next_frame(&mut self, mut input: impl FnMut() -> [f32; 2]) -> [f32; 2] {
        while self.pos >= 1.0 {
            self.prev = self.next;
            self.next = input();
            self.pos -= 1.0;
        }
        let t = self.pos as f32;
        self.pos += self.step;
        [self.prev[0] + (self.next[0] - self.prev[0]) * t, self.prev[1] + (self.next[1] - self.prev[1]) * t]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_buffers_then_plays_and_caps_delay() {
        let j = Jitter::default();
        let mut out = vec![1i16; 960];
        j.push(&vec![5; samples(30)]);
        j.pull(&mut out);
        assert!(out.iter().all(|&v| v == 0), "silent while filling");
        j.push(&vec![5; samples(40)]);
        j.pull(&mut out);
        assert!(out.iter().all(|&v| v == 5), "plays once 60 ms are in");
        j.push(&vec![7; samples(500)]);
        assert!(j.state.lock().unwrap().samples.len() <= samples(MAX_MS), "delay capped");
    }

    #[test]
    fn resampler_keeps_matching_rates_exact() {
        let mut r = Resampler::new(48_000, 48_000);
        let mut src = (0..10).map(|i| [i as f32, -(i as f32)]);
        let outs: Vec<_> = (0..9).map(|_| r.next_frame(|| src.next().unwrap())).collect();
        assert_eq!(outs[3], [2.0, -2.0]); // one frame of delay, otherwise a copy
        // 44.1 → 48 kHz yields ~48/44.1 as many frames.
        let mut up = Resampler::new(44_100, 48_000);
        let mut pulled = 0;
        for _ in 0..48_000 {
            up.next_frame(|| {
                pulled += 1;
                [0.0; 2]
            });
        }
        assert!((44_090..44_110).contains(&pulled), "{pulled}");
    }

    #[test]
    fn channel_mapping() {
        assert_eq!(to_stereo(&[0.5]), [0.5, 0.5]);
        assert_eq!(to_stereo(&[0.1, 0.2, 0.0, 0.0, 0.0, 0.0]), [0.1, 0.2]);
        let mut six = [9.0; 6];
        from_stereo([0.3, 0.4], &mut six);
        assert_eq!(six, [0.3, 0.4, 0.0, 0.0, 0.0, 0.0]);
    }
}
