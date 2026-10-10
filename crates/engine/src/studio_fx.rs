//! TikTok-style studio recipes on top of the editing commands: stickers and overlays, B-roll
//! cut-ins, a blurred-background fill for landscape footage in a vertical frame, colour looks,
//! effect presets and quick transitions (zoom, whip, flash, glitch, spin) with their sound.
//! Every recipe is one undo step.

use std::path::PathBuf;

use filmcraft_color::lut::Lut3d;
use filmcraft_geom::Vec2;
use filmcraft_project::{ClipId, Interpolation, ParamValue, TrackId, TrackKind};
use filmcraft_time::{Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, f64_p, has_seq, str_p, u64_p};
use crate::studio::{at_p, float_at, import, motion_keys, path_str, secs, sub_dir, video_clip_at};
use crate::{EngineError, Result, Session};

fn frame(s: &Session) -> Result<(f64, f64)> {
    s.active_sequence().map(|q| (q.settings.width.max(1) as f64, q.settings.height.max(1) as f64)).ok_or(EngineError::NoSequence)
}

fn sec(x: f64) -> Tick {
    Tick::from_seconds_f64(x)
}

/// An unlocked video track above `min_index` with nothing in `[t, t + dur)`; adds one on top
/// when every such track is busy.
fn free_video_track(s: &mut Session, t: Tick, dur: Tick, min_index: usize) -> Result<TrackId> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let r = TimeRange::new(t, dur);
    let free =
        q.video_tracks.iter().enumerate().skip(min_index).find(|(_, tr)| !tr.locked && tr.items.iter().all(|it| it.end() <= r.start || it.start >= r.end()));
    if let Some((_, tr)) = free {
        return Ok(tr.id);
    }
    s.execute("sequence.addTracks", json!({"video": 1, "audio": 0}))?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    q.video_tracks.last().map(|t| t.id).ok_or_else(|| EngineError::Other("could not add a video track".into()))
}

/// Place the picture of `item` at `t` on `track` for `dur` (no sound); returns the clip.
fn place_picture(s: &mut Session, item: filmcraft_project::ItemId, track: TrackId, t: Tick, dur: Option<Tick>, source_in: Option<Tick>) -> Result<ClipId> {
    let mut q = json!({"item": item.0, "track": track.0, "time": t.0, "audio": false});
    if let Some(d) = dur {
        q["duration"] = json!(d.0);
    }
    if let Some(i) = source_in {
        q["sourceIn"] = json!(i.0);
    }
    let r = s.execute("timeline.place", q)?;
    r["clips"].as_array().and_then(|a| a.first()).and_then(Value::as_u64).map(ClipId).ok_or_else(|| EngineError::Other("nothing was placed".into()))
}

/// Set static values and keyframes `(sequence time, value)` on effect `effect` of clip `c`,
/// adding the effect (before the intrinsic ones) when the clip has none. One undo step.
pub(crate) fn fx_edit(
    s: &mut Session,
    label: &str,
    c: ClipId,
    effect: &str,
    statics: Vec<(&str, ParamValue)>,
    keys: Vec<(&str, Vec<(Tick, ParamValue)>)>,
) -> Result<()> {
    let def = filmcraft_project::find_effect(effect).ok_or_else(|| EngineError::Other(format!("no effect `{effect}`")))?;
    let effect = effect.to_string();
    let statics: Vec<(String, ParamValue)> = statics.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    let keys: Vec<(String, Vec<(Tick, ParamValue)>)> = keys.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    s.edit_sequence(label, move |q, _, _| {
        let (_, it) = q.find_item_mut(c).ok_or(filmcraft_edit::EditError::NoItem(c))?;
        let lo = it.start;
        let hi = (it.end() - Tick(1)).max(lo);
        let times: Vec<(String, Vec<(Tick, ParamValue)>)> =
            keys.into_iter().map(|(k, v)| (k, v.into_iter().map(|(t, x)| (it.source_time_at(t.clamp(lo, hi)), x)).collect())).collect();
        if it.effect_mut(&effect).is_none() {
            let pos = it.effects.iter().position(|e| e.def().is_some_and(|d| d.intrinsic)).unwrap_or(it.effects.len());
            it.effects.insert(pos, def.instance());
        }
        let e = it.effect_mut(&effect).ok_or_else(|| EngineError::Other(format!("could not add `{effect}`")))?;
        for (k, v) in statics {
            match e.param_mut(&k) {
                Some(p) => {
                    p.keyframes.clear();
                    p.value = v;
                }
                None => {
                    e.params.insert(k, filmcraft_project::Param::new(v));
                }
            }
        }
        for (k, list) in times {
            if e.param_mut(&k).is_none() {
                let first = list.first().map(|x| x.1.clone()).unwrap_or(ParamValue::Float(0.0));
                e.params.insert(k.clone(), filmcraft_project::Param::new(first));
            }
            let Some(p) = e.param_mut(&k) else { continue };
            for (mt, v) in list {
                p.put_keyframe(mt, v);
                if let Some(kf) = p.keyframes.iter_mut().find(|kf| kf.time == mt) {
                    kf.interp = Interpolation::Bezier;
                }
            }
        }
        Ok(())
    })?;
    Ok(())
}

fn f(x: f64) -> ParamValue {
    ParamValue::Float(x)
}

fn scale_at(s: &Session, c: ClipId, t: Tick) -> f64 {
    match float_at(s, c, "scale", t) {
        Some(ParamValue::Float(x)) if x.is_finite() => x,
        _ => 100.0,
    }
}

fn pos_at(s: &Session, c: ClipId, t: Tick) -> Option<Vec2> {
    match float_at(s, c, "position", t) {
        Some(ParamValue::Vec2(v)) if v.x.is_finite() && v.y.is_finite() => Some(v),
        _ => None,
    }
}

// ------------------------------------------------------------------------------- overlays

/// Named spots as fractions of the frame, inside TikTok's safe zone (the app's buttons cover the
/// bottom fifth and the right edge).
pub const SPOTS: &[(&str, f64, f64)] = &[
    ("center", 0.5, 0.5),
    ("top", 0.5, 0.22),
    ("bottom", 0.5, 0.68),
    ("left", 0.27, 0.5),
    ("right", 0.7, 0.5),
    ("top-left", 0.27, 0.22),
    ("top-right", 0.7, 0.22),
    ("bottom-left", 0.27, 0.66),
    ("bottom-right", 0.68, 0.66),
];

fn spot(p: &Value) -> Result<(f64, f64)> {
    if let Some(a) = p.get("position").and_then(Value::as_array) {
        let x = a.first().and_then(Value::as_f64).unwrap_or(0.5);
        let y = a.get(1).and_then(Value::as_f64).unwrap_or(0.5);
        return Ok((x.clamp(-0.5, 1.5), y.clamp(-0.5, 1.5)));
    }
    let name = str_p(p, "position").unwrap_or("center").to_ascii_lowercase().replace([' ', '_'], "-");
    SPOTS.iter().find(|s| s.0 == name).map(|s| (s.1, s.2)).ok_or_else(|| {
        let all: Vec<&str> = SPOTS.iter().map(|s| s.0).collect();
        bad("studio.overlay", format!("position: [x, y] fractions or one of {}", all.join(", ")))
    })
}

