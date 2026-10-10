//! The user's own free API keys for the libraries that need one (Pexels, Pixabay, Freesound).
//!
//! A key comes from the environment (`PEXELS_API_KEY`, `PIXABAY_API_KEY`, `FREESOUND_API_KEY`)
//! or from `<studio folder>/api-keys.json`, written with owner-only permissions on Unix. Keys are
//! checked to be plain tokens (so they cannot inject headers or URL parts) and are never returned
//! by any tool: [`status`] only says which services have one.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::err;
use crate::AutomationError;

/// (service, environment variable, where to get a free key, what it unlocks).
pub const SERVICES: &[(&str, &str, &str, &str)] = &[
    ("pexels", "PEXELS_API_KEY", "https://www.pexels.com/api/ (free, instant)", "vertical stock videos and photos, no credit required"),
    ("pixabay", "PIXABAY_API_KEY", "https://pixabay.com/api/docs/ (free account)", "stock videos, photos, illustrations and vectors, no credit required"),
    ("freesound", "FREESOUND_API_KEY", "https://freesound.org/apiv2/apply (free account)", "600 000+ sound effects (CC0 and CC BY only)"),
];

fn file(dir: &Path) -> PathBuf {
    dir.join("api-keys.json")
}

/// A plausible API key: 8–128 letters, digits, `-` or `_`.
pub fn valid(key: &str) -> bool {
    (8..=128).contains(&key.len()) && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn service(name: &str) -> Result<&'static (&'static str, &'static str, &'static str, &'static str), AutomationError> {
    let n = name.trim().to_ascii_lowercase();
    SERVICES.iter().find(|s| s.0 == n).ok_or_else(|| err(format!("unknown service `{n}` (pexels, pixabay or freesound)")))
}

/// The key for `name`, from the environment first, then the keys file in `dir`.
pub fn get(name: &str, dir: Option<&Path>) -> Option<String> {
    let s = service(name).ok()?;
    if let Ok(k) = std::env::var(s.1)
        && valid(k.trim())
    {
        return Some(k.trim().to_string());
    }
    let v: Value = serde_json::from_slice(&std::fs::read(file(dir?)).ok()?).ok()?;
    v[s.0].as_str().map(str::trim).filter(|k| valid(k)).map(str::to_string)
}

/// Store (or with `None`, forget) the key for `name` in `dir/api-keys.json`.
pub fn set(name: &str, key: Option<&str>, dir: &Path) -> Result<(), AutomationError> {
    let s = service(name)?;
    let path = file(dir);
    let mut v: Value = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    match key.map(str::trim) {
        Some(k) if valid(k) => v[s.0] = json!(k),
        Some(_) => return Err(err("that does not look like an API key (8–128 letters, digits, - or _)")),
        None => {
            if let Some(o) = v.as_object_mut() {
                o.remove(s.0);
            }
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| err(format!("{}: {e}", dir.display())))?;
    write_private(&path, v.to_string().as_bytes())
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), AutomationError> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut f =
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path).map_err(|e| err(format!("{}: {e}", path.display())))?;
    // an older file may have been created with wider permissions
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    f.write_all(bytes).map_err(|e| err(format!("{}: {e}", path.display())))
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), AutomationError> {
    std::fs::write(path, bytes).map_err(|e| err(format!("{}: {e}", path.display())))
}

/// Which services have a key (never the keys themselves) and how to get one.
pub fn status(dir: Option<&Path>) -> Vec<Value> {
    SERVICES
        .iter()
        .map(|(n, env, url, what)| json!({"service": n, "configured": get(n, dir).is_some(), "unlocks": what, "getAKey": url, "envVar": env}))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_validated_stored_privately_and_never_listed() {
        assert!(valid("abcDEF123_-xyz"));
        assert!(!valid("short"));
        assert!(!valid("has space in it"));
        assert!(!valid("evil\r\nX-Header: 1"));
        assert!(!valid("a&key=b&c=dddd"));
        let dir = std::env::temp_dir().join(format!("filmcraft-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(set("pexels", Some("bad key!"), &dir).is_err());
        assert!(set("nope", Some("abcdefgh1234"), &dir).is_err());
        set("pixabay", Some("  abcdefgh1234  "), &dir).unwrap();
        assert_eq!(get("pixabay", Some(&dir)).as_deref(), Some("abcdefgh1234"));
        let st = status(Some(&dir));
        assert!(st.iter().any(|s| s["service"] == "pixabay" && s["configured"] == true));
        assert!(!serde_json::to_string(&st).unwrap().contains("abcdefgh1234"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(file(&dir)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        set("pixabay", None, &dir).unwrap();
        assert_eq!(get("pixabay", Some(&dir)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
