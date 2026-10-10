//! Online libraries for agents: openly licensed music and sound effects (Openverse: Freesound,
//! Jamendo, Wikimedia Commons, ccMixter…) and Google Fonts.
//!
//! Only licences that allow use in any video, including monetised ones, are searched or accepted:
//! CC0, public domain (PDM), CC BY and CC BY-SA. Every download is saved with an `.attribution`
//! sidecar holding the credit line, which the tool result also returns so it can go in the video
//! description. Responses are treated as hostile: HTTPS only, sizes capped, file names built from
//! sanitised ids (never from the server's paths), and font files only from the Google Fonts
//! repository on GitHub.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::AutomationError;

/// Licences usable in any video (monetised included).
pub const LICENSES: &str = "cc0,pdm,by,by-sa";
const MAX_AUDIO_BYTES: u64 = 80 << 20;
const MAX_FONT_BYTES: u64 = 25 << 20;
const MAX_JSON_BYTES: u64 = 8 << 20;
const USER_AGENT: &str = concat!("FilmCraft/", env!("CARGO_PKG_VERSION"), " (+https://github.com/storytold/filmcraft)");

fn err(e: impl std::fmt::Display) -> AutomationError {
    AutomationError::Other(e.to_string())
}

fn agent() -> ureq::Agent {
    let provider = std::sync::Arc::new(rustls_rustcrypto::provider());
    ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::Rustls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .unversioned_rustls_crypto_provider(provider)
                .build(),
        )
        .timeout_connect(Some(std::time::Duration::from_secs(20)))
        .timeout_global(Some(std::time::Duration::from_secs(180)))
        .https_only(true)
        .build()
        .into()
}

fn get_bytes(url: &str, cap: u64) -> Result<Vec<u8>, AutomationError> {
    if !url.starts_with("https://") {
        return Err(err(format!("refusing a non-HTTPS URL: {url}")));
    }
    let resp = agent().get(url).header("User-Agent", USER_AGENT).call().map_err(|e| err(format!("{url}: {e}")))?;
    let mut body = resp.into_body();
    let mut out = Vec::new();
    body.as_reader().take(cap + 1).read_to_end(&mut out).map_err(|e| err(format!("{url}: {e}")))?;
    if out.len() as u64 > cap {
        return Err(err(format!("{url}: larger than {} MB", cap >> 20)));
    }
    Ok(out)
}

fn get_json(url: &str) -> Result<Value, AutomationError> {
    serde_json::from_slice(&get_bytes(url, MAX_JSON_BYTES)?).map_err(|e| err(format!("{url}: not JSON ({e})")))
}

/// Percent-encode a query value.
pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Letters, digits, `-` and `_` only (at most 80), for file names built from remote ids.
pub fn safe_name(s: &str) -> String {
    let v: String = s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(80).collect();
    if v.is_empty() { "file".into() } else { v }
}

fn license_ok(l: &str) -> bool {
    matches!(l.to_ascii_lowercase().as_str(), "cc0" | "pdm" | "by" | "by-sa")
}

// ------------------------------------------------------------------------------- Openverse

/// The Openverse search URL for `query`. `kind`: `music` (tracks longer than about 30 s), `sfx`
/// (short sounds) or anything else for both. Openverse's `category` filter only matches sources
/// whose music is all non-commercial, so length is the filter that works.
pub fn openverse_url(query: &str, kind: &str, limit: u32) -> String {
    let cat = match kind {
        "music" => "&length=medium,long",
        "sfx" | "sound" | "sound_effect" => "&length=short",
        _ => "",
    };
    format!("https://api.openverse.org/v1/audio/?q={}&license={LICENSES}{cat}&page_size={}&mature=false", encode(query), limit.clamp(1, 50))
}

