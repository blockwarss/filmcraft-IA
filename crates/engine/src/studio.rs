//! Studio: high-level editing recipes for agents (and people) cutting short-form and YouTube
//! content — the moves a professional editor makes dozens of times per video, as one command each.
//!
//! | command | what it does |
//! |---|---|
//! | `studio.sfx.list` / `studio.sfx.add` | built-in, synthesised sound effects ([`filmcraft_audio_dsp::sfx`]): list them, drop one on a free audio track |
//! | `studio.music.styles` / `studio.music.generate` | original music beds ([`filmcraft_audio_dsp::music`]) with their exact beat grid |
//! | `studio.silences` / `studio.cutSilences` | find / ripple-delete the silent parts of the recorded sound (jump cuts) |
//! | `studio.beats` / `studio.markBeats` / `studio.beatCut` / `studio.beatMontage` | beat grid of a music clip; markers, cuts and a montage on the beat |
//! | `studio.punchZoom` / `studio.shake` | keyframed zoom punch-in and camera shake |
//! | `studio.format` | turn the sequence vertical (TikTok / Reels / Shorts), square or landscape, filling the frame |
//! | `studio.captions` | styled captions from the transcript (TikTok, gaming, YouTube, minimal) |
//! | `studio.title` | styled, animated title text (hook, gaming, meme, subscribe, lower third, neon) |
//! | `studio.guide` | put a clip on a "Guide" audio track (a trending sound to cut to, left out of the export) |
//! | `studio.library.*` | the user's own sound / music / clip folders: scan, search, place |
//! | `studio.fonts.install` / `studio.fonts.list` | install a font file for titles and captions |
//! | `studio.exportSettings` | `file.exportMedia` parameters for TikTok / Reels / Shorts / YouTube |
//!
//! Every recipe that edits is one undo step ([`Session::compound`]). Generated sounds and music
//! are written as WAV files to the studio folder (`<data dir>/studio`, see [`library_dir`]) and
//! imported like any other media, so projects stay portable and the files can be reused.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use filmcraft_audio_dsp::{music, sfx, synth};
use filmcraft_project::{ClipId, Interpolation, ItemId, ItemKind, Label, Marker, MarkerId, MarkerKind, MediaRef, ParamValue, TrackId, TrackKind};
use filmcraft_time::{TICKS_PER_SECOND, Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, f64_p, has_seq, str_p, time_p, u64_p};
use crate::{EngineError, Result, Session};

// ------------------------------------------------------------------------------- folders

fn override_dir() -> &'static Mutex<Option<PathBuf>> {
    static D: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    D.get_or_init(Default::default)
}

/// Use `dir` as the studio folder instead of `<data dir>/studio` (hosts and tests).
pub fn set_library_dir(dir: Option<PathBuf>) {
    *override_dir().lock().unwrap_or_else(|e| e.into_inner()) = dir;
}

/// The studio folder: generated sounds (`sfx/`), music (`music/`), installed fonts (`fonts/`),
/// downloads (`downloads/`) and the user's own library (`my-library/` plus the folders listed in
/// `library.json`). `None` when the platform has no data folder (the web).
pub fn library_dir() -> Option<PathBuf> {
    if let Some(d) = override_dir().lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Some(d);
    }
    crate::autosave::default_data_dir().map(|d| d.join("studio"))
}

pub(crate) fn sub_dir(name: &str) -> Result<PathBuf> {
    let d = library_dir().ok_or_else(|| EngineError::Other("no studio folder on this platform".into()))?.join(name);
    std::fs::create_dir_all(&d).map_err(|e| EngineError::Other(format!("{}: {e}", d.display())))?;
    Ok(d)
}

pub(crate) fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

// ------------------------------------------------------------------------------- helpers

pub(crate) fn secs(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

fn label_p(p: &Value) -> Label {
    str_p(p, "color").and_then(Label::from_name).unwrap_or(Label::Cerulean)
}

/// The project item that already holds the file at `path`.
fn item_for_path(s: &Session, path: &str) -> Option<ItemId> {
    s.project.items.iter().find_map(|(id, it)| match &it.kind {
        ItemKind::Media(m) => matches!(&m.media, MediaRef::File { path: q } if q == path).then_some(*id),
        _ => None,
    })
}

/// Import `path` (or reuse the item that already holds it).
pub(crate) fn import(s: &mut Session, path: &str) -> Result<ItemId> {
    if let Some(i) = item_for_path(s, path) {
        return Ok(i);
    }
    let r = s.execute("file.import", json!({"paths": [path]}))?;
    r["items"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(Value::as_u64)
        .map(ItemId)
        .ok_or_else(|| EngineError::Other(format!("could not import {path}: {}", r["errors"])))
}

/// An unlocked audio track with nothing in `[t, t + dur)`, preferring `prefer`; adds a track
/// when every one is busy.
fn free_audio_track(s: &mut Session, t: Tick, dur: Tick, skip_first: bool) -> Result<TrackId> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let r = TimeRange::new(t, dur);
    let free = q
        .audio_tracks
        .iter()
        .skip(usize::from(skip_first && q.audio_tracks.len() > 1))
        .find(|tr| !tr.locked && tr.items.iter().all(|it| it.end() <= r.start || it.start >= r.end()) && !tr.name.eq_ignore_ascii_case(GUIDE));
    if let Some(tr) = free {
        return Ok(tr.id);
    }
    s.execute("sequence.addTracks", json!({"video": 0, "audio": 1}))?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    q.audio_tracks.last().map(|t| t.id).ok_or_else(|| EngineError::Other("could not add an audio track".into()))
}

/// Place an audio item at `t` on a free audio track (or `track`) with a gain; returns the clip.
fn place_audio(s: &mut Session, item: ItemId, t: Tick, track: Option<&Value>, gain_db: f64, skip_first: bool) -> Result<(ClipId, TrackId)> {
    let dur = s.project.item(item).map(|i| i.duration()).unwrap_or_default();
    let tid = match track {
        Some(v) if !v.is_null() => crate::commands::track_p(s, &json!({"track": v}), "track", "studio")?.ok_or_else(|| bad("studio", "no such track"))?,
        _ => free_audio_track(s, t, dur, skip_first)?,
    };
    let r = s.execute("timeline.place", json!({"item": item.0, "track": tid.0, "time": t.0, "video": false}))?;
    let clip =
        r["clips"].as_array().and_then(|a| a.first()).and_then(Value::as_u64).map(ClipId).ok_or_else(|| EngineError::Other("nothing was placed".into()))?;
    if gain_db.abs() > 1e-6 {
        s.execute("clip.audioGain", json!({"clips": [clip.0], "mode": "set", "db": gain_db.clamp(-60.0, 24.0)}))?;
    }
    Ok((clip, tid))
}

fn write_wav(s: &Session, path: &Path, b: &synth::Buf) -> Result<()> {
    let bytes = filmcraft_export::write_wav(&b.interleaved(), 2, b.sample_rate, 16);
    s.services.write_file(&path_str(path), &bytes).map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))
}

fn exists(s: &Session, path: &Path) -> bool {
    s.services.file_size(&path_str(path)).is_ok_and(|n| n > 44)
}

/// `time` / `seconds` / `frame` / `timecode`, else the playhead.
pub(crate) fn at_p(s: &Session, p: &Value) -> Tick {
    time_p(s, p, "").unwrap_or(s.playhead()).max(Tick::ZERO)
}

fn sample_rate(s: &Session) -> u32 {
    s.active_sequence().map(|q| q.settings.sample_rate).unwrap_or(48_000).clamp(8_000, 192_000)
}

/// Snap a range inward to whole frames.
fn snap_in(s: &Session, r: TimeRange) -> Option<TimeRange> {
    let fd = s.sequence_rate().frame_duration().0.max(1);
    let a = r.start.0.div_euclid(fd) * fd + if r.start.0.rem_euclid(fd) == 0 { 0 } else { fd };
    let b = r.end().0.div_euclid(fd) * fd;
    (b > a).then(|| TimeRange::new(Tick(a), Tick(b - a)))
}

// ------------------------------------------------------------------------------- SFX

fn sfx_list(_s: &mut Session, p: &Value) -> Result<Value> {
    let cat = str_p(p, "category").map(str::to_ascii_lowercase);
    let q = str_p(p, "query").map(str::to_ascii_lowercase);
    let v: Vec<Value> = sfx::CATALOG
        .iter()
        .filter(|x| cat.as_deref().is_none_or(|c| x.category == c))
        .filter(|x| {
            q.as_deref()
                .is_none_or(|q| x.id.contains(q) || x.label.to_ascii_lowercase().contains(q) || x.tags.iter().any(|t| t.contains(q)) || x.hint.contains(q))
        })
        .map(|x| json!({"id": x.id, "label": x.label, "category": x.category, "tags": x.tags, "use": x.hint}))
        .collect();
    Ok(
        json!({"sounds": v, "categories": ["transition", "impact", "ui", "meme", "game", "rhythm"], "variants": "pass `variant` 1, 2, … for slightly different takes of the same sound"}),
    )
}

/// Render (or reuse) the WAV of a built-in sound.
pub fn sfx_file(s: &Session, id: &str, variant: u32) -> Result<PathBuf> {
    let info = sfx::info(id).ok_or_else(|| bad("studio.sfx.add", format!("unknown sound `{id}` (studio.sfx.list lists them)")))?;
    let sr = sample_rate(s);
    let path = sub_dir("sfx")?.join(format!("{}-v{variant}-{sr}.wav", info.id));
    if !exists(s, &path) {
        let b = sfx::render(info.id, variant, sr).ok_or_else(|| bad("studio.sfx.add", "unknown sound"))?;
        write_wav(s, &path, &b)?;
    }
    Ok(path)
}