/// Entrance animations of stickers and overlays.
pub const ANIMATIONS: &[&str] = &["pop", "bounce", "fade", "slide", "spin", "none"];

fn overlay(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "studio.overlay";
    let path = str_p(p, "path").ok_or_else(|| bad(CMD, "need `path` (a picture, sticker or video)"))?.to_string();
    let t = at_p(s, p);
    let (fw, fh) = frame(s)?;
    let (fx, fy) = spot(p)?;
    let width = f64_p(p, "width").unwrap_or(0.4).clamp(0.02, 3.0);
    let rot = f64_p(p, "rotation").unwrap_or(0.0).clamp(-3600.0, 3600.0);
    let animate = str_p(p, "animate").unwrap_or("pop").to_ascii_lowercase();
    if !ANIMATIONS.contains(&animate.as_str()) {
        return Err(bad(CMD, format!("animate: {}", ANIMATIONS.join(", "))));
    }
    let outro = bool_p(p, "outro").unwrap_or(true);
    s.compound("Overlay", |s| {
        let item = import(s, &path)?;
        let is_still = s.project.item(item).and_then(|i| i.as_media()).is_some_and(|m| m.info.kind == filmcraft_media::MediaKind::Still);
        let natural = s.project.item(item).map(|i| i.duration()).unwrap_or_default();
        let dur = match f64_p(p, "durationSeconds") {
            Some(d) => sec(d.clamp(0.1, 3600.0)),
            None if is_still || natural <= Tick::ZERO => sec(3.0),
            None => natural,
        };
        let tr = free_video_track(s, t, dur, 1)?;
        let clip = place_picture(s, item, tr, t, Some(dur), None)?;
        let (sw, sh) = filmcraft_render::source_size(&s.project, item).unwrap_or((fw as u32, fh as u32));
        let scale = width * fw / f64::from(sw.max(1)) * 100.0;
        let pos = Vec2::new(fx * fw, fy * fh);
        let anchor = Vec2::new(f64::from(sw) / 2.0, f64::from(sh) / 2.0);
        fx_edit(
            s,
            "Overlay",
            clip,
            "motion",
            vec![
                ("scale", f(scale)),
                ("uniform_scale", ParamValue::Bool(true)),
                ("position", ParamValue::Vec2(pos)),
                ("anchor", ParamValue::Vec2(anchor)),
                ("rotation", f(rot)),
            ],
            vec![],
        )?;
        let end = t + dur;
        let k = |a: f64| t + sec(a);
        match animate.as_str() {
            "pop" => motion_keys(s, "Overlay", clip, "scale", vec![(t, f(0.0)), (k(0.09), f(scale * 1.15)), (k(0.16), f(scale))], true)?,
            "bounce" => {
                motion_keys(s, "Overlay", clip, "scale", vec![(t, f(0.0)), (k(0.12), f(scale * 1.22)), (k(0.22), f(scale * 0.9)), (k(0.32), f(scale))], true)?
            }
            "slide" => {
                motion_keys(s, "Overlay", clip, "position", vec![(t, ParamValue::Vec2(Vec2::new(pos.x, fh * 1.2))), (k(0.25), ParamValue::Vec2(pos))], true)?
            }
            "spin" => {
                motion_keys(s, "Overlay", clip, "rotation", vec![(t, f(rot - 180.0)), (k(0.3), f(rot))], true)?;
                motion_keys(s, "Overlay", clip, "scale", vec![(t, f(0.0)), (k(0.3), f(scale))], true)?
            }
            "fade" => {
                fx_edit(s, "Overlay", clip, "opacity", vec![], vec![("opacity", vec![(t, f(0.0)), (k(0.2), f(100.0))])])?;
                2
            }
            _ => 0,
        };
        if outro && animate != "none" && dur > sec(0.6) {
            if animate == "fade" {
                fx_edit(s, "Overlay", clip, "opacity", vec![], vec![("opacity", vec![(end - sec(0.2), f(100.0)), (end - Tick(1), f(0.0))])])?;
            } else {
                motion_keys(s, "Overlay", clip, "scale", vec![(end - sec(0.12), f(scale)), (end - Tick(1), f(0.0))], true)?;
            }
        }
        Ok(json!({"clip": clip.0, "track": tr.0, "at": secs(t), "duration": secs(dur), "position": [pos.x, pos.y], "scale": scale, "animate": animate}))
    })
}

// ------------------------------------------------------------------------------- B-roll

fn broll(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "studio.broll";
    let path = str_p(p, "path").ok_or_else(|| bad(CMD, "need `path` (a video or picture)"))?.to_string();
    let t = at_p(s, p);
    let dur = sec(f64_p(p, "durationSeconds").unwrap_or(2.5).clamp(0.1, 3600.0));
    let push = bool_p(p, "pushIn");
    s.compound("B-roll", |s| {
        let item = import(s, &path)?;
        let is_still = s.project.item(item).and_then(|i| i.as_media()).is_some_and(|m| m.info.kind == filmcraft_media::MediaKind::Still);
        let tr = free_video_track(s, t, dur, 1)?;
        // long agency footage (NASA, archives) opens on a title slate: start past it, at the
        // most detailed of a few candidate moments
        let natural = s.project.item(item).map(|i| i.duration()).unwrap_or_default();
        let skip = bool_p(p, "skipIntro").unwrap_or(false) && !is_still && natural > sec(20.0);
        let source_in = f64_p(p, "sourceSeconds").map(|x| sec(x.clamp(0.0, 1e6))).or_else(|| skip.then(|| liveliest(s, item, natural)));
        let clip = place_picture(s, item, tr, t, Some(dur), source_in)?;
        s.execute("clip.fillFrame", json!({"clips": [clip.0]}))?;
        // pictures get a slow push-in (Ken Burns) so they don't look frozen
        if push.unwrap_or(is_still) {
            let base = scale_at(s, clip, t);
            let end = s.active_sequence().and_then(|q| q.find_item(clip)).map(|(_, it)| it.end()).unwrap_or(t + dur);
            motion_keys(s, "B-roll", clip, "scale", vec![(t, f(base)), (end - Tick(1), f(base * 1.12))], false)?;
        }
        let end = s.active_sequence().and_then(|q| q.find_item(clip)).map(|(_, it)| it.end()).unwrap_or(t + dur);
        Ok(json!({"clip": clip.0, "track": tr.0, "at": secs(t), "duration": secs(end - t)}))
    })
}

