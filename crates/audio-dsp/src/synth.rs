//! Sound synthesis building blocks shared by [`crate::sfx`] (one-shot sound effects) and
//! [`crate::music`] (procedural music beds).
//!
//! Everything is deterministic: randomness comes from [`Rng`] seeded by the caller, so the same
//! sound id, variant and sample rate always render the same samples (the engine caches the files
//! by those keys). Buffers are planar stereo ([`Buf`]); the helpers never index past a buffer
//! (they use iterators and `get`), and every length derived from a parameter is capped by the
//! caller ([`MAX_SECONDS`]).

use crate::biquad::{Biquad, Coeffs, FilterType};
use std::f32::consts::{PI, TAU};

/// Longest buffer any generator renders (seconds). Requests beyond it are clamped.
pub const MAX_SECONDS: f32 = 600.0;

/// Small, fast, deterministic PRNG (xorshift64*).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // splitmix the seed so neighbouring seeds give unrelated streams; never zero
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Rng((z ^ (z >> 31)) | 1)
    }
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
    /// Uniform in `[-1, 1)`.
    pub fn bipolar(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.unit()
    }
    pub fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }
    /// An index below `n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { self.next_u32() as usize % n }
    }
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> Option<&'a T> {
        xs.get(self.below(xs.len()))
    }
}

/// Planar stereo buffer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Buf {
    pub sample_rate: u32,
    pub l: Vec<f32>,
    pub r: Vec<f32>,
}

/// Frames in `seconds` at `sr` (negative / NaN → 0, capped at [`MAX_SECONDS`]).
pub fn frames(sr: u32, seconds: f32) -> usize {
    let s = if seconds.is_finite() { seconds.clamp(0.0, MAX_SECONDS) } else { 0.0 };
    (s * sr as f32).round() as usize
}

