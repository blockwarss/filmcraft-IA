//! Pictures, footage and keyed sound libraries.
//!
//! No key needed: Openverse images (CC0 / CC BY / CC BY-SA photos and illustrations), NASA's
//! image and video library (US public domain) and Wikimedia Commons video (free licences only).
//! With the user's free key ([`super::keys`]): Pexels and Pixabay (stock videos and photos, free
//! for commercial use without credit) and Freesound (CC0 and CC BY sounds only).
//!
//! Ids carry their source: `ovi:` Openverse image, `nasa:`, `wm:` Wikimedia Commons, `px:p` /
//! `px:v` Pexels photo / video, `pb:i` / `pb:v` Pixabay image / video, `fs:` Freesound. A download
//! asks the source for the item again, so file URLs always come from the source, never from the
//! caller.

use std::path::Path;

use serde_json::{Value, json};

use super::{Download, LICENSES, MAX_IMAGE_BYTES, MAX_VIDEO_BYTES, check_query, encode, encode_path, err, fetch_json, keys, safe_name, save, sidecar, stem};
use crate::AutomationError;

/// Search options shared by every source.
#[derive(Clone, Debug)]
pub struct Opts {
    /// `video`, `image` (or `photo` / `illustration`) or `any`.
    pub kind: String,
    /// Prefer tall (9:16) results: TikTok, Reels, Shorts.
    pub vertical: bool,
    pub limit: u32,
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn https(u: &str) -> Option<String> {
    let u = u.trim();
    if let Some(rest) = u.strip_prefix("http://") {
        return Some(format!("https://{rest}"));
    }
    u.starts_with("https://").then(|| u.to_string())
}

fn ext_of(url: &str, allowed: &[&str], fallback: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    path.rsplit('.').next().map(str::to_ascii_lowercase).filter(|e| allowed.contains(&e.as_str())).unwrap_or_else(|| fallback.to_string())
}

fn strip_html(h: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in h.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect()
}

fn item(id: String, source: &str, kind: &str, title: &str, creator: &str, license: &str, credit: bool, attribution: String) -> Value {
    json!({"id": id, "source": source, "kind": kind, "title": title, "creator": creator, "license": license, "needsCredit": credit, "attribution": attribution})
}

fn dims(v: &mut Value, w: Option<u64>, h: Option<u64>) {
    if let (Some(w), Some(h)) = (w, h) {
        v["width"] = json!(w);
        v["height"] = json!(h);
        v["vertical"] = json!(h > w);
    }
}

// ------------------------------------------------------------------------------- Openverse images

pub fn openverse_images_url(query: &str, o: &Opts) -> String {
    let mut u = format!("https://api.openverse.org/v1/images/?q={}&license={LICENSES}&page_size={}&mature=false", encode(query), o.limit.clamp(1, 50));
    match o.kind.as_str() {
        "photo" => u.push_str("&category=photograph"),
        "illustration" => u.push_str("&category=illustration,digitized_artwork"),
        "sticker" | "png" => u.push_str("&extension=png"),
        _ => {}
    }
    if o.vertical {
        u.push_str("&aspect_ratio=tall");
    }
    u
}

fn ov_license(r: &Value) -> Option<(String, bool)> {
    let l = r["license"].as_str()?.to_ascii_lowercase();
    if !matches!(l.as_str(), "cc0" | "pdm" | "by" | "by-sa") {
        return None;
    }
    let name = match l.as_str() {
        "cc0" => "CC0".to_string(),
        "pdm" => "Public Domain".to_string(),
        _ => format!("CC {} {}", l.to_ascii_uppercase(), s(&r["license_version"])).trim().to_string(),
    };
    Some((name, !matches!(l.as_str(), "cc0" | "pdm")))
}

pub fn parse_openverse_images(v: &Value) -> Vec<Value> {
    v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["url"].as_str().is_some_and(|u| u.starts_with("https://")))
        .filter_map(|r| {
            let (lic, credit) = ov_license(r)?;
            let title = s(&r["title"]);
            let att = r["attribution"].as_str().map(str::to_string).unwrap_or_else(|| format!("\"{title}\" by {} ({lic})", s(&r["creator"])));
            let mut it = item(format!("ovi:{}", safe_name(r["id"].as_str()?)), "openverse", "image", &title, &s(&r["creator"]), &lic, credit, att);
            dims(&mut it, r["width"].as_u64(), r["height"].as_u64());
            it["preview"] = r["thumbnail"].clone();
            it["page"] = r["foreign_landing_url"].clone();
            it["provider"] = r["source"].clone();
            Some(it)
        })
        .collect()
}

