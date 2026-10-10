//! Kevin MacLeod's Incompetech catalogue: about 1 500 tracks under CC BY 4.0, with mood ("feel"),
//! tempo, instruments and length, published as JSON for agents (incompetech.com/llms.txt).
//!
//! The catalogue is cached for a day next to the downloads. Results carry ids `km:<slug>`; a
//! download looks the slug up in the catalogue again, so the file name in the URL always comes
//! from Incompetech's own list, never from the caller.

use std::path::Path;

use serde_json::{Value, json};

use super::{Download, encode_path, err, fetch, sidecar, stem};
use crate::AutomationError;

const CATALOG_URL: &str = "https://incompetech.com/music/royalty-free/pieces.json";
const GENRES_URL: &str = "https://incompetech.com/music/royalty-free/genre.json";
const MP3_BASE: &str = "https://incompetech.com/music/royalty-free/mp3-royaltyfree/";
const LICENSE: &str = "CC BY 4.0";
const LICENSE_URL: &str = "https://creativecommons.org/licenses/by/4.0/";
const MAX_CATALOG_BYTES: u64 = 16 << 20;
const MAX_MP3_BYTES: u64 = 60 << 20;
const DAY: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// The id slug of a catalogue file name: `"The Britons.mp3"` → `the-britons`.
pub fn slug(filename: &str) -> String {
    let base = filename.strip_suffix(".mp3").unwrap_or(filename);
    let s: String = base.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    s.split('-').filter(|w| !w.is_empty()).collect::<Vec<_>>().join("-").chars().take(80).collect()
}

/// `"00:05:07"` → 307 seconds.
pub fn length_seconds(s: &str) -> Option<u64> {
    let parts: Vec<u64> = s.split(':').map(|p| p.trim().parse::<u64>().ok()).collect::<Option<Vec<_>>>()?;
    match parts.as_slice() {
        [h, m, sec] => Some(h.saturating_mul(3600).saturating_add(m.saturating_mul(60)).saturating_add(*sec)),
        [m, sec] => Some(m.saturating_mul(60).saturating_add(*sec)),
        [sec] => Some(*sec),
        _ => None,
    }
}

/// A file name Incompetech serves: an `.mp3` name with no path parts.
fn safe_filename(f: &str) -> bool {
    f.ends_with(".mp3") && f.len() < 200 && !f.contains(['/', '\\']) && !f.contains("..") && !f.starts_with('.') && !f.chars().any(char::is_control)
}

/// Read a cached document, or fetch and cache it when it is missing or older than a day.
fn cached(url: &str, cache: &Path) -> Result<Value, AutomationError> {
    let fresh = std::fs::metadata(cache).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < DAY);
    if fresh && let Some(v) = std::fs::read(cache).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
        return Ok(v);
    }
    match fetch(url, MAX_CATALOG_BYTES, &[]) {
        Ok(bytes) => {
            let v: Value = serde_json::from_slice(&bytes).map_err(|e| err(format!("{url}: not JSON ({e})")))?;
            if let Some(d) = cache.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(cache, &bytes);
            Ok(v)
        }
        // offline: an old copy is better than nothing
        Err(e) => std::fs::read(cache).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).ok_or(e),
    }
}

fn genre_names(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| Some((g["id"].as_u64().map(|i| i.to_string()).or_else(|| g["id"].as_str().map(str::to_string))?, g["genre"].as_str()?.to_string())))
        .collect()
}

/// Search filters.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub min_bpm: Option<f64>,
    pub max_bpm: Option<f64>,
    pub min_seconds: Option<f64>,
    pub max_seconds: Option<f64>,
}