impl Buf {
    pub fn new(sample_rate: u32, frames: usize) -> Self {
        Buf { sample_rate, l: vec![0.0; frames], r: vec![0.0; frames] }
    }
    pub fn seconds(sample_rate: u32, seconds: f32) -> Self {
        Self::new(sample_rate, frames(sample_rate, seconds))
    }
    /// A centred stereo buffer from a mono signal.
    pub fn from_mono(sample_rate: u32, m: &[f32]) -> Self {
        Buf { sample_rate, l: m.to_vec(), r: m.to_vec() }
    }
    pub fn len(&self) -> usize {
        self.l.len().min(self.r.len())
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn duration(&self) -> f32 {
        self.len() as f32 / self.sample_rate.max(1) as f32
    }
    /// Mix a mono signal in at frame `at` with `gain` and equal-power `pan` (−1 left … 1 right).
    /// What falls past the end is dropped.
    pub fn add_mono(&mut self, at: usize, src: &[f32], gain: f32, pan: f32) {
        let a = (pan.clamp(-1.0, 1.0) + 1.0) * PI / 4.0;
        let (gl, gr) = (a.cos() * gain, a.sin() * gain);
        for (d, s) in self.l.iter_mut().skip(at).zip(src) {
            *d += s * gl;
        }
        for (d, s) in self.r.iter_mut().skip(at).zip(src) {
            *d += s * gr;
        }
    }
    /// Mix a mono signal whose pan moves over time (`pan(t)` with `t` in seconds from `at`).
    pub fn add_mono_moving(&mut self, at: usize, src: &[f32], gain: f32, pan: impl Fn(f32) -> f32) {
        let sr = self.sample_rate.max(1) as f32;
        for (i, s) in src.iter().enumerate() {
            let a = (pan(i as f32 / sr).clamp(-1.0, 1.0) + 1.0) * PI / 4.0;
            if let Some(d) = self.l.get_mut(at + i) {
                *d += s * a.cos() * gain;
            }
            if let Some(d) = self.r.get_mut(at + i) {
                *d += s * a.sin() * gain;
            }
        }
    }
    /// Mix another stereo buffer in at frame `at`.
    pub fn add(&mut self, at: usize, src: &Buf, gain: f32) {
        for (d, s) in self.l.iter_mut().skip(at).zip(&src.l) {
            *d += s * gain;
        }
        for (d, s) in self.r.iter_mut().skip(at).zip(&src.r) {
            *d += s * gain;
        }
    }
    pub fn peak(&self) -> f32 {
        self.l.iter().chain(&self.r).fold(0.0f32, |m, v| if v.is_finite() { m.max(v.abs()) } else { m })
    }
    /// Scale so the peak is `target` (silence stays silence; NaN/∞ samples become 0).
    pub fn normalize(&mut self, target: f32) {
        for v in self.l.iter_mut().chain(self.r.iter_mut()) {
            if !v.is_finite() {
                *v = 0.0;
            }
        }
        let p = self.peak();
        if p > 1e-9 {
            let g = target / p;
            for v in self.l.iter_mut().chain(self.r.iter_mut()) {
                *v *= g;
            }
        }
    }
    pub fn gain(&mut self, g: f32) {
        for v in self.l.iter_mut().chain(self.r.iter_mut()) {
            *v *= g;
        }
    }
    /// Linear fade over the first `n` frames.
    pub fn fade_in(&mut self, n: usize) {
        let n = n.min(self.len()).max(1);
        for ch in [&mut self.l, &mut self.r] {
            for (i, v) in ch.iter_mut().take(n).enumerate() {
                *v *= i as f32 / n as f32;
            }
        }
    }
    /// Equal-power-ish fade over the last `n` frames.
    pub fn fade_out(&mut self, n: usize) {
        let len = self.len();
        let n = n.min(len).max(1);
        for ch in [&mut self.l, &mut self.r] {
            for (i, v) in ch.iter_mut().enumerate().skip(len - n) {
                let x = (len - i) as f32 / n as f32;
                *v *= x * x;
            }
        }
    }
    /// Keep the first `n` frames.
    pub fn truncate(&mut self, n: usize) {
        self.l.truncate(n);
        self.r.truncate(n);
    }
    /// Interleaved L, R, L, R…
    pub fn interleaved(&self) -> Vec<f32> {
        self.l.iter().zip(&self.r).flat_map(|(l, r)| [*l, *r]).collect()
    }
    /// Apply `f` to both channels.
    pub fn each(&mut self, mut f: impl FnMut(&mut Vec<f32>)) {
        f(&mut self.l);
        f(&mut self.r);
    }
}

/// Oscillator shapes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wave {
    Sine,
    Triangle,
    Saw,
    /// Pulse with the given duty cycle (0.5 = square).
    Pulse(f32),
}

/// PolyBLEP residual (band-limits the discontinuities of saw and pulse waves).
fn blep(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        0.0
    } else if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

/// One sample of `w` at `phase` (cycles, 0..1) with phase increment `dt` (cycles per sample).
pub fn osc(w: Wave, phase: f32, dt: f32) -> f32 {
    let t = phase - phase.floor();
    match w {
        Wave::Sine => (TAU * t).sin(),
        Wave::Triangle => 1.0 - 4.0 * (t - 0.5).abs(),
        Wave::Saw => 2.0 * t - 1.0 - blep(t, dt),
        Wave::Pulse(d) => {
            let d = d.clamp(0.05, 0.95);
            let mut v = if t < d { 1.0 } else { -1.0 };
            v += blep(t, dt);
            let t2 = t - d + if t < d { 1.0 } else { 0.0 };
            v - blep(t2 - t2.floor(), dt)
        }
    }
}