fn sfx_add(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_p(p, "sound").or_else(|| str_p(p, "id")).ok_or_else(|| bad("studio.sfx.add", "need `sound` (an id from studio.sfx.list)"))?.to_string();
    let variant = u64_p(p, "variant").unwrap_or(0).min(999) as u32;
    let t = at_p(s, p);
    let gain = f64_p(p, "gainDb").unwrap_or(0.0);
    let path = sfx_file(s, &id, variant)?;
    let track = p.get("track").cloned();
    s.compound(&format!("Add Sound Effect ({id})"), |s| {
        let item = import(s, &path_str(&path))?;
        let (clip, tid) = place_audio(s, item, t, track.as_ref(), gain, true)?;
        let dur = s.project.item(item).map(|i| secs(i.duration())).unwrap_or(0.0);
        Ok(json!({"clip": clip.0, "item": item.0, "track": tid.0, "path": path_str(&path), "seconds": secs(t), "duration": dur}))
    })
}

// ------------------------------------------------------------------------------- music

fn music_styles(_s: &mut Session, _p: &Value) -> Result<Value> {
    let v: Vec<Value> = music::STYLES
        .iter()
        .map(|x| json!({"id": x.id, "label": x.label, "bpm": x.bpm, "mode": if x.minor { "minor" } else { "major" }, "use": x.hint}))
        .collect();
    Ok(json!({"styles": v}))
}

fn key_p(p: &Value) -> Result<Option<i32>> {
    const CMD: &str = "studio.music.generate";
    match p.get("key") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(n.as_i64().map(|k| k.rem_euclid(12) as i32)),
        Some(Value::String(k)) => {
            // "A", "F#", "Bb", "C#m", "A minor"
            let k = k.trim().replace('♯', "#").replace('♭', "b");
            let k = k.trim_end_matches(" minor").trim_end_matches(" major").trim_end_matches('m').trim();
            let mut c = k.chars();
            let letter = c.next().map(|x| x.to_ascii_uppercase()).ok_or_else(|| bad(CMD, "empty `key`"))?;
            let base = music::NAMES
                .iter()
                .position(|n| n.len() == 1 && n.starts_with(letter))
                .ok_or_else(|| bad(CMD, format!("unknown key `{k}` (C, C#, D … B, or 0–11)")))? as i32;
            let shift = match c.as_str() {
                "" => 0,
                "#" => 1,
                "b" => -1,
                _ => return Err(bad(CMD, format!("unknown key `{k}` (C, C#, D … B, or 0–11)"))),
            };
            Ok(Some((base + shift).rem_euclid(12)))
        }
        Some(_) => Err(bad(CMD, "`key` must be a note name or 0–11")),
    }
}

/// Beat grid sidecar written next to generated music (media time, seconds).
fn beats_sidecar(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".beats.json");
    PathBuf::from(s)
}

fn music_generate(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "studio.music.generate";
    let style = str_p(p, "style").unwrap_or("phonk").to_string();
    let info = music::style(&style).ok_or_else(|| bad(CMD, music::MusicError::UnknownStyle(style.clone()).to_string()))?;
    let seq_len = s.active_sequence().map(|q| secs(q.duration())).unwrap_or(0.0);
    let seconds = f64_p(p, "durationSeconds").or_else(|| f64_p(p, "length")).unwrap_or(if seq_len > 2.0 { seq_len } else { 30.0 });
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(bad(CMD, "`durationSeconds` must be a positive number"));
    }
    let seconds = seconds.clamp(2.0, 300.0) as f32;
    let bpm = f64_p(p, "bpm").map(|b| b as f32);
    let key = key_p(p)?;
    let seed = u64_p(p, "seed").unwrap_or(1);
    let sr = sample_rate(s);
    let params = music::MusicParams { style: info.id.into(), seconds, bpm, key, seed };
    let tag =
        format!("{}-{}bpm-{}-s{seed}-{:.0}s-{sr}", info.id, bpm.unwrap_or(info.bpm).round(), key.map_or("auto".into(), |k| k.to_string()), seconds * 10.0);
    let path = sub_dir("music")?.join(format!("{tag}.wav"));
    let side = beats_sidecar(&path);
    let grid: Value = match s.services.read_file(&path_str(&side)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).filter(|_| exists(s, &path)) {
        Some(g) => g,
        None => {
            let t = music::generate(&params, sr).map_err(|e| bad(CMD, e.to_string()))?;
            write_wav(s, &path, &t.buf)?;
            let g = json!({"style": info.id, "bpm": t.bpm, "key": music::NAMES.get(t.key as usize).copied().unwrap_or("C"), "mode": if t.minor { "minor" } else { "major" }, "beats": t.beats, "downbeats": t.downbeats, "drop": t.drop, "seconds": seconds});
            let _ = s.services.write_file(&path_str(&side), g.to_string().as_bytes());
            g
        }
    };
    let place = bool_p(p, "place").unwrap_or(true);
    let at = time_p(s, p, "").unwrap_or(Tick::ZERO).max(Tick::ZERO);
    let gain = f64_p(p, "gainDb").unwrap_or(-8.0);
    let track = p.get("track").cloned();
    let ms = |t: f64| (t * 1000.0).round() / 1000.0;
    let to_seq = |v: &Value| -> Vec<f64> { v.as_array().map(|a| a.iter().filter_map(Value::as_f64).map(|b| ms(secs(at) + b)).collect()).unwrap_or_default() };
    let (beats, downbeats) = (to_seq(&grid["beats"]), to_seq(&grid["downbeats"]));
    let drop = grid["drop"].as_f64().map(|d| ms(secs(at) + d));
    s.compound(&format!("Generate Music ({})", info.label), |s| {
        let item = import(s, &path_str(&path))?;
        let mut out = json!({"item": item.0, "path": path_str(&path), "style": info.id, "bpm": grid["bpm"], "key": grid["key"], "mode": grid["mode"], "duration": seconds, "licence": "original music generated by FilmCraft: free to use anywhere"});
        if place && s.active_sequence().is_some() {
            let (clip, tid) = place_audio(s, item, at, track.as_ref(), gain, true)?;
            let _ = s.execute("essentialSound.setType", json!({"clips": [clip.0], "type": "music"}));
            out["clip"] = json!(clip.0);
            out["track"] = json!(tid.0);
            out["beats"] = json!(beats);
            out["downbeats"] = json!(downbeats);
            out["drop"] = json!(drop);
            out["note"] = json!("beats/downbeats/drop are sequence times in seconds: cut, zoom and add impacts on them");
        }
        Ok(out)
    })
}

// ------------------------------------------------------------------------------- silences

const WINDOW_S: f64 = 0.02;
const ANALYSIS_SR: u32 = 16_000;
/// Longest stretch of sound analysed (4 hours of sequence time).
const MAX_WINDOWS: usize = 720_000;

fn is_music(it: &filmcraft_project::TrackItem) -> bool {
    it.essential.as_ref().is_some_and(|e| e.kind == filmcraft_project::essential::AudioType::Music)
}

/// The audio clips whose sound decides what is silent.
fn speech_clips(s: &Session, p: &Value) -> Vec<ClipId> {
    let Some(q) = s.active_sequence() else { return vec![] };
    let is_audio = |c: &ClipId| q.find_item(*c).and_then(|(t, _)| q.track(t)).is_some_and(|t| t.kind == TrackKind::Audio);
    if let Some(a) = p.get("clips").and_then(Value::as_array) {
        return a.iter().filter_map(Value::as_u64).map(ClipId).filter(is_audio).collect();
    }
    let names: Option<Vec<String>> =
        p.get("tracks").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_ascii_lowercase)).collect());
    let mut by_track = Vec::new();
    let mut linked = Vec::new();
    for (i, t) in q.audio_tracks.iter().enumerate() {
        if t.muted || t.name.eq_ignore_ascii_case(GUIDE) {
            continue;
        }
        let tag = format!("a{}", i + 1);
        let named = names.as_ref().is_some_and(|n| n.contains(&tag) || n.contains(&t.name.to_ascii_lowercase()));
        for it in t.items.iter().filter(|it| it.enabled) {
            if named {
                by_track.push(it.id);
            } else if it.link.is_some() && !is_music(it) {
                linked.push(it.id);
            }
        }
    }
    if names.is_some() {
        return by_track;
    }
    if !linked.is_empty() {
        return linked;
    }
    q.audio_tracks.iter().filter(|t| !t.muted && !t.name.eq_ignore_ascii_case(GUIDE)).flat_map(|t| t.items.iter().filter(|i| i.enabled).map(|i| i.id)).collect()
}

/// Sound level (dBFS, max over the clips) per 20 ms window from `origin`.
fn levels(s: &Session, clips: &[ClipId]) -> Result<(Tick, Vec<f32>)> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let items: Vec<_> = clips.iter().filter_map(|c| q.find_item(*c).map(|(_, it)| it.clone())).collect();
    let origin = items.iter().map(|i| i.start).min().unwrap_or_default();
    let end = items.iter().map(|i| i.end()).max().unwrap_or_default();
    let win = Tick::from_seconds_f64(WINDOW_S);
    let n = ((end.0 - origin.0).max(0) / win.0.max(1)) as usize;
    if n > MAX_WINDOWS {
        return Err(EngineError::Other("the sound to analyse is longer than 4 hours: pass `clips` or `tracks` to narrow it".into()));
    }
    let mut lv = vec![f32::NEG_INFINITY; n];
    let per = (ANALYSIS_SR as f64 * WINDOW_S) as usize;
    for it in &items {
        let first = ((it.start.0 - origin.0) / win.0.max(1)).max(0) as usize;
        let count = (it.duration.0 / win.0.max(1)).max(0) as usize;
        let forward = (it.speed - 1.0).abs() < 1e-9 && !it.reverse && it.frame_hold.is_none();
        let src = if forward { s.source(it.item) } else { None };
        let Some(src) = src else {
            // can't analyse (speed change, offline…): never call it silent
            for v in lv.iter_mut().skip(first).take(count) {
                *v = 0.0;
            }
            continue;
        };
        let gain = 10f32.powf(it.gain_db as f32 / 20.0);
        let m0 = it.source_in.to_units_floor(ANALYSIS_SR as i64);
        let chunk = 1500; // windows per read (30 s)
        let mut w = 0usize;
        while w < count {
            let k = chunk.min(count - w);
            let buf = src.audio(m0 + (w * per) as i64, k * per, ANALYSIS_SR).map_err(|e| EngineError::Other(format!("reading {}: {e}", it.name)))?;
            for j in 0..k {
                let mut sum = 0.0f64;
                let mut cnt = 0usize;
                for ch in &buf.channels {
                    for v in ch.iter().skip(j * per).take(per) {
                        sum += (*v as f64) * (*v as f64);
                        cnt += 1;
                    }
                }
                let rms = (sum / cnt.max(1) as f64).sqrt() as f32 * gain;
                let db = if rms > 1e-7 { 20.0 * rms.log10() } else { -140.0 };
                if let Some(slot) = lv.get_mut(first + w + j) {
                    *slot = slot.max(db);
                }
            }
            w += k;
        }
    }
    Ok((origin, lv))
}