/// The most detailed (highest luma spread) of a few moments between 20 % and 75 % of `dur`, so a
/// B-roll cut-in skips title slates and black stretches.
fn liveliest(s: &Session, item: filmcraft_project::ItemId, dur: Tick) -> Tick {
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let mut best = (f64::MIN, Tick(dur.0 / 3));
    for frac in [0.2, 0.33, 0.45, 0.6, 0.75] {
        let t = Tick((dur.0 as f64 * frac) as i64);
        let Some(img) = filmcraft_render::render_item(&s.project, item, t, 0.1, &provider) else { continue };
        let px = img.to_rgba8();
        let n = (px.len() / 4).max(1) as f64;
        let lum: Vec<f64> = px
            .chunks_exact(4)
            .filter_map(|c| match c {
                [r, g, b, _] => Some(0.2126 * f64::from(*r) + 0.7152 * f64::from(*g) + 0.0722 * f64::from(*b)),
                _ => None,
            })
            .collect();
        let mean = lum.iter().sum::<f64>() / n;
        let spread = (lum.iter().map(|l| (l - mean) * (l - mean)).sum::<f64>() / n).sqrt() + mean * 0.1;
        if spread > best.0 {
            best = (spread, t);
        }
    }
    best.1
}

// ------------------------------------------------------------------------------- blur fill

/// Landscape footage in a vertical frame: the clip stays whole (fit) over a blurred, darkened,
/// frame-filling copy of itself, the standard TikTok look instead of black bars.
fn blur_fill(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "studio.blurFill";
    let (fw, fh) = frame(s)?;
    let frame_ar = fw / fh;
    let blur = f64_p(p, "blur").unwrap_or(60.0).clamp(0.0, 3000.0);
    let darken = f64_p(p, "darken").unwrap_or(-25.0).clamp(-100.0, 0.0);
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let asked: Option<Vec<ClipId>> = p.get("clips").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(ClipId).collect());
    // (clip, track index, item, start, duration, source in)
    let mut targets = Vec::new();
    let mut skipped = Vec::new();
    for (ti, tr) in q.video_tracks.iter().enumerate() {
        if tr.locked || (asked.is_none() && ti > 0) {
            continue;
        }
        for it in &tr.items {
            if asked.as_ref().is_some_and(|a| !a.contains(&it.id)) {
                continue;
            }
            let Some((w, h)) = filmcraft_render::source_size(&s.project, it.item) else { continue };
            let ar = f64::from(w.max(1)) / f64::from(h.max(1));
            if (ar / frame_ar - 1.0).abs() < 0.05 {
                continue;
            }
            if (it.speed - 1.0).abs() > 1e-9 {
                skipped.push(json!({"clip": it.id.0, "why": "speed changed: apply the blur fill before changing speed"}));
                continue;
            }
            targets.push((it.id, ti, it.item, it.start, it.duration, it.source_in));
        }
    }
    if targets.is_empty() {
        return Err(bad(CMD, "no clip whose shape differs from the frame (pass `clips`, or set the frame with studio.format first)"));
    }
    s.compound("Blur Fill", |s| {
        let mut out = Vec::new();
        for (c, ti, item, start, dur, src_in) in targets {
            let tr = free_video_track(s, start, dur, ti + 1)?;
            let sharp = place_picture(s, item, tr, start, Some(dur), Some(src_in))?;
            s.execute("clip.fitToFrame", json!({"clips": [sharp.0]}))?;
            s.execute("clip.fillFrame", json!({"clips": [c.0]}))?;
            fx_edit(s, "Blur Fill", c, "gaussian_blur", vec![("blurriness", f(blur)), ("repeat_edge", ParamValue::Bool(true))], vec![])?;
            fx_edit(s, "Blur Fill", c, "brightness_contrast", vec![("brightness", f(darken))], vec![])?;
            out.push(json!({"background": c.0, "foreground": sharp.0}));
        }
        Ok(json!({"clips": out, "skipped": skipped}))
    })
}

// ------------------------------------------------------------------------------- looks

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn saturate(c: [f32; 3], k: f32) -> [f32; 3] {
    let y = luma(c);
    [y + (c[0] - y) * k, y + (c[1] - y) * k, y + (c[2] - y) * k]
}

/// Smooth S-curve contrast around mid grey (`k` > 1 adds contrast).
fn contrast(c: [f32; 3], k: f32) -> [f32; 3] {
    c.map(|v| (0.5 + (v - 0.5) * k).clamp(0.0, 1.0))
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Tint shadows towards `sh` and highlights towards `hi` (colours as offsets).
fn split_tone(c: [f32; 3], sh: [f32; 3], hi: [f32; 3]) -> [f32; 3] {
    let y = luma(c).clamp(0.0, 1.0);
    let (ws, wh) = ((1.0 - y) * (1.0 - y), y * y);
    [c[0] + sh[0] * ws + hi[0] * wh, c[1] + sh[1] * ws + hi[1] * wh, c[2] + sh[2] * ws + hi[2] * wh]
}

/// Lift blacks / lower whites (a faded, matte print).
fn fade(c: [f32; 3], lift: f32, gain: f32) -> [f32; 3] {
    c.map(|v| lift + v * (gain - lift))
}

/// Creative looks: (id, label, built-in Lumetri look or `None` for a generated LUT).
pub const LOOKS: &[(&str, &str, Option<&str>)] = &[
    ("teal_orange", "Teal & orange blockbuster", Some("builtin:look-teal-orange")),
    ("warm", "Warm film", Some("builtin:look-warm-film")),
    ("cold", "Cool blue", Some("builtin:look-cool-blue")),
    ("bleach", "Bleach bypass", Some("builtin:look-bleach-bypass")),
    ("faded", "Faded matte", Some("builtin:look-faded-matte")),
    ("bw", "Black & white", Some("builtin:look-monochrome")),
    ("golden", "Golden hour", Some("builtin:look-golden-hour")),
    ("night", "Night", Some("builtin:look-night")),
    ("vibrant", "Vibrant pop (TikTok punch)", None),
    ("cyberpunk", "Cyberpunk neon (teal shadows, magenta highlights)", None),
    ("vaporwave", "Vaporwave pastel pink / cyan", None),
    ("vintage", "Vintage 70s film", None),
    ("sepia", "Sepia", None),
    ("noir", "High-contrast noir", None),
    ("dreamy", "Dreamy soft pastel", None),
    ("moody", "Moody dark teal", None),
    ("sunset", "Sunset orange / pink", None),
    ("matrix", "Matrix green", None),
    ("cinematic", "Cinematic (soft contrast, teal shadows, warm skin)", None),
];

/// The colour transform of a generated look (input and output 0…1, display-referred).
pub fn look_fn(id: &str) -> Option<fn([f32; 3]) -> [f32; 3]> {
    let g: fn([f32; 3]) -> [f32; 3] = match id {
        "vibrant" => |c| contrast(saturate(c, 1.38), 1.12),
        "cyberpunk" => |c| contrast(saturate(split_tone(c, [-0.06, 0.08, 0.16], [0.14, -0.04, 0.12]), 1.3), 1.15),
        "vaporwave" => |c| fade(split_tone(saturate(c, 1.1), [0.02, 0.06, 0.14], [0.14, 0.0, 0.08]), 0.06, 0.97),
        "vintage" => |c| fade(saturate(split_tone(c, [0.02, 0.05, -0.02], [0.08, 0.04, -0.06]), 0.78), 0.08, 0.93),
        "sepia" => |c| {
            let y = luma(c);
            mix(c, [y * 1.07 + 0.03, y * 0.87 + 0.02, y * 0.62], 0.9)
        },
        "noir" => |c| {
            let y = luma(c);
            contrast([y, y, y], 1.45)
        },
        "dreamy" => |c| fade(saturate(contrast(split_tone(c, [0.03, 0.0, 0.06], [0.06, 0.03, 0.0]), 0.88), 0.85), 0.07, 1.0),
        "moody" => |c| fade(contrast(saturate(split_tone(c, [-0.03, 0.04, 0.07], [0.03, 0.01, -0.03]), 0.7), 1.08), 0.03, 0.88),
        "sunset" => |c| saturate(split_tone(c, [0.06, -0.01, 0.05], [0.16, 0.05, -0.08]), 1.15),
        "matrix" => |c| contrast(split_tone(saturate(c, 0.6), [-0.04, 0.06, -0.04], [-0.02, 0.1, -0.06]), 1.1),
        "cinematic" => |c| fade(contrast(split_tone(saturate(c, 0.92), [-0.04, 0.02, 0.06], [0.05, 0.02, -0.04]), 1.06), 0.03, 0.97),
        _ => return None,
    };
    Some(g)
}

/// The `.cube` text of a generated look (33³, clamped to 0…1).
pub fn look_cube(id: &str) -> Option<String> {
    let g = look_fn(id)?;
    let mut lut = Lut3d::from_fn(33, |c| g(c).map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }));
    lut.title = format!("FilmCraft look: {id}");
    Some(lut.to_cube())
}