/// The results of an Openverse response, keeping only usable licences and HTTPS files.
pub fn parse_openverse(v: &Value) -> Vec<Value> {
    v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["license"].as_str().is_some_and(license_ok) && r["url"].as_str().is_some_and(|u| u.starts_with("https://")))
        .map(|r| {
            let tags: Vec<&str> = r["tags"].as_array().into_iter().flatten().filter_map(|t| t["name"].as_str()).take(12).collect();
            json!({
                "id": r["id"],
                "title": r["title"],
                "creator": r["creator"],
                "license": format!("CC {} {}", r["license"].as_str().unwrap_or("").to_ascii_uppercase(), r["license_version"].as_str().unwrap_or("")).trim().replace("CC CC0", "CC0").replace("CC PDM", "Public Domain"),
                "durationSeconds": r["duration"].as_f64().map(|ms| (ms / 100.0).round() / 10.0),
                "source": r["source"],
                "category": r["category"],
                "genres": r["genres"],
                "tags": tags,
                "page": r["foreign_landing_url"],
                "attribution": r["attribution"],
                "needsCredit": !matches!(r["license"].as_str(), Some("cc0" | "pdm")),
            })
        })
        .collect()
}

pub fn openverse_search(query: &str, kind: &str, limit: u32) -> Result<Vec<Value>, AutomationError> {
    if query.trim().is_empty() || query.len() > 200 {
        return Err(err("`query` must be 1–200 characters"));
    }
    let r = parse_openverse(&get_json(&openverse_url(query, kind, limit))?);
    if r.is_empty() && kind != "any" {
        // the length filter can be too strict for rare queries
        return Ok(parse_openverse(&get_json(&openverse_url(query, "any", limit))?));
    }
    Ok(r)
}

/// A downloaded track: where it is and the credit line.
pub struct Download {
    pub path: PathBuf,
    pub title: String,
    pub attribution: String,
    pub license: String,
    pub needs_credit: bool,
}

/// Download Openverse audio `id` into `dir` (reused when already there) with its sidecar.
pub fn openverse_download(id: &str, dir: &Path) -> Result<Download, AutomationError> {
    let id = safe_name(id);
    let r = get_json(&format!("https://api.openverse.org/v1/audio/{id}/"))?;
    let lic = r["license"].as_str().unwrap_or("");
    if !license_ok(lic) {
        return Err(err(format!("licence `{lic}` does not allow use in every video; pick a CC0 / CC BY / CC BY-SA result")));
    }
    let url = r["url"].as_str().filter(|u| u.starts_with("https://")).ok_or_else(|| err("no HTTPS file for this result"))?;
    let ext = r["filetype"]
        .as_str()
        .map(str::to_ascii_lowercase)
        .filter(|e| matches!(e.as_str(), "mp3" | "wav" | "flac" | "ogg" | "opus" | "m4a" | "aif" | "aiff"))
        .unwrap_or_else(|| "mp3".into());
    let title = r["title"].as_str().unwrap_or("untitled").to_string();
    let attribution =
        r["attribution"].as_str().map(str::to_string).unwrap_or_else(|| format!("\"{title}\" by {} ({lic})", r["creator"].as_str().unwrap_or("unknown")));
    std::fs::create_dir_all(dir).map_err(|e| err(format!("{}: {e}", dir.display())))?;
    let stem = format!("{}-{}", safe_name(&title.to_ascii_lowercase().replace(' ', "-")), &id[..id.len().min(8)]);
    let path = dir.join(format!("{stem}.{ext}"));
    if !path.exists() {
        let bytes = get_bytes(url, MAX_AUDIO_BYTES)?;
        let part = dir.join(format!("{stem}.part"));
        std::fs::write(&part, &bytes).map_err(|e| err(format!("{}: {e}", part.display())))?;
        std::fs::rename(&part, &path).map_err(|e| err(e.to_string()))?;
    }
    let side = format!(
        "asset:        {}\ntitle:        {title}\nauthor:       {}\nsource:       {}\nlicense:      {lic} {}\nlicense-url:  {}\nattribution:  {attribution}\n",
        path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        r["creator"].as_str().unwrap_or("unknown"),
        r["foreign_landing_url"].as_str().unwrap_or(""),
        r["license_version"].as_str().unwrap_or(""),
        r["license_url"].as_str().unwrap_or(""),
    );
    let _ = std::fs::write(PathBuf::from(format!("{}.attribution", path.display())), side);
    Ok(Download { path, title, attribution, license: lic.to_string(), needs_credit: !matches!(lic, "cc0" | "pdm") })
}