/// Threshold between the noise floor and speech: 30 % of the way up from the 10th percentile to
/// the 95th, at least 6 dB above the floor, within −60 … −18 dBFS.
pub fn auto_threshold(levels: &[f32]) -> f32 {
    let mut v: Vec<f32> = levels.iter().copied().filter(|x| x.is_finite() && *x > -120.0).collect();
    if v.is_empty() {
        return -45.0;
    }
    v.sort_by(f32::total_cmp);
    let at = |q: f32| v.get(((v.len() - 1) as f32 * q) as usize).copied().unwrap_or(-60.0);
    let (floor, loud) = (at(0.10), at(0.95));
    (floor + 0.3 * (loud - floor)).max(floor + 6.0).clamp(-60.0, -18.0)
}

/// Silent ranges (sequence time) of the levels.
fn silent_ranges(s: &Session, origin: Tick, lv: &[f32], thr: f32, min: f64, pad: f64) -> Vec<TimeRange> {
    let win = Tick::from_seconds_f64(WINDOW_S);
    let min_w = (min / WINDOW_S).ceil().max(1.0) as usize;
    let pad_t = Tick::from_seconds_f64(pad.max(0.0));
    let mut out = Vec::new();
    let mut i = 0;
    while i < lv.len() {
        if lv.get(i).is_some_and(|v| *v < thr) {
            let a = i;
            while lv.get(i).is_some_and(|v| *v < thr) {
                i += 1;
            }
            if i - a >= min_w {
                let start = origin + Tick(win.0 * a as i64) + pad_t;
                let end = origin + Tick(win.0 * i as i64) - pad_t;
                // keep air at the very start / end of the sound only when it is the whole run
                if end > start
                    && let Some(r) = snap_in(s, TimeRange::new(start, end - start))
                {
                    out.push(r);
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

fn silence_params(s: &Session, p: &Value) -> Result<(Vec<TimeRange>, f32, usize)> {
    let clips = speech_clips(s, p);
    if clips.is_empty() {
        return Err(bad("studio.silences", "no audio clip to analyse: put the recording on an audio track, or pass `clips` / `tracks`"));
    }
    let (origin, lv) = levels(s, &clips)?;
    let thr = match p.get("thresholdDb") {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(-45.0).clamp(-100.0, 0.0) as f32,
        _ => auto_threshold(&lv),
    };
    let min = f64_p(p, "minSeconds").unwrap_or(0.5).clamp(0.05, 60.0);
    let pad = f64_p(p, "padSeconds").unwrap_or(0.12).clamp(0.0, 2.0);
    Ok((silent_ranges(s, origin, &lv, thr, min, pad), thr, clips.len()))
}

fn ranges_json(r: &[TimeRange]) -> Vec<Value> {
    r.iter()
        .map(|r| json!({"start": secs(r.start), "end": secs(r.end()), "duration": secs(r.duration), "startTicks": r.start.0, "durationTicks": r.duration.0}))
        .collect()
}

fn silences(s: &mut Session, p: &Value) -> Result<Value> {
    let (r, thr, n) = silence_params(s, p)?;
    let total: f64 = r.iter().map(|x| secs(x.duration)).sum();
    Ok(json!({"silences": ranges_json(&r), "count": r.len(), "totalSeconds": total, "thresholdDb": thr, "clipsAnalysed": n}))
}

fn cut_silences(s: &mut Session, p: &Value) -> Result<Value> {
    let (r, thr, _) = silence_params(s, p)?;
    if r.is_empty() {
        return Ok(json!({"removed": 0, "seconds": 0.0, "thresholdDb": thr}));
    }
    let n = r.len();
    let list = ranges_json(&r);
    let total = s.edit_sequence("Cut Silences", |q, ctx, _| Ok(filmcraft_edit::transcript::ripple_delete_ranges(q, r, ctx)))?;
    Ok(json!({"removed": n, "seconds": secs(total), "thresholdDb": thr, "cuts": list}))
}

// ------------------------------------------------------------------------------- beats

/// The music clip a beat command uses: `clip`, else a selected audio clip, else the clip on a
/// "Guide" track, else the longest clip marked as music, else the longest audio clip not linked
/// to picture.
fn music_clip(s: &Session, p: &Value) -> Result<ClipId> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if let Some(c) = u64_p(p, "clip").map(ClipId) {
        return q.find_item(c).map(|_| c).ok_or_else(|| bad("studio.beats", "no such clip"));
    }
    let audio = |c: &ClipId| q.find_item(*c).and_then(|(t, _)| q.track(t)).is_some_and(|t| t.kind == TrackKind::Audio);
    if let Some(c) = s.state.selection.iter().copied().find(audio) {
        return Ok(c);
    }
    let guide = q.audio_tracks.iter().filter(|t| t.name.eq_ignore_ascii_case(GUIDE)).flat_map(|t| t.items.iter()).max_by_key(|i| i.duration.0);
    let music = q.audio_tracks.iter().flat_map(|t| t.items.iter()).filter(|i| is_music(i)).max_by_key(|i| i.duration.0);
    let loose = q.audio_tracks.iter().flat_map(|t| t.items.iter()).filter(|i| i.link.is_none()).max_by_key(|i| i.duration.0);
    guide.or(music).or(loose).map(|i| i.id).ok_or_else(|| bad("studio.beats", "no music clip: generate one with studio.music.generate or pass `clip`"))
}

struct Grid {
    bpm: f64,
    /// Sequence times (seconds).
    beats: Vec<f64>,
    downbeats: Vec<f64>,
    drop: Option<f64>,
    exact: bool,
}

/// Beat grid of a timeline clip in sequence time: exact for generated music (its sidecar),
/// measured otherwise (up to 10 minutes of the clip).
fn grid(s: &Session, c: ClipId) -> Result<Grid> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, it) = q.find_item(c).ok_or_else(|| bad("studio.beats", "no such clip"))?;
    let speed = if it.speed.is_finite() && it.speed > 0.0 { it.speed } else { 1.0 };
    let to_seq = |m: f64| secs(it.start) + (m - secs(it.source_in)) / speed;
    let inside = |t: &f64| *t >= secs(it.start) - 1e-6 && *t < secs(it.end());
    let path = match s.project.item(it.item).map(|i| &i.kind) {
        Some(ItemKind::Media(m)) => match &m.media {
            MediaRef::File { path } => Some(path.clone()),
            _ => None,
        },
        _ => None,
    };
    if let Some(g) =
        path.and_then(|p| s.services.read_file(&path_str(&beats_sidecar(Path::new(&p)))).ok()).and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    {
        let list =
            |k: &str| -> Vec<f64> { g[k].as_array().map(|a| a.iter().filter_map(Value::as_f64).map(to_seq).filter(inside).collect()).unwrap_or_default() };
        return Ok(Grid {
            bpm: g["bpm"].as_f64().unwrap_or(120.0) * speed,
            beats: list("beats"),
            downbeats: list("downbeats"),
            drop: g["drop"].as_f64().map(to_seq).filter(inside),
            exact: true,
        });
    }
    if it.reverse {
        return Err(bad("studio.beats", "reversed clips have no beat grid"));
    }
    let src = s.source(it.item).ok_or_else(|| EngineError::Other("the clip's media is offline".into()))?;
    let sr = 22_050u32;
    let m0 = it.source_in.to_units_floor(sr as i64);
    let media_len = Tick((it.duration.0 as f64 * speed) as i64).min(Tick::from_seconds_f64(600.0));
    let len = media_len.to_units_floor(sr as i64).max(0) as usize;
    let buf = src.audio(m0, len, sr).map_err(|e| EngineError::Other(format!("reading the clip's audio: {e}")))?;
    let refs: Vec<&[f32]> = buf.channels.iter().map(Vec::as_slice).collect();
    let a = filmcraft_audio_dsp::remix::analyze(&refs, sr).map_err(|e| EngineError::Other(format!("no beat found: {e:?}")))?;
    let lead = filmcraft_audio_dsp::remix::beat_lead(sr);
    let beats: Vec<f64> = a.beats.iter().map(|b| to_seq(secs(it.source_in) + (b + lead) as f64 / sr as f64)).filter(inside).collect();
    let downbeats = beats.iter().copied().step_by(4).collect();
    Ok(Grid { bpm: a.bpm * speed, beats, downbeats, drop: None, exact: false })
}

fn every_p(p: &Value) -> usize {
    u64_p(p, "every").unwrap_or(1).clamp(1, 64) as usize
}

fn beats(s: &mut Session, p: &Value) -> Result<Value> {
    let c = music_clip(s, p)?;
    let g = grid(s, c)?;
    Ok(json!({"clip": c.0, "bpm": g.bpm, "exact": g.exact, "beats": g.beats, "downbeats": g.downbeats, "drop": g.drop, "count": g.beats.len()}))
}

fn mark_beats(s: &mut Session, p: &Value) -> Result<Value> {
    let c = music_clip(s, p)?;
    let g = grid(s, c)?;
    let every = every_p(p);
    let color = label_p(p);
    let marks: Vec<f64> = if bool_p(p, "downbeats").unwrap_or(false) { g.downbeats.clone() } else { g.beats.iter().copied().step_by(every).collect() };
    let drop = g.drop;
    let n = marks.len();
    s.edit_sequence("Mark Beats", |q, ctx, _| {
        for (i, t) in marks.iter().enumerate() {
            let name = if drop.is_some_and(|d| (d - t).abs() < 1e-3) { "DROP".to_string() } else { format!("Beat {}", i + 1) };
            q.markers.push(Marker {
                id: MarkerId(ctx.alloc()),
                start: Tick::from_seconds_f64(*t),
                duration: Tick::ZERO,
                name,
                comment: String::new(),
                kind: MarkerKind::Comment,
                color,
            });
        }
        q.markers.sort_by_key(|m| m.start);
        Ok(())
    })?;
    Ok(json!({"markers": n, "bpm": g.bpm}))
}

/// Video tracks named in `tracks` (`"V1"`, …), else every unlocked video track.
fn video_tracks(s: &Session, p: &Value) -> Vec<TrackId> {
    let Some(q) = s.active_sequence() else { return vec![] };
    let names: Option<Vec<String>> =
        p.get("tracks").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_ascii_lowercase)).collect());
    q.video_tracks
        .iter()
        .enumerate()
        .filter(|(i, t)| !t.locked && names.as_ref().is_none_or(|n| n.contains(&format!("v{}", i + 1)) || n.contains(&t.name.to_ascii_lowercase())))
        .map(|(_, t)| t.id)
        .collect()
}