fn openverse_image_download(id: &str, dir: &Path) -> Result<Download, AutomationError> {
    let r = fetch_json(&format!("https://api.openverse.org/v1/images/{}/", safe_name(id)), &[])?;
    let (lic, credit) = ov_license(&r).ok_or_else(|| err("this picture's licence does not allow use in every video"))?;
    let url = r["url"].as_str().and_then(https).ok_or_else(|| err("no HTTPS file for this picture"))?;
    let ext = r["filetype"]
        .as_str()
        .map(str::to_ascii_lowercase)
        .filter(|e| matches!(e.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif"))
        .unwrap_or_else(|| ext_of(&url, &["jpg", "jpeg", "png", "webp", "gif"], "jpg"));
    let title = r["title"].as_str().unwrap_or("picture").to_string();
    let att = r["attribution"].as_str().map(str::to_string).unwrap_or_else(|| format!("\"{title}\" by {} ({lic})", s(&r["creator"])));
    let st = stem(&title, id);
    let side = sidecar(&dir.join(format!("{st}.{ext}")), &title, &s(&r["creator"]), &s(&r["foreign_landing_url"]), &lic, &s(&r["license_url"]), &att);
    let path = save(&url, MAX_IMAGE_BYTES, &[], dir, &st, &ext, &side)?;
    Ok(Download { path, title, attribution: att, license: lic, needs_credit: credit, kind: "image".into() })
}

// ------------------------------------------------------------------------------- NASA

pub fn nasa_url(query: &str, o: &Opts) -> String {
    let media = if o.kind == "video" {
        "video"
    } else if o.kind == "any" {
        "image,video"
    } else {
        "image"
    };
    format!("https://images-api.nasa.gov/search?q={}&media_type={media}&page_size={}", encode(query), o.limit.clamp(1, 50))
}

fn nasa_credit(d: &Value) -> (String, bool) {
    let who =
        d["photographer"].as_str().or_else(|| d["secondary_creator"].as_str()).map(str::trim).filter(|w| !w.is_empty() && !w.eq_ignore_ascii_case("NASA"));
    match who {
        // footage shot by a partner can carry its own rights: credit it
        Some(w) => (format!("Credit: NASA / {w}"), true),
        None => ("Credit: NASA".to_string(), false),
    }
}

pub fn parse_nasa(v: &Value) -> Vec<Value> {
    v["collection"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let d = r["data"].get(0)?;
            let nid = d["nasa_id"].as_str()?;
            if nid.len() > 200 || nid.contains(['/', '\\']) || nid.contains("..") {
                return None;
            }
            let kind = if d["media_type"] == "video" {
                "video"
            } else if d["media_type"] == "image" {
                "image"
            } else {
                return None;
            };
            let (att, credit) = nasa_credit(d);
            let mut it = item(
                format!("nasa:{nid}"),
                "nasa",
                kind,
                &s(&d["title"]),
                d["photographer"].as_str().unwrap_or("NASA"),
                "NASA media (US public domain)",
                credit,
                att,
            );
            it["preview"] = r["links"].get(0).map(|l| l["href"].clone()).unwrap_or(Value::Null);
            it["description"] = json!(strip_html(d["description"].as_str().unwrap_or("")));
            Some(it)
        })
        .collect()
}

/// The file to fetch from a NASA asset list: for video the 1080p-ish `~large.mp4` first, for
/// pictures `~large.jpg` before the (sometimes huge) original.
pub fn pick_nasa(asset: &Value, video: bool) -> Option<String> {
    let hrefs: Vec<String> = asset["collection"]["items"].as_array()?.iter().filter_map(|i| i["href"].as_str().and_then(https)).collect();
    let order: &[&str] =
        if video { &["~large.mp4", "~medium.mp4", "~orig.mp4", "~mobile.mp4"] } else { &["~large.jpg", "~orig.jpg", "~orig.png", "~medium.jpg"] };
    order.iter().find_map(|suf| hrefs.iter().find(|h| h.ends_with(suf) && h.starts_with("https://images-assets.nasa.gov/")).cloned())
}

fn nasa_download(nid: &str, dir: &Path) -> Result<Download, AutomationError> {
    if nid.is_empty() || nid.len() > 200 || nid.contains(['/', '\\']) || nid.contains("..") {
        return Err(err("bad NASA id"));
    }
    let meta = fetch_json(&format!("https://images-api.nasa.gov/search?nasa_id={}", encode(nid)), &[])?;
    let d = meta["collection"]["items"].get(0).and_then(|i| i["data"].get(0)).cloned().unwrap_or(Value::Null);
    let video = d["media_type"] == "video";
    let asset = fetch_json(&format!("https://images-api.nasa.gov/asset/{}", encode_path(nid)), &[])?;
    let url = pick_nasa(&asset, video).ok_or_else(|| err("no downloadable file for this NASA item"))?;
    let title = d["title"].as_str().unwrap_or(nid).to_string();
    let (att, credit) = nasa_credit(&d);
    let ext = if video { "mp4".to_string() } else { ext_of(&url, &["jpg", "png"], "jpg") };
    let st = stem(&title, nid);
    let side = sidecar(
        &dir.join(format!("{st}.{ext}")),
        &title,
        &att,
        &format!("https://images.nasa.gov/details/{}", encode_path(nid)),
        "NASA media (US public domain)",
        "https://www.nasa.gov/nasa-brand-center/images-and-media/",
        &att,
    );
    let path = save(&url, if video { MAX_VIDEO_BYTES } else { MAX_IMAGE_BYTES }, &[], dir, &st, &ext, &side)?;
    Ok(Download {
        path,
        title,
        attribution: att,
        license: "NASA media (US public domain; NASA logos may not be used to imply endorsement)".into(),
        needs_credit: credit,
        kind: if video { "video" } else { "image" }.into(),
    })
}

// ------------------------------------------------------------------------------- Wikimedia Commons

const WM_PROPS: &str = "prop=videoinfo&viprop=url|size|mime|extmetadata|derivatives&viextmetadatafilter=LicenseShortName|LicenseUrl|Artist|ObjectName";

pub fn wikimedia_url(query: &str, o: &Opts) -> String {
    format!(
        "https://commons.wikimedia.org/w/api.php?action=query&format=json&formatversion=2&generator=search&gsrsearch={}+filetype:video&gsrnamespace=6&gsrlimit={}&{WM_PROPS}",
        encode(query),
        o.limit.clamp(1, 50)
    )
}

/// A Commons licence usable in any video: CC0, public domain, CC BY or CC BY-SA (no NC / ND).
pub fn wm_license_ok(l: &str) -> bool {
    let l = l.to_ascii_lowercase();
    if ["-nc", " nc", "-nd", " nd"].iter().any(|x| l.contains(x)) {
        return false;
    }
    l.starts_with("cc0") || l.contains("public domain") || l.starts_with("pd") || l.starts_with("cc by") || l.starts_with("cc-by")
}

/// The best video file of a Commons page: a VP9 WebM derivative no taller than 1080 lines (the
/// tallest such), else the original when it is WebM or MP4.
pub fn pick_wikimedia(info: &Value) -> Option<(String, String)> {
    let mut best: Option<(u64, String)> = None;
    for d in info["derivatives"].as_array().into_iter().flatten() {
        let key = d["transcodekey"].as_str().unwrap_or("");
        let h = d["height"].as_u64().unwrap_or(0);
        let w = d["width"].as_u64().unwrap_or(0);
        let short = h.min(w);
        let Some(src) = d["src"].as_str().and_then(https) else { continue };
        if key.ends_with("vp9.webm") && short <= 1080 && src.starts_with("https://upload.wikimedia.org/") && best.as_ref().is_none_or(|b| short > b.0) {
            best = Some((short, src));
        }
    }
    if let Some((_, u)) = best {
        return Some((u, "webm".into()));
    }
    let url = info["url"].as_str().and_then(https).filter(|u| u.starts_with("https://upload.wikimedia.org/"))?;
    match info["mime"].as_str()? {
        "video/webm" => Some((url, "webm".into())),
        "video/mp4" => Some((url, "mp4".into())),
        _ => None,
    }
}

fn wm_meta(info: &Value) -> (String, String, String) {
    let m = &info["extmetadata"];
    (
        strip_html(m["LicenseShortName"]["value"].as_str().unwrap_or("")),
        strip_html(m["Artist"]["value"].as_str().unwrap_or("unknown")),
        s(&m["LicenseUrl"]["value"]),
    )
}

pub fn parse_wikimedia(v: &Value) -> Vec<Value> {
    v["query"]["pages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let info = p["videoinfo"].get(0)?;
            let (lic, artist, _) = wm_meta(info);
            if !wm_license_ok(&lic) || pick_wikimedia(info).is_none() {
                return None;
            }
            let title = s(&p["title"]).trim_start_matches("File:").to_string();
            let credit = !(lic.to_ascii_lowercase().starts_with("cc0")
                || lic.to_ascii_lowercase().contains("public domain")
                || lic.to_ascii_lowercase().starts_with("pd"));
            let att = format!("\"{title}\" by {artist}, {lic}, via Wikimedia Commons");
            let mut it = item(format!("wm:{}", p["pageid"].as_u64()?), "wikimedia", "video", &title, &artist, &lic, credit, att);
            dims(&mut it, info["width"].as_u64(), info["height"].as_u64());
            it["durationSeconds"] = info["duration"].clone();
            it["page"] = info["descriptionurl"].clone();
            Some(it)
        })
        .collect()
}