fn target_clips(s: &Session, p: &Value, t: Tick) -> Result<Vec<ClipId>> {
    if let Some(a) = p.get("clips").and_then(Value::as_array) {
        return Ok(a.iter().filter_map(Value::as_u64).map(ClipId).collect());
    }
    if bool_p(p, "all").unwrap_or(false) {
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        let v: Vec<ClipId> = q
            .video_tracks
            .iter()
            .filter(|tr| !tr.locked)
            .flat_map(|tr| tr.items.iter())
            // footage and photos, not stickers (stills with transparency)
            .filter(|it| {
                s.project
                    .item(it.item)
                    .and_then(|i| i.as_media())
                    .is_some_and(|m| !(m.info.kind == filmcraft_media::MediaKind::Still && m.info.video.as_ref().is_some_and(|v| v.has_alpha)))
            })
            .map(|it| it.id)
            .collect();
        return Ok(v);
    }
    let sel: Vec<ClipId> = s
        .state
        .selection
        .iter()
        .copied()
        .filter(|c| {
            s.active_sequence()
                .and_then(|q| q.find_item(*c))
                .is_some_and(|(tid, _)| s.active_sequence().and_then(|q| q.track(tid)).is_some_and(|tr| tr.kind == TrackKind::Video))
        })
        .collect();
    if !sel.is_empty() {
        return Ok(sel);
    }
    Ok(vec![video_clip_at(s, p, t)?])
}

fn look(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "studio.look";
    let name = str_p(p, "look").unwrap_or("vibrant").to_ascii_lowercase().replace([' ', '-', '&'], "_").replace("__", "_");
    let &(id, label, builtin) = LOOKS.iter().find(|l| l.0 == name).ok_or_else(|| {
        let all: Vec<&str> = LOOKS.iter().map(|l| l.0).collect();
        bad(CMD, format!("unknown look `{name}` ({})", all.join(", ")))
    })?;
    let intensity = f64_p(p, "intensity").unwrap_or(100.0).clamp(0.0, 200.0);
    let t = at_p(s, p);
    let clips = target_clips(s, p, t)?;
    let lut_arg = match builtin {
        Some(b) => json!({"lut": b}),
        None => {
            let dir: PathBuf = sub_dir("looks")?;
            let file = dir.join(format!("filmcraft-{id}.cube"));
            if !file.exists() {
                let text = look_cube(id).ok_or_else(|| bad(CMD, "no such look"))?;
                std::fs::write(&file, text).map_err(|e| EngineError::Other(format!("{}: {e}", file.display())))?;
            }
            json!({"path": path_str(&file)})
        }
    };
    s.compound(&format!("Look: {label}"), |s| {
        for c in &clips {
            let mut q = lut_arg.clone();
            q["clip"] = json!(c.0);
            s.execute("lumetri.setLook", q)?;
            fx_edit(s, "Look", *c, "lumetri", vec![("look_intensity", f(intensity))], vec![])?;
        }
        Ok(json!({"look": id, "label": label, "clips": clips.iter().map(|c| c.0).collect::<Vec<_>>(), "intensity": intensity}))
    })
}

// ------------------------------------------------------------------------------- effect presets

/// Effect presets: (id, what it does, timed: applies around `seconds` instead of the whole clip).
pub const FX: &[(&str, &str, bool)] = &[
    ("vhs", "VHS tape: noise, horizontal smear, washed contrast", false),
    ("grain", "film grain", false),
    ("vignette", "dark corners that pull the eye to the centre", false),
    ("letterbox", "cinematic black bars top and bottom", false),
    ("blur", "blur the whole clip (backgrounds, censoring)", false),
    ("pixelate", "mosaic (censor a clip)", false),
    ("mirror", "flip horizontally", false),
    ("pushin", "slow push-in over the whole clip (keeps static shots alive)", false),
    ("fadein", "fade from black at the clip start", false),
    ("fadeout", "fade to black at the clip end", false),
    ("blurin", "start blurred and come into focus", false),
    ("flash", "white flash at `seconds`", true),
    ("glitch", "digital glitch burst at `seconds` (jitter, smear, noise)", true),
    ("zoompulse", "quick zoom pulse on a beat at `seconds`", true),
];