// ------------------------------------------------------------------------------- fonts

/// Popular open-licence fonts for video titles and captions (all on Google Fonts).
pub const FONTS: &[(&str, &str, &str)] = &[
    ("Montserrat", "sans", "the TikTok / YouTube caption standard; bold and black weights read well"),
    ("Poppins", "sans", "friendly, rounded; captions and vlog titles"),
    ("Inter", "sans", "neutral UI sans; clean captions"),
    ("Bebas Neue", "display", "tall condensed capitals; hooks, thumbnails, cinematic titles"),
    ("Anton", "display", "heavy condensed; meme captions, hype titles"),
    ("Oswald", "display", "condensed; lower thirds, sports"),
    ("Bangers", "comic", "comic-book capitals; funny gaming moments"),
    ("Luckiest Guy", "comic", "chunky cartoon capitals; kids / gaming titles"),
    ("Lilita One", "display", "rounded heavy; gaming captions"),
    ("Titan One", "display", "bubbly heavy; playful titles"),
    ("Bungee", "display", "urban block capitals; gaming, street style"),
    ("Archivo Black", "sans", "very bold grotesk; punchy captions"),
    ("Rubik", "sans", "soft geometric; modern captions"),
    ("Kanit", "sans", "sporty; esports overlays"),
    ("Press Start 2P", "pixel", "8-bit arcade pixel font; retro and block-game edits"),
    ("VT323", "pixel", "terminal pixel font; retro, hacker"),
    ("Silkscreen", "pixel", "small pixel capitals; HUD-style labels"),
    ("Pixelify Sans", "pixel", "modern pixel font; Minecraft-style edits"),
    ("Russo One", "display", "techno; esports, cars"),
    ("Black Ops One", "display", "military stencil; FPS / shooter edits"),
    ("Orbitron", "display", "sci-fi geometric; futuristic titles"),
    ("Audiowide", "display", "racing / synthwave"),
    ("Teko", "display", "condensed tech; scoreboards"),
    ("Permanent Marker", "handwritten", "marker handwriting; vlog notes, arrows"),
    ("Caveat", "handwritten", "casual handwriting; vlog annotations"),
    ("Righteous", "display", "retro rounded; chill vlogs"),
    ("Monoton", "display", "neon outline; night / club"),
    ("Creepster", "display", "horror drip; spooky edits"),
];

/// Curated fonts matching `query` (name, kind or use; empty = all).
pub fn font_search(query: &str) -> Vec<Value> {
    let q = query.trim().to_ascii_lowercase();
    FONTS
        .iter()
        .filter(|(n, k, u)| q.is_empty() || n.to_ascii_lowercase().contains(&q) || k.contains(&q) || u.contains(&q))
        .map(|(n, k, u)| json!({"family": n, "kind": k, "use": u, "licence": "SIL OFL / Apache 2.0 (free for videos)"}))
        .collect()
}

/// The folder a family is saved in (`"Press Start 2P"` → `pressstart2p`).
pub fn font_dir(family: &str) -> String {
    family.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase()
}

/// The download manifest of a family from fonts.google.com (the JSON after the `)]}'` guard).
pub fn parse_font_manifest(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    serde_json::from_str::<Value>(text.get(start..)?).ok().map(|v| v["manifest"].clone()).filter(|m| m.is_object())
}