fn wikimedia_download(pageid: &str, dir: &Path) -> Result<Download, AutomationError> {
    let id: u64 = pageid.parse().map_err(|_| err("bad Wikimedia id"))?;
    let v = fetch_json(&format!("https://commons.wikimedia.org/w/api.php?action=query&format=json&formatversion=2&pageids={id}&{WM_PROPS}"), &[])?;
    let p = v["query"]["pages"].get(0).ok_or_else(|| err("no such Wikimedia Commons file"))?;
    let info = p["videoinfo"].get(0).ok_or_else(|| err("not a video"))?;
    let (lic, artist, lic_url) = wm_meta(info);
    if !wm_license_ok(&lic) {
        return Err(err(format!("licence `{lic}` does not allow use in every video")));
    }
    let (url, ext) = pick_wikimedia(info).ok_or_else(|| err("no playable WebM / MP4 file for this video"))?;
    let title = s(&p["title"]).trim_start_matches("File:").to_string();
    let att = format!("\"{title}\" by {artist}, {lic}, via Wikimedia Commons");
    let credit = wm_license_ok(&lic) && !(lic.to_ascii_lowercase().starts_with("cc0") || lic.to_ascii_lowercase().contains("public domain"));
    let st = stem(&title, &id.to_string());
    let side = sidecar(&dir.join(format!("{st}.{ext}")), &title, &artist, &s(&info["descriptionurl"]), &lic, &lic_url, &att);
    let path = save(&url, MAX_VIDEO_BYTES, &[], dir, &st, &ext, &side)?;
    Ok(Download { path, title, attribution: att, license: lic, needs_credit: credit, kind: "video".into() })
}

// ------------------------------------------------------------------------------- Pexels

const PEXELS_LICENSE: &str = "Pexels License (free, no credit required)";

pub fn pexels_url(query: &str, o: &Opts) -> String {
    let n = o.limit.clamp(1, 80);
    let orient = if o.vertical { "&orientation=portrait" } else { "" };
    if o.kind == "video" {
        format!("https://api.pexels.com/videos/search?query={}&per_page={n}{orient}", encode(query))
    } else {
        format!("https://api.pexels.com/v1/search?query={}&per_page={n}{orient}", encode(query))
    }
}

