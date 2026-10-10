//! Procedural music beds: original, royalty-free background tracks generated from a style, a
//! tempo, a key and a seed.
//!
//! The generator writes on a 16th-note grid. A track has an intro (no kick), a main part, and for
//! longer tracks a two-bar break followed by a "drop" at the midpoint, then a last bar that rings
//! out. Because the music is generated, its beat grid is known exactly: [`Track::beats`],
//! [`Track::downbeats`] and [`Track::drop`] let an editor cut picture precisely on the beat.
//!
//! Everything is deterministic for a given [`MusicParams`] and sample rate.

use crate::biquad::FilterType::{BandPass, HighPass, LowPass};
use crate::sfx::{cowbell, hat, kick, snare};
use crate::synth::*;

/// A music style.
#[derive(Clone, Copy, Debug)]
pub struct StyleInfo {
    pub id: &'static str,
    pub label: &'static str,
    /// Default tempo (beats per minute).
    pub bpm: f32,
    pub minor: bool,
    /// What the style suits, for agents choosing one.
    pub hint: &'static str,
}

/// Every style.
pub const STYLES: &[StyleInfo] = &[
    StyleInfo {
        id: "phonk",
        label: "Drift Phonk",
        bpm: 130.0,
        minor: true,
        hint: "gaming edits, Minecraft/PvP montages, car edits, aggressive TikToks (cowbell melody, distorted 808)",
    },
    StyleInfo {
        id: "trap",
        label: "Trap",
        bpm: 140.0,
        minor: true,
        hint: "hype edits, flex moments, gaming highlights (half-time drums, hi-hat rolls, 808 glides)",
    },
    StyleInfo {
        id: "edm",
        label: "EDM / House",
        bpm: 126.0,
        minor: true,
        hint: "energetic vlogs, travel, fast montages (four-on-the-floor, pumping supersaws)",
    },
    StyleInfo {
        id: "chiptune",
        label: "Chiptune 8-bit",
        bpm: 150.0,
        minor: false,
        hint: "retro and Minecraft-style gaming, funny edits (square leads, arpeggios)",
    },
    StyleInfo {
        id: "lofi",
        label: "Lo-fi Hip Hop",
        bpm: 82.0,
        minor: true,
        hint: "chill vlogs, building timelapses, study/relax content (swing, dusty keys, vinyl)",
    },
    StyleInfo {
        id: "chill",
        label: "Chill Pop",
        bpm: 100.0,
        minor: false,
        hint: "YouTube vlogs, tutorials, background under voice (plucks, soft drums, major key)",
    },
    StyleInfo {
        id: "cinematic",
        label: "Epic Cinematic",
        bpm: 90.0,
        minor: true,
        hint: "intros, trailers, boss fights, dramatic reveals (big drums, strings, brass)",
    },
];

pub fn style(id: &str) -> Option<&'static StyleInfo> {
    let k = id.trim().to_ascii_lowercase();
    STYLES.iter().find(|s| s.id == k || s.label.eq_ignore_ascii_case(&k))
}

/// What to generate.
#[derive(Clone, Debug)]
pub struct MusicParams {
    pub style: String,
    /// Length in seconds (clamped to 2 … 300).
    pub seconds: f32,
    /// Tempo; the style's default when `None` (clamped to 50 … 200).
    pub bpm: Option<f32>,
    /// Key root as a pitch class (0 = C … 11 = B); seeded when `None`.
    pub key: Option<i32>,
    pub seed: u64,
}

/// A generated track and its exact timing.
#[derive(Clone, Debug)]
pub struct Track {
    pub buf: Buf,
    pub bpm: f32,
    /// Pitch class of the key root (0 = C).
    pub key: i32,
    pub minor: bool,
    /// Every beat (seconds), inside the track.
    pub beats: Vec<f32>,
    /// The first beat of every bar (seconds).
    pub downbeats: Vec<f32>,
    /// Where the drop lands (seconds), for tracks long enough to have one.
    pub drop: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MusicError {
    UnknownStyle(String),
}

impl std::fmt::Display for MusicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MusicError::UnknownStyle(s) => {
                let all: Vec<&str> = STYLES.iter().map(|s| s.id).collect();
                write!(f, "unknown music style `{s}` (styles: {})", all.join(", "))
            }
        }
    }
}

impl std::error::Error for MusicError {}

