//! Kenney's sound packs (kenney.nl): hundreds of game, interface, impact, sci-fi and announcer
//! sounds, all CC0 (no credit needed, any use). A pack is downloaded once as a zip and unpacked
//! into the studio library, where `studio.library.list` / `studio.library.add` find its sounds.
//!
//! The zip link is read from the pack's page and must point at Kenney's own media folder for
//! that pack. Unpacking keeps audio files and licence texts only, rebuilds every path from
//! sanitised parts (no `..`, no absolute paths) and caps the number of files, each file's size and
//! the total, so a hostile archive cannot escape the folder or fill the disk.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{err, fetch};
use crate::AutomationError;

/// (slug, title, what is inside).
pub const PACKS: &[(&str, &str, &str)] = &[
    ("impact-sounds", "Impact Sounds", "about 130 punches, hits, footsteps on many surfaces, metal, wood, glass and plate impacts"),
    ("interface-sounds", "Interface Sounds", "100 clicks, confirmations, errors, back / close, drops, toggles, scrolls and glitches"),
    ("ui-audio", "UI Audio", "50 clicks, switches and rollovers"),
    ("digital-audio", "Digital Audio", "60 retro synth lasers, power-ups, zaps, phasers, tones and jumps"),
    ("sci-fi-sounds", "Sci-fi Sounds", "70 lasers, engines, force fields, doors, explosions and computer noises"),
    ("rpg-audio", "RPG Audio", "50 cloth, coins, doors, books, creaks, chops and footsteps"),
    ("casino-audio", "Casino Audio", "50 card, chip and dice sounds"),
    ("music-jingles", "Music Jingles", "85 short jingles (8-bit, hits, steel drums, pizzicato, saxophone) for wins, fails and transitions"),
    ("voiceover-pack", "Voiceover Pack", "announcer voices (male and female): 'game over', 'level up', 'mission completed', 'congratulations', 'go', numbers…"),
    (
        "voiceover-pack-fighter",
        "Voiceover Pack (Fighter)",
        "fighting-game announcer: 'fight', 'combo', 'combo breaker', 'flawless victory', 'final round', numbers…",
    ),
];

const MAX_ZIP_BYTES: u64 = 250 << 20;
const MAX_PAGE_BYTES: u64 = 2 << 20;
const MAX_ENTRIES: usize = 4_000;
const MAX_FILE_BYTES: u64 = 60 << 20;
const MAX_TOTAL_BYTES: u64 = 600 << 20;
const KEEP: &[&str] = &["ogg", "wav", "mp3", "flac", "txt"];

/// The pack folder name in the library.
pub fn folder(slug: &str) -> String {
    format!("kenney-{}", super::safe_name(slug))
}

fn known(slug: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    PACKS.iter().find(|p| p.0 == slug)
}

/// The packs, with whether each is already installed under `dir`.
pub fn list(dir: &Path) -> Vec<Value> {
    PACKS
        .iter()
        .map(|(slug, title, what)| {
            let d = dir.join(folder(slug));
            json!({"pack": slug, "title": title, "contents": what, "license": "CC0 (no credit needed)", "installed": d.is_dir(), "folder": d.to_string_lossy()})
        })
        .collect()
}

/// The pack's zip link in its page: `https://kenney.nl/media/pages/assets/<slug>/…/<name>.zip`.
pub fn zip_link(page: &str, slug: &str) -> Option<String> {
    let prefix = format!("https://kenney.nl/media/pages/assets/{slug}/");
    let mut rest = page;
    while let Some(i) = rest.find(&prefix) {
        let tail = rest.get(i..)?;
        let end = tail.find(['\'', '"', ' ', '<', '>']).unwrap_or(tail.len());
        let url = tail.get(..end)?;
        let path = url.get(prefix.len()..)?;
        let ok = url.ends_with(".zip") && !path.contains("..") && path.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'));
        if ok {
            return Some(url.to_string());
        }
        rest = tail.get(end.max(1)..)?;
    }
    None
}