fn fx(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "studio.fx";
    let name = str_p(p, "fx").ok_or_else(|| bad(CMD, "need `fx` (studio.fx.list lists them)"))?.to_ascii_lowercase().replace([' ', '-', '_'], "");
    let &(id, what, timed) = FX.iter().find(|x| x.0 == name).ok_or_else(|| {
        let all: Vec<&str> = FX.iter().map(|x| x.0).collect();
        bad(CMD, format!("unknown fx `{name}` ({})", all.join(", ")))
    })?;
    let t = at_p(s, p);
    let clips = if timed { vec![video_clip_at(s, p, t)?] } else { target_clips(s, p, t)? };
    let amount = f64_p(p, "amount").unwrap_or(1.0).clamp(0.0, 5.0);
    let d = f64_p(p, "durationSeconds");
    let (fw, fh) = frame(s)?;
    s.compound(&format!("FX: {id}"), |s| {
        for &c in &clips {
            let (start, end) = s.active_sequence().and_then(|q| q.find_item(c)).map(|(_, it)| (it.start, it.end())).ok_or_else(|| bad(CMD, "no such clip"))?;
            match id {
                "vhs" => {
                    fx_edit(s, "VHS", c, "noise", vec![("amount", f(9.0 * amount)), ("color", ParamValue::Bool(true))], vec![])?;
                    fx_edit(s, "VHS", c, "directional_blur", vec![("direction", f(90.0)), ("length", f(3.0 * amount))], vec![])?;
                    fx_edit(s, "VHS", c, "brightness_contrast", vec![("brightness", f(4.0)), ("contrast", f(-14.0 * amount))], vec![])?;
                }
                "grain" => fx_edit(s, "Grain", c, "noise", vec![("amount", f(6.0 * amount)), ("color", ParamValue::Bool(false))], vec![])?,
                "vignette" => fx_edit(
                    s,
                    "Vignette",
                    c,
                    "lumetri",
                    vec![("vignette_on", ParamValue::Bool(true)), ("vignette_amount", f((-1.6 * amount).max(-5.0))), ("vignette_midpoint", f(42.0))],
                    vec![],
                )?,
                "letterbox" => {
                    // bars that leave a 2.39:1 picture
                    let keep = (fw / 2.39 / fh).min(1.0);
                    let bar = ((1.0 - keep) / 2.0 * 100.0).clamp(0.0, 45.0);
                    fx_edit(s, "Letterbox", c, "crop", vec![("top", f(bar)), ("bottom", f(bar))], vec![])?
                }
                "blur" => fx_edit(s, "Blur", c, "gaussian_blur", vec![("blurriness", f(40.0 * amount)), ("repeat_edge", ParamValue::Bool(true))], vec![])?,
                "pixelate" => {
                    let blocks = (40.0 / amount.max(0.2)).clamp(4.0, 400.0);
                    fx_edit(s, "Pixelate", c, "mosaic", vec![("horizontal", f(blocks)), ("vertical", f((blocks * fh / fw).max(4.0)))], vec![])?
                }
                "mirror" => fx_edit(s, "Mirror", c, "horizontal_flip", vec![], vec![])?,
                "pushin" => {
                    let base = scale_at(s, c, start);
                    motion_keys(s, "Push In", c, "scale", vec![(start, f(base)), (end - Tick(1), f(base * (1.0 + 0.1 * amount)))], false)?;
                }
                "fadein" => fx_edit(
                    s,
                    "Fade In",
                    c,
                    "opacity",
                    vec![],
                    vec![("opacity", vec![(start, f(0.0)), (start + sec(d.unwrap_or(0.5).clamp(0.05, 30.0)), f(100.0))])],
                )?,
                "fadeout" => fx_edit(
                    s,
                    "Fade Out",
                    c,
                    "opacity",
                    vec![],
                    vec![("opacity", vec![(end - sec(d.unwrap_or(0.5).clamp(0.05, 30.0)), f(100.0)), (end - Tick(1), f(0.0))])],
                )?,
                "blurin" => fx_edit(
                    s,
                    "Blur In",
                    c,
                    "gaussian_blur",
                    vec![("repeat_edge", ParamValue::Bool(true))],
                    vec![("blurriness", vec![(start, f(80.0 * amount)), (start + sec(d.unwrap_or(0.5).clamp(0.05, 30.0)), f(0.0))])],
                )?,
                "flash" => flash(s, c, t, sec(d.unwrap_or(0.25).clamp(0.05, 5.0)), amount)?,
                "glitch" => glitch(s, c, t, sec(d.unwrap_or(0.3).clamp(0.05, 5.0)), amount, fw)?,
                "zoompulse" => {
                    let base = scale_at(s, c, t);
                    let h = d.unwrap_or(0.2).clamp(0.05, 5.0);
                    motion_keys(
                        s,
                        "Zoom Pulse",
                        c,
                        "scale",
                        vec![(t, f(base)), (t + sec(h * 0.35), f(base * (1.0 + 0.12 * amount))), (t + sec(h), f(base))],
                        true,
                    )?;
                }
                _ => {}
            }
        }
        Ok(json!({"fx": id, "what": what, "clips": clips.iter().map(|c| c.0).collect::<Vec<_>>(), "at": if timed { Some(secs(t)) } else { None }}))
    })
}

fn flash(s: &mut Session, c: ClipId, t: Tick, d: Tick, amount: f64) -> Result<()> {
    let half = Tick(d.0 / 2);
    let peak = (100.0 * amount).clamp(0.0, 100.0);
    fx_edit(s, "Flash", c, "brightness_contrast", vec![], vec![("brightness", vec![(t - half, f(0.0)), (t, f(peak)), (t + half, f(0.0))])])?;
    fx_edit(s, "Flash", c, "lumetri", vec![], vec![("exposure", vec![(t - half, f(0.0)), (t, f(2.5 * amount.min(2.0))), (t + half, f(0.0))])])
}

fn glitch(s: &mut Session, c: ClipId, t: Tick, d: Tick, amount: f64, fw: f64) -> Result<()> {
    let Some(pos) = pos_at(s, c, t) else { return Err(bad("studio.fx", "the clip's Motion position is not set")) };
    let mut rng = filmcraft_audio_dsp::synth::Rng::new(t.0.unsigned_abs() ^ 0x9e37);
    let steps = 6u32;
    let mut pk = vec![(t, ParamValue::Vec2(pos))];
    let mut bk = vec![(t, f(0.0))];
    let mut nk = vec![(t, f(0.0))];
    for i in 1..steps {
        let at = t + Tick(d.0 / i64::from(steps) * i64::from(i));
        let jump = f64::from(rng.bipolar()) * fw * 0.04 * amount;
        pk.push((at, ParamValue::Vec2(Vec2::new(pos.x + jump, pos.y))));
        bk.push((at, f(if i % 2 == 0 { 0.0 } else { 40.0 * amount })));
        nk.push((at, f(if i % 2 == 0 { 10.0 } else { 45.0 * amount.min(2.0) })));
    }
    pk.push((t + d, ParamValue::Vec2(pos)));
    bk.push((t + d, f(0.0)));
    nk.push((t + d, f(0.0)));
    motion_keys(s, "Glitch", c, "position", pk, false)?;
    fx_edit(s, "Glitch", c, "directional_blur", vec![("direction", f(90.0))], vec![("length", bk)])?;
    fx_edit(s, "Glitch", c, "noise", vec![("color", ParamValue::Bool(true))], vec![("amount", nk)])
}

// ------------------------------------------------------------------------------- transitions