pub fn parse_pexels(v: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for p in v["photos"].as_array().into_iter().flatten() {
        let Some(id) = p["id"].as_u64() else { continue };
        let who = s(&p["photographer"]);
        let mut it = item(format!("px:p{id}"), "pexels", "image", &s(&p["alt"]), &who, PEXELS_LICENSE, false, format!("Photo by {who} on Pexels"));
        dims(&mut it, p["width"].as_u64(), p["height"].as_u64());
        it["preview"] = p["src"]["medium"].clone();
        it["page"] = p["url"].clone();
        out.push(it);
    }
    for p in v["videos"].as_array().into_iter().flatten() {
        let Some(id) = p["id"].as_u64() else { continue };
        let who = s(&p["user"]["name"]);
        let title = s(&p["url"]).trim_end_matches('/').rsplit('/').next().unwrap_or("").replace('-', " ");
        let mut it = item(format!("px:v{id}"), "pexels", "video", &title, &who, PEXELS_LICENSE, false, format!("Video by {who} on Pexels"));
        dims(&mut it, p["width"].as_u64(), p["height"].as_u64());
        it["durationSeconds"] = p["duration"].clone();
        it["preview"] = p["image"].clone();
        it["page"] = p["url"].clone();
        out.push(it);
    }
    out
}

/// The Pexels rendition to download: MP4, the largest whose short side is at most 1080.
pub fn pick_pexels_video(v: &Value) -> Option<String> {
    v["video_files"]
        .as_array()?
        .iter()
        .filter(|f| f["file_type"] == "video/mp4")
        .filter_map(|f| Some((f["width"].as_u64()?.min(f["height"].as_u64()?), f["link"].as_str().and_then(https)?)))
        .filter(|(short, _)| *short <= 1080)
        .max_by_key(|(short, _)| *short)
        .map(|(_, l)| l)
}

fn pexels_download(id: &str, dir: &Path, key: &str) -> Result<Download, AutomationError> {
    let auth = [("Authorization", key)];
    let (kind, num) = id.split_at(id.char_indices().nth(1).map_or(id.len(), |(i, _)| i));
    let num: u64 = num.parse().map_err(|_| err("bad Pexels id"))?;
    if kind == "v" {
        let v = fetch_json(&format!("https://api.pexels.com/videos/videos/{num}"), &auth)?;
        let url = pick_pexels_video(&v).ok_or_else(|| err("no MP4 file for this Pexels video"))?;
        let who = s(&v["user"]["name"]);
        let title = s(&v["url"]).trim_end_matches('/').rsplit('/').next().unwrap_or("pexels video").replace('-', " ");
        let att = format!("Video by {who} on Pexels");
        let st = stem(&title, &format!("px{num}"));
        let side = sidecar(&dir.join(format!("{st}.mp4")), &title, &who, &s(&v["url"]), PEXELS_LICENSE, "https://www.pexels.com/license/", &att);
        let path = save(&url, MAX_VIDEO_BYTES, &[], dir, &st, "mp4", &side)?;
        Ok(Download { path, title, attribution: att, license: PEXELS_LICENSE.into(), needs_credit: false, kind: "video".into() })
    } else {
        let p = fetch_json(&format!("https://api.pexels.com/v1/photos/{num}"), &auth)?;
        let url = p["src"]["large2x"].as_str().or_else(|| p["src"]["original"].as_str()).and_then(https).ok_or_else(|| err("no file for this Pexels photo"))?;
        let who = s(&p["photographer"]);
        let title = p["alt"].as_str().filter(|a| !a.is_empty()).unwrap_or("pexels photo").to_string();
        let att = format!("Photo by {who} on Pexels");
        let ext = ext_of(&url, &["jpg", "jpeg", "png", "webp"], "jpg");
        let st = stem(&title, &format!("px{num}"));
        let side = sidecar(&dir.join(format!("{st}.{ext}")), &title, &who, &s(&p["url"]), PEXELS_LICENSE, "https://www.pexels.com/license/", &att);
        let path = save(&url, MAX_IMAGE_BYTES, &[], dir, &st, &ext, &side)?;
        Ok(Download { path, title, attribution: att, license: PEXELS_LICENSE.into(), needs_credit: false, kind: "image".into() })
    }
}

// ------------------------------------------------------------------------------- Pixabay

const PIXABAY_LICENSE: &str = "Pixabay Content License (free, no credit required)";

pub fn pixabay_url(query: &str, o: &Opts, key: &str) -> String {
    let n = o.limit.clamp(3, 200);
    let q: String = query.chars().take(100).collect();
    if o.kind == "video" {
        format!("https://pixabay.com/api/videos/?key={key}&q={}&safesearch=true&per_page={n}", encode(&q))
    } else {
        let t = match o.kind.as_str() {
            "photo" => "photo",
            "illustration" => "illustration",
            "vector" => "vector",
            _ => "all",
        };
        let orient = if o.vertical { "&orientation=vertical" } else { "" };
        format!("https://pixabay.com/api/?key={key}&q={}&image_type={t}&safesearch=true&per_page={n}{orient}", encode(&q))
    }
}