/// A safe relative path for an archive entry, or `None` to skip it: every part made of letters,
/// digits, `-`, `_`, `.` and spaces (no `..`, no hidden parts), at most 6 deep, with a kept
/// extension.
pub fn entry_path(name: &str) -> Option<PathBuf> {
    if name.ends_with('/') || name.starts_with('/') || name.contains('\\') || name.contains(':') {
        return None;
    }
    let parts: Vec<&str> = name.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() || parts.len() > 6 {
        return None;
    }
    let mut out = PathBuf::new();
    for p in &parts {
        let ok = p.len() <= 100
            && !p.starts_with('.')
            && *p != ".."
            && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ' | '(' | ')' | '\''));
        if !ok {
            return None;
        }
        out.push(p);
    }
    let ext = out.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase)?;
    KEEP.contains(&ext.as_str()).then_some(out)
}

/// Unpack the kept files of `zip` into `dest`. Returns the audio files written.
pub fn unpack(zip: &[u8], dest: &Path) -> Result<Vec<PathBuf>, AutomationError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).map_err(|e| err(format!("not a zip archive: {e}")))?;
    if archive.len() > MAX_ENTRIES {
        return Err(err(format!("the archive has {} entries (at most {MAX_ENTRIES})", archive.len())));
    }
    std::fs::create_dir_all(dest).map_err(|e| err(format!("{}: {e}", dest.display())))?;
    let mut total: u64 = 0;
    let mut audio = Vec::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| err(format!("archive entry {i}: {e}")))?;
        if !f.is_file() {
            continue;
        }
        let Some(rel) = entry_path(f.name()) else { continue };
        if f.size() > MAX_FILE_BYTES {
            continue;
        }
        let mut buf = Vec::new();
        // the declared size can lie: read at most the cap
        (&mut f).take(MAX_FILE_BYTES + 1).read_to_end(&mut buf).map_err(|e| err(format!("{}: {e}", rel.display())))?;
        if buf.len() as u64 > MAX_FILE_BYTES {
            continue;
        }
        total = total.saturating_add(buf.len() as u64);
        if total > MAX_TOTAL_BYTES {
            return Err(err(format!("the archive unpacks to more than {} MB", MAX_TOTAL_BYTES >> 20)));
        }
        let out = dest.join(&rel);
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d).map_err(|e| err(format!("{}: {e}", d.display())))?;
        }
        std::fs::write(&out, &buf).map_err(|e| err(format!("{}: {e}", out.display())))?;
        if !rel.extension().is_some_and(|e| e.eq_ignore_ascii_case("txt")) {
            audio.push(out);
        }
    }
    Ok(audio)
}

/// Download and unpack pack `slug` into `dir/kenney-<slug>/` (skipped when already there).
pub fn install(slug: &str, dir: &Path) -> Result<Value, AutomationError> {
    let slug = slug.trim().to_ascii_lowercase();
    let (_, title, what) = known(&slug).ok_or_else(|| {
        let all: Vec<&str> = PACKS.iter().map(|p| p.0).collect();
        err(format!("unknown pack `{slug}` ({})", all.join(", ")))
    })?;
    let dest = dir.join(folder(&slug));
    let done = dest.join(".installed");
    if done.exists() {
        let mut files = Vec::new();
        collect(&dest, 0, &mut files);
        return Ok(summary(&slug, title, what, &dest, &files, true));
    }
    let page = String::from_utf8_lossy(&fetch(&format!("https://kenney.nl/assets/{slug}"), MAX_PAGE_BYTES, &[])?).into_owned();
    if !page.contains("CC0") {
        return Err(err(format!("the page of `{slug}` no longer says CC0; not downloading it")));
    }
    let url = zip_link(&page, &slug).ok_or_else(|| err(format!("no download link on the page of `{slug}`")))?;
    let zip = fetch(&url, MAX_ZIP_BYTES, &[])?;
    let files = unpack(&zip, &dest)?;
    let credit = format!(
        "asset:        {title} (Kenney)\nauthor:       Kenney (www.kenney.nl)\nsource:       https://kenney.nl/assets/{slug}\nlicense:      CC0 1.0\nlicense-url:  https://creativecommons.org/publicdomain/zero/1.0/\nattribution:  not required (\"Sounds by Kenney (kenney.nl), CC0\" is appreciated)\n"
    );
    let _ = std::fs::write(dest.join("ATTRIBUTION.txt"), credit);
    let _ = std::fs::write(&done, url);
    Ok(summary(&slug, title, what, &dest, &files, false))
}