/// A tone of `seconds` whose frequency (`freq(t)`, Hz) and amplitude (`amp(t)`) follow functions
/// of the time in seconds.
pub fn tone(sr: u32, seconds: f32, w: Wave, freq: impl Fn(f32) -> f32, amp: impl Fn(f32) -> f32) -> Vec<f32> {
    let n = frames(sr, seconds);
    let srf = sr.max(1) as f32;
    let mut ph = 0.0f32;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / srf;
        let f = freq(t).clamp(0.0, srf * 0.45);
        let dt = f / srf;
        out.push(osc(w, ph, dt) * amp(t));
        ph += dt;
        if ph >= 1.0 {
            ph -= ph.floor();
        }
    }
    out
}

/// Several detuned oscillators summed (supersaw-style), normalised by voice count.
pub fn unison(sr: u32, seconds: f32, w: Wave, hz: f32, voices: usize, cents: f32, amp: impl Fn(f32) -> f32, rng: &mut Rng) -> Vec<f32> {
    let voices = voices.clamp(1, 16);
    let mut out = vec![0.0; frames(sr, seconds)];
    for v in 0..voices {
        let spread = if voices == 1 { 0.0 } else { (v as f32 / (voices - 1) as f32) * 2.0 - 1.0 };
        let f = hz * 2f32.powf(spread * cents / 1200.0);
        let start = rng.unit();
        let n = out.len();
        let srf = sr.max(1) as f32;
        let dt = (f / srf).clamp(0.0, 0.45);
        let mut ph = start;
        for (i, o) in out.iter_mut().enumerate().take(n) {
            *o += osc(w, ph, dt) * amp(i as f32 / srf);
            ph += dt;
            if ph >= 1.0 {
                ph -= 1.0;
            }
        }
    }
    let k = 1.0 / (voices as f32).sqrt();
    out.iter_mut().for_each(|v| *v *= k);
    out
}

/// Noise colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Noise {
    White,
    /// −3 dB/octave (Paul Kellet's economy filter).
    Pink,
    /// −6 dB/octave (leaky integrator).
    Brown,
}

/// Noise of `seconds` shaped by `amp(t)`.
pub fn noise(sr: u32, seconds: f32, colour: Noise, rng: &mut Rng, amp: impl Fn(f32) -> f32) -> Vec<f32> {
    let n = frames(sr, seconds);
    let srf = sr.max(1) as f32;
    let (mut b0, mut b1, mut b2, mut br) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    (0..n)
        .map(|i| {
            let w = rng.bipolar();
            let v = match colour {
                Noise::White => w,
                Noise::Pink => {
                    b0 = 0.99765 * b0 + w * 0.099_046;
                    b1 = 0.96300 * b1 + w * 0.296_516_4;
                    b2 = 0.57000 * b2 + w * 1.052_691_3;
                    (b0 + b1 + b2 + w * 0.1848) * 0.25
                }
                Noise::Brown => {
                    br = (br + w * 0.02) * 0.998;
                    br * 3.0
                }
            };
            v * amp(i as f32 / srf)
        })
        .collect()
}

/// Filter in place with a cutoff that can move over time (`freq(t)`, Hz). Coefficients are
/// redesigned every 16 samples.
pub fn filter(sr: u32, buf: &mut [f32], kind: FilterType, freq: impl Fn(f32) -> f32, q: f32) {
    let srf = sr.max(1) as f32;
    let mut bq = Biquad::new();
    let mut c = Coeffs::IDENTITY;
    for (i, v) in buf.iter_mut().enumerate() {
        if i % 16 == 0 {
            let f = freq(i as f32 / srf);
            let f = if f.is_finite() { f.clamp(10.0, srf * 0.45) } else { 1000.0 };
            c = Coeffs::design(kind, srf as f64, f as f64, q.max(0.05) as f64, 0.0);
        }
        *v = bq.tick(&c, *v as f64) as f32;
    }
}

/// Peaking / shelving EQ band (static).
pub fn eq(sr: u32, buf: &mut [f32], kind: FilterType, freq: f32, q: f32, gain_db: f32) {
    let c = Coeffs::design(kind, sr.max(1) as f64, freq as f64, q.max(0.05) as f64, gain_db as f64);
    Biquad::new().process(&c, buf);
}