/// The Pixabay video rendition: the largest one with a URL.
fn pick_pixabay_video(h: &Value) -> Option<(String, u64, u64)> {
    ["large", "medium", "small", "tiny"].iter().find_map(|k| {
        let r = &h["videos"][*k];
        let u = r["url"].as_str().filter(|u| !u.is_empty()).and_then(https)?;
        Some((u, r["width"].as_u64().unwrap_or(0), r["height"].as_u64().unwrap_or(0)))
    })
}

pub fn parse_pixabay(v: &Value, video: bool, vertical: bool) -> Vec<Value> {
    v["hits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| {
            let id = h["id"].as_u64()?;
            let who = s(&h["user"]);
            let tags = s(&h["tags"]);
            let mut it = if video {
                let (_, w, hh) = pick_pixabay_video(h)?;
                if vertical && hh <= w {
                    return None;
                }
                let mut it = item(format!("pb:v{id}"), "pixabay", "video", &tags, &who, PIXABAY_LICENSE, false, format!("Video by {who} from Pixabay"));
                dims(&mut it, Some(w), Some(hh));
                it["durationSeconds"] = h["duration"].clone();
                it
            } else {
                let mut it = item(format!("pb:i{id}"), "pixabay", "image", &tags, &who, PIXABAY_LICENSE, false, format!("Image by {who} from Pixabay"));
                dims(&mut it, h["imageWidth"].as_u64(), h["imageHeight"].as_u64());
                it["preview"] = h["previewURL"].clone();
                it
            };
            it["page"] = h["pageURL"].clone();
            Some(it)
        })
        .collect()
}

fn pixabay_download(id: &str, dir: &Path, key: &str) -> Result<Download, AutomationError> {
    let (kind, num) = id.split_at(id.char_indices().nth(1).map_or(id.len(), |(i, _)| i));
    let num: u64 = num.parse().map_err(|_| err("bad Pixabay id"))?;
    let video = kind == "v";
    let url = if video { format!("https://pixabay.com/api/videos/?key={key}&id={num}") } else { format!("https://pixabay.com/api/?key={key}&id={num}") };
    let v = fetch_json(&url, &[])?;
    let h = v["hits"].get(0).ok_or_else(|| err("no such Pixabay item"))?;
    let who = s(&h["user"]);
    let title = h["tags"].as_str().filter(|t| !t.is_empty()).unwrap_or("pixabay").to_string();
    let (file, ext, cap, att) = if video {
        let (u, _, _) = pick_pixabay_video(h).ok_or_else(|| err("no file for this Pixabay video"))?;
        (u, "mp4".to_string(), MAX_VIDEO_BYTES, format!("Video by {who} from Pixabay"))
    } else {
        let u = h["largeImageURL"].as_str().or_else(|| h["webformatURL"].as_str()).and_then(https).ok_or_else(|| err("no file for this Pixabay image"))?;
        let e = ext_of(&u, &["jpg", "jpeg", "png"], "jpg");
        (u, e, MAX_IMAGE_BYTES, format!("Image by {who} from Pixabay"))
    };
    let st = stem(&title, &format!("pb{num}"));
    let side =
        sidecar(&dir.join(format!("{st}.{ext}")), &title, &who, &s(&h["pageURL"]), PIXABAY_LICENSE, "https://pixabay.com/service/license-summary/", &att);
    let path = save(&file, cap, &[], dir, &st, &ext, &side)?;
    Ok(Download { path, title, attribution: att, license: PIXABAY_LICENSE.into(), needs_credit: false, kind: if video { "video" } else { "image" }.into() })
}

// ------------------------------------------------------------------------------- Freesound

const FS_FIELDS: &str = "id,name,username,license,duration,tags,previews,url";

pub fn freesound_url(query: &str, limit: u32, max_seconds: Option<f64>) -> String {
    let mut u = format!(
        "https://freesound.org/apiv2/search/text/?query={}&page_size={}&fields={FS_FIELDS}",
        encode(query),
        limit.clamp(1, 50).saturating_mul(2).min(150)
    );
    if let Some(m) = max_seconds.filter(|m| m.is_finite() && *m > 0.0) {
        u.push_str(&format!("&filter={}", encode(&format!("duration:[0 TO {}]", m.min(3600.0)))));
    }
    u
}

/// A Freesound licence URL usable in any video: CC0 or CC BY (never NC or Sampling+).
pub fn fs_license(url: &str) -> Option<(&'static str, bool)> {
    let u = url.to_ascii_lowercase();
    if u.contains("publicdomain/zero") {
        Some(("CC0", false))
    } else if u.contains("/licenses/by/") {
        Some(("CC BY", true))
    } else {
        None
    }
}

pub fn parse_freesound(v: &Value, limit: u32) -> Vec<Value> {
    v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let id = r["id"].as_u64()?;
            let (lic, credit) = fs_license(r["license"].as_str()?)?;
            let name = s(&r["name"]);
            let who = s(&r["username"]);
            let mut it = item(format!("fs:{id}"), "freesound", "sfx", &name, &who, lic, credit, format!("\"{name}\" by {who} (freesound.org/s/{id}) — {lic}"));
            it["durationSeconds"] = r["duration"].clone();
            it["tags"] = json!(r["tags"].as_array().map(|t| t.iter().take(12).cloned().collect::<Vec<_>>()));
            it["page"] = r["url"].clone();
            Some(it)
        })
        .take(limit.clamp(1, 50) as usize)
        .collect()
}