/// Rank the catalogue for `query` (words matched against title, feel, genre, instruments and
/// description; title and feel count most).
pub fn search_catalog(catalog: &Value, genres: &[(String, String)], query: &str, f: &Filter, limit: usize) -> Vec<Value> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).filter(|w| w.len() > 1).take(12).collect();
    let mut hits: Vec<(u32, Value)> = Vec::new();
    for p in catalog.as_array().into_iter().flatten() {
        let Some(file) = p["filename"].as_str().filter(|f| safe_filename(f)) else { continue };
        let text = |k: &str| p[k].as_str().unwrap_or("").to_lowercase();
        let (title, feel, instr, desc) = (text("title"), text("feel"), text("instruments"), text("description"));
        let gid = p["genre"].as_str().map(str::to_string).or_else(|| p["genre"].as_u64().map(|g| g.to_string())).unwrap_or_default();
        let genre = genres.iter().find(|g| g.0 == gid).map(|g| g.1.clone()).unwrap_or_default();
        let bpm = p["bpm"].as_str().and_then(|b| b.trim().parse::<f64>().ok()).or_else(|| p["bpm"].as_f64());
        let secs = p["length"].as_str().and_then(length_seconds);
        if f.min_bpm.is_some_and(|m| bpm.is_none_or(|b| b < m)) || f.max_bpm.is_some_and(|m| bpm.is_none_or(|b| b > m)) {
            continue;
        }
        if f.min_seconds.is_some_and(|m| secs.is_none_or(|s| (s as f64) < m)) || f.max_seconds.is_some_and(|m| secs.is_none_or(|s| (s as f64) > m)) {
            continue;
        }
        let g = genre.to_lowercase();
        let score: u32 = words
            .iter()
            .map(|w| {
                let mut sc = 0;
                if title.contains(w.as_str()) {
                    sc += 4;
                }
                if feel.contains(w.as_str()) {
                    sc += 3;
                }
                if g.contains(w.as_str()) {
                    sc += 2;
                }
                if instr.contains(w.as_str()) || desc.contains(w.as_str()) {
                    sc += 1;
                }
                sc
            })
            .sum();
        if !words.is_empty() && score == 0 {
            continue;
        }
        let title_raw = p["title"].as_str().unwrap_or("");
        hits.push((
            score,
            json!({
                "id": format!("km:{}", slug(file)),
                "source": "incompetech",
                "kind": "music",
                "title": title_raw,
                "creator": "Kevin MacLeod",
                "license": LICENSE,
                "needsCredit": true,
                "attribution": attribution(title_raw),
                "durationSeconds": secs,
                "bpm": bpm,
                "feel": p["feel"],
                "genre": genre,
                "instruments": p["instruments"],
                "description": p["description"].as_str().map(|d| d.chars().take(220).collect::<String>()),
            }),
        ));
    }
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    hits.into_iter().take(limit.clamp(1, 50)).map(|h| h.1).collect()
}

/// The credit line Kevin MacLeod asks for.
pub fn attribution(title: &str) -> String {
    format!("\"{title}\" Kevin MacLeod (incompetech.com) — Licensed under Creative Commons: By Attribution 4.0 License, {LICENSE_URL}")
}

/// Search the catalogue (cached in `cache_dir`).
pub fn search(query: &str, f: &Filter, limit: usize, cache_dir: &Path) -> Result<Vec<Value>, AutomationError> {
    if query.len() > 200 {
        return Err(err("`query` must be at most 200 characters"));
    }
    let catalog = cached(CATALOG_URL, &cache_dir.join("incompetech-pieces.json"))?;
    let genres = cached(GENRES_URL, &cache_dir.join("incompetech-genres.json")).map(|g| genre_names(&g)).unwrap_or_default();
    Ok(search_catalog(&catalog, &genres, query, f, limit))
}