pub const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const MINOR: [i32; 7] = [0, 2, 3, 5, 7, 8, 10];
const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

/// Bar energy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Intro,
    Main,
    Break,
    Drop,
    Outro,
}

struct Ctx {
    sr: u32,
    step: f32,
    swing: f32,
    scale: &'static [i32; 7],
    root: i32,
}

impl Ctx {
    /// Frame of 16th `step` of `bar` (odd 16ths pushed by the swing).
    fn at(&self, bar: usize, step: usize) -> usize {
        let s = (bar * 16 + step) as f32 * self.step + if step % 2 == 1 { self.swing * self.step } else { 0.0 };
        frames(self.sr, s)
    }
    /// MIDI note of scale degree `d` (any integer; wraps octaves) above `base` octave.
    fn note(&self, d: i32, octave: i32) -> f32 {
        let o = d.div_euclid(7);
        let i = d.rem_euclid(7) as usize;
        let pc = self.scale.get(i).copied().unwrap_or(0);
        (12 * (octave + 1 + o) + self.root + pc) as f32
    }
    fn secs(&self, steps: f32) -> f32 {
        steps * self.step
    }
}

/// A repeating melodic motif: per 16th, a scale degree or a rest.
fn motif(rng: &mut Rng, density: f32, span: i32, eighths: bool) -> Vec<Option<i32>> {
    let mut d = rng.below(3) as i32;
    (0..16)
        .map(|i| {
            if eighths && i % 2 == 1 {
                return None;
            }
            let strong = i % 4 == 0;
            if rng.chance(if strong { density + 0.25 } else { density }) {
                d = (d + [-2, -1, -1, 0, 1, 1, 2, 3].get(rng.below(8)).copied().unwrap_or(0)).clamp(-1, span);
                Some(d)
            } else {
                None
            }
        })
        .collect()
}

/// A 16-step pattern string (`x` hit, `o` soft hit, anything else rest) as (step, gain).
fn pat(p: &str) -> Vec<(usize, f32)> {
    p.chars()
        .take(16)
        .enumerate()
        .filter_map(|(i, c)| match c {
            'x' => Some((i, 1.0)),
            'o' => Some((i, 0.55)),
            _ => None,
        })
        .collect()
}