/// Attack (linear) then exponential decay with time constant `tau`; 0 before 0.
pub fn perc(t: f32, attack: f32, tau: f32) -> f32 {
    if t < 0.0 {
        0.0
    } else if t < attack {
        t / attack.max(1e-6)
    } else {
        (-(t - attack) / tau.max(1e-6)).exp()
    }
}

/// ADSR for a note held `hold` seconds (attack, decay, sustain level, release).
pub fn adsr(t: f32, a: f32, d: f32, s: f32, r: f32, hold: f32) -> f32 {
    let level = |t: f32| {
        if t < a {
            t / a.max(1e-6)
        } else if t < a + d {
            1.0 - (1.0 - s) * (t - a) / d.max(1e-6)
        } else {
            s
        }
    };
    if t < 0.0 {
        0.0
    } else if t < hold {
        level(t)
    } else {
        let rel = (t - hold) / r.max(1e-6);
        if rel >= 1.0 { 0.0 } else { level(hold) * (1.0 - rel) }
    }
}

/// Smooth bell from 0 to 1 and back over `[0, len]`, peaking at `peak` (fraction).
pub fn bell(t: f32, len: f32, peak: f32) -> f32 {
    if t <= 0.0 || t >= len || len <= 0.0 {
        return 0.0;
    }
    let x = t / len;
    let p = peak.clamp(0.05, 0.95);
    let y = if x < p { x / p } else { (1.0 - x) / (1.0 - p) };
    (y * PI / 2.0).sin().powi(2)
}

/// Exponential glide from `a` to `b` over `len` seconds (then holds `b`).
pub fn glide(t: f32, len: f32, a: f32, b: f32) -> f32 {
    let x = (t / len.max(1e-6)).clamp(0.0, 1.0);
    let (a, b) = (a.max(1e-3), b.max(1e-3));
    a * (b / a).powf(x)
}

/// Soft clip (`tanh` drive, gain-compensated).
pub fn drive(buf: &mut [f32], amount: f32) {
    let k = amount.max(0.01);
    let norm = 1.0 / k.tanh();
    buf.iter_mut().for_each(|v| *v = (*v * k).tanh() * norm);
}

/// Reduce bit depth and sample rate (lo-fi / 8-bit grit).
pub fn bitcrush(buf: &mut [f32], bits: u32, hold: usize) {
    let steps = (1u32 << bits.clamp(1, 16)) as f32 / 2.0;
    let hold = hold.max(1);
    let mut last = 0.0;
    for (i, v) in buf.iter_mut().enumerate() {
        if i % hold == 0 {
            last = (*v * steps).round() / steps;
        }
        *v = last;
    }
}

/// MIDI note number to Hz (A4 = 69 = 440 Hz).
pub fn midi_hz(n: f32) -> f32 {
    440.0 * 2f32.powf((n - 69.0) / 12.0)
}

/// Plucked string (Karplus–Strong) of `seconds` at `hz`; `damping` 0..1 (higher = darker, shorter).
pub fn pluck(sr: u32, hz: f32, seconds: f32, damping: f32, rng: &mut Rng) -> Vec<f32> {
    let n = frames(sr, seconds);
    let period = ((sr as f32 / hz.max(20.0)).round() as usize).clamp(2, 4096);
    let mut line: Vec<f32> = (0..period).map(|_| rng.bipolar()).collect();
    let d = 0.5 - damping.clamp(0.0, 1.0) * 0.02;
    let mut out = Vec::with_capacity(n);
    let mut idx = 0usize;
    for _ in 0..n {
        let a = line.get(idx).copied().unwrap_or(0.0);
        let b = line.get((idx + 1) % period).copied().unwrap_or(0.0);
        let v = (a + b) * d;
        if let Some(slot) = line.get_mut(idx) {
            *slot = v;
        }
        out.push(a);
        idx = (idx + 1) % period;
    }
    out
}