/// Download catalogue track `slug` (the part after `km:`) into `dir`.
pub fn download(slug_id: &str, dir: &Path, cache_dir: &Path) -> Result<Download, AutomationError> {
    let catalog = cached(CATALOG_URL, &cache_dir.join("incompetech-pieces.json"))?;
    let piece = catalog
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["filename"].as_str().is_some_and(|f| safe_filename(f) && slug(f) == slug_id))
        .ok_or_else(|| err(format!("no Incompetech track `km:{slug_id}` (search again with library_search)")))?;
    let file = piece["filename"].as_str().unwrap_or_default();
    let title = piece["title"].as_str().unwrap_or("untitled").to_string();
    let att = attribution(&title);
    let st = stem(&title, "km");
    let path = dir.join(format!("{}.mp3", super::safe_name(&st)));
    let side = sidecar(&path, &title, "Kevin MacLeod", "https://incompetech.com", LICENSE, LICENSE_URL, &att);
    let path = super::save(&format!("{MP3_BASE}{}", encode_path(file)), MAX_MP3_BYTES, &[], dir, &st, "mp3", &side)?;
    Ok(Download { path, title, attribution: att, license: LICENSE.into(), needs_credit: true, kind: "audio".into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_lengths_and_names() {
        assert_eq!(slug("The Britons.mp3"), "the-britons");
        assert_eq!(slug("Monkeys Spinning Monkeys.mp3"), "monkeys-spinning-monkeys");
        assert_eq!(slug("../../x.mp3"), "x");
        assert_eq!(length_seconds("00:05:07"), Some(307));
        assert_eq!(length_seconds("2:03"), Some(123));
        assert_eq!(length_seconds("a:b"), None);
        assert_eq!(length_seconds("99999999999999999999:00:00"), None);
        assert!(safe_filename("Sneaky Snitch.mp3"));
        assert!(!safe_filename("../etc/passwd.mp3"));
        assert!(!safe_filename("x.exe"));
        assert!(!safe_filename("a\nb.mp3"));
    }

    #[test]
    fn catalogue_search_ranks_and_filters() {
        let cat = json!([
            {"title": "Sneaky Snitch", "filename": "Sneaky Snitch.mp3", "length": "00:02:16", "instruments": "Piano, Pizzicato", "genre": "24", "bpm": "116", "feel": "Mysterious, Funny", "description": "Funny sneaking."},
            {"title": "Volatile Reaction", "filename": "Volatile Reaction.mp3", "length": "00:03:34", "instruments": "Synths, Drums", "genre": "7", "bpm": "170", "feel": "Intense, Driving", "description": "Electronic action."},
            {"title": "Bad", "filename": "../evil.mp3", "length": "00:01:00", "genre": "7", "bpm": "120", "feel": "Funny"},
            {"title": "No bpm", "filename": "No bpm.mp3", "length": "00:01:00", "genre": 7, "feel": "funny"},
            "garbage",
        ]);
        let g = genre_names(&json!([{"id": 7, "genre": "Electronica"}, {"id": 24, "genre": "Unclassifiable"}]));
        let r = search_catalog(&cat, &g, "funny", &Filter::default(), 10);
        assert_eq!(r.len(), 2, "{r:?}");
        assert_eq!(r[0]["id"], "km:sneaky-snitch");
        assert_eq!(r[0]["needsCredit"], true);
        assert!(r[0]["attribution"].as_str().unwrap().contains("Kevin MacLeod"));
        let fast = search_catalog(&cat, &g, "electronica", &Filter { min_bpm: Some(140.0), ..Filter::default() }, 10);
        assert_eq!(fast.len(), 1);
        assert_eq!(fast[0]["title"], "Volatile Reaction");
        assert_eq!(fast[0]["durationSeconds"], 214);
        let short = search_catalog(&cat, &g, "", &Filter { max_seconds: Some(150.0), ..Filter::default() }, 10);
        assert_eq!(short.len(), 2);
        assert!(search_catalog(&json!({"x": 1}), &g, "a", &Filter::default(), 5).is_empty());
    }

    /// Live: `cargo test -p filmcraft-automation incompetech -- --ignored`.
    #[test]
    #[ignore = "needs the network"]
    fn live_incompetech() {
        let dir = std::env::temp_dir().join(format!("filmcraft-km-{}", std::process::id()));
        let r = search("upbeat funny", &Filter { max_seconds: Some(200.0), ..Filter::default() }, 5, &dir).unwrap();
        assert!(!r.is_empty());
        let id = r[0]["id"].as_str().unwrap().trim_start_matches("km:").to_string();
        let d = download(&id, &dir.join("audio"), &dir).unwrap();
        assert!(std::fs::metadata(&d.path).unwrap().len() > 100_000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