pub fn generate(p: &MusicParams, sr: u32) -> Result<Track, MusicError> {
    let st = style(&p.style).ok_or_else(|| MusicError::UnknownStyle(p.style.clone()))?;
    let sr = sr.clamp(8_000, 192_000);
    let seconds = if p.seconds.is_finite() { p.seconds.clamp(2.0, 300.0) } else { 30.0 };
    let bpm = p.bpm.filter(|b| b.is_finite()).unwrap_or(st.bpm).clamp(50.0, 200.0);
    let mut rng = Rng::new(p.seed ^ (st.bpm as u64) << 40);
    let root = p.key.map(|k| k.rem_euclid(12)).unwrap_or_else(|| rng.below(12) as i32);
    let beat = 60.0 / bpm;
    let bar_len = beat * 4.0;
    let bars = ((seconds / bar_len).ceil() as usize).clamp(1, 400);
    let swing = if st.id == "lofi" { 0.22 } else { 0.0 };
    let cx = Ctx { sr, step: beat / 4.0, swing, scale: if st.minor { &MINOR } else { &MAJOR }, root };

    // structure
    let intro = if bars >= 8 {
        2
    } else if bars >= 4 {
        1
    } else {
        0
    };
    let drop_bar = (bars >= 12).then(|| (bars / 2).max(intro + 2));
    let part = |b: usize| -> Part {
        if bars > 2 && b + 1 == bars {
            Part::Outro
        } else if b < intro {
            Part::Intro
        } else if drop_bar.is_some_and(|d| b + 2 >= d && b < d) {
            Part::Break
        } else if drop_bar.is_some_and(|d| b >= d) {
            Part::Drop
        } else {
            Part::Main
        }
    };

    // harmony: a 4-chord loop (scale degrees)
    let progs: &[[i32; 4]] =
        if st.minor { &[[0, 5, 2, 6], [0, 3, 5, 4], [0, 6, 5, 6], [0, 5, 3, 4]] } else { &[[0, 4, 5, 3], [0, 5, 3, 4], [3, 4, 0, 5], [0, 3, 4, 3]] };
    let prog = progs.get(rng.below(progs.len())).copied().unwrap_or([0, 5, 2, 6]);
    let chord_of = |b: usize| prog.get(b % 4).copied().unwrap_or(0);

    let total = frames(sr, bars as f32 * bar_len + 3.0);
    let mut drums = Buf::new(sr, total);
    let mut music = Buf::new(sr, total);
    let mut pump_music = false;

    // pre-rendered drum voices (a few takes each so repeats aren't identical)
    let kicks: Vec<Vec<f32>> = (0..3).map(|i| kick(sr, 1.0 + i as f32 * 0.01, 0.45)).collect();
    let snares: Vec<Vec<f32>> = (0..4).map(|_| snare(sr, &mut rng)).collect();
    let hats: Vec<Vec<f32>> = (0..4).map(|_| hat(sr, &mut rng, false)).collect();
    let open_hat = hat(sr, &mut rng, true);
    let mut clap = noise(sr, 0.25, Noise::White, &mut rng, |t| perc(t, 0.001, 0.06) * (1.0 - 0.6 * ((t * 90.0) as u32 % 2) as f32 * f32::from(t < 0.03)));
    filter(sr, &mut clap, BandPass, |_| 1500.0, 0.8);
    let mut crash = noise(sr, 2.0, Noise::White, &mut rng, |t| perc(t, 0.001, 0.6));
    filter(sr, &mut crash, HighPass, |_| 4000.0, 0.7);

    let motif_a = motif(&mut rng, 0.45, 7, st.id == "chill" || st.id == "cinematic");
    let motif_b = motif(&mut rng, 0.55, 8, false);

    for b in 0..bars {
        let pt = part(b);
        let deg = chord_of(b);
        let full = matches!(pt, Part::Main | Part::Drop);
        let hot = pt == Part::Drop;
        let pick = |v: &Vec<Vec<f32>>, i: usize| v.get(i % v.len().max(1)).cloned().unwrap_or_default();

        if pt == Part::Outro {
            // ring out: crash + sustained chord + bass
            drums.add_mono(cx.at(b, 0), &crash, 0.35, 0.0);
            drums.add_mono(cx.at(b, 0), &pick(&kicks, 0), 0.9, 0.0);
            let len = bar_len + 1.5;
            for (k, d) in [0, 2, 4].iter().enumerate() {
                let hz = midi_hz(cx.note(deg + d, 4));
                let v = tone(sr, len, Wave::Triangle, |_| hz, |t| adsr(t, 0.02, 0.3, 0.6, 1.2, bar_len) * 0.18);
                music.add_mono(cx.at(b, 0), &v, 1.0, (k as f32 - 1.0) * 0.4);
            }
            let bass_hz = midi_hz(cx.note(deg, 2));
            let bass = tone(sr, len, Wave::Sine, |_| bass_hz, |t| adsr(t, 0.01, 0.2, 0.7, 1.0, bar_len) * 0.5);
            music.add_mono(cx.at(b, 0), &bass, 1.0, 0.0);
            continue;
        }
        if hot && drop_bar == Some(b) {
            drums.add_mono(cx.at(b, 0), &crash, 0.4, 0.0);
        }
        if pt == Part::Break && drop_bar == Some(b + 1) {
            // riser into the drop
            let len = bar_len;
            let mut r = noise(sr, len, Noise::White, &mut rng, |t| (t / len).powi(2) * 0.5);
            filter(sr, &mut r, BandPass, |t| glide(t, len, 400.0, 8000.0), 1.0);
            drums.add_mono(cx.at(b, 0), &r, 0.6, 0.0);
        }

        match st.id {
            "phonk" => {
                if full {
                    for (s, g) in pat("x.....x...x..x..") {
                        drums.add_mono(cx.at(b, s), &pick(&kicks, s), g, 0.0);
                    }
                    for (s, g) in pat("....x.......x...") {
                        drums.add_mono(cx.at(b, s), &clap, g * 0.8, 0.0);
                        drums.add_mono(cx.at(b, s), &pick(&snares, b), g * 0.5, 0.0);
                    }
                }
                if pt != Part::Intro {
                    for s in (0..16).step_by(2) {
                        drums.add_mono(cx.at(b, s), &pick(&hats, s + b), if s % 4 == 0 { 0.35 } else { 0.22 }, 0.25);
                    }
                }
                // cowbell melody (the phonk signature): the motif, an octave up in the drop
                let m = if b % 4 == 3 { &motif_b } else { &motif_a };
                for (s, d) in m.iter().enumerate() {
                    if let Some(d) = d {
                        let hz = midi_hz(cx.note(*d + deg.min(2), if hot { 5 } else { 4 }));
                        let v = cowbell(sr, hz, 0.3);
                        music.add_mono(cx.at(b, s), &v, if pt == Part::Intro { 0.25 } else { 0.42 }, if s % 2 == 0 { -0.15 } else { 0.15 });
                    }
                }
                if full {
                    // distorted 808 on the kicks, gliding to the chord root
                    for (s, _) in pat("x.....x...x.....") {
                        let hz = midi_hz(cx.note(deg, 1));
                        let len = cx.secs(if s == 10 { 6.0 } else { 4.0 });
                        let mut v = tone(sr, len, Wave::Sine, |t| glide(t, 0.05, hz * 1.5, hz), |t| adsr(t, 0.003, 0.2, 0.75, 0.05, len - 0.05));
                        drive(&mut v, if hot { 5.0 } else { 3.0 });
                        filter(sr, &mut v, LowPass, |_| 2500.0, 0.7);
                        music.add_mono(cx.at(b, s), &v, 0.55, 0.0);
                    }
                }
            }
            "trap" => {
                if full {
                    for (s, g) in pat(if b % 2 == 0 { "x......x..x....." } else { "x.x.......x..x.." }) {
                        drums.add_mono(cx.at(b, s), &pick(&kicks, s), g, 0.0);
                    }
                    for (s, g) in pat("........x.......") {
                        drums.add_mono(cx.at(b, s), &pick(&snares, b), g, 0.0);
                        drums.add_mono(cx.at(b, s), &clap, g * 0.6, 0.0);
                    }
                }
                if pt != Part::Intro {
                    for s in 0..16 {
                        let roll = full && (s == 14 || (b % 2 == 1 && s == 6)) && rng.chance(0.7);
                        if roll {
                            for k in 0..3 {
                                let at = cx.at(b, s) + frames(sr, cx.secs(k as f32 / 3.0));
                                drums.add_mono(at, &pick(&hats, k), 0.25, 0.3);
                            }
                        } else if s % 2 == 0 || hot {
                            drums.add_mono(cx.at(b, s), &pick(&hats, s), if s % 4 == 0 { 0.3 } else { 0.18 }, 0.3);
                        }
                    }
                }
                // dark bell melody
                for (s, d) in motif_a.iter().enumerate() {
                    if let Some(d) = d.filter(|_| s % 2 == 0) {
                        let hz = midi_hz(cx.note(d + deg.min(2), 5));
                        let v = tone(sr, 0.6, Wave::Sine, |_| hz, |t| perc(t, 0.002, 0.25) * 0.5);
                        let h = tone(sr, 0.6, Wave::Sine, |_| hz * 2.76, |t| perc(t, 0.001, 0.08) * 0.12);
                        music.add_mono(cx.at(b, s), &v, 0.5, 0.2);
                        music.add_mono(cx.at(b, s), &h, 0.5, 0.2);
                    }
                }
                // pad
                let pad_len = bar_len;
                for d in [0, 2, 4] {
                    let hz = midi_hz(cx.note(deg + d, 3));
                    let mut v = unison(sr, pad_len, Wave::Saw, hz, 3, 15.0, |t| adsr(t, 0.4, 0.2, 0.8, 0.3, pad_len - 0.3) * 0.12, &mut rng);
                    filter(sr, &mut v, LowPass, |_| 900.0, 0.7);
                    music.add_mono(cx.at(b, 0), &v, 0.7, 0.0);
                }
                if full {
                    for (s, _) in pat("x......x..x.....") {
                        let hz = midi_hz(cx.note(deg, 1));
                        let next = midi_hz(cx.note(chord_of(b + 1), 1));
                        let len = cx.secs(if s == 10 { 6.0 } else { 7.0 });
                        let mut v = tone(
                            sr,
                            len,
                            Wave::Sine,
                            |t| if s == 10 && t > len * 0.6 { glide(t - len * 0.6, len * 0.3, hz, next) } else { hz },
                            |t| adsr(t, 0.003, 0.3, 0.8, 0.05, len - 0.05),
                        );
                        drive(&mut v, 2.2);
                        music.add_mono(cx.at(b, s), &v, 0.55, 0.0);
                    }
                }
            }
            "edm" => {
                pump_music = true;
                if full || pt == Part::Break {
                    if pt != Part::Break {
                        for (s, g) in pat("x...x...x...x...") {
                            drums.add_mono(cx.at(b, s), &pick(&kicks, 0), g, 0.0);
                        }
                    }
                    for (s, g) in pat("....x.......x...") {
                        drums.add_mono(cx.at(b, s), &clap, g * 0.7, 0.0);
                    }
                    for (s, g) in pat("..x...x...x...x.") {
                        drums.add_mono(cx.at(b, s), &open_hat, g * 0.22, 0.2);
                    }
                }
                // supersaw chord stabs on the off-beats (sustained in the intro / break)
                let stab = !matches!(pt, Part::Intro | Part::Break);
                let positions: Vec<usize> = if stab { vec![2, 6, 10, 14] } else { vec![0] };
                let len = if stab { cx.secs(1.6) } else { bar_len };
                for s in positions {
                    for d in [0, 2, 4, 7] {
                        let hz = midi_hz(cx.note(deg + d, 4));
                        let mut v = unison(sr, len, Wave::Saw, hz, 5, 22.0, |t| adsr(t, 0.005, 0.1, 0.7, 0.05, len - 0.05) * 0.12, &mut rng);
                        filter(sr, &mut v, LowPass, |_| if hot { 7000.0 } else { 3000.0 }, 0.7);
                        music.add_mono(cx.at(b, s), &v, 0.8, if d % 2 == 0 { -0.3 } else { 0.3 });
                    }
                }
                if full {
                    for s in [2usize, 6, 10, 14] {
                        let hz = midi_hz(cx.note(deg, 2));
                        let v = tone(sr, cx.secs(1.8), Wave::Saw, |_| hz, |t| perc(t, 0.003, 0.12) * 0.5);
                        let mut v = v;
                        filter(sr, &mut v, LowPass, |_| 900.0, 1.0);
                        music.add_mono(cx.at(b, s), &v, 0.8, 0.0);
                    }
                }
                if hot {
                    for (s, d) in motif_b.iter().enumerate() {
                        if let Some(d) = d {
                            let v = pluck(sr, midi_hz(cx.note(*d + deg.min(2), 5)), 0.4, 0.3, &mut rng);
                            music.add_mono(cx.at(b, s), &v, 0.3, if s % 4 < 2 { -0.4 } else { 0.4 });
                        }
                    }
                }
            }
            "chiptune" => {
                if full {
                    for (s, g) in pat("x...x...x...x...") {
                        let v = tone(sr, 0.12, Wave::Pulse(0.5), |t| glide(t, 0.06, 180.0, 50.0), |t| perc(t, 0.001, 0.04) * 0.6);
                        drums.add_mono(cx.at(b, s), &v, g, 0.0);
                    }
                    for (s, g) in pat("....x.......x...") {
                        let mut v = noise(sr, 0.15, Noise::White, &mut rng, |t| perc(t, 0.001, 0.04));
                        bitcrush(&mut v, 4, 6);
                        drums.add_mono(cx.at(b, s), &v, g * 0.5, 0.0);
                    }
                }
                if pt != Part::Intro {
                    for s in (0..16).step_by(2) {
                        let mut v = noise(sr, 0.03, Noise::White, &mut rng, |t| perc(t, 0.0005, 0.008));
                        bitcrush(&mut v, 4, 3);
                        drums.add_mono(cx.at(b, s), &v, 0.18, 0.3);
                    }
                }
                // triangle bass arpeggio
                for s in (0..16).step_by(2) {
                    let d = [0, 4, 7, 4].get((s / 2) % 4).copied().unwrap_or(0);
                    let hz = midi_hz(cx.note(deg + d, 2));
                    let v = tone(sr, cx.secs(1.8), Wave::Triangle, |_| hz, |t| adsr(t, 0.002, 0.05, 0.8, 0.02, cx.secs(1.6)) * 0.5);
                    music.add_mono(cx.at(b, s), &v, 0.9, 0.0);
                }
                // square lead
                if pt != Part::Intro {
                    let m = if b % 4 == 3 { &motif_b } else { &motif_a };
                    for (s, d) in m.iter().enumerate() {
                        if let Some(d) = d {
                            let hz = midi_hz(cx.note(*d + deg.min(2), 5));
                            let len = cx.secs(1.8);
                            let v = tone(
                                sr,
                                len,
                                Wave::Pulse(0.25),
                                |t| hz * (1.0 + 0.006 * (std::f32::consts::TAU * 6.0 * t).sin()),
                                |t| adsr(t, 0.002, 0.05, 0.7, 0.03, len - 0.03) * 0.28,
                            );
                            music.add_mono(cx.at(b, s), &v, 0.9, -0.15);
                        }
                    }
                }
                // fast chord arpeggio (32nds) in the drop
                if hot {
                    for k in 0..32 {
                        let d = [0, 2, 4].get(k % 3).copied().unwrap_or(0);
                        let hz = midi_hz(cx.note(deg + d, 4));
                        let v = tone(sr, cx.secs(0.5), Wave::Pulse(0.125), |_| hz, |_| 0.12);
                        music.add_mono(cx.at(b, 0) + frames(sr, cx.secs(k as f32 / 2.0)), &v, 0.8, 0.35);
                    }
                }
            }
            "lofi" => {
                if full {
                    for (s, g) in pat("x......x.x......") {
                        drums.add_mono(cx.at(b, s), &pick(&kicks, 1), g * 0.85, 0.0);
                    }
                    for (s, g) in pat("....x.......x..o") {
                        drums.add_mono(cx.at(b, s), &pick(&snares, s + b), g * 0.6, 0.05);
                    }
                }
                if pt != Part::Intro {
                    for s in (0..16).step_by(2) {
                        drums.add_mono(cx.at(b, s), &pick(&hats, s), if s % 4 == 0 { 0.18 } else { 0.11 }, -0.2);
                    }
                }
                // electric piano 7th chords
                let len = bar_len;
                for (k, d) in [0, 2, 4, 6].iter().enumerate() {
                    let hz = midi_hz(cx.note(deg + d, 3 + i32::from(k == 3)));
                    let ep = tone(
                        sr,
                        len,
                        Wave::Sine,
                        |_| hz,
                        |t| adsr(t, 0.01, 0.6, 0.5, 0.4, len - 0.4) * (0.85 + 0.15 * (std::f32::consts::TAU * 4.5 * t).sin()) * 0.16,
                    );
                    let h = tone(sr, len, Wave::Sine, |_| hz * 2.0, |t| perc(t, 0.005, 0.2) * 0.05);
                    music.add_mono(cx.at(b, k % 2), &ep, 1.0, (k as f32 - 1.5) * 0.25);
                    music.add_mono(cx.at(b, 0), &h, 1.0, 0.0);
                }
                if pt != Part::Intro {
                    let hz = midi_hz(cx.note(deg, 2));
                    for (s, _) in pat("x......x.x......") {
                        let v = tone(sr, cx.secs(5.0), Wave::Sine, |_| hz, |t| adsr(t, 0.01, 0.2, 0.7, 0.1, cx.secs(4.5)) * 0.45);
                        music.add_mono(cx.at(b, s), &v, 1.0, 0.0);
                    }
                }
            }
            "chill" => {
                if full {
                    for (s, g) in pat("x.....x...x.....") {
                        drums.add_mono(cx.at(b, s), &pick(&kicks, 2), g * 0.8, 0.0);
                    }
                    for (s, g) in pat("....x.......x...") {
                        drums.add_mono(cx.at(b, s), &clap, g * 0.55, 0.0);
                    }
                }
                if pt != Part::Intro {
                    for s in 0..16 {
                        let mut v = noise(sr, 0.05, Noise::White, &mut rng, |t| perc(t, 0.004, 0.012));
                        filter(sr, &mut v, BandPass, |_| 7000.0, 1.2);
                        drums.add_mono(cx.at(b, s), &v, if s % 2 == 0 { 0.06 } else { 0.035 }, 0.35);
                    }
                }
                // plucked arpeggio
                for s in (0..16).step_by(if hot { 1 } else { 2 }) {
                    let d = [0, 2, 4, 7, 4, 2].get((s / if hot { 1 } else { 2 }) % 6).copied().unwrap_or(0);
                    let mut v = pluck(sr, midi_hz(cx.note(deg + d, 4)), 0.6, 0.6, &mut rng);
                    filter(sr, &mut v, LowPass, |_| 3500.0, 0.7);
                    music.add_mono(cx.at(b, s), &v, 0.32, if s % 4 < 2 { -0.35 } else { 0.35 });
                }
                let len = bar_len;
                for d in [0, 2, 4] {
                    let hz = midi_hz(cx.note(deg + d, 3));
                    let mut v = unison(sr, len, Wave::Triangle, hz, 2, 8.0, |t| adsr(t, 0.3, 0.2, 0.8, 0.3, len - 0.3) * 0.1, &mut rng);
                    filter(sr, &mut v, LowPass, |_| 1800.0, 0.7);
                    music.add_mono(cx.at(b, 0), &v, 0.8, 0.0);
                }
                if pt != Part::Intro {
                    let hz = midi_hz(cx.note(deg, 2));
                    let v = tone(sr, bar_len, Wave::Sine, |_| hz, |t| adsr(t, 0.01, 0.3, 0.6, 0.2, bar_len - 0.2) * 0.4);
                    music.add_mono(cx.at(b, 0), &v, 1.0, 0.0);
                }
            }
            _ => {
                // cinematic
                if full || pt == Part::Break {
                    let p = if hot { "x..x..x.x..x.x.x" } else { "x.......x..x...." };
                    for (s, g) in pat(p) {
                        let tom = tone(sr, 0.8, Wave::Sine, |t| glide(t, 0.2, 110.0, 55.0), |t| perc(t, 0.002, 0.25));
                        let mut skin = noise(sr, 0.3, Noise::Brown, &mut rng, |t| perc(t, 0.001, 0.05));
                        filter(sr, &mut skin, LowPass, |_| 1200.0, 0.7);
                        drums.add_mono(cx.at(b, s), &tom, g, (s as f32 / 15.0 - 0.5) * 0.4);
                        drums.add_mono(cx.at(b, s), &skin, g * 0.6, 0.0);
                    }
                }
                let len = bar_len + 0.3;
                for (k, d) in [0, 2, 4, 7].iter().enumerate() {
                    let hz = midi_hz(cx.note(deg + d, 3));
                    let mut v = unison(sr, len, Wave::Saw, hz, 4, 12.0, |t| adsr(t, 0.6, 0.3, 0.85, 0.5, len - 0.5) * 0.09, &mut rng);
                    filter(sr, &mut v, LowPass, |_| if hot { 3500.0 } else { 1600.0 }, 0.7);
                    music.add_mono(cx.at(b, 0), &v, 1.0, (k as f32 - 1.5) * 0.4);
                }
                let drone = midi_hz(cx.note(0, 1));
                let v = tone(sr, len, Wave::Saw, |_| drone, |t| adsr(t, 0.3, 0.2, 0.9, 0.4, len - 0.4) * 0.12);
                let mut v = v;
                filter(sr, &mut v, LowPass, |_| 300.0, 0.7);
                music.add_mono(cx.at(b, 0), &v, 1.0, 0.0);
                if hot {
                    for s in [0usize, 10] {
                        for d in [0, 4, 7] {
                            let hz = midi_hz(cx.note(deg + d, 3));
                            let mut br = unison(sr, cx.secs(3.0), Wave::Saw, hz, 3, 8.0, |t| adsr(t, 0.03, 0.15, 0.6, 0.1, cx.secs(2.5)) * 0.12, &mut rng);
                            filter(sr, &mut br, LowPass, |t| 800.0 + 2000.0 * perc(t, 0.02, 0.15), 1.2);
                            music.add_mono(cx.at(b, s), &br, 1.0, 0.0);
                        }
                    }
                }
            }
        }
    }

    // buses
    if st.id == "lofi" {
        // dusty: low-pass, vinyl crackle
        music.each(|c| filter(sr, c, LowPass, |_| 3800.0, 0.6));
        drums.each(|c| filter(sr, c, LowPass, |_| 6000.0, 0.6));
        let len = music.len();
        let mut crackle = vec![0.0f32; len];
        let mut i = 0usize;
        while i < len {
            if let Some(v) = crackle.get_mut(i) {
                *v = rng.bipolar() * 0.25;
            }
            i += frames(sr, rng.range(0.01, 0.12)).max(1);
        }
        filter(sr, &mut crackle, BandPass, |_| 3000.0, 0.7);
        music.add_mono(0, &crackle, 0.3, 0.0);
    }
    if pump_music {
        // side-chain style pump on every beat
        for (i, (l, r)) in music.l.iter_mut().zip(music.r.iter_mut()).enumerate() {
            let t = i as f32 / sr as f32;
            let ph = t % beat;
            let g = 1.0 - 0.75 * (-ph / 0.09).exp();
            *l *= g;
            *r *= g;
        }
    }
    let (room, wet) = match st.id {
        "cinematic" => (0.9, 0.35),
        "lofi" | "chill" => (0.6, 0.2),
        "trap" | "phonk" => (0.5, 0.12),
        _ => (0.5, 0.1),
    };
    let music = reverb(&music, room, 0.5, wet, 0.0);
    let mut mix = drums;
    mix.add(0, &music, 1.0);
    let n = frames(sr, seconds);
    mix.truncate(n);
    // a cut that doesn't land on the last bar's end fades out over at most 2 s
    let bar_aligned = ((seconds / bar_len).fract() < 0.02) || (seconds / bar_len).fract() > 0.98;
    mix.fade_out(if bar_aligned { frames(sr, 0.05) } else { frames(sr, (seconds * 0.15).min(2.0)) });
    mix.fade_in(frames(sr, 0.005));
    master(&mut mix);

    let beats: Vec<f32> = (0..).map(|i| i as f32 * beat).take_while(|t| *t < seconds).collect();
    let downbeats: Vec<f32> = beats.iter().copied().step_by(4).collect();
    let drop = drop_bar.map(|d| d as f32 * bar_len).filter(|t| *t < seconds);
    Ok(Track { buf: mix, bpm, key: root, minor: st.minor, beats, downbeats, drop })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(b: &Buf, from: f32, to: f32) -> f32 {
        let sr = b.sample_rate as f32;
        let (a, z) = ((from * sr) as usize, ((to * sr) as usize).min(b.len()));
        let n = z.saturating_sub(a).max(1) as f32;
        (b.l.iter().skip(a).take(z - a).map(|v| v * v).sum::<f32>() / n).sqrt()
    }

    #[test]
    fn every_style_renders_the_requested_length() {
        for s in STYLES {
            let t = generate(&MusicParams { style: s.id.into(), seconds: 8.0, bpm: None, key: Some(9), seed: 1 }, 22_050).unwrap();
            assert_eq!(t.buf.len(), frames(22_050, 8.0), "{}", s.id);
            assert!(t.buf.peak() > 0.5 && t.buf.peak() <= 0.9, "{} peak {}", s.id, t.buf.peak());
            assert!(rms(&t.buf, 2.0, 7.0) > 0.02, "{} is too quiet", s.id);
            assert!(t.buf.l.iter().chain(&t.buf.r).all(|v| v.is_finite()), "{}", s.id);
            assert_eq!(t.bpm, s.bpm);
            assert!((t.beats[1] - 60.0 / s.bpm).abs() < 1e-4);
        }
    }

    #[test]
    fn long_tracks_have_a_drop_on_a_downbeat() {
        let t = generate(&MusicParams { style: "phonk".into(), seconds: 30.0, bpm: Some(128.0), key: None, seed: 4 }, 16_000).unwrap();
        let d = t.drop.unwrap();
        assert!(t.downbeats.iter().any(|b| (b - d).abs() < 1e-3), "{d} not on a downbeat");
        assert!(d > 10.0 && d < 20.0);
        assert!(t.beats.iter().all(|b| *b < 30.0));
    }

    #[test]
    fn hostile_params_are_clamped_and_deterministic() {
        let p = MusicParams { style: "LOFI".into(), seconds: f32::NAN, bpm: Some(-5.0), key: Some(-13), seed: u64::MAX };
        let a = generate(&p, 0).unwrap();
        assert_eq!(a.bpm, 50.0);
        assert_eq!(a.key, 11);
        let p2 = MusicParams { seconds: 1e9, ..p.clone() };
        assert!(generate(&p2, 8_000).unwrap().buf.duration() <= 300.5);
        let small = MusicParams { style: "edm".into(), seconds: 3.0, bpm: None, key: Some(0), seed: 9 };
        assert_eq!(generate(&small, 16_000).unwrap().buf, generate(&small, 16_000).unwrap().buf);
        assert!(matches!(generate(&MusicParams { style: "polka".into(), ..small }, 16_000), Err(MusicError::UnknownStyle(_))));
    }
}