/// Transition styles: (id, sound effect, what it does).
pub const TRANSITIONS: &[(&str, &str, &str)] = &[
    ("zoom", "zoom_in", "zoom through the cut (scale up into blur, out of blur on the next shot)"),
    ("whip", "whip", "whip pan: the shot flies sideways with motion blur, the next one slides in"),
    ("flash", "flash", "white flash on the cut"),
    ("glitch", "glitch", "digital glitch across the cut"),
    ("spin", "swish", "the shot spins out, the next spins in"),
    ("shake", "impact", "hard cut with a camera shake on the new shot"),
];

/// The cut nearest `t` on video track `track` (default: the lowest track with a cut there):
/// (outgoing clip, incoming clip, cut time).
fn find_cut(s: &Session, p: &Value, t: Tick) -> Result<(ClipId, ClipId, Tick)> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let want = u64_p(p, "track").map(|n| n.saturating_sub(1) as usize);
    let mut best: Option<(i64, ClipId, ClipId, Tick)> = None;
    for (ti, tr) in q.video_tracks.iter().enumerate() {
        if want.is_some_and(|w| w != ti) {
            continue;
        }
        let mut items: Vec<_> = tr.items.iter().collect();
        items.sort_by_key(|it| it.start);
        for w in items.windows(2) {
            let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
            if (b.start - a.end()).0.abs() > Tick::from_seconds_f64(0.05).0 {
                continue;
            }
            let dist = (b.start - t).0.saturating_abs();
            if best.as_ref().is_none_or(|x| dist < x.0) {
                best = Some((dist, a.id, b.id, b.start));
            }
        }
        if want.is_none() && best.as_ref().is_some_and(|b| b.0 <= Tick::from_seconds_f64(1.0).0) {
            break;
        }
    }
    best.map(|b| (b.1, b.2, b.3)).ok_or_else(|| bad("studio.transition", "no cut between two touching clips on a video track"))
}

fn transition(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "studio.transition";
    let name = str_p(p, "style").unwrap_or("zoom").to_ascii_lowercase();
    let &(id, sound, what) = TRANSITIONS.iter().find(|x| x.0 == name).ok_or_else(|| {
        let all: Vec<&str> = TRANSITIONS.iter().map(|x| x.0).collect();
        bad(CMD, format!("unknown style `{name}` ({})", all.join(", ")))
    })?;
    let t = at_p(s, p);
    let (a, b, cut) = find_cut(s, p, t)?;
    let d = f64_p(p, "durationSeconds").unwrap_or(0.3).clamp(0.06, 3.0);
    let half = sec(d / 2.0);
    let with_sound = bool_p(p, "sound").unwrap_or(true);
    let (fw, _) = frame(s)?;
    s.compound(&format!("Transition: {id}"), |s| {
        let before = cut - half;
        let last = cut - Tick(1);
        match id {
            "zoom" => {
                let (sa, sb) = (scale_at(s, a, before), scale_at(s, b, cut));
                motion_keys(s, "Zoom Transition", a, "scale", vec![(before, f(sa)), (last, f(sa * 1.8))], true)?;
                motion_keys(s, "Zoom Transition", b, "scale", vec![(cut, f(sb * 1.8)), (cut + half, f(sb))], true)?;
                fx_edit(
                    s,
                    "Zoom Transition",
                    a,
                    "gaussian_blur",
                    vec![("repeat_edge", ParamValue::Bool(true))],
                    vec![("blurriness", vec![(before, f(0.0)), (last, f(45.0))])],
                )?;
                fx_edit(
                    s,
                    "Zoom Transition",
                    b,
                    "gaussian_blur",
                    vec![("repeat_edge", ParamValue::Bool(true))],
                    vec![("blurriness", vec![(cut, f(45.0)), (cut + half, f(0.0))])],
                )?;
            }
            "whip" => {
                // Offset wraps the picture around, so the frame never shows black while it flies
                let size = |s: &Session, c: ClipId| {
                    s.active_sequence()
                        .and_then(|q| q.find_item(c))
                        .and_then(|(_, it)| filmcraft_render::source_size(&s.project, it.item))
                        .map(|(w, h)| (f64::from(w), f64::from(h)))
                };
                let (aw, ah) = size(s, a).unwrap_or((fw, fw));
                let (bw, bh) = size(s, b).unwrap_or((fw, fw));
                let pt = |x: f64, y: f64| ParamValue::Vec2(Vec2::new(x, y));
                fx_edit(s, "Whip", a, "offset", vec![], vec![("shift", vec![(before, pt(aw / 2.0, ah / 2.0)), (last, pt(0.0, ah / 2.0))])])?;
                fx_edit(s, "Whip", b, "offset", vec![], vec![("shift", vec![(cut, pt(bw, bh / 2.0)), (cut + half, pt(bw / 2.0, bh / 2.0))])])?;
                fx_edit(s, "Whip", a, "directional_blur", vec![("direction", f(90.0))], vec![("length", vec![(before, f(0.0)), (last, f(160.0))])])?;
                fx_edit(s, "Whip", b, "directional_blur", vec![("direction", f(90.0))], vec![("length", vec![(cut, f(160.0)), (cut + half, f(0.0))])])?;
            }
            "flash" => {
                fx_edit(s, "Flash", a, "brightness_contrast", vec![], vec![("brightness", vec![(before, f(0.0)), (last, f(100.0))])])?;
                fx_edit(s, "Flash", a, "lumetri", vec![], vec![("exposure", vec![(before, f(0.0)), (last, f(3.0))])])?;
                fx_edit(s, "Flash", b, "brightness_contrast", vec![], vec![("brightness", vec![(cut, f(100.0)), (cut + half, f(0.0))])])?;
                fx_edit(s, "Flash", b, "lumetri", vec![], vec![("exposure", vec![(cut, f(3.0)), (cut + half, f(0.0))])])?;
            }
            "glitch" => {
                glitch(s, a, before, half, 1.0, fw)?;
                glitch(s, b, cut, half, 1.0, fw)?;
            }
            "spin" => {
                let (sa, sb) = (scale_at(s, a, before), scale_at(s, b, cut));
                motion_keys(s, "Spin", a, "rotation", vec![(before, f(0.0)), (last, f(180.0))], true)?;
                motion_keys(s, "Spin", a, "scale", vec![(before, f(sa)), (last, f(sa * 1.6))], true)?;
                motion_keys(s, "Spin", b, "rotation", vec![(cut, f(-180.0)), (cut + half, f(0.0))], true)?;
                motion_keys(s, "Spin", b, "scale", vec![(cut, f(sb * 1.6)), (cut + half, f(sb))], true)?;
            }
            "shake" => {
                s.execute("studio.shake", json!({"clip": b.0, "time": cut.0, "durationSeconds": d.max(0.25)}))?;
            }
            _ => {}
        }
        let sfx = if with_sound {
            let sfx_at = if id == "shake" || id == "flash" { cut } else { before.max(Tick::ZERO) };
            s.execute("studio.sfx.add", json!({"sound": sound, "time": sfx_at.0, "gainDb": -3.0})).ok().map(|r| r["clip"].clone())
        } else {
            None
        };
        Ok(json!({"style": id, "what": what, "cut": secs(cut), "from": a.0, "to": b.0, "sound": sfx.map(|c| json!({"id": sound, "clip": c}))}))
    })
}