fn beat_cut(s: &mut Session, p: &Value) -> Result<Value> {
    let c = music_clip(s, p)?;
    let g = grid(s, c)?;
    let every = every_p(p).max(1);
    let tracks = video_tracks(s, p);
    if tracks.is_empty() {
        return Err(bad("studio.beatCut", "no unlocked video track to cut"));
    }
    let from = f64_p(p, "fromSeconds").unwrap_or(f64::MIN);
    let to = f64_p(p, "toSeconds").unwrap_or(f64::MAX);
    let fd = s.sequence_rate().frame_duration().0.max(1);
    let cuts: Vec<Tick> = g
        .beats
        .iter()
        .copied()
        .step_by(every)
        .filter(|t| *t > from && *t < to)
        .map(|t| Tick((Tick::from_seconds_f64(t).0 + fd / 2).div_euclid(fd) * fd))
        .collect();
    let made = s.edit_sequence("Cut on the Beat", |q, ctx, _| {
        let mut n = 0;
        for t in &cuts {
            n += filmcraft_edit::razor(q, &tracks, *t, ctx).len();
        }
        Ok(n)
    })?;
    Ok(json!({"cuts": made, "beats": cuts.len(), "bpm": g.bpm}))
}

fn beat_montage(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "studio.beatMontage";
    let items: Vec<ItemId> = p.get("items").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(ItemId).collect()).unwrap_or_default();
    if items.is_empty() {
        return Err(bad(CMD, "need `items`: the project items (clips, images) to cut together"));
    }
    if items.len() > 2000 {
        return Err(bad(CMD, "at most 2000 shots"));
    }
    let c = music_clip(s, p)?;
    let g = grid(s, c)?;
    let per = u64_p(p, "beatsPerShot").unwrap_or(2).clamp(1, 64) as usize;
    let start = time_p(s, p, "").map(secs);
    let first = g.beats.iter().position(|b| start.is_none_or(|t| *b >= t - 1e-6)).ok_or_else(|| bad(CMD, "no beat after the start time"))?;
    let marks: Vec<f64> = g.beats.iter().skip(first).copied().step_by(per).collect();
    let track = p.get("track").cloned().unwrap_or(json!("V1"));
    let audio = bool_p(p, "audio").unwrap_or(false);
    let offsets: Vec<f64> = p.get("sourceOffsets").and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect()).unwrap_or_default();
    let fd = s.sequence_rate().frame_duration().0.max(1);
    let snap = |t: f64| Tick((Tick::from_seconds_f64(t).0 + fd / 2).div_euclid(fd) * fd);
    s.compound("Beat Montage", |s| {
        let mut placed = Vec::new();
        for (k, w) in marks.windows(2).enumerate() {
            let (Some(&a), Some(&b)) = (w.first(), w.get(1)) else { continue };
            let Some(&item) = items.get(k % items.len()) else { continue };
            let (ta, tb) = (snap(a), snap(b));
            if tb <= ta {
                continue;
            }
            let len = s.project.item(item).map(|i| i.duration()).unwrap_or_default();
            let off = Tick::from_seconds_f64(offsets.get(k % items.len().max(1)).copied().unwrap_or(0.0).max(0.0));
            let need = tb - ta;
            let still = len.0 <= 0;
            let sin = if still || len - off >= need { off } else { Tick::ZERO };
            let mut q = json!({"item": item.0, "track": track, "time": ta.0, "duration": need.0, "audio": audio, "video": true});
            if !still {
                q["sourceIn"] = json!(sin.0);
            }
            let r = s.execute("timeline.place", q)?;
            placed.extend(r["clips"].as_array().cloned().unwrap_or_default());
        }
        Ok(json!({"shots": marks.len().saturating_sub(1), "clips": placed, "beatsPerShot": per, "bpm": g.bpm}))
    })
}

// ------------------------------------------------------------------------------- motion

/// The video clip a motion recipe acts on: `clip`, else the selected video clip under the time,
/// else the topmost enabled video clip under it.
pub(crate) fn video_clip_at(s: &Session, p: &Value, t: Tick) -> Result<ClipId> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if let Some(c) = u64_p(p, "clip").map(ClipId) {
        return q.find_item(c).map(|_| c).ok_or_else(|| bad("studio", "no such clip"));
    }
    let under =
        |c: &ClipId| q.find_item(*c).is_some_and(|(tid, it)| q.track(tid).is_some_and(|tr| tr.kind == TrackKind::Video) && it.start <= t && t < it.end());
    if let Some(c) = s.state.selection.iter().copied().find(under) {
        return Ok(c);
    }
    q.video_tracks
        .iter()
        .rev()
        .filter(|tr| tr.enabled)
        .find_map(|tr| tr.items.iter().find(|it| it.enabled && it.start <= t && t < it.end()).map(|it| it.id))
        .ok_or_else(|| bad("studio", "no video clip at that time"))
}

/// Write keyframes `(sequence time, value)` on the clip's Motion `param` (one undo step).
pub(crate) fn motion_keys(s: &mut Session, label: &str, c: ClipId, param: &str, keys: Vec<(Tick, ParamValue)>, smooth: bool) -> Result<usize> {
    let n = keys.len();
    let param = param.to_string();
    s.edit_sequence(label, move |q, _, _| {
        let (_, it) = q.find_item_mut(c).ok_or(filmcraft_edit::EditError::NoItem(c))?;
        let lo = it.start;
        let hi = (it.end() - Tick(1)).max(lo);
        let times: Vec<(Tick, ParamValue)> = keys.into_iter().map(|(t, v)| (it.source_time_at(t.clamp(lo, hi)), v)).collect();
        let e = it.effect_mut("motion").ok_or_else(|| EngineError::Other("the clip has no Motion effect".into()))?;
        let prm = e.params.get_mut(&param).ok_or_else(|| EngineError::Other(format!("Motion has no `{param}`")))?;
        for (mt, v) in times {
            prm.put_keyframe(mt, v);
            if let Some(k) = prm.keyframes.iter_mut().find(|k| k.time == mt) {
                k.interp = if smooth { Interpolation::Bezier } else { Interpolation::Linear };
            }
        }
        Ok(())
    })?;
    Ok(n)
}

pub(crate) fn float_at(s: &Session, c: ClipId, param: &str, t: Tick) -> Option<ParamValue> {
    let q = s.active_sequence()?;
    let (_, it) = q.find_item(c)?;
    let mt = it.source_time_at(t.clamp(it.start, (it.end() - Tick(1)).max(it.start)));
    it.effect("motion")?.params.get(param).map(|p| p.value_at(mt))
}

fn punch_zoom(s: &mut Session, p: &Value) -> Result<Value> {
    let t = at_p(s, p);
    let c = video_clip_at(s, p, t)?;
    let zoom = f64_p(p, "scale").unwrap_or(130.0).clamp(1.0, 1000.0) / 100.0;
    let tin = Tick::from_seconds_f64(f64_p(p, "inSeconds").unwrap_or(0.08).clamp(0.0, 10.0));
    let hold = Tick::from_seconds_f64(f64_p(p, "holdSeconds").unwrap_or(0.6).clamp(0.0, 600.0));
    let tout = Tick::from_seconds_f64(f64_p(p, "outSeconds").unwrap_or(0.2).clamp(0.0, 10.0));
    let stay = bool_p(p, "stay").unwrap_or(false);
    let ParamValue::Float(base) = float_at(s, c, "scale", t).unwrap_or(ParamValue::Float(100.0)) else {
        return Err(bad("studio.punchZoom", "Motion scale is not a number"));
    };
    let mut keys = vec![(t, ParamValue::Float(base)), (t + tin, ParamValue::Float(base * zoom))];
    if !stay {
        keys.push((t + tin + hold, ParamValue::Float(base * zoom)));
        keys.push((t + tin + hold + tout, ParamValue::Float(base)));
    }
    let focus = p.get("focus").and_then(Value::as_array).and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?)));
    s.compound("Punch Zoom", |s| {
        let n = motion_keys(s, "Punch Zoom", c, "scale", keys.clone(), !stay && tin.0 > 0)?;
        if let Some((fx, fy)) = focus {
            // zoom about a point of the frame (fractions 0…1): the position moves away from it
            let (w, h) = s.active_sequence().map(|q| (q.settings.width as f64, q.settings.height as f64)).unwrap_or((1920.0, 1080.0));
            let (fx, fy) = (fx.clamp(0.0, 1.0) * w, fy.clamp(0.0, 1.0) * h);
            if let Some(ParamValue::Vec2(pos)) = float_at(s, c, "position", t) {
                let moved = |k: f64| ParamValue::Vec2(filmcraft_geom::Vec2::new(fx + (pos.x - fx) * k, fy + (pos.y - fy) * k));
                let pk: Vec<(Tick, ParamValue)> = keys
                    .iter()
                    .map(|(tt, v)| (*tt, if matches!(v, ParamValue::Float(x) if (*x - base).abs() > 1e-9) { moved(zoom) } else { moved(1.0) }))
                    .collect();
                motion_keys(s, "Punch Zoom", c, "position", pk, !stay && tin.0 > 0)?;
            }
        }
        Ok(json!({"clip": c.0, "keyframes": n, "from": base, "to": base * zoom, "at": secs(t)}))
    })
}