fn collect(d: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 6 || out.len() > MAX_ENTRIES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(d) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, depth + 1, out);
        } else if p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "ogg" | "wav" | "mp3" | "flac")) {
            out.push(p);
        }
    }
}

fn summary(slug: &str, title: &str, what: &str, dest: &Path, files: &[PathBuf], already: bool) -> Value {
    let mut names: Vec<String> = files.iter().filter_map(|f| f.file_stem().map(|n| n.to_string_lossy().into_owned())).collect();
    names.sort();
    names.dedup();
    json!({
        "pack": slug,
        "title": title,
        "contents": what,
        "folder": dest.to_string_lossy(),
        "sounds": files.len(),
        "alreadyInstalled": already,
        "examples": names.iter().take(40).collect::<Vec<_>>(),
        "license": "CC0 — no credit needed",
        "next": "studio.library.list {query} finds these sounds; studio.library.add {query | path, seconds} places one",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (n, b) in entries {
            w.start_file(*n, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
            w.write_all(b).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn links_must_point_at_the_pack() {
        let page = "<a href='https://evil.example/x.zip'>no</a><a href='https://kenney.nl/media/pages/assets/other/1/x.zip'>no</a>\
            <a id='donate-text' href='https://kenney.nl/media/pages/assets/impact-sounds/87b4dd-16775/kenney_impact-sounds.zip' onclick>ok</a>";
        assert_eq!(
            zip_link(page, "impact-sounds").as_deref(),
            Some("https://kenney.nl/media/pages/assets/impact-sounds/87b4dd-16775/kenney_impact-sounds.zip")
        );
        assert_eq!(zip_link("https://kenney.nl/media/pages/assets/impact-sounds/../../x.zip'", "impact-sounds"), None);
        assert_eq!(zip_link("nothing here", "impact-sounds"), None);
        assert!(known("impact-sounds").is_some() && known("../x").is_none());
    }

    #[test]
    fn entry_paths_never_escape() {
        assert_eq!(entry_path("Audio/footstep_carpet_000.ogg"), Some(PathBuf::from("Audio/footstep_carpet_000.ogg")));
        assert_eq!(entry_path("License.txt"), Some(PathBuf::from("License.txt")));
        assert_eq!(entry_path("Male/it's_a_tie.ogg"), Some(PathBuf::from("Male/it's_a_tie.ogg")));
        for bad in
            ["../x.ogg", "/etc/x.ogg", "a/../../x.ogg", "C:/x.ogg", "a\\b.ogg", ".hidden.ogg", "Audio/", "x.exe", "a\"b.ogg", "a/b/c/d/e/f/g.ogg", "x;rm.ogg"]
        {
            assert_eq!(entry_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn unpacks_audio_only_inside_the_folder() {
        let dir = std::env::temp_dir().join(format!("filmcraft-pack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let z = zip_of(&[("Audio/hit_000.ogg", b"OggS..."), ("../escape.ogg", b"x"), ("run.sh", b"#!/bin/sh"), ("License.txt", b"CC0")]);
        let files = unpack(&z, &dir).unwrap();
        assert_eq!(files, vec![dir.join("Audio/hit_000.ogg")]);
        assert!(dir.join("License.txt").exists());
        assert!(!dir.join("run.sh").exists());
        assert!(!dir.parent().unwrap().join("escape.ogg").exists());
        assert!(unpack(b"not a zip", &dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lists_every_pack() {
        let l = list(Path::new("/nonexistent"));
        assert_eq!(l.len(), PACKS.len());
        assert_eq!(l[0]["installed"], false);
        assert_eq!(folder("../x"), "kenney-x");
    }

    /// Live: `cargo test -p filmcraft-automation packs -- --ignored`.
    #[test]
    #[ignore = "needs the network"]
    fn live_kenney() {
        let dir = std::env::temp_dir().join(format!("filmcraft-kenney-{}", std::process::id()));
        let r = install("impact-sounds", &dir).unwrap();
        assert!(r["sounds"].as_u64().unwrap() > 50, "{r}");
        let again = install("impact-sounds", &dir).unwrap();
        assert_eq!(again["alreadyInstalled"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