/// Font files to fetch from a manifest: safe `.ttf` / `.otf` names served from fonts.gstatic.com.
/// The family's static styles (Regular, Medium, SemiBold, Bold, ExtraBold, Black and Italic /
/// Bold Italic) when it has them, else its variable files.
pub fn pick_font_files(manifest: &Value) -> Vec<(String, String)> {
    let all: Vec<(String, String, bool)> = manifest["fileRefs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let path = f["filename"].as_str()?;
            let url = f["url"].as_str()?;
            let name = path.rsplit('/').next()?;
            let lower = name.to_ascii_lowercase();
            let ok = !name.is_empty() && !name.starts_with('.') && !name.contains('\\') && (lower.ends_with(".ttf") || lower.ends_with(".otf"));
            (ok && url.starts_with("https://fonts.gstatic.com/")).then(|| (name.to_string(), url.to_string(), path.starts_with("static/")))
        })
        .collect();
    let wanted = ["regular", "medium", "semibold", "bold", "extrabold", "black", "italic", "bolditalic"];
    let statics: Vec<(String, String)> = all
        .iter()
        .filter(|(n, _, st)| {
            *st && n.rsplit_once('-').is_some_and(|(_, style)| wanted.iter().any(|w| style.to_ascii_lowercase().starts_with(&format!("{w}."))))
        })
        .map(|(n, u, _)| (n.clone(), u.clone()))
        .collect();
    if !statics.is_empty() {
        return statics;
    }
    all.into_iter().filter(|(_, _, st)| !st).map(|(n, u, _)| (n, u)).take(4).collect()
}