fn shake(s: &mut Session, p: &Value) -> Result<Value> {
    let t = at_p(s, p);
    let c = video_clip_at(s, p, t)?;
    let dur = f64_p(p, "durationSeconds").unwrap_or(0.35).clamp(0.05, 10.0);
    let amount = f64_p(p, "amount").unwrap_or(30.0).clamp(0.0, 2000.0);
    let hz = f64_p(p, "frequency").unwrap_or(20.0).clamp(1.0, 60.0);
    let Some(ParamValue::Vec2(pos)) = float_at(s, c, "position", t) else {
        return Err(bad("studio.shake", "the clip's Motion position is not set"));
    };
    let mut rng = synth::Rng::new(u64_p(p, "seed").unwrap_or(7));
    let steps = (dur * hz).ceil().clamp(2.0, 600.0) as usize;
    let mut keys = vec![(t, ParamValue::Vec2(pos))];
    for i in 1..steps {
        let decay = 1.0 - i as f64 / steps as f64;
        let (dx, dy) = (rng.bipolar() as f64 * amount * decay, rng.bipolar() as f64 * amount * decay);
        keys.push((t + Tick::from_seconds_f64(i as f64 / hz), ParamValue::Vec2(filmcraft_geom::Vec2::new(pos.x + dx, pos.y + dy))));
    }
    keys.push((t + Tick::from_seconds_f64(dur), ParamValue::Vec2(pos)));
    let ParamValue::Float(base) = float_at(s, c, "scale", t).unwrap_or(ParamValue::Float(100.0)) else {
        return Err(bad("studio.shake", "Motion scale is not a number"));
    };
    // a little overscan hides the frame edges while it shakes
    let over = base * (1.0 + 2.2 * amount / s.active_sequence().map(|q| q.settings.width.max(1) as f64).unwrap_or(1920.0));
    let sk = vec![
        (t, ParamValue::Float(base)),
        (t + Tick::from_seconds_f64(0.01), ParamValue::Float(over)),
        (t + Tick::from_seconds_f64(dur), ParamValue::Float(over)),
        (t + Tick::from_seconds_f64(dur + 0.01), ParamValue::Float(base)),
    ];
    s.compound("Camera Shake", |s| {
        let n = motion_keys(s, "Camera Shake", c, "position", keys, false)?;
        motion_keys(s, "Camera Shake", c, "scale", sk, false)?;
        Ok(json!({"clip": c.0, "keyframes": n, "at": secs(t), "duration": dur}))
    })
}

// ------------------------------------------------------------------------------- format

/// Frame sizes by platform.
pub const FORMATS: &[(&str, u32, u32, &str)] = &[
    ("tiktok", 1080, 1920, "TikTok / Reels / Shorts (9:16)"),
    ("reels", 1080, 1920, "Instagram Reels (9:16)"),
    ("shorts", 1080, 1920, "YouTube Shorts (9:16)"),
    ("vertical", 1080, 1920, "vertical 9:16"),
    ("portrait", 1080, 1350, "Instagram feed portrait (4:5)"),
    ("square", 1080, 1080, "square (1:1)"),
    ("youtube", 1920, 1080, "YouTube / landscape 1080p (16:9)"),
    ("landscape", 1920, 1080, "landscape 1080p (16:9)"),
    ("youtube4k", 3840, 2160, "YouTube 4K (16:9)"),
];

fn format_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "preset").unwrap_or("tiktok").to_ascii_lowercase();
    let &(id, w, h, label) = FORMATS.iter().find(|f| f.0 == name).ok_or_else(|| {
        let all: Vec<&str> = FORMATS.iter().map(|f| f.0).collect();
        bad("studio.format", format!("unknown preset `{name}` ({})", all.join(", ")))
    })?;
    let fill = bool_p(p, "fill").unwrap_or(true);
    s.compound(&format!("Format for {label}"), |s| {
        s.execute("sequence.settings", json!({"width": w, "height": h}))?;
        let mut n = 0;
        if fill {
            let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
            let clips: Vec<u64> = q
                .video_tracks
                .iter()
                .filter(|t| !t.locked)
                .flat_map(|t| t.items.iter())
                .filter(|it| s.project.item(it.item).is_some_and(|i| matches!(i.kind, ItemKind::Media(_)) && i.has_video()))
                .map(|it| it.id.0)
                .collect();
            if !clips.is_empty() {
                n = clips.len();
                s.execute("clip.fillFrame", json!({"clips": clips}))?;
            }
        }
        Ok(json!({"preset": id, "width": w, "height": h, "filled": n}))
    })
}

// ------------------------------------------------------------------------------- captions & titles

/// Caption looks: (id, max chars per line, lines, max seconds, size as a fraction of the frame
/// height, colour, outline (fraction of size), background, anchor, margin).
const CAPTION_STYLES: &[(&str, u64, u64, f64, f64, &str, f64, bool, &str, f64)] = &[
    ("tiktok", 16, 1, 1.4, 0.052, "#ffffff", 0.12, false, "bottom", 0.30),
    ("gaming", 14, 1, 1.2, 0.058, "#ffe135", 0.14, false, "bottom", 0.27),
    ("youtube", 42, 2, 5.0, 0.045, "#ffffff", 0.0, true, "bottom", 0.08),
    ("minimal", 32, 2, 4.0, 0.038, "#ffffff", 0.06, false, "bottom", 0.10),
    ("bold", 20, 1, 2.0, 0.065, "#ffffff", 0.16, false, "middle", 0.0),
];

fn captions(s: &mut Session, p: &Value) -> Result<Value> {
    let style = str_p(p, "style").unwrap_or("tiktok").to_ascii_lowercase();
    let &(id, chars, lines, max_s, size, color, outline, bg, anchor, margin) = CAPTION_STYLES.iter().find(|c| c.0 == style).ok_or_else(|| {
        let all: Vec<&str> = CAPTION_STYLES.iter().map(|c| c.0).collect();
        bad("studio.captions", format!("unknown style `{style}` ({})", all.join(", ")))
    })?;
    let h = s.active_sequence().map(|q| q.settings.height as f64).unwrap_or(1080.0);
    let px = (size * h).round();
    let color = str_p(p, "color").unwrap_or(color).to_string();
    let font = str_p(p, "font").unwrap_or("Inter").to_string();
    let chars = u64_p(p, "maxChars").unwrap_or(chars).clamp(4, 80);
    s.compound(&format!("Captions ({id})"), |s| {
        let r = s.execute("transcript.createCaptions", json!({"maxChars": chars, "lines": lines, "maxSeconds": max_s, "minSeconds": 0.3, "gapFrames": 0, "name": format!("Captions {id}")})).map_err(|e| {
            EngineError::Other(format!("{e} — captions come from the transcript: run `transcript.generate` (speech-to-text build) or `transcript.set` first"))
        })?;
        let mut st = json!({"track": "C1", "font": font, "size": px, "color": color, "background": bg, "align": "center", "anchor": anchor, "margin": margin, "outline": (outline * px).round(), "outlineColor": "#000000"});
        if bg {
            st["backgroundColor"] = json!("#000000b0");
        }
        s.execute("captions.setStyle", st)?;
        Ok(json!({"style": id, "captions": r.get("captions").cloned().unwrap_or(r.clone()), "size": px, "burnIn": "export with file.exportMedia {burnCaptions: true} to bake them into the picture"}))
    })
}

/// Title looks: (id, size as a fraction of frame height, fill, stroke, stroke width (fraction of
/// size), background colour, shadow, vertical position).
const TITLE_STYLES: &[(&str, f64, &str, &str, f64, Option<&str>, bool, &str)] = &[
    ("hook", 0.062, "#ffffff", "#000000", 0.12, None, true, "top"),
    ("gaming", 0.072, "#ffd60a", "#000000", 0.14, None, true, "center"),
    ("meme", 0.065, "#ffffff", "#000000", 0.13, None, false, "top"),
    ("subscribe", 0.045, "#ffffff", "#000000", 0.0, Some("#ff0033"), true, "bottom"),
    ("lowerthird", 0.04, "#ffffff", "#000000", 0.0, Some("#111111"), false, "bottom"),
    ("neon", 0.07, "#7df9ff", "#00b3ff", 0.03, None, true, "center"),
    ("clean", 0.05, "#ffffff", "#000000", 0.0, None, true, "center"),
    // TikTok's own text looks
    ("boxed", 0.045, "#000000", "#000000", 0.0, Some("#ffffff"), false, "top"),
    ("boxeddark", 0.045, "#ffffff", "#000000", 0.0, Some("#000000b3"), false, "top"),
    ("highlight", 0.05, "#000000", "#000000", 0.0, Some("#ffe600"), false, "top"),
    ("reply", 0.036, "#161823", "#000000", 0.0, Some("#ffffff"), true, "top"),
    ("part2", 0.075, "#ffffff", "#000000", 0.0, Some("#fe2c55"), true, "top"),
    ("news", 0.042, "#ffffff", "#000000", 0.0, Some("#d00000"), true, "bottom"),
    ("warning", 0.05, "#000000", "#000000", 0.0, Some("#ffcc00"), true, "center"),
    ("quote", 0.048, "#ffffff", "#000000", 0.0, None, true, "center"),
    ("cinematic", 0.04, "#f2f2f2", "#000000", 0.0, None, false, "center"),
    ("countdown", 0.2, "#ffffff", "#000000", 0.08, None, true, "center"),
    ("pixel", 0.05, "#39ff14", "#000000", 0.18, None, false, "top"),
    ("horror", 0.07, "#c1121f", "#000000", 0.06, None, true, "center"),
    ("pastel", 0.055, "#ffb3d9", "#ffffff", 0.1, None, true, "center"),
    ("fire", 0.07, "#ffb703", "#d62828", 0.12, None, true, "center"),
];