fn freesound_download(id: &str, dir: &Path, key: &str) -> Result<Download, AutomationError> {
    let num: u64 = id.parse().map_err(|_| err("bad Freesound id"))?;
    let auth = format!("Token {key}");
    let r = fetch_json(&format!("https://freesound.org/apiv2/sounds/{num}/?fields={FS_FIELDS}"), &[("Authorization", &auth)])?;
    let (lic, credit) = fs_license(r["license"].as_str().unwrap_or("")).ok_or_else(|| err("this sound's licence does not allow use in every video"))?;
    let url = r["previews"]["preview-hq-mp3"].as_str().and_then(https).ok_or_else(|| err("no MP3 preview for this sound"))?;
    let name = s(&r["name"]);
    let who = s(&r["username"]);
    let att = format!("\"{name}\" by {who} (freesound.org/s/{num}) — {lic}");
    let st = stem(&name, &format!("fs{num}"));
    let side = sidecar(&dir.join(format!("{st}.mp3")), &name, &who, &s(&r["url"]), lic, &s(&r["license"]), &att);
    let path = save(&url, super::MAX_AUDIO_BYTES, &[], dir, &st, "mp3", &side)?;
    Ok(Download { path, title: name, attribution: att, license: lic.into(), needs_credit: credit, kind: "audio".into() })
}

/// Freesound search (needs the user's key).
pub fn freesound_search(query: &str, limit: u32, max_seconds: Option<f64>, key: &str) -> Result<Vec<Value>, AutomationError> {
    check_query(query)?;
    let auth = format!("Token {key}");
    Ok(parse_freesound(&fetch_json(&freesound_url(query, limit, max_seconds), &[("Authorization", &auth)])?, limit))
}

// ------------------------------------------------------------------------------- dispatch

/// Search pictures and footage. `source`: `openverse`, `nasa`, `wikimedia`, `pexels`, `pixabay`
/// or `all` (every source that answers; keyed ones only with a key). Returns the results and,
/// per source, an error or a note.
pub fn search(query: &str, source: &str, o: &Opts, key_dir: Option<&Path>) -> Result<(Vec<Value>, Vec<Value>), AutomationError> {
    check_query(query)?;
    let video = o.kind == "video";
    let all = source == "all" || source.is_empty();
    let mut results = Vec::new();
    let mut notes = Vec::new();
    fn run(results: &mut Vec<Value>, notes: &mut Vec<Value>, name: &str, r: Result<Vec<Value>, AutomationError>) {
        match r {
            Ok(v) => results.extend(v),
            Err(e) => notes.push(json!({"source": name, "error": e.to_string()})),
        }
    }
    let px = keys::get("pexels", key_dir);
    let pb = keys::get("pixabay", key_dir);
    if all || source == "pexels" {
        match &px {
            Some(k) => run(&mut results, &mut notes, "pexels", fetch_json(&pexels_url(query, o), &[("Authorization", k)]).map(|v| parse_pexels(&v))),
            None if source == "pexels" => {
                return Err(err("Pexels needs your free API key: get one at https://www.pexels.com/api/ then call library_keys {service: \"pexels\", key}"));
            }
            None => notes.push(json!({"source": "pexels", "note": "add a free Pexels key (library_keys) for millions of vertical stock videos and photos"})),
        }
    }
    if all || source == "pixabay" {
        match &pb {
            Some(k) => run(&mut results, &mut notes, "pixabay", fetch_json(&pixabay_url(query, o, k), &[]).map(|v| parse_pixabay(&v, video, o.vertical))),
            None if source == "pixabay" => {
                return Err(err(
                    "Pixabay needs your free API key: get one at https://pixabay.com/api/docs/ then call library_keys {service: \"pixabay\", key}",
                ));
            }
            None => notes.push(json!({"source": "pixabay", "note": "add a free Pixabay key (library_keys) for stock videos, photos and illustrations"})),
        }
    }
    if (all && video) || source == "wikimedia" {
        run(&mut results, &mut notes, "wikimedia", fetch_json(&wikimedia_url(query, o), &[]).map(|v| parse_wikimedia(&v)));
    }
    if all || source == "nasa" {
        run(&mut results, &mut notes, "nasa", fetch_json(&nasa_url(query, o), &[]).map(|v| parse_nasa(&v)));
    }
    if (all && !video) || source == "openverse" {
        run(&mut results, &mut notes, "openverse", fetch_json(&openverse_images_url(query, o), &[]).map(|v| parse_openverse_images(&v)));
    }
    if !matches!(source, "all" | "" | "openverse" | "nasa" | "wikimedia" | "pexels" | "pixabay") {
        return Err(err("source: all, pexels, pixabay, openverse, nasa or wikimedia"));
    }
    if o.vertical {
        // tall results first; still list the rest (they can be cropped to 9:16)
        results.sort_by_key(|r| !r["vertical"].as_bool().unwrap_or(false));
    }
    Ok((results, notes))
}