/// Download a Google Fonts family into `dir/<family>/` with its licence text.
pub fn google_font(family: &str, dir: &Path) -> Result<Vec<PathBuf>, AutomationError> {
    let d = font_dir(family);
    if d.is_empty() || family.len() > 60 {
        return Err(err("`family` must be a Google Fonts family name, e.g. \"Bebas Neue\""));
    }
    let text = String::from_utf8_lossy(&get_bytes(
        &format!("https://fonts.google.com/download/list?family={}", encode(family.trim()).replace('+', "%20")),
        MAX_JSON_BYTES,
    )?)
    .into_owned();
    let manifest = parse_font_manifest(&text).ok_or_else(|| err(format!("no Google Fonts family \"{family}\" (check the spelling on fonts.google.com)")))?;
    let files = pick_font_files(&manifest);
    if files.is_empty() {
        return Err(err(format!("\"{family}\" has no .ttf/.otf files to download")));
    }
    let out_dir = dir.join(&d);
    std::fs::create_dir_all(&out_dir).map_err(|e| err(format!("{}: {e}", out_dir.display())))?;
    let mut paths = Vec::new();
    for (name, url) in files.iter().take(10) {
        let p = out_dir.join(name);
        if !p.exists() {
            let bytes = get_bytes(url, MAX_FONT_BYTES)?;
            std::fs::write(&p, bytes).map_err(|e| err(format!("{}: {e}", p.display())))?;
        }
        paths.push(p);
    }
    for f in manifest["files"].as_array().into_iter().flatten() {
        if let (Some(n), Some(c)) = (f["filename"].as_str(), f["contents"].as_str())
            && matches!(n, "OFL.txt" | "LICENSE.txt" | "UFL.txt")
        {
            let _ = std::fs::write(out_dir.join(n), c);
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_encoded_and_licences_restricted() {
        let u = openverse_url("épic boss & fight", "music", 500);
        assert!(u.starts_with("https://api.openverse.org/v1/audio/?q=%C3%A9pic+boss+%26+fight"), "{u}");
        assert!(u.contains("license=cc0,pdm,by,by-sa") && u.contains("length=medium,long") && u.contains("page_size=50"));
        assert!(openverse_url("x", "sfx", 3).contains("length=short"));
        assert!(!openverse_url("x", "any", 0).contains("length"));
        assert_eq!(safe_name("../../etc/passwd"), "etcpasswd");
        assert_eq!(safe_name("///"), "file");
        assert_eq!(font_dir("Press Start 2P"), "pressstart2p");
        assert_eq!(font_dir("../x y"), "xy");
    }

    #[test]
    fn openverse_results_keep_usable_licences_only() {
        let v = json!({"results": [
            {"id": "a", "title": "Epic", "url": "https://cdn.example/a.mp3", "license": "by", "license_version": "4.0", "duration": 61234, "creator": "Ann", "attribution": "\"Epic\" by Ann is licensed under CC BY 4.0.", "tags": [{"name": "epic"}]},
            {"id": "b", "title": "NC", "url": "https://cdn.example/b.mp3", "license": "by-nc", "duration": 1000},
            {"id": "c", "title": "Plain http", "url": "http://cdn.example/c.mp3", "license": "cc0"},
            {"id": "d", "title": "Zero", "url": "https://cdn.example/d.wav", "license": "cc0"},
            {"id": 5},
        ]});
        let r = parse_openverse(&v);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["license"], "CC BY 4.0");
        assert_eq!(r[0]["durationSeconds"], 61.2);
        assert_eq!(r[0]["needsCredit"], true);
        assert_eq!(r[1]["license"], "CC0");
        assert_eq!(r[1]["needsCredit"], false);
        assert!(parse_openverse(&json!("garbage")).is_empty());
        assert!(openverse_search("", "music", 5).is_err());
    }

    #[test]
    fn font_manifests_are_filtered_to_safe_files() {
        let text = r#")]}'
{"zipName": "X.zip", "manifest": {"files": [{"filename": "OFL.txt", "contents": "licence"}], "fileRefs": [
  {"filename": "static/Mont-Thin.ttf", "url": "https://fonts.gstatic.com/s/a"},
  {"filename": "static/Mont-Bold.ttf", "url": "https://fonts.gstatic.com/s/b"},
  {"filename": "static/Mont-BoldItalic.ttf", "url": "https://fonts.gstatic.com/s/c"},
  {"filename": "static/Evil-Regular.ttf", "url": "https://evil.example/x"},
  {"filename": "Mont-VariableFont_wght.ttf", "url": "https://fonts.gstatic.com/s/v"}
]}}"#;
        let m = parse_font_manifest(text).unwrap();
        let f = pick_font_files(&m);
        assert_eq!(f.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["Mont-Bold.ttf", "Mont-BoldItalic.ttf"]);
        // without static styles: the variable file
        let v = json!({"fileRefs": [{"filename": "Bangers-Regular.ttf", "url": "https://fonts.gstatic.com/s/b"}, {"filename": "../../x.ttf", "url": "http://fonts.gstatic.com/x"}]});
        assert_eq!(pick_font_files(&v), vec![("Bangers-Regular.ttf".to_string(), "https://fonts.gstatic.com/s/b".to_string())]);
        assert!(parse_font_manifest("not json").is_none());
        assert!(parse_font_manifest(")]}' {\"manifest\": 3}").is_none());
        assert!(!font_search("pixel").is_empty());
        assert_eq!(font_search("").len(), FONTS.len());
    }

    /// Live network check: `cargo test -p filmcraft-automation online -- --ignored`.
    #[test]
    #[ignore = "needs the network"]
    fn live_openverse_and_google_fonts() {
        let dir = std::env::temp_dir().join(format!("filmcraft-online-{}", std::process::id()));
        let r = openverse_search("epic orchestral", "music", 5).unwrap();
        assert!(!r.is_empty());
        let sfx = openverse_search("whoosh", "sfx", 5).unwrap();
        assert!(!sfx.is_empty());
        let id = sfx[0]["id"].as_str().unwrap();
        let d = openverse_download(id, &dir.join("audio")).unwrap();
        assert!(std::fs::metadata(&d.path).unwrap().len() > 1000);
        assert!(!d.attribution.is_empty());
        let fonts = google_font("Bebas Neue", &dir.join("fonts")).unwrap();
        assert!(!fonts.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