/// Title entrance animations.
pub const TITLE_ANIMATIONS: &[&str] = &["pop", "bounce", "zoom", "slide", "drop", "fade", "wipe", "shake", "none"];

fn title(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "studio.title";
    let text = str_p(p, "text").ok_or_else(|| bad(CMD, "need `text`"))?.to_string();
    if text.chars().count() > 500 {
        return Err(bad(CMD, "`text` is longer than 500 characters"));
    }
    let style = str_p(p, "style").unwrap_or("hook").to_ascii_lowercase().replace([' ', '_', '-'], "");
    let &(id, size, fill, stroke, sw, bg, shadow, pos) = TITLE_STYLES.iter().find(|t| t.0 == style).ok_or_else(|| {
        let all: Vec<&str> = TITLE_STYLES.iter().map(|t| t.0).collect();
        bad(CMD, format!("unknown style `{style}` ({})", all.join(", ")))
    })?;
    let (w, h) = s.active_sequence().map(|q| (q.settings.width as f64, q.settings.height as f64)).ok_or(EngineError::NoSequence)?;
    // as big as the style asks, but the longest line fits 90 % of the width (bold capitals are
    // about 0.62 em wide)
    let longest = text.lines().map(|l| l.chars().count()).max().unwrap_or(1).max(1) as f64;
    let fit = 0.9 * w / (0.62 * longest);
    let px = (size * h * f64_p(p, "sizeScale").unwrap_or(1.0).clamp(0.2, 5.0)).min(fit).max(12.0).round();
    let y = match str_p(p, "position").unwrap_or(pos) {
        "top" => h * 0.2,
        "bottom" => h * 0.8,
        _ => h * 0.5,
    };
    let x = if id == "lowerthird" { w * 0.08 } else { w / 2.0 };
    let t = at_p(s, p);
    let dur = f64_p(p, "durationSeconds").unwrap_or(2.5).clamp(0.1, 600.0);
    let font = str_p(p, "font").map(str::to_string);
    let fill = str_p(p, "color").unwrap_or(fill).to_string();
    let animate = str_p(p, "animate").unwrap_or("pop").to_ascii_lowercase();
    if !TITLE_ANIMATIONS.contains(&animate.as_str()) {
        return Err(bad(CMD, format!("animate: {}", TITLE_ANIMATIONS.join(", "))));
    }
    s.compound(&format!("Title ({id})"), |s| {
        let mut q = json!({"text": text, "size": px, "position": [x, y], "seconds": dur, "time": t.0, "newClip": true});
        if let Some(f) = &font {
            q["font"] = json!(f);
        }
        let r = s.execute("graphics.newText", q)?;
        let clip = r["clip"].as_u64().map(ClipId).ok_or_else(|| EngineError::Other(format!("no title clip in {r}")))?;
        let mut props = json!({"font_style": "Bold", "fill_color": fill, "align": if id == "lowerthird" { "left" } else { "center" }});
        if sw > 0.0 {
            props["stroke"] = json!(true);
            props["stroke_color"] = json!(stroke);
            props["stroke_width"] = json!((sw * px).round());
        }
        if let Some(b) = bg {
            props["background"] = json!(true);
            props["background_color"] = json!(b);
            props["background_size"] = json!((px * 0.35).round());
            props["background_radius"] = json!((px * 0.2).round());
        }
        if shadow {
            props["shadow"] = json!(true);
            if id == "neon" {
                props["shadow_color"] = json!(stroke);
                props["shadow_distance"] = json!(0);
                props["shadow_blur"] = json!(px * 0.6);
                props["shadow_size"] = json!(px * 0.08);
            }
        }
        s.execute("graphics.set", json!({"clip": clip.0, "props": props}))?;
        animate_title(s, clip, t, &animate, (w, h))?;
        Ok(json!({"clip": clip.0, "style": id, "size": px, "at": secs(t), "duration": dur}))
    })
}

/// Keyframe a title's entrance (sequence time `t`, frame size `(w, h)`).
fn animate_title(s: &mut Session, clip: ClipId, t: Tick, animate: &str, (w, h): (f64, f64)) -> Result<()> {
    let f = Tick::from_seconds_f64;
    let v = ParamValue::Float;
    let at = |x: f64, y: f64| ParamValue::Vec2(filmcraft_geom::Vec2::new(x, y));
    let (cx, cy) = (w / 2.0, h / 2.0);
    match animate {
        "pop" => {
            motion_keys(s, "Title", clip, "scale", vec![(t, v(0.0)), (t + f(0.09), v(115.0)), (t + f(0.16), v(100.0))], true)?;
        }
        "bounce" => {
            motion_keys(
                s,
                "Title",
                clip,
                "scale",
                vec![(t, v(0.0)), (t + f(0.12), v(122.0)), (t + f(0.22), v(90.0)), (t + f(0.32), v(104.0)), (t + f(0.4), v(100.0))],
                true,
            )?;
        }
        "zoom" => {
            motion_keys(s, "Title", clip, "scale", vec![(t, v(170.0)), (t + f(0.18), v(100.0))], true)?;
            crate::studio_fx::fx_edit(s, "Title", clip, "opacity", vec![], vec![("opacity", vec![(t, v(0.0)), (t + f(0.12), v(100.0))])])?;
        }
        "slide" => {
            motion_keys(s, "Title", clip, "position", vec![(t, at(cx, cy + h * 0.08)), (t + f(0.22), at(cx, cy))], true)?;
            crate::studio_fx::fx_edit(s, "Title", clip, "opacity", vec![], vec![("opacity", vec![(t, v(0.0)), (t + f(0.15), v(100.0))])])?;
        }
        "drop" => {
            motion_keys(s, "Title", clip, "position", vec![(t, at(cx, cy - h * 0.6)), (t + f(0.2), at(cx, cy + h * 0.015)), (t + f(0.3), at(cx, cy))], true)?;
        }
        "fade" => {
            crate::studio_fx::fx_edit(s, "Title", clip, "opacity", vec![], vec![("opacity", vec![(t, v(0.0)), (t + f(0.3), v(100.0))])])?;
        }
        "wipe" => {
            // reveals left to right, like typing
            crate::studio_fx::fx_edit(s, "Title", clip, "crop", vec![], vec![("right", vec![(t, v(100.0)), (t + f(0.6), v(0.0))])])?;
        }
        "shake" => {
            let mut rng = synth::Rng::new(t.0.unsigned_abs() ^ 0x5eed);
            let mut keys = vec![(t, at(cx, cy))];
            for i in 1..8 {
                let k = 1.0 - f64::from(i) / 8.0;
                keys.push((t + f(f64::from(i) * 0.04), at(cx + f64::from(rng.bipolar()) * w * 0.02 * k, cy + f64::from(rng.bipolar()) * h * 0.01 * k)));
            }
            keys.push((t + f(0.34), at(cx, cy)));
            motion_keys(s, "Title", clip, "position", keys, false)?;
        }
        _ => {}
    }
    Ok(())
}

// ------------------------------------------------------------------------------- guide track

/// Name of the audio track that holds a trending sound to cut to, left out of the export.
pub const GUIDE: &str = "Guide";

fn guide(s: &mut Session, p: &Value) -> Result<Value> {
    let mute = bool_p(p, "mute");
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let existing = q.audio_tracks.iter().find(|t| t.name.eq_ignore_ascii_case(GUIDE)).map(|t| t.id);
    let clip = u64_p(p, "clip")
        .map(ClipId)
        .or_else(|| s.state.selection.iter().copied().find(|c| q.find_item(*c).and_then(|(t, _)| q.track(t)).is_some_and(|t| t.kind == TrackKind::Audio)));
    s.compound("Guide Track", |s| {
        let tid = match existing {
            Some(t) => t,
            None => {
                s.execute("sequence.addTracks", json!({"video": 0, "audio": 1}))?;
                let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
                let t = q.audio_tracks.last().map(|t| t.id).ok_or_else(|| EngineError::Other("could not add a track".into()))?;
                s.edit_sequence("Name Track", |q, _, _| {
                    if let Some(tr) = q.audio_tracks.iter_mut().find(|x| x.id == t) {
                        tr.name = GUIDE.into();
                    }
                    Ok(())
                })?;
                t
            }
        };
        if let Some(c) = clip {
            let start = s.active_sequence().and_then(|q| q.find_item(c)).map(|(_, it)| it.start).ok_or_else(|| bad("studio.guide", "no such clip"))?;
            let in_place = s.active_sequence().and_then(|q| q.find_item(c)).is_some_and(|(t, _)| t == tid);
            if !in_place {
                s.execute("timeline.move", json!({"moves": [{"clip": c.0, "track": tid.0, "time": start.0}], "linked": false}))?;
            }
        }
        if let Some(m) = mute {
            s.edit_sequence("Mute Guide", |q, _, _| {
                if let Some(tr) = q.audio_tracks.iter_mut().find(|x| x.id == tid) {
                    tr.muted = m;
                }
                Ok(())
            })?;
        }
        let muted = s.active_sequence().and_then(|q| q.track(tid)).is_some_and(|t| t.muted);
        Ok(json!({"track": tid.0, "muted": muted, "note": "mute the Guide track (studio.guide {mute: true}) before exporting, then add the sound in the app you post to"}))
    })
}

