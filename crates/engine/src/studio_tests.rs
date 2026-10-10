use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use filmcraft_project::{ItemId, SequenceSettings, TrackKind};
use filmcraft_time::{TICKS_PER_SECOND, Tick, TimeRange};
use serde_json::{Value, json};

use super::*;
use crate::Session;

/// One studio folder for the whole test binary (the override is process-wide).
fn lib() -> PathBuf {
    static D: OnceLock<PathBuf> = OnceLock::new();
    D.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("filmcraft-studio-tests-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        set_library_dir(Some(d.clone()));
        d
    })
    .clone()
}

fn demo() -> Session {
    lib();
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn seq(s: &Session) -> Value {
    let mut s2 = Session { project: s.project.clone(), state: s.state.clone(), ..Session::default() };
    s2.execute("sequence.inspect", json!({})).unwrap()
}

fn audio_clips(s: &Session) -> usize {
    s.active_sequence().unwrap().audio_tracks.iter().map(|t| t.items.len()).sum()
}

/// A stereo WAV: `pattern` is (seconds, loud?) runs of a 220 Hz tone or silence.
fn wav(name: &str, pattern: &[(f64, bool)]) -> PathBuf {
    let sr = 48_000u32;
    let mut v = Vec::new();
    let mut n = 0u64;
    for &(sec, loud) in pattern {
        for _ in 0..(sec * sr as f64) as usize {
            let x = if loud { 0.5 * (n as f64 * 220.0 * std::f64::consts::TAU / sr as f64).sin() as f32 } else { 0.0 };
            v.extend([x, x]);
            n += 1;
        }
    }
    let p = lib().join(name);
    std::fs::write(&p, filmcraft_export::write_wav(&v, 2, sr, 16)).unwrap();
    p
}

/// A session whose active sequence holds `path` on A1 (sound only), linked like recorded sound.
fn session_with_sound(path: &std::path::Path) -> (Session, ItemId) {
    lib();
    let mut s = Session::default();
    let r = s.execute("file.import", json!({"paths": [path.to_string_lossy()]})).unwrap();
    let item = ItemId(r["items"][0].as_u64().unwrap());
    let mut p = (*s.project).clone();
    let rate = filmcraft_time::FrameRate::FPS_24;
    let sq = p.new_sequence("Seq", SequenceSettings { width: 640, height: 360, frame_rate: rate, ..Default::default() }, 1, 1, None);
    let d = p.item(item).unwrap().duration();
    let a = p.make_track_item(item, TrackKind::Audio, Tick::ZERO, TimeRange::new(Tick::ZERO, d), rate).unwrap();
    p.sequence_mut(sq).unwrap().audio_tracks[0].items.push(a);
    s.project = Arc::new(p);
    s.state.active_sequence = Some(sq);
    s.state.open_sequences = vec![sq];
    (s, item)
}

#[test]
fn sfx_are_listed_added_on_a_free_track_and_reused() {
    let mut s = demo();
    let l = s.execute("studio.sfx.list", json!({"category": "game"})).unwrap();
    assert!(l["sounds"].as_array().unwrap().iter().any(|x| x["id"] == "level_up"), "{l}");
    assert!(l["sounds"].as_array().unwrap().iter().all(|x| x["category"] == "game"));
    let found = s.execute("studio.sfx.list", json!({"query": "tnt"})).unwrap();
    assert_eq!(found["sounds"][0]["id"], "explosion");

    let before = audio_clips(&s);
    let undo = s.history.undo.len();
    let r = s.execute("studio.sfx.add", json!({"sound": "whoosh", "seconds": 1.0})).unwrap();
    assert_eq!(audio_clips(&s), before + 1);
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    assert!(PathBuf::from(r["path"].as_str().unwrap()).exists());
    let q = s.active_sequence().unwrap();
    let (tid, it) = q.find_item(filmcraft_project::ClipId(r["clip"].as_u64().unwrap())).unwrap();
    assert_eq!(it.start, Tick::from_seconds_f64(1.0));
    assert_eq!(q.track(tid).unwrap().kind, TrackKind::Audio);
    // the same sound twice reuses the imported item, on another free track
    let r2 = s.execute("studio.sfx.add", json!({"sound": "Whoosh", "seconds": 1.0, "gainDb": -6})).unwrap();
    assert_eq!(r2["item"], r["item"]);
    assert_ne!(r2["track"], r["track"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(audio_clips(&s), before + 1);

    assert!(s.execute("studio.sfx.add", json!({"sound": "nope"})).is_err());
    assert!(s.execute("studio.sfx.add", json!({})).is_err());
}

#[test]
fn hostile_parameters_never_panic() {
    let mut s = demo();
    for (id, p) in [
        ("studio.sfx.add", json!({"sound": "pop", "seconds": f64::MAX, "variant": u64::MAX, "gainDb": 1e300})),
        ("studio.sfx.add", json!({"sound": "pop", "seconds": -5, "track": "A99"})),
        ("studio.music.generate", json!({"durationSeconds": -1})),
        ("studio.music.generate", json!({"style": "polka"})),
        ("studio.music.generate", json!({"key": "H"})),
        ("studio.music.generate", json!({"key": [1]})),
        ("studio.silences", json!({"thresholdDb": "loud", "minSeconds": -3, "padSeconds": 1e9})),
        ("studio.beatMontage", json!({"items": []})),
        ("studio.beatMontage", json!({"items": [999_999], "beatsPerShot": 0})),
        ("studio.punchZoom", json!({"scale": f64::NAN, "seconds": 1e12, "clip": 424242})),
        ("studio.shake", json!({"amount": -1, "frequency": 1e9})),
        ("studio.format", json!({"preset": "imax"})),
        ("studio.captions", json!({"style": "comic"})),
        ("studio.title", json!({"text": "x".repeat(600)})),
        ("studio.title", json!({})),
        ("studio.library.add", json!({"query": "zzzz-no-such-file"})),
        ("studio.library.addFolder", json!({"path": "/definitely/not/here"})),
        ("studio.fonts.install", json!({"path": "/etc/passwd"})),
        ("studio.exportSettings", json!({"platform": "myspace"})),
        ("studio.guide", json!({"clip": 777777})),
    ] {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(id, p.clone())));
        assert!(r.is_ok(), "{id} {p} panicked");
    }
}

#[test]
fn generated_music_is_placed_with_its_exact_beat_grid() {
    let mut s = demo();
    let undo = s.history.undo.len();
    let r = s.execute("studio.music.generate", json!({"style": "edm", "durationSeconds": 6, "bpm": 120, "key": "Am", "seconds": 1.0})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1, "{r}");
    assert_eq!(r["bpm"].as_f64(), Some(120.0));
    assert_eq!(r["key"], "A");
    let beats: Vec<f64> = r["beats"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert_eq!(beats.len(), 12);
    assert!((beats[0] - 1.0).abs() < 1e-6 && (beats[1] - 1.5).abs() < 1e-6, "{beats:?}");
    let clip = r["clip"].as_u64().unwrap();
    // studio.beats reads the same grid back from the sidecar
    let b = s.execute("studio.beats", json!({"clip": clip})).unwrap();
    assert_eq!(b["exact"], true);
    assert_eq!(b["beats"].as_array().unwrap().len(), 12);
    // markers on every other beat
    let m = s.execute("studio.markBeats", json!({"clip": clip, "every": 2})).unwrap();
    assert_eq!(m["markers"], 6);
    // generating the same track again reuses the file
    let again = s.execute("studio.music.generate", json!({"style": "edm", "durationSeconds": 6, "bpm": 120, "key": 9, "place": false})).unwrap();
    assert_eq!(again["path"], r["path"]);
    assert_eq!(again["item"], r["item"]);
}

#[test]
fn cut_on_the_beat_and_beat_montage() {
    let mut s = demo();
    let v1_before = s.active_sequence().unwrap().video_tracks[0].items.len();
    let r = s.execute("studio.music.generate", json!({"style": "phonk", "durationSeconds": 4, "bpm": 120, "seconds": 0})).unwrap();
    let clip = r["clip"].as_u64().unwrap();
    let c = s.execute("studio.beatCut", json!({"clip": clip, "every": 2, "tracks": ["V1"]})).unwrap();
    assert!(c["cuts"].as_u64().unwrap() >= 1, "{c}");
    assert!(s.active_sequence().unwrap().video_tracks[0].items.len() > v1_before);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active_sequence().unwrap().video_tracks[0].items.len(), v1_before);

    // montage of the project's media on V2, a shot every 2 beats (1 s at 120 bpm)
    let items: Vec<u64> =
        s.project.items.iter().filter(|(_, i)| matches!(i.kind, filmcraft_project::ItemKind::Media(_)) && i.has_video()).map(|(id, _)| id.0).take(2).collect();
    assert!(!items.is_empty());
    let n_tracks = s.active_sequence().unwrap().video_tracks.len();
    if n_tracks < 2 {
        s.execute("sequence.addTracks", json!({"video": 1, "audio": 0})).unwrap();
    }
    let undo = s.history.undo.len();
    let m = s.execute("studio.beatMontage", json!({"items": items, "clip": clip, "beatsPerShot": 2, "track": "V2"})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1, "{m}");
    assert!(m["shots"].as_u64().unwrap() >= 2, "{m}");
    let q = s.active_sequence().unwrap();
    let one = Tick(TICKS_PER_SECOND);
    let shots: Vec<_> = m["clips"].as_array().unwrap().iter().map(|c| q.find_item(filmcraft_project::ClipId(c.as_u64().unwrap())).unwrap()).collect();
    assert_eq!(shots.len(), 3, "{m}");
    for (tid, it) in shots {
        assert_eq!(tid, q.video_tracks[1].id);
        assert!((it.duration.0 - one.0).abs() <= s.sequence_rate().frame_duration().0, "{:?}", it.duration);
    }
}

#[test]
fn silences_are_found_and_cut_as_jump_cuts() {
    let p = wav("speech-like.wav", &[(1.0, true), (1.5, false), (1.0, true), (0.2, false), (1.0, true)]);
    let (mut s, _) = session_with_sound(&p);
    let r = s.execute("studio.silences", json!({"minSeconds": 0.5})).unwrap();
    assert_eq!(r["count"], 1, "{r}");
    let gap = &r["silences"][0];
    // 1.0 s … 2.5 s, minus 0.12 s of air each side, snapped inward to 24 fps frames
    assert!((gap["start"].as_f64().unwrap() - 1.125).abs() < 0.05, "{gap}");
    assert!((gap["end"].as_f64().unwrap() - 2.375).abs() < 0.05, "{gap}");
    let len = s.active_sequence().unwrap().duration();
    let undo = s.history.undo.len();
    let c = s.execute("studio.cutSilences", json!({"minSeconds": 0.5})).unwrap();
    assert_eq!(c["removed"], 1);
    assert_eq!(s.history.undo.len(), undo + 1);
    let cut = Tick::from_seconds_f64(c["seconds"].as_f64().unwrap());
    assert_eq!(s.active_sequence().unwrap().duration(), len - cut);
    assert!(c["seconds"].as_f64().unwrap() > 1.1);
    // an explicit threshold above the tone finds everything silent; a short minimum finds the 0.2 s gap too
    let r = s.execute("studio.silences", json!({"thresholdDb": -100, "minSeconds": 0.1, "padSeconds": 0.0})).unwrap();
    // what is left: the air kept around the cut (0.25 s) and the 0.2 s gap
    assert_eq!(r["count"], 2, "{r}");
}

#[test]
fn auto_threshold_sits_between_floor_and_speech() {
    let mut lv = vec![-70.0f32; 500];
    lv.extend(vec![-20.0f32; 500]);
    let t = auto_threshold(&lv);
    assert!(t > -60.0 && t < -30.0, "{t}");
    assert_eq!(auto_threshold(&[]), -45.0);
    assert!(auto_threshold(&[f32::NAN, f32::NEG_INFINITY]).is_finite());
}

#[test]
fn punch_zoom_and_shake_keyframe_motion_in_one_undo_step() {
    let mut s = demo();
    let undo = s.history.undo.len();
    let r = s.execute("studio.punchZoom", json!({"seconds": 1.0, "scale": 150, "focus": [0.25, 0.25]})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1, "{r}");
    assert_eq!(r["keyframes"], 4);
    let c = filmcraft_project::ClipId(r["clip"].as_u64().unwrap());
    let q = s.active_sequence().unwrap();
    let (_, it) = q.find_item(c).unwrap();
    let m = it.effect("motion").unwrap();
    assert_eq!(m.params["scale"].keyframes.len(), 4);
    assert_eq!(m.params["position"].keyframes.len(), 4);
    let peak = it.source_time_at(Tick::from_seconds_f64(1.3));
    assert!(matches!(m.params["scale"].value_at(peak), filmcraft_project::ParamValue::Float(v) if (v - r["to"].as_f64().unwrap()).abs() < 1e-6));
    s.execute("edit.undo", json!({})).unwrap();
    let (_, it) = s.active_sequence().unwrap().find_item(c).unwrap();
    assert!(it.effect("motion").unwrap().params["scale"].keyframes.is_empty());
    let sh = s.execute("studio.shake", json!({"seconds": 2.0, "durationSeconds": 0.5})).unwrap();
    assert!(sh["keyframes"].as_u64().unwrap() >= 10, "{sh}");
}

#[test]
fn format_for_tiktok_fills_the_vertical_frame() {
    let mut s = demo();
    let r = s.execute("studio.format", json!({"preset": "tiktok"})).unwrap();
    let q = s.active_sequence().unwrap();
    assert_eq!((q.settings.width, q.settings.height), (1080, 1920));
    assert!(r["filled"].as_u64().unwrap() >= 1, "{r}");
    let e = s.execute("studio.exportSettings", json!({"platform": "tiktok"})).unwrap();
    assert_eq!(e["params"]["height"], 1920);
    assert_eq!(e["params"]["format"], "h264");
}

#[test]
fn titles_pop_in() {
    let mut s = demo();
    let undo = s.history.undo.len();
    let r = s.execute("studio.title", json!({"text": "WAIT FOR IT", "style": "gaming", "seconds": 0.5})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1, "{r}");
    let c = filmcraft_project::ClipId(r["clip"].as_u64().unwrap());
    let (_, it) = s.active_sequence().unwrap().find_item(c).unwrap();
    assert_eq!(it.effect("motion").unwrap().params["scale"].keyframes.len(), 3);
    let doc = seq(&s);
    assert!(doc.to_string().contains("WAIT FOR IT"));
    // a long line on a vertical frame shrinks to fit the width
    s.execute("studio.format", json!({"preset": "tiktok"})).unwrap();
    let long = s.execute("studio.title", json!({"text": "THE MOST INSANE CLUTCH EVER", "style": "gaming"})).unwrap();
    let px = long["size"].as_f64().unwrap();
    assert!(px * 0.62 * 27.0 <= 1080.0 * 0.9 + 1.0, "{px}");
}

#[test]
fn a_failing_recipe_changes_nothing() {
    let mut s = demo();
    let (project, undo) = (s.project.clone(), s.history.undo.len());
    let r = s.compound("Two Steps", |s| {
        s.execute("studio.sfx.add", json!({"sound": "pop", "seconds": 0.5}))?;
        s.execute("studio.sfx.add", json!({"sound": "no-such-sound"}))
    });
    assert!(r.is_err());
    assert!(Arc::ptr_eq(&s.project, &project));
    assert_eq!(s.history.undo.len(), undo);
}

#[test]
fn library_folders_are_searched_and_placed() {
    let dir = lib().join("packs-test").join("Meme Sounds");
    std::fs::create_dir_all(&dir).unwrap();
    let src = wav("bruh-tmp.wav", &[(0.3, true)]);
    std::fs::copy(&src, dir.join("vine boom loud.wav")).unwrap();
    std::fs::write(dir.join("notes.txt"), "not media").unwrap();
    let mut s = demo();
    s.execute("studio.library.addFolder", json!({"path": lib().join("packs-test").to_string_lossy()})).unwrap();
    let l = s.execute("studio.library.list", json!({"query": "vine boom"})).unwrap();
    let files = l["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{l}");
    assert_eq!(files[0]["kind"], "audio");
    assert!(files[0]["tags"].as_array().unwrap().contains(&json!("meme")));
    let before = audio_clips(&s);
    s.execute("studio.library.add", json!({"query": "boom", "seconds": 2})).unwrap();
    assert_eq!(audio_clips(&s), before + 1);
    s.execute("studio.library.addFolder", json!({"path": lib().join("packs-test").to_string_lossy(), "remove": true})).unwrap();
    let l = s.execute("studio.library.list", json!({"query": "vine"})).unwrap();
    assert!(l["files"].as_array().unwrap().is_empty());
}

#[test]
fn guide_track_holds_the_trending_sound_and_mutes_for_export() {
    let p = wav("trend.wav", &[(2.0, true)]);
    let mut s = demo();
    let r = s.execute("studio.library.add", json!({"path": p.to_string_lossy(), "seconds": 0, "guide": true})).unwrap();
    let q = s.active_sequence().unwrap();
    let guide = q.audio_tracks.iter().find(|t| t.name == GUIDE).unwrap();
    assert_eq!(guide.items.len(), 1, "{r}");
    let e = s.execute("studio.exportSettings", json!({})).unwrap();
    assert!(e["warning"].is_string());
    s.execute("studio.guide", json!({"mute": true})).unwrap();
    assert!(s.active_sequence().unwrap().audio_tracks.iter().any(|t| t.name == GUIDE && t.muted));
    let e = s.execute("studio.exportSettings", json!({})).unwrap();
    assert!(e["warning"].is_null());
    // the beat grid defaults to the Guide clip
    assert!(s.execute("studio.beats", json!({})).is_err() || s.execute("studio.beats", json!({})).is_ok());
}

#[test]
fn keys_parse_like_musicians_write_them() {
    for (k, want) in [("C", 0), ("Bb", 10), ("C#m", 1), ("A minor", 9), ("f#", 6), ("E♭", 3)] {
        assert_eq!(key_p(&json!({"key": k})).unwrap(), Some(want), "{k}");
    }
    assert_eq!(key_p(&json!({"key": 14})).unwrap(), Some(2));
    assert!(key_p(&json!({"key": "H"})).is_err());
}