// ------------------------------------------------------------------------------- karaoke captions

/// One timed word (sequence time).
#[derive(Clone, Debug, PartialEq)]
pub struct TimedWord {
    pub text: String,
    pub start: Tick,
    pub end: Tick,
}

/// Group words into short caption chunks: at most `max_words` words and `max_chars` characters,
/// broken at pauses longer than `pause` and after sentence ends.
pub fn karaoke_chunks(words: &[TimedWord], max_words: usize, max_chars: usize, pause: Tick) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut a = 0;
    let mut chars = 0;
    for (i, w) in words.iter().enumerate() {
        let n = w.text.chars().count();
        let gap = i > a && words.get(i.wrapping_sub(1)).is_some_and(|p| w.start - p.end > pause);
        let full = i > a && (i - a >= max_words.max(1) || chars + 1 + n > max_chars.max(1));
        if gap || full {
            out.push(a..i);
            a = i;
            chars = 0;
        }
        chars += n + usize::from(chars > 0);
        if w.text.ends_with(['.', '!', '?', '…']) {
            out.push(a..i + 1);
            a = i + 1;
            chars = 0;
        }
    }
    if a < words.len() {
        out.push(a..words.len());
    }
    out
}

/// Karaoke looks: (id, text colour, active-word colour, outline colour, background).
pub const KARAOKE_STYLES: &[(&str, &str, &str, &str, Option<&str>)] = &[
    ("yellow", "#ffffff", "#ffe135", "#000000", None),
    ("green", "#ffffff", "#39ff14", "#000000", None),
    ("red", "#ffffff", "#ff2d55", "#000000", None),
    ("cyan", "#ffffff", "#25f4ee", "#000000", None),
    ("boxed", "#000000", "#fe2c55", "#000000", Some("#ffffff")),
    ("dark", "#ffffff", "#ffe135", "#000000", Some("#000000b3")),
];

fn karaoke_words(s: &Session, p: &Value) -> Result<Vec<TimedWord>> {
    const CMD: &str = "studio.karaoke";
    if let Some(a) = p.get("words").and_then(Value::as_array) {
        let mut v = Vec::new();
        for w in a.iter().take(5000) {
            let text = w.get("text").and_then(Value::as_str).map(str::trim).unwrap_or("");
            let (Some(st), Some(en)) = (w.get("start").and_then(Value::as_f64), w.get("end").and_then(Value::as_f64)) else {
                return Err(bad(CMD, "each word needs `text`, `start` and `end` (sequence seconds)"));
            };
            if text.is_empty() || !st.is_finite() || !en.is_finite() || st < 0.0 || en < st {
                return Err(bad(CMD, format!("bad word `{text}` ({st} → {en})")));
            }
            v.push(TimedWord { text: text.chars().take(60).collect(), start: sec(st.min(1e6)), end: sec(en.min(1e6)) });
        }
        v.sort_by_key(|w| w.start);
        return Ok(v);
    }
    let words = crate::transcript::sequence_words(s);
    if words.is_empty() {
        let why = if crate::transcript::can_transcribe(s).is_ok() {
            "the sequence has no transcript yet: run transcript.generate first (it downloads the speech model once with transcript.downloadModel)"
        } else {
            "the sequence has no transcript and this build has no speech-to-text: pass `words` [{text, start, end}] with the timing, or use a FilmCraft build with the `whisper` feature"
        };
        return Err(bad(CMD, why));
    }
    let (from, to) = (f64_p(p, "fromSeconds").map(sec), f64_p(p, "toSeconds").map(sec));
    Ok(words
        .into_iter()
        .filter(|w| from.is_none_or(|f| w.end > f) && to.is_none_or(|t| w.start < t))
        .map(|w| TimedWord { text: w.text.trim().to_string(), start: w.start, end: w.end })
        .filter(|w| !w.text.is_empty())
        .take(5000)
        .collect())
}