// ------------------------------------------------------------------------------- user library

const AUDIO_EXT: &[&str] = &["wav", "mp3", "flac", "ogg", "opus", "m4a", "aac", "aif", "aiff"];
const VIDEO_EXT: &[&str] = &["mp4", "mov", "m4v", "mkv", "webm", "mxf"];
const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"];
const MAX_LIBRARY_FILES: usize = 20_000;

fn library_config() -> Option<PathBuf> {
    library_dir().map(|d| d.join("library.json"))
}

/// The folders searched: `my-library/` (created on first use) plus the ones added with
/// `studio.library.addFolder`.
fn library_folders() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(d) = sub_dir("my-library") {
        v.push(d);
    }
    // online downloads and installed sound packs (library_download, library_packs)
    if let Some(d) = library_dir().map(|d| d.join("downloads")).filter(|d| d.is_dir()) {
        v.push(d);
    }
    if let Some(cfg) = library_config().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
        for f in cfg["folders"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            let p = PathBuf::from(f);
            if !v.contains(&p) {
                v.push(p);
            }
        }
    }
    v
}

fn kind_of(p: &Path) -> Option<&'static str> {
    let e = p.extension()?.to_str()?.to_ascii_lowercase();
    if AUDIO_EXT.contains(&e.as_str()) {
        Some("audio")
    } else if VIDEO_EXT.contains(&e.as_str()) {
        Some("video")
    } else if IMAGE_EXT.contains(&e.as_str()) {
        Some("image")
    } else {
        None
    }
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 8 || out.len() >= MAX_LIBRARY_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if out.len() >= MAX_LIBRARY_FILES {
            return;
        }
        let hidden = p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'));
        if hidden {
            continue;
        }
        if p.is_dir() {
            walk(&p, depth + 1, out);
        } else if kind_of(&p).is_some() {
            out.push(p);
        }
    }
}

/// Words of a path (file name and folders) for search.
fn words(p: &Path, root: &Path) -> Vec<String> {
    let rel = p.strip_prefix(root).unwrap_or(p);
    let mut w: Vec<String> =
        rel.with_extension("").to_string_lossy().split(|c: char| !c.is_alphanumeric()).filter(|x| !x.is_empty()).map(str::to_lowercase).collect();
    w.dedup();
    w
}

fn library_list(_s: &mut Session, p: &Value) -> Result<Value> {
    let query: Vec<String> = str_p(p, "query").unwrap_or("").split_whitespace().map(str::to_lowercase).collect();
    let kind = str_p(p, "kind").map(str::to_ascii_lowercase);
    let limit = u64_p(p, "limit").unwrap_or(50).clamp(1, 1000) as usize;
    let folders = library_folders();
    let mut hits: Vec<(usize, Value)> = Vec::new();
    let mut total = 0;
    for root in &folders {
        let mut files = Vec::new();
        walk(root, 0, &mut files);
        total += files.len();
        for f in files {
            let Some(k) = kind_of(&f) else { continue };
            if kind.as_deref().is_some_and(|x| x != k) {
                continue;
            }
            let w = words(&f, root);
            // the file's own name counts more than its folders (a pack folder named "fighter"
            // must not make every sound in it match "fight" equally)
            let stem: Vec<String> = f
                .file_stem()
                .map(|n| n.to_string_lossy().split(|c: char| !c.is_alphanumeric()).filter(|x| !x.is_empty()).map(str::to_lowercase).collect())
                .unwrap_or_default();
            let score: usize = query
                .iter()
                .map(|q| {
                    if stem.iter().any(|x| x == q) {
                        4
                    } else if stem.iter().any(|x| x.contains(q.as_str())) {
                        3
                    } else if w.iter().any(|x| x.contains(q.as_str())) {
                        1
                    } else {
                        0
                    }
                })
                .sum();
            if !query.is_empty() && score == 0 {
                continue;
            }
            let name = f.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            hits.push((score, json!({"path": path_str(&f), "name": name, "kind": k, "tags": w})));
        }
    }
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    let files: Vec<Value> = hits.into_iter().take(limit).map(|(_, v)| v).collect();
    Ok(
        json!({"files": files, "scanned": total, "folders": folders.iter().map(|f| path_str(f)).collect::<Vec<_>>(), "tip": "drop sound packs, music and clips into the first folder (or add yours with studio.library.addFolder)"}),
    )
}

fn library_add_folder(_s: &mut Session, p: &Value) -> Result<Value> {
    let f = str_p(p, "path").ok_or_else(|| bad("studio.library.addFolder", "need `path`"))?;
    let path = PathBuf::from(f);
    if !path.is_dir() {
        return Err(bad("studio.library.addFolder", format!("{f} is not a folder")));
    }
    let cfg = library_config().ok_or_else(|| EngineError::Other("no studio folder on this platform".into()))?;
    let mut v: Value = std::fs::read(&cfg).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| json!({"folders": []}));
    let remove = bool_p(p, "remove").unwrap_or(false);
    let mut list: Vec<String> = v["folders"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect();
    list.retain(|x| x != f);
    if !remove {
        list.push(f.to_string());
    }
    v["folders"] = json!(list);
    if let Some(d) = cfg.parent() {
        std::fs::create_dir_all(d).map_err(|e| EngineError::Other(e.to_string()))?;
    }
    std::fs::write(&cfg, v.to_string()).map_err(|e| EngineError::Other(format!("{}: {e}", cfg.display())))?;
    Ok(json!({"folders": library_folders().iter().map(|f| path_str(f)).collect::<Vec<_>>()}))
}

fn library_add(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "studio.library.add";
    let path = match str_p(p, "path") {
        Some(x) => x.to_string(),
        None => {
            let q = str_p(p, "query").ok_or_else(|| bad(CMD, "need `path` or `query`"))?;
            let r = library_list(s, &json!({"query": q, "limit": 1, "kind": p.get("kind")}))?;
            r["files"].get(0).and_then(|f| f["path"].as_str()).map(str::to_string).ok_or_else(|| bad(CMD, format!("nothing in the library matches `{q}`")))?
        }
    };
    let t = at_p(s, p);
    let gain = f64_p(p, "gainDb").unwrap_or(0.0);
    let track = p.get("track").cloned();
    let as_guide = bool_p(p, "guide").unwrap_or(false);
    let audio_only = kind_of(Path::new(&path)) == Some("audio");
    s.compound("Add from Library", |s| {
        let item = import(s, &path)?;
        if audio_only {
            let (clip, tid) = place_audio(s, item, t, track.as_ref(), gain, true)?;
            let mut out = json!({"clip": clip.0, "item": item.0, "track": tid.0, "path": path});
            if as_guide {
                out["guide"] = guide(s, &json!({"clip": clip.0}))?;
            }
            Ok(out)
        } else {
            let mut q = json!({"item": item.0, "time": t.0});
            if let Some(tr) = &track {
                q["track"] = tr.clone();
            }
            let r = s.execute("timeline.place", q)?;
            Ok(json!({"clips": r["clips"], "item": item.0, "path": path}))
        }
    })
}

// ------------------------------------------------------------------------------- fonts

fn fonts_dir() -> Result<PathBuf> {
    sub_dir("fonts")
}

/// Register every font in the studio fonts folder (once per process; later installs register
/// themselves).
pub fn register_user_fonts() -> usize {
    static DONE: OnceLock<usize> = OnceLock::new();
    *DONE.get_or_init(|| match library_dir() {
        Some(d) if d.join("fonts").is_dir() => filmcraft_text::fonts::add_source(&filmcraft_text::fonts::DirectorySource { dirs: vec![d.join("fonts")] }),
        _ => 0,
    })
}

fn fonts_install(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "studio.fonts.install";
    let src = str_p(p, "path").ok_or_else(|| bad(CMD, "need `path` (a .ttf / .otf file)"))?;
    let ext = Path::new(src).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    if !matches!(ext.as_str(), "ttf" | "otf" | "ttc") {
        return Err(bad(CMD, "only .ttf, .otf and .ttc fonts"));
    }
    let bytes = s.services.read_file(src).map_err(|e| EngineError::Other(format!("{src}: {e}")))?;
    if bytes.len() > 64 << 20 {
        return Err(bad(CMD, "font file larger than 64 MB"));
    }
    let name = Path::new(src).file_name().ok_or_else(|| bad(CMD, "bad path"))?;
    let dest = fonts_dir()?.join(name);
    if Path::new(src) != dest {
        std::fs::write(&dest, &bytes).map_err(|e| EngineError::Other(format!("{}: {e}", dest.display())))?;
    }
    register_user_fonts();
    let faces = filmcraft_text::fonts::add_font_data(bytes);
    if faces.is_empty() {
        let _ = std::fs::remove_file(&dest);
        return Err(bad(CMD, "not a font file FilmCraft can read"));
    }
    let families: Vec<Value> = faces.iter().map(|f| filmcraft_text::fonts::face(*f)).map(|f| json!({"family": f.info.family, "style": f.info.style})).collect();
    Ok(
        json!({"installed": families, "path": path_str(&dest), "licence": "make sure the font's licence allows your use (Google Fonts / OFL fonts are free for videos)"}),
    )
}

fn fonts_list(_s: &mut Session, _p: &Value) -> Result<Value> {
    register_user_fonts();
    // a query: never creates the folder
    let dir = library_dir().map(|d| d.join("fonts"));
    let user: Vec<Value> = filmcraft_text::fonts::all_faces()
        .iter()
        .filter(|f| {
            matches!(&f.info.data, filmcraft_text::fonts::FaceData::File(p) if dir.as_ref().is_some_and(|d| p.starts_with(d))) || f.info.origin == "user"
        })
        .map(|f| json!({"family": f.info.family, "style": f.info.style}))
        .collect();
    Ok(json!({"installed": user, "folder": dir.map(|d| path_str(&d)), "all": "fonts.list lists every font (bundled, system and installed)"}))
}