/// Download a visual or keyed-audio result by id into `dir/<video|images|audio>/`.
pub fn download(id: &str, dir: &Path, key_dir: Option<&Path>) -> Result<Download, AutomationError> {
    let (src, rest) = id.split_once(':').ok_or_else(|| err(format!("unknown id `{id}`")))?;
    let need = |name: &str| keys::get(name, key_dir).ok_or_else(|| err(format!("{name} needs your API key: library_keys {{service: \"{name}\", key}}")));
    match src {
        "ovi" => openverse_image_download(rest, &dir.join("images")),
        "nasa" => nasa_download(rest, &dir.join("nasa")),
        "wm" => wikimedia_download(rest, &dir.join("video")),
        "px" => pexels_download(rest, &dir.join("stock"), &need("pexels")?),
        "pb" => pixabay_download(rest, &dir.join("stock"), &need("pixabay")?),
        "fs" => freesound_download(rest, &dir.join("audio"), &need("freesound")?),
        _ => Err(err(format!("unknown id `{id}`"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(kind: &str, vertical: bool) -> Opts {
        Opts { kind: kind.into(), vertical, limit: 10 }
    }

    #[test]
    fn urls() {
        let u = openverse_images_url("neon city", &o("photo", true));
        assert!(u.contains("q=neon+city") && u.contains("category=photograph") && u.contains("aspect_ratio=tall") && u.contains("license=cc0,pdm,by,by-sa"));
        assert!(nasa_url("mars", &o("video", false)).contains("media_type=video"));
        assert!(pexels_url("city", &o("video", true)).starts_with("https://api.pexels.com/videos/search?query=city&per_page=10&orientation=portrait"));
        assert!(pexels_url("city", &o("image", false)).starts_with("https://api.pexels.com/v1/search"));
        let pb = pixabay_url("a&key=x", &o("image", true), "KEY123456");
        assert!(pb.contains("q=a%26key%3Dx") && pb.contains("orientation=vertical"));
        assert!(!super::super::redact(&pb).contains("KEY123456"));
        assert!(wikimedia_url("drone", &o("video", false)).contains("gsrsearch=drone+filetype:video"));
        let f = freesound_url("whoosh", 10, Some(3.0));
        assert!(f.contains("page_size=20") && f.contains("filter=duration%3A%5B0+TO+3%5D"), "{f}");
    }

    #[test]
    fn openverse_images_keep_free_licences() {
        let v = json!({"results": [
            {"id": "abc", "title": "Neon", "creator": "Ann", "url": "https://x/a.jpg", "license": "by", "license_version": "2.0", "width": 1080, "height": 1920},
            {"id": "nc", "title": "NC", "url": "https://x/b.jpg", "license": "by-nc"},
            {"id": "http", "title": "Plain", "url": "http://x/c.jpg", "license": "cc0"},
        ]});
        let r = parse_openverse_images(&v);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["id"], "ovi:abc");
        assert_eq!(r[0]["license"], "CC BY 2.0");
        assert_eq!(r[0]["vertical"], true);
    }

    #[test]
    fn nasa_items_and_files() {
        let v = json!({"collection": {"items": [
            {"data": [{"nasa_id": "KSC-1", "title": "Launch", "media_type": "video", "photographer": "Rocket Lab"}], "links": [{"href": "https://x/t.jpg"}]},
            {"data": [{"nasa_id": "PIA1", "title": "Mars", "media_type": "image"}]},
            {"data": [{"nasa_id": "../bad", "media_type": "image"}]},
            {"data": [{"nasa_id": "A", "media_type": "audio"}]},
        ]}});
        let r = parse_nasa(&v);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["needsCredit"], true);
        assert_eq!(r[0]["attribution"], "Credit: NASA / Rocket Lab");
        assert_eq!(r[1]["needsCredit"], false);
        let asset = json!({"collection": {"items": [
            {"href": "http://images-assets.nasa.gov/video/K/K~orig.mp4"},
            {"href": "http://images-assets.nasa.gov/video/K/K~large.mp4"},
            {"href": "https://evil.example/K~large.mp4"},
            {"href": "http://images-assets.nasa.gov/video/K/K~large.jpg"},
        ]}});
        assert_eq!(pick_nasa(&asset, true).as_deref(), Some("https://images-assets.nasa.gov/video/K/K~large.mp4"));
        assert_eq!(pick_nasa(&asset, false).as_deref(), Some("https://images-assets.nasa.gov/video/K/K~large.jpg"));
        assert_eq!(pick_nasa(&json!({}), true), None);
    }

    #[test]
    fn wikimedia_licences_and_files() {
        for ok in ["CC0", "Public domain", "PD-USGov", "CC BY 4.0", "CC BY-SA 3.0", "cc-by-sa-4.0"] {
            assert!(wm_license_ok(ok), "{ok}");
        }
        for bad in ["CC BY-NC 4.0", "CC BY-NC-SA 2.0", "CC BY-ND 4.0", "GFDL", "Fair use", ""] {
            assert!(!wm_license_ok(bad), "{bad}");
        }
        let info = json!({"url": "https://upload.wikimedia.org/a.ogv", "mime": "video/ogg", "derivatives": [
            {"src": "https://upload.wikimedia.org/t/a.ogv.480p.vp9.webm", "transcodekey": "480p.vp9.webm", "width": 854, "height": 480},
            {"src": "https://upload.wikimedia.org/t/a.ogv.1080p.vp9.webm", "transcodekey": "1080p.vp9.webm", "width": 1920, "height": 1080},
            {"src": "https://upload.wikimedia.org/t/a.ogv.2160p.vp9.webm", "transcodekey": "2160p.vp9.webm", "width": 3840, "height": 2160},
            {"src": "https://evil.example/x.webm", "transcodekey": "720p.vp9.webm", "width": 1280, "height": 720},
        ]});
        assert_eq!(pick_wikimedia(&info).unwrap().0, "https://upload.wikimedia.org/t/a.ogv.1080p.vp9.webm");
        assert_eq!(pick_wikimedia(&json!({"url": "https://upload.wikimedia.org/a.ogv", "mime": "video/ogg"})), None);
        let v = json!({"query": {"pages": [
            {"pageid": 5, "title": "File:Drone.webm", "videoinfo": [{"url": "https://upload.wikimedia.org/d.webm", "mime": "video/webm", "width": 1920, "height": 1080, "duration": 12.5,
              "extmetadata": {"LicenseShortName": {"value": "CC BY-SA 4.0"}, "Artist": {"value": "<a href='x'>Bob</a>"}}}]},
            {"pageid": 6, "title": "File:NC.webm", "videoinfo": [{"url": "https://upload.wikimedia.org/n.webm", "mime": "video/webm", "extmetadata": {"LicenseShortName": {"value": "CC BY-NC 4.0"}}}]},
        ]}});
        let r = parse_wikimedia(&v);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["id"], "wm:5");
        assert_eq!(r[0]["creator"], "Bob");
        assert_eq!(r[0]["attribution"], "\"Drone.webm\" by Bob, CC BY-SA 4.0, via Wikimedia Commons");
    }

    #[test]
    fn pexels_and_pixabay() {
        let v = json!({"videos": [{"id": 7, "width": 1080, "height": 1920, "duration": 9, "url": "https://www.pexels.com/video/neon-city-7/", "user": {"name": "Ann"},
            "video_files": [
                {"file_type": "video/mp4", "width": 2160, "height": 3840, "link": "https://videos.pexels.com/4k.mp4"},
                {"file_type": "video/mp4", "width": 1080, "height": 1920, "link": "https://videos.pexels.com/hd.mp4"},
                {"file_type": "video/mp4", "width": 540, "height": 960, "link": "https://videos.pexels.com/sd.mp4"}]}]});
        let r = parse_pexels(&v);
        assert_eq!(r[0]["id"], "px:v7");
        assert_eq!(r[0]["title"], "neon city 7");
        assert_eq!(r[0]["vertical"], true);
        assert_eq!(pick_pexels_video(&v["videos"][0]).as_deref(), Some("https://videos.pexels.com/hd.mp4"));
        let pv = json!({"hits": [
            {"id": 1, "tags": "city, night", "user": "Bo", "duration": 10, "videos": {"large": {"url": "", "width": 0, "height": 0}, "medium": {"url": "https://cdn.pixabay.com/m.mp4", "width": 720, "height": 1280}}},
            {"id": 2, "tags": "wide", "user": "Cy", "videos": {"medium": {"url": "https://cdn.pixabay.com/w.mp4", "width": 1920, "height": 1080}}},
        ]});
        assert_eq!(parse_pixabay(&pv, true, true).len(), 1);
        assert_eq!(parse_pixabay(&pv, true, false).len(), 2);
        assert_eq!(parse_pixabay(&pv, true, true)[0]["id"], "pb:v1");
    }

    #[test]
    fn freesound_licences() {
        assert_eq!(fs_license("http://creativecommons.org/publicdomain/zero/1.0/"), Some(("CC0", false)));
        assert_eq!(fs_license("https://creativecommons.org/licenses/by/4.0/"), Some(("CC BY", true)));
        assert_eq!(fs_license("http://creativecommons.org/licenses/by-nc/3.0/"), None);
        assert_eq!(fs_license("http://creativecommons.org/licenses/sampling+/1.0/"), None);
        let v = json!({"results": [
            {"id": 1, "name": "whoosh", "username": "a", "license": "http://creativecommons.org/publicdomain/zero/1.0/", "duration": 1.2},
            {"id": 2, "name": "nc", "username": "b", "license": "http://creativecommons.org/licenses/by-nc/4.0/"},
        ]});
        let r = parse_freesound(&v, 10);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["id"], "fs:1");
    }

    #[test]
    fn bad_ids_and_sources() {
        let d = std::env::temp_dir();
        assert!(download("nope", &d, None).is_err());
        assert!(download("zz:1", &d, None).is_err());
        assert!(download("nasa:../x", &d, None).is_err());
        assert!(download("wm:abc", &d, None).is_err());
        assert!(search("", "all", &o("video", true), None).is_err());
        assert!(strip_html("<b>Bob</b> <i>Smith</i>") == "Bob Smith");
        assert_eq!(https("http://a/b").as_deref(), Some("https://a/b"));
        assert_eq!(https("ftp://a"), None);
        assert_eq!(ext_of("https://x/a.PNG?x=1", &["png"], "jpg"), "png");
        assert_eq!(ext_of("https://x/a", &["png"], "jpg"), "jpg");
    }

    /// Live: `cargo test -p filmcraft-automation visual -- --ignored`.
    #[test]
    #[ignore = "needs the network"]
    fn live_visual() {
        let dir = std::env::temp_dir().join(format!("filmcraft-visual-{}", std::process::id()));
        let (img, _) = search("sunset beach", "openverse", &o("photo", true), None).unwrap();
        assert!(!img.is_empty());
        let d = download(img[0]["id"].as_str().unwrap(), &dir, None).unwrap();
        assert!(std::fs::metadata(&d.path).unwrap().len() > 1000);
        let (vid, _) = search("rocket launch", "nasa", &o("video", false), None).unwrap();
        assert!(!vid.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