/// Freeverb-style stereo reverb (8 parallel combs, 4 series all-passes per side, tunings spread
/// between sides). `room` 0..1 is the decay, `damp` 0..1 the high-frequency damping, `mix` the wet
/// level. The tail is added: the output is longer by `tail` seconds.
pub fn reverb(input: &Buf, room: f32, damp: f32, mix: f32, tail: f32) -> Buf {
    const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
    const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
    const SPREAD: usize = 23;
    let sr = input.sample_rate.max(1);
    let scale = sr as f32 / 44_100.0;
    let n = input.len() + frames(sr, tail);
    let mut out = Buf::new(sr, n);
    let feedback = 0.7 + 0.28 * room.clamp(0.0, 1.0);
    let damp = damp.clamp(0.0, 1.0) * 0.4;
    for (side, (src, dst)) in [(&input.l, &mut out.l), (&input.r, &mut out.r)].into_iter().enumerate() {
        let off = side * SPREAD;
        let mut combs: Vec<(Vec<f32>, usize, f32)> = COMBS.iter().map(|&c| (vec![0.0; (((c + off) as f32 * scale) as usize).max(1)], 0usize, 0.0f32)).collect();
        let mut aps: Vec<(Vec<f32>, usize)> = ALLPASSES.iter().map(|&a| (vec![0.0; (((a + off) as f32 * scale) as usize).max(1)], 0usize)).collect();
        for (i, o) in dst.iter_mut().enumerate() {
            let x = src.get(i).copied().unwrap_or(0.0) * 0.015;
            let mut acc = 0.0;
            for (buf, idx, store) in combs.iter_mut() {
                let len = buf.len();
                let y = buf.get(*idx).copied().unwrap_or(0.0);
                *store = crate::flush32(y * (1.0 - damp) + *store * damp);
                if let Some(slot) = buf.get_mut(*idx) {
                    *slot = x + *store * feedback;
                }
                *idx = (*idx + 1) % len;
                acc += y;
            }
            for (buf, idx) in aps.iter_mut() {
                let len = buf.len();
                let b = buf.get(*idx).copied().unwrap_or(0.0);
                if let Some(slot) = buf.get_mut(*idx) {
                    *slot = crate::flush32(acc + b * 0.5);
                }
                acc = b - acc;
                *idx = (*idx + 1) % len;
            }
            *o = src.get(i).copied().unwrap_or(0.0) + acc * mix * 3.0;
        }
    }
    out
}

/// Stereo feedback delay (ping-pong when `pingpong`), wet added on top of the dry signal; the
/// output grows by `tail` seconds.
pub fn delay(input: &Buf, seconds: f32, feedback: f32, mix: f32, pingpong: bool, tail: f32) -> Buf {
    let sr = input.sample_rate.max(1);
    let d = frames(sr, seconds).max(1);
    let n = input.len() + frames(sr, tail);
    let mut out = Buf::new(sr, n);
    let fb = feedback.clamp(0.0, 0.95);
    let (mut bl, mut br) = (vec![0.0f32; d], vec![0.0f32; d]);
    for i in 0..n {
        let (xl, xr) = (input.l.get(i).copied().unwrap_or(0.0), input.r.get(i).copied().unwrap_or(0.0));
        let k = i % d;
        let (yl, yr) = (bl.get(k).copied().unwrap_or(0.0), br.get(k).copied().unwrap_or(0.0));
        let (nl, nr) = if pingpong { (xl * 0.7 + xr * 0.3 + yr * fb, yl * fb) } else { (xl + yl * fb, xr + yr * fb) };
        if let Some(s) = bl.get_mut(k) {
            *s = crate::flush32(nl);
        }
        if let Some(s) = br.get_mut(k) {
            *s = crate::flush32(nr);
        }
        if let Some(o) = out.l.get_mut(i) {
            *o = xl + yl * mix;
        }
        if let Some(o) = out.r.get_mut(i) {
            *o = xr + yr * mix;
        }
    }
    out
}