// ------------------------------------------------------------------------------- export

fn export_settings(s: &mut Session, p: &Value) -> Result<Value> {
    let platform = str_p(p, "platform").unwrap_or("tiktok").to_ascii_lowercase();
    let (w, h) = s.active_sequence().map(|q| (q.settings.width, q.settings.height)).unwrap_or((1080, 1920));
    let (kbps, lufs, note) = match platform.as_str() {
        "tiktok" | "reels" | "instagram" => {
            (16_000, -14.0, "9:16, H.264 + AAC, under 10 minutes; keep text out of the bottom 20 % and right 15 % (app buttons)")
        }
        "shorts" => (16_000, -14.0, "9:16, at most 3 minutes"),
        "youtube" => (if w >= 3840 { 45_000 } else { 16_000 }, -14.0, "16:9; YouTube re-encodes, a high bitrate keeps detail"),
        "twitter" | "x" => (12_000, -14.0, "at most 2:20 for most accounts"),
        _ => return Err(bad("studio.exportSettings", "platform: tiktok, reels, shorts, youtube or x")),
    };
    let guide_live = s.active_sequence().is_some_and(|q| q.audio_tracks.iter().any(|t| t.name.eq_ignore_ascii_case(GUIDE) && !t.muted && !t.items.is_empty()));
    Ok(json!({
        "command": "file.exportMedia",
        "params": {"format": "h264", "width": w, "height": h, "bitrateKbps": kbps, "bitrateMode": "vbr2Pass", "audio": true, "loudnessLufs": lufs, "burnCaptions": true, "range": "entire", "wait": true, "path": "<absolute path>.mp4"},
        "notes": note,
        "warning": if guide_live { Some("the Guide track is not muted: run studio.guide {mute: true} first, or the trending sound ends up in the file") } else { None },
    }))
}

// ------------------------------------------------------------------------------- Claude

/// Port the desktop app listens on for Claude unless configured otherwise.
pub const CLAUDE_PORT: u16 = 9876;

/// `<data dir>/claude.json`: whether the desktop app opens its control port for Claude (Claude
/// Desktop, Claude Code, the Claude panel) when it starts, and on which port. Written by
/// `filmcraft-cli claude install` and the Claude panel.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClaudeSettings {
    pub listen: bool,
    pub port: u16,
    /// The Claude panel's model (a Claude Code `--model` alias; empty: Claude Code's default).
    pub model: String,
    /// The Claude panel's effort level (`--effort`; empty: the default).
    pub effort: String,
}

impl Default for ClaudeSettings {
    fn default() -> Self {
        ClaudeSettings { listen: false, port: CLAUDE_PORT, model: String::new(), effort: String::new() }
    }
}

fn claude_settings_path() -> Option<PathBuf> {
    crate::autosave::default_data_dir().map(|d| d.join("claude.json"))
}

/// The saved settings (defaults when missing or unreadable; a port below 1024 reads as the default).
pub fn claude_settings() -> ClaudeSettings {
    let mut c: ClaudeSettings = claude_settings_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if c.port < 1024 {
        c.port = CLAUDE_PORT;
    }
    c
}

pub fn save_claude_settings(c: &ClaudeSettings) -> std::result::Result<PathBuf, String> {
    let p = claude_settings_path().ok_or("no data folder on this platform")?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let text = serde_json::to_string_pretty(c).map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(p)
}

// ------------------------------------------------------------------------------- registry

fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}

macro_rules! spec {
    ($id:literal, $label:literal, $params:literal, $en:expr, $run:expr, $journal:expr) => {
        CommandSpec { id: $id, label: $label, menu: &[], shortcut: None, params: $params, enabled: $en, run: $run, journal: $journal }
    };
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        spec!("studio.sfx.list", "List Sound Effects", r#"{"category":"transition|impact|ui|meme|game|rhythm"?,"query":str?}"#, always, sfx_list, false),
        spec!(
            "studio.sfx.add",
            "Add Sound Effect",
            r#"{"sound":id (studio.sfx.list),"time":ticks|"seconds":f64|"frame"|"timecode"? (default playhead),"variant":n=0,"gainDb":f64=0,"track":"A2"|id? (default: a free audio track)}"#,
            has_seq,
            sfx_add,
            true
        ),
        spec!("studio.music.styles", "List Music Styles", "{}", always, music_styles, false),
        spec!(
            "studio.music.generate",
            "Generate Music",
            r#"{"style":"phonk|trap|edm|chiptune|lofi|chill|cinematic"="phonk","durationSeconds":f64? (default: the sequence length, else 30; ≤ 300),"bpm":f64?,"key":"A"|"F#"|0..11?,"seed":n=1 (another seed = another tune),"place":bool=true,"time"|"seconds"? (default 0),"gainDb":f64=-8,"track":"A3"|id?}"#,
            always,
            music_generate,
            true
        ),
        spec!(
            "studio.silences",
            "Find Silences",
            r#"{"thresholdDb":f64|"auto"="auto","minSeconds":f64=0.5,"padSeconds":f64=0.12,"clips":[id]?|"tracks":["A1"]? (default: the sound recorded with the picture)}"#,
            has_seq,
            silences,
            false
        ),
        spec!(
            "studio.cutSilences",
            "Cut Silences (Jump Cuts)",
            r#"{"thresholdDb":f64|"auto"="auto","minSeconds":f64=0.5,"padSeconds":f64=0.12,"clips":[id]?|"tracks":["A1"]?}"#,
            has_seq,
            cut_silences,
            true
        ),
        spec!("studio.beats", "Beat Grid", r#"{"clip":id? (default: selected audio clip, Guide track, music clip)}"#, has_seq, beats, false),
        spec!("studio.markBeats", "Mark Beats", r#"{"clip":id?,"every":n=1,"downbeats":bool=false,"color":label?}"#, has_seq, mark_beats, true),
        spec!(
            "studio.beatCut",
            "Cut on the Beat",
            r#"{"clip":id? (music),"every":n=1 (cut every n beats),"tracks":["V1"]?,"fromSeconds":f64?,"toSeconds":f64?}"#,
            has_seq,
            beat_cut,
            true
        ),
        spec!(
            "studio.beatMontage",
            "Beat Montage",
            r#"{"items":[id] (shots, used in order and looped),"clip":id? (music),"beatsPerShot":n=2,"time"|"seconds"? (first beat at or after),"track":"V1","audio":bool=false,"sourceOffsets":[seconds]?}"#,
            has_seq,
            beat_montage,
            true
        ),
        spec!(
            "studio.punchZoom",
            "Punch Zoom",
            r#"{"clip":id?,"time"|"seconds"? (default playhead),"scale":percent=130,"inSeconds":0.08,"holdSeconds":0.6,"outSeconds":0.2,"stay":bool=false,"focus":[x,y]? (0..1 of the frame)}"#,
            has_seq,
            punch_zoom,
            true
        ),
        spec!(
            "studio.shake",
            "Camera Shake",
            r#"{"clip":id?,"time"|"seconds"?,"durationSeconds":0.35,"amount":px=30,"frequency":hz=20,"seed":n?}"#,
            has_seq,
            shake,
            true
        ),
        spec!(
            "studio.format",
            "Format for Platform",
            r#"{"preset":"tiktok|reels|shorts|vertical|portrait|square|youtube|landscape|youtube4k"="tiktok","fill":bool=true}"#,
            has_seq,
            format_cmd,
            true
        ),
        spec!(
            "studio.captions",
            "Styled Captions",
            r##"{"style":"tiktok|gaming|youtube|minimal|bold"="tiktok","font":str?,"color":"#rrggbb"?,"maxChars":n?}"##,
            has_seq,
            captions,
            true
        ),
        spec!(
            "studio.title",
            "Styled Title",
            r##"{"text":str,"style":"hook|gaming|meme|subscribe|lowerThird|neon|clean|boxed|boxedDark|highlight|reply|part2|news|warning|quote|cinematic|countdown|pixel|horror|pastel|fire"="hook","time"|"seconds"?,"durationSeconds":2.5,"position":"top|center|bottom"?,"font":str?,"color":"#rrggbb"?,"sizeScale":1,"animate":"pop|bounce|zoom|slide|drop|fade|wipe|shake|none"="pop"}"##,
            has_seq,
            title,
            true
        ),
        spec!("studio.guide", "Guide Track", r#"{"clip":id? (moved onto the Guide track),"mute":bool? (mute it for export)}"#, has_seq, guide, true),
        spec!("studio.library.list", "Search My Library", r#"{"query":str?,"kind":"audio|video|image"?,"limit":n=50}"#, always, library_list, false),
        spec!(
            "studio.library.add",
            "Add from My Library",
            r#"{"path":str|"query":str,"kind":"audio|video|image"?,"time"|"seconds"?,"track":str|id?,"gainDb":f64=0,"guide":bool=false}"#,
            has_seq,
            library_add,
            true
        ),
        spec!("studio.library.addFolder", "Add Library Folder", r#"{"path":str,"remove":bool=false}"#, always, library_add_folder, true),
        spec!("studio.fonts.install", "Install Font", r#"{"path":".ttf|.otf|.ttc"}"#, always, fonts_install, true),
        spec!("studio.fonts.list", "Installed Fonts", "{}", always, fonts_list, false),
        spec!(
            "studio.exportSettings",
            "Export Settings for Platform",
            r#"{"platform":"tiktok|reels|shorts|youtube|x"="tiktok"}"#,
            always,
            export_settings,
            false
        ),
    ]
}

#[cfg(test)]
#[path = "studio_tests.rs"]
mod tests;