fn karaoke(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "studio.karaoke";
    let name = str_p(p, "style").unwrap_or("yellow").to_ascii_lowercase();
    let &(id, fill, active, outline, bg) = KARAOKE_STYLES.iter().find(|k| k.0 == name).ok_or_else(|| {
        let all: Vec<&str> = KARAOKE_STYLES.iter().map(|k| k.0).collect();
        bad(CMD, format!("unknown style `{name}` ({})", all.join(", ")))
    })?;
    let active = str_p(p, "highlight").unwrap_or(active).to_string();
    let fill = str_p(p, "color").unwrap_or(fill).to_string();
    let one_word = str_p(p, "mode").is_some_and(|m| m.eq_ignore_ascii_case("word"));
    let max_words = if one_word { 1 } else { u64_p(p, "wordsPerLine").unwrap_or(3).clamp(1, 8) as usize };
    let max_chars = u64_p(p, "maxChars").unwrap_or(16).clamp(4, 60) as usize;
    let upper = bool_p(p, "uppercase").unwrap_or(true);
    let font = str_p(p, "font").map(str::to_string);
    let words = karaoke_words(s, p)?;
    if words.is_empty() {
        return Err(bad(CMD, "no words in that range"));
    }
    let (w, h) = frame(s)?;
    let y = match str_p(p, "position").unwrap_or("bottom") {
        "top" => h * 0.22,
        "center" | "middle" => h * 0.5,
        _ => h * 0.7,
    };
    // the longest line fits 90 % of the width (bold capitals ≈ 0.62 em)
    let px = (0.055 * h * f64_p(p, "sizeScale").unwrap_or(1.0).clamp(0.3, 4.0)).min(0.9 * w / (0.62 * max_chars as f64)).max(12.0).round();
    let chunks = karaoke_chunks(&words, max_words, max_chars, sec(0.6));
    let rate = s.sequence_rate();
    let fd = rate.frame_duration();
    let first = words.first().map(|x| x.start).unwrap_or_default();
    let last = words.last().map(|x| x.end.max(x.start + fd)).unwrap_or_default();
    s.compound(&format!("Karaoke Captions ({id})"), |s| {
        let tid = free_video_track(s, first, last - first, 1)?;
        let track = s.active_sequence().and_then(|q| q.video_tracks.iter().position(|t| t.id == tid)).ok_or(EngineError::NoSequence)?;
        let mut clips = 0usize;
        let mut prev_end = Tick::ZERO;
        for (ci, r) in chunks.iter().enumerate() {
            let Some(group) = words.get(r.clone()) else { continue };
            let next_chunk = chunks.get(ci + 1).and_then(|n| words.get(n.start)).map(|x| x.start);
            let texts: Vec<String> = group.iter().map(|x| if upper { x.text.to_uppercase() } else { x.text.clone() }).collect();
            let line = texts.join(" ");
            // character offsets of each word in the line
            let mut offs = Vec::with_capacity(texts.len());
            let mut at = 0usize;
            for t in &texts {
                let n = t.chars().count();
                offs.push((at, at + n));
                at += n + 1;
            }
            let chunk_end = group.last().map(|x| x.end.max(x.start + sec(0.25))).unwrap_or(prev_end);
            // the line stays a little after its last word, up to the next line
            let chunk_end = match next_chunk {
                Some(n) => (chunk_end + sec(0.3)).min(n).max(chunk_end.min(n)),
                None => chunk_end + sec(0.15),
            };
            for (wi, word) in group.iter().enumerate() {
                let start = rate.snap(if wi == 0 { word.start } else { word.start.max(prev_end) }).max(prev_end);
                let end = group.get(wi + 1).map_or(chunk_end, |n| n.start);
                let end = rate.snap(end).max(start + fd);
                if let Some(n) = next_chunk.map(|n| rate.snap(n))
                    && end > n
                    && n > start
                {
                    // never overlap the next line
                    prev_end = n;
                } else {
                    prev_end = end;
                }
                let dur = prev_end - start;
                if dur <= Tick::ZERO {
                    continue;
                }
                let mut q = json!({"text": line, "size": px, "position": [w / 2.0, y], "seconds": secs(dur), "time": start.0, "newClip": true, "track": track});
                if let Some(f) = &font {
                    q["font"] = json!(f);
                }
                let r = s.execute("graphics.newText", q)?;
                let clip = r["clip"].as_u64().map(ClipId).ok_or_else(|| EngineError::Other(format!("no caption clip in {r}")))?;
                let mut props = json!({"font_style": "Bold", "fill_color": fill, "align": "center", "stroke": true, "stroke_color": outline, "stroke_width": (px * 0.12).round(), "shadow": true});
                if let Some(b) = bg {
                    props["stroke"] = json!(false);
                    props["background"] = json!(true);
                    props["background_color"] = json!(b);
                    props["background_size"] = json!((px * 0.3).round());
                    props["background_radius"] = json!((px * 0.2).round());
                }
                s.execute("graphics.set", json!({"clip": clip.0, "props": props}))?;
                if let Some(&(a, b)) = offs.get(wi) {
                    s.execute("graphics.setCharStyle", json!({"clip": clip.0, "start": a, "end": b, "style": {"color": active, "size": (px * 1.12).round()}}))?;
                }
                // the line pops in on its first word (every word in one-word mode)
                if wi == 0 {
                    let k = |x: f64| start + sec(x);
                    motion_keys(s, "Karaoke", clip, "scale", vec![(start, f(70.0)), (k(0.07), f(108.0)), (k(0.12), f(100.0))], true)?;
                }
                clips += 1;
            }
        }
        Ok(json!({"style": id, "words": words.len(), "lines": chunks.len(), "clips": clips, "track": tid.0, "size": px, "from": secs(first), "to": secs(last)}))
    })
}

// ------------------------------------------------------------------------------- lists

fn fx_list(_s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(json!({
        "fx": FX.iter().map(|(id, what, timed)| json!({"id": id, "what": what, "timed": timed})).collect::<Vec<_>>(),
        "looks": LOOKS.iter().map(|(id, label, _)| json!({"id": id, "label": label})).collect::<Vec<_>>(),
        "transitions": TRANSITIONS.iter().map(|(id, sound, what)| json!({"id": id, "sound": sound, "what": what})).collect::<Vec<_>>(),
        "overlayPositions": SPOTS.iter().map(|s| s.0).collect::<Vec<_>>(),
        "overlayAnimations": ANIMATIONS,
    }))
}

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
        spec!("studio.fx.list", "List Effects, Looks & Transitions", "{}", always, fx_list, false),
        spec!(
            "studio.karaoke",
            "Karaoke Captions",
            r##"{"style":"yellow|green|red|cyan|boxed|dark"="yellow","mode":"line|word"="line","wordsPerLine":n=3,"maxChars":n=16,"position":"bottom|center|top","highlight":"#rrggbb"?,"color":"#rrggbb"?,"font":str?,"sizeScale":1,"uppercase":bool=true,"fromSeconds"?,"toSeconds"?,"words":[{"text","start","end"}]? (sequence seconds; default: the sequence transcript)}"##,
            has_seq,
            karaoke,
            true
        ),
        spec!(
            "studio.overlay",
            "Add Overlay / Sticker",
            r#"{"path":str,"seconds"|"time"?,"durationSeconds":f64? (pictures: 3),"position":"center|top|bottom|left|right|top-left|top-right|bottom-left|bottom-right"|[x,y] fractions,"width":fraction of frame width=0.4,"rotation":deg=0,"animate":"pop|bounce|fade|slide|spin|none"="pop","outro":bool=true}"#,
            has_seq,
            overlay,
            true
        ),
        spec!(
            "studio.broll",
            "Add B-roll",
            r#"{"path":str,"seconds"|"time"? (default playhead),"durationSeconds":f64=2.5,"sourceSeconds":f64? (where to start in the source),"skipIntro":bool=false (long footage: start a third in, past title slates),"pushIn":bool? (default: pictures only)}"#,
            has_seq,
            broll,
            true
        ),
        spec!(
            "studio.blurFill",
            "Blurred Background Fill",
            r#"{"clips":[id]? (default: every V1 clip whose shape differs from the frame),"blur":f64=60,"darken":f64=-25}"#,
            has_seq,
            blur_fill,
            true
        ),
        spec!(
            "studio.look",
            "Apply Colour Look",
            r#"{"look":"vibrant|cyberpunk|vaporwave|vintage|sepia|noir|dreamy|moody|sunset|matrix|cinematic|teal_orange|warm|cold|bleach|faded|bw|golden|night","intensity":0..200=100,"clips":[id]?|"all":bool? (default: selection, else the clip at the playhead)}"#,
            has_seq,
            look,
            true
        ),
        spec!(
            "studio.fx",
            "Apply Effect Preset",
            r#"{"fx":"vhs|grain|vignette|letterbox|blur|pixelate|mirror|pushin|fadein|fadeout|blurin|flash|glitch|zoompulse","amount":f64=1,"durationSeconds":f64?,"seconds"|"time"? (timed fx and clip choice),"clips":[id]?|"all":bool?}"#,
            has_seq,
            fx,
            true
        ),
        spec!(
            "studio.transition",
            "Add Transition",
            r#"{"style":"zoom|whip|flash|glitch|spin|shake"="zoom","seconds"|"time"? (the nearest cut; default playhead),"track":n? (1 = V1),"durationSeconds":f64=0.3,"sound":bool=true}"#,
            has_seq,
            transition,
            true
        ),
    ]
}

#[cfg(test)]
#[path = "studio_fx_tests.rs"]
mod tests;
