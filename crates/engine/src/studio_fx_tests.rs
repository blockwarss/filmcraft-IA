use std::path::PathBuf;
use std::sync::OnceLock;

use filmcraft_project::{ClipId, ParamValue};
use filmcraft_time::Tick;
use serde_json::json;

use super::*;
use crate::Session;

/// The same studio folder as `studio_tests` (the override is process-wide).
fn lib() -> PathBuf {
    static D: OnceLock<PathBuf> = OnceLock::new();
    D.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("filmcraft-studio-tests-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        crate::studio::set_library_dir(Some(d.clone()));
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

/// A `w`×`h` PNG with a transparent border (a sticker).
fn png(name: &str, w: u32, h: u32) -> String {
    let p = lib().join(name);
    let img =
        image::RgbaImage::from_fn(
            w,
            h,
            |x, y| if x > 2 && y > 2 && x < w - 3 && y < h - 3 { image::Rgba([255, 200, 0, 255]) } else { image::Rgba([0, 0, 0, 0]) },
        );
    img.save(&p).unwrap();
    p.to_string_lossy().into_owned()
}

/// Within a frame of `secs` (placements snap to whole frames).
fn close(t: Tick, secs: f64) -> bool {
    (t - Tick::from_seconds_f64(secs)).0.abs() <= Tick::from_seconds_f64(0.05).0
}

fn clip(r: &serde_json::Value, key: &str) -> ClipId {
    ClipId(r[key].as_u64().unwrap_or_else(|| panic!("no `{key}` in {r}")))
}

#[test]
fn stickers_land_on_a_new_top_track_sized_placed_and_animated() {
    let mut s = demo();
    s.execute("studio.format", json!({"preset": "tiktok"})).unwrap();
    let path = png("skull.png", 64, 64);
    let undo = s.history.undo.len();
    let tracks = s.active_sequence().unwrap().video_tracks.len();
    let r = s.execute("studio.overlay", json!({"path": path, "seconds": 1.0, "position": "top-right", "width": 0.25, "animate": "pop"})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1, "{r}");
    let q = s.active_sequence().unwrap();
    assert!(q.video_tracks.len() >= tracks.min(2));
    let c = clip(&r, "clip");
    let (tid, it) = q.find_item(c).unwrap();
    assert_ne!(Some(tid), q.video_tracks.first().map(|t| t.id), "never on V1");
    assert_eq!(it.start, Tick::from_seconds_f64(1.0));
    assert!(close(it.duration, 3.0), "{:?}", it.duration);
    let m = it.effect("motion").unwrap();
    // 25 % of 1080 px from a 64 px picture, and the pop + outro keys
    assert!((r["scale"].as_f64().unwrap() - 0.25 * 1080.0 / 64.0 * 100.0).abs() < 1e-6);
    assert_eq!(m.params["scale"].keyframes.len(), 5);
    assert!(matches!(m.params["position"].value, ParamValue::Vec2(v) if (v.x - 0.7 * 1080.0).abs() < 1e-6 && (v.y - 0.22 * 1920.0).abs() < 1e-6));
    for a in ANIMATIONS {
        s.execute("studio.overlay", json!({"path": path, "seconds": 0.5, "animate": a, "position": [0.4, 0.4]})).unwrap();
    }
    assert!(s.execute("studio.overlay", json!({"path": path, "animate": "explode"})).is_err());
    assert!(s.execute("studio.overlay", json!({"path": path, "position": "nowhere"})).is_err());
    assert!(s.execute("studio.overlay", json!({})).is_err());
}

#[test]
fn broll_fills_the_frame_above_the_main_track() {
    let mut s = demo();
    s.execute("studio.format", json!({"preset": "tiktok"})).unwrap();
    let path = png("city.png", 1920, 1080);
    let r = s.execute("studio.broll", json!({"path": path, "seconds": 0.5, "durationSeconds": 2.0})).unwrap();
    let q = s.active_sequence().unwrap();
    let (tid, it) = q.find_item(clip(&r, "clip")).unwrap();
    assert_ne!(Some(tid), q.video_tracks.first().map(|t| t.id));
    assert!(close(it.duration, 2.0), "{:?}", it.duration);
    let m = it.effect("motion").unwrap();
    // fills 1920 tall from 1080 px, then a push-in for a still
    assert_eq!(m.params["scale"].keyframes.len(), 2, "{r}");
}

#[test]
fn blur_fill_puts_the_whole_shot_over_a_blurred_copy() {
    let mut s = demo();
    s.execute("sequence.settings", json!({"width": 1080, "height": 1920})).unwrap();
    let undo = s.history.undo.len();
    let r = s.execute("studio.blurFill", json!({})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1, "{r}");
    let pairs = r["clips"].as_array().unwrap();
    assert!(!pairs.is_empty(), "{r}");
    let q = s.active_sequence().unwrap();
    let bg = clip(&pairs[0], "background");
    let fg = clip(&pairs[0], "foreground");
    let (_, b) = q.find_item(bg).unwrap();
    let (_, f) = q.find_item(fg).unwrap();
    assert!(b.effect("gaussian_blur").is_some() && b.effect("brightness_contrast").is_some());
    assert!(f.effect("gaussian_blur").is_none());
    assert_eq!((b.start, b.duration, b.source_in), (f.start, f.duration, f.source_in));
    let scale = |it: &filmcraft_project::TrackItem| match it.effect("motion").unwrap().params["scale"].value {
        ParamValue::Float(v) => v,
        _ => 0.0,
    };
    assert!(scale(b) > scale(f), "the background fills, the foreground fits");
    // a frame-shaped clip needs nothing
    let mut s = demo();
    assert!(s.execute("studio.blurFill", json!({})).is_err());
}

#[test]
fn generated_looks_are_sane_and_applied_through_lumetri() {
    for (id, _, builtin) in LOOKS {
        if builtin.is_some() {
            continue;
        }
        let g = look_fn(id).unwrap();
        for c in [[0.0f32, 0.0, 0.0], [1.0, 1.0, 1.0], [0.5, 0.5, 0.5], [1.0, 0.0, 0.0], [0.2, 0.6, 0.9]] {
            assert!(g(c).iter().all(|v| v.is_finite()), "{id}");
        }
        let cube = look_cube(id).unwrap();
        assert!(filmcraft_color::lut::Lut3d::parse_cube(&cube).is_ok(), "{id}");
    }
    assert!(look_fn("nope").is_none());
    let mut s = demo();
    let undo = s.history.undo.len();
    let r = s.execute("studio.look", json!({"look": "cyberpunk", "seconds": 1.0, "intensity": 80})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1, "{r}");
    let c = ClipId(r["clips"][0].as_u64().unwrap());
    let (_, it) = s.active_sequence().unwrap().find_item(c).unwrap();
    let lum = it.effect("lumetri").unwrap();
    assert!(matches!(&lum.params["look_lut"].value, ParamValue::Text(t) if t.starts_with("lib:")));
    assert!(matches!(lum.params["look_intensity"].value, ParamValue::Float(v) if (v - 80.0).abs() < 1e-9));
    let b = s.execute("studio.look", json!({"look": "Teal Orange", "all": true})).unwrap();
    assert!(!b["clips"].as_array().unwrap().is_empty());
    assert!(s.execute("studio.look", json!({"look": "nope"})).is_err());
}

#[test]
fn every_effect_preset_applies_in_one_undo_step() {
    for (id, _, _) in FX {
        let mut s = demo();
        let undo = s.history.undo.len();
        let r = s.execute("studio.fx", json!({"fx": id, "seconds": 1.0}));
        assert!(r.is_ok(), "{id}: {r:?}");
        assert_eq!(s.history.undo.len(), undo + 1, "{id}");
    }
    let mut s = demo();
    assert!(s.execute("studio.fx", json!({"fx": "nope"})).is_err());
    assert!(s.execute("studio.fx", json!({})).is_err());
    let r = s.execute("studio.fx", json!({"fx": "VHS", "all": true})).unwrap();
    assert!(!r["clips"].as_array().unwrap().is_empty());
    let list = s.execute("studio.fx.list", json!({})).unwrap();
    assert_eq!(list["fx"].as_array().unwrap().len(), FX.len());
}

#[test]
fn transitions_keyframe_both_sides_of_the_cut_and_add_their_sound() {
    for (id, _, _) in TRANSITIONS {
        let mut s = demo();
        s.execute("timeline.razor", json!({"seconds": 2.0})).unwrap();
        let undo = s.history.undo.len();
        let r = s.execute("studio.transition", json!({"style": id, "seconds": 2.1})).unwrap();
        assert_eq!(s.history.undo.len(), undo + 1, "{id}: {r}");
        assert!((r["cut"].as_f64().unwrap() - 2.0).abs() < 0.05, "{id}: {r}");
        assert!(r["sound"]["clip"].is_u64(), "{id}: {r}");
        let q = s.active_sequence().unwrap();
        let (_, b) = q.find_item(clip(&r, "to")).unwrap();
        let keyed = b.effects.iter().any(|e| e.params.values().any(|p| !p.keyframes.is_empty()));
        assert!(keyed, "{id}");
    }
    let mut s = demo();
    s.execute("timeline.razor", json!({"seconds": 2.0})).unwrap();
    let quiet = s.execute("studio.transition", json!({"style": "zoom", "seconds": 2.0, "sound": false})).unwrap();
    assert!(quiet["sound"].is_null());
    assert!(s.execute("studio.transition", json!({"style": "nope"})).is_err());
}

#[test]
fn hostile_parameters_never_panic() {
    let mut s = demo();
    let path = png("tiny.png", 1, 1);
    for p in [
        json!({"path": path, "width": 1e300, "rotation": f64::MAX, "seconds": -5, "durationSeconds": 1e300, "position": [1e300, -1e300]}),
        json!({"path": path, "width": -1, "durationSeconds": 0, "position": []}),
        json!({"path": "/no/such/file.png"}),
        json!({"path": 3}),
    ] {
        let _ = s.execute("studio.overlay", p.clone());
        let _ = s.execute("studio.broll", p);
    }
    for p in
        [json!({"fx": "glitch", "amount": 1e300, "durationSeconds": -1}), json!({"fx": "letterbox", "clips": [999999]}), json!({"fx": "pixelate", "amount": 0})]
    {
        let _ = s.execute("studio.fx", p);
    }
    let _ = s.execute("studio.look", json!({"look": "vibrant", "clips": [123456789], "intensity": f64::NAN}));
    let _ = s.execute("studio.transition", json!({"track": 99, "durationSeconds": 1e9}));
    let _ = s.execute("studio.blurFill", json!({"clips": [], "blur": -1e300}));
}

/// Renders a showcase of the recipes to PNGs for a human to look at:
/// `FILMCRAFT_SHOWCASE=<dir> cargo test -p filmcraft-engine showcase -- --ignored`.
#[test]
#[ignore = "writes pictures for review"]
fn showcase() {
    let Ok(out) = std::env::var("FILMCRAFT_SHOWCASE") else { return };
    std::fs::create_dir_all(&out).unwrap();
    let shot = |s: &mut Session, name: &str, secs: f64| {
        s.execute("sequence.setPlayhead", json!({"seconds": secs})).ok();
        let path = format!("{out}/{name}.png");
        s.execute("file.exportFrame", json!({"target": "program", "time": Tick::from_seconds_f64(secs).0, "path": path})).unwrap();
    };
    let sticker = png("sticker.png", 256, 256);
    // vertical frame from landscape demo footage
    let mut s = demo();
    s.execute("sequence.settings", json!({"width": 1080, "height": 1920})).unwrap();
    s.execute("studio.blurFill", json!({})).unwrap();
    shot(&mut s, "blurfill", 1.0);
    s.execute(
        "studio.title",
        json!({"text": "Reply to @maya's comment\nhow did you do that??", "style": "reply", "seconds": 0.0, "durationSeconds": 10.0, "animate": "none"}),
    )
    .unwrap();
    s.execute("studio.title", json!({"text": "PART 2", "style": "part2", "seconds": 0.0, "durationSeconds": 10.0, "animate": "none", "position": "center"}))
        .unwrap();
    s.execute(
        "studio.overlay",
        json!({"path": sticker, "seconds": 0.0, "durationSeconds": 10.0, "position": "bottom-right", "width": 0.25, "animate": "none"}),
    )
    .unwrap();
    shot(&mut s, "titles-reply-part2-sticker", 1.0);
    for look in ["vibrant", "cyberpunk", "vintage", "noir", "vaporwave"] {
        let mut s = demo();
        s.execute("studio.look", json!({"look": look, "all": true})).unwrap();
        shot(&mut s, &format!("look-{look}"), 1.0);
    }
    for fx in ["vhs", "vignette", "letterbox"] {
        let mut s = demo();
        s.execute("studio.fx", json!({"fx": fx, "all": true})).unwrap();
        shot(&mut s, &format!("fx-{fx}"), 1.0);
    }
    let mut s = demo();
    s.execute("studio.fx", json!({"fx": "glitch", "seconds": 1.0})).unwrap();
    shot(&mut s, "fx-glitch-mid", 1.05);
    for style in ["zoom", "whip", "flash"] {
        let mut s = demo();
        s.execute("timeline.razor", json!({"seconds": 2.0})).unwrap();
        s.execute("studio.transition", json!({"style": style, "seconds": 2.0})).unwrap();
        shot(&mut s, &format!("transition-{style}-before"), 1.93);
        shot(&mut s, &format!("transition-{style}-after"), 2.04);
    }
    let mut s = demo();
    for (style, pos) in [("gaming", "top"), ("highlight", "center"), ("news", "bottom")] {
        s.execute(
            "studio.title",
            json!({"text": style.to_uppercase(), "style": style, "seconds": 0.0, "durationSeconds": 10.0, "animate": "none", "position": pos}),
        )
        .unwrap();
    }
    shot(&mut s, "titles-mix", 1.0);
}