/// Remove DC offset (one-pole high-pass at about 10 Hz). Asymmetric pulse waves and envelopes
/// otherwise leave the waveform off-centre, which wastes headroom and thumps at cuts.
pub fn dc_block(sr: u32, buf: &mut [f32]) {
    let r = 1.0 - (TAU * 10.0 / sr.max(1) as f32).min(0.5);
    let (mut x1, mut y1) = (0.0f32, 0.0f32);
    for v in buf.iter_mut() {
        let y = *v - x1 + r * y1;
        x1 = *v;
        y1 = crate::flush32(y);
        *v = y;
    }
}

/// Master bus: DC removal, gentle saturation, then peak-normalise to −1 dBFS.
pub fn master(buf: &mut Buf) {
    let sr = buf.sample_rate;
    buf.each(|c| dc_block(sr, c));
    buf.normalize(1.0);
    buf.each(|c| drive(c, 1.2));
    buf.normalize(0.89);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_bounded() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        for _ in 0..1000 {
            let (x, y) = (a.unit(), b.unit());
            assert_eq!(x, y);
            assert!((0.0..1.0).contains(&x));
        }
        assert_ne!(Rng::new(1).next_u32(), Rng::new(2).next_u32());
        assert_eq!(Rng::new(3).below(0), 0);
        assert!(Rng::new(3).pick::<u8>(&[]).is_none());
    }

    #[test]
    fn hostile_lengths_are_capped() {
        assert_eq!(frames(48_000, f32::NAN), 0);
        assert_eq!(frames(48_000, -3.0), 0);
        assert_eq!(frames(48_000, f32::INFINITY), 0);
        assert_eq!(frames(48_000, 1e9), frames(48_000, MAX_SECONDS));
        assert!(tone(0, 0.01, Wave::Saw, |_| 440.0, |_| 1.0).iter().all(|v| v.is_finite()));
        assert!(pluck(48_000, 0.0, 0.01, 2.0, &mut Rng::new(1)).iter().all(|v| v.is_finite()));
    }

    #[test]
    fn oscillators_stay_in_range() {
        for w in [Wave::Sine, Wave::Triangle, Wave::Saw, Wave::Pulse(0.5), Wave::Pulse(0.1)] {
            let t = tone(48_000, 0.1, w, |t| 100.0 + 5000.0 * t, |_| 1.0);
            assert!(t.iter().all(|v| v.abs() <= 1.6), "{w:?}");
        }
    }

    #[test]
    fn reverb_and_delay_add_a_tail() {
        let mut b = Buf::seconds(48_000, 0.1);
        b.l[0] = 1.0;
        b.r[0] = 1.0;
        let r = reverb(&b, 0.8, 0.5, 0.5, 1.0);
        assert_eq!(r.len(), b.len() + 48_000);
        assert!(r.l[30_000].abs() > 0.0 && r.peak().is_finite());
        let d = delay(&b, 0.05, 0.5, 0.5, true, 0.5);
        assert!(d.r.iter().skip(2000).any(|v| v.abs() > 0.01));
    }

    #[test]
    fn dc_block_centres_a_pulse_wave() {
        let mut p = tone(48_000, 1.0, Wave::Pulse(0.2), |_| 220.0, |_| 1.0);
        let before = p.iter().sum::<f32>() / p.len() as f32;
        dc_block(48_000, &mut p);
        let after = p.iter().skip(24_000).sum::<f32>() / 24_000.0;
        assert!(before < -0.5 && after.abs() < 0.01, "{before} {after}");
    }

    #[test]
    fn normalize_scrubs_non_finite() {
        let mut b = Buf::from_mono(48_000, &[0.5, f32::NAN, f32::INFINITY, -0.25]);
        b.normalize(1.0);
        assert_eq!(b.l, vec![1.0, 0.0, 0.0, -0.5]);
    }
}
