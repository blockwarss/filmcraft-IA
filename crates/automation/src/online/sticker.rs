//! Emoji stickers: a 512 px transparent PNG of any emoji, from Google's Noto Emoji (falling back
//! to OpenMoji), cached in the studio folder. Ask by the emoji itself (`💀`) or by a common name
//! (`skull`, `fire`, `crying laughing`…).

use std::path::Path;

use super::{Download, MAX_IMAGE_BYTES, save, sidecar};
use crate::AutomationError;

/// Common names → emoji, for agents and users who type words.
pub const NAMES: &[(&str, &str)] = &[
    ("skull", "💀"),
    ("dead", "💀"),
    ("fire", "🔥"),
    ("lit", "🔥"),
    ("crying laughing", "😂"),
    ("laughing", "😂"),
    ("lol", "😂"),
    ("rofl", "🤣"),
    ("sob", "😭"),
    ("crying", "😭"),
    ("eyes", "👀"),
    ("100", "💯"),
    ("clown", "🤡"),
    ("goat", "🐐"),
    ("moai", "🗿"),
    ("heart", "❤️"),
    ("love", "😍"),
    ("heart eyes", "😍"),
    ("thumbs up", "👍"),
    ("trophy", "🏆"),
    ("money", "💰"),
    ("money bag", "💰"),
    ("scream", "😱"),
    ("shocked", "😱"),
    ("mind blown", "🤯"),
    ("cool", "😎"),
    ("sunglasses", "😎"),
    ("pleading", "🥺"),
    ("pray", "🙏"),
    ("rocket", "🚀"),
    ("lightning", "⚡"),
    ("boom", "💥"),
    ("explosion", "💥"),
    ("sparkles", "✨"),
    ("party", "🎉"),
    ("nerd", "🤓"),
    ("thinking", "🤔"),
    ("neutral", "😐"),
    ("eye roll", "🙄"),
    ("sad", "😢"),
    ("angry", "😡"),
    ("party face", "🥳"),
    ("ghost", "👻"),
    ("robot", "🤖"),
    ("alien", "👽"),
    ("game", "🎮"),
    ("controller", "🎮"),
    ("target", "🎯"),
    ("camera", "📸"),
    ("megaphone", "📢"),
    ("siren", "🚨"),
    ("alert", "🚨"),
    ("check", "✅"),
    ("cross", "❌"),
    ("wrong", "❌"),
    ("question", "❓"),
    ("exclamation", "❗"),
    ("brain", "🧠"),
    ("melting", "🫠"),
    ("salute", "🫡"),
    ("smiling tear", "🥲"),
    ("zany", "🤪"),
    ("cap", "🧢"),
    ("muscle", "💪"),
    ("flex", "💪"),
    ("crown", "👑"),
    ("star", "⭐"),
    ("gem", "💎"),
    ("diamond", "💎"),
    ("pointing", "👉"),
    ("point down", "👇"),
    ("point up", "☝️"),
    ("ok", "👌"),
    ("clap", "👏"),
    ("wave", "👋"),
    ("hundred", "💯"),
    ("cold", "🥶"),
    ("hot", "🥵"),
    ("sweat", "😅"),
    ("grimace", "😬"),
    ("shush", "🤫"),
    ("monkey", "🙈"),
    ("see no evil", "🙈"),
    ("pizza", "🍕"),
    ("popcorn", "🍿"),
    ("clock", "⏰"),
    ("warning", "⚠️"),
    ("no entry", "⛔"),
    ("red flag", "🚩"),
    ("chart up", "📈"),
    ("chart down", "📉"),
];

/// The emoji for `input`: the emoji itself, or a name from [`NAMES`] (case-insensitive).
pub fn resolve(input: &str) -> Option<String> {
    let t = input.trim();
    if t.is_empty() || t.chars().count() > 40 {
        return None;
    }
    let lower = t.to_lowercase().replace(['_', '-'], " ");
    if let Some((_, e)) = NAMES.iter().find(|(n, _)| *n == lower) {
        return Some((*e).to_string());
    }
    // an emoji: no letters or digits of its own (keycaps aside), at most 10 code points
    let ok = t.chars().count() <= 10 && t.chars().any(|c| (c as u32) >= 0x2000) && t.chars().all(|c| !c.is_ascii_alphabetic());
    ok.then(|| t.to_string())
}

/// Code points as Noto names them: `1f441_fe0f_200d_1f5e8_fe0f`.
pub fn noto_code(e: &str) -> String {
    e.chars().map(|c| format!("{:x}", c as u32)).collect::<Vec<_>>().join("_")
}

/// Code points as OpenMoji names them: `1F441-FE0F-200D-1F5E8-FE0F` (without a trailing FE0F on
/// single characters).
pub fn openmoji_code(e: &str) -> String {
    let cps: Vec<u32> = e.chars().map(|c| c as u32).collect();
    let cps: Vec<u32> = if cps.len() == 2 && cps.get(1) == Some(&0xFE0F) { cps.into_iter().take(1).collect() } else { cps };
    cps.iter().map(|c| format!("{c:X}")).collect::<Vec<_>>().join("-")
}

/// Download `emoji` (an emoji or a name) as a PNG into `dir`.
pub fn download(input: &str, dir: &Path) -> Result<Download, AutomationError> {
    let e = resolve(input).ok_or_else(|| super::err(format!("`{input}` is not an emoji or a known emoji name (e.g. 💀, fire, skull, crying laughing)")))?;
    let code = noto_code(&e);
    let name = format!("emoji-{code}");
    let noto_att = "Emoji: Noto Emoji by Google (CC BY 4.0)";
    let noto = sidecar(
        &dir.join(format!("{name}.png")),
        &e,
        "Google (Noto Emoji)",
        "https://googlefonts.github.io/noto-emoji-animation/",
        "CC BY 4.0",
        "https://creativecommons.org/licenses/by/4.0/",
        noto_att,
    );
    match save(&format!("https://fonts.gstatic.com/s/e/notoemoji/latest/{code}/512.png"), MAX_IMAGE_BYTES, &[], dir, &name, "png", &noto) {
        Ok(path) => Ok(Download { path, title: e, attribution: noto_att.into(), license: "CC BY 4.0".into(), needs_credit: true, kind: "image".into() }),
        Err(first) => {
            let om = openmoji_code(&e);
            let att = "Emoji: OpenMoji (openmoji.org), CC BY-SA 4.0";
            let name = format!("openmoji-{}", om.to_ascii_lowercase());
            let side = sidecar(
                &dir.join(format!("{name}.png")),
                &e,
                "OpenMoji",
                "https://openmoji.org",
                "CC BY-SA 4.0",
                "https://creativecommons.org/licenses/by-sa/4.0/",
                att,
            );
            save(
                &format!("https://raw.githubusercontent.com/hfg-gmuend/openmoji/master/color/618x618/{om}.png"),
                MAX_IMAGE_BYTES,
                &[],
                dir,
                &name,
                "png",
                &side,
            )
            .map(|path| Download { path, title: e.clone(), attribution: att.into(), license: "CC BY-SA 4.0".into(), needs_credit: true, kind: "image".into() })
            .map_err(|_| first)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_codes() {
        assert_eq!(resolve("skull").as_deref(), Some("💀"));
        assert_eq!(resolve("Crying_Laughing").as_deref(), Some("😂"));
        assert_eq!(resolve("💀").as_deref(), Some("💀"));
        assert_eq!(resolve("not an emoji"), None);
        assert_eq!(resolve("../../etc"), None);
        assert_eq!(resolve(""), None);
        assert_eq!(noto_code("💀"), "1f480");
        assert_eq!(noto_code("❤️"), "2764_fe0f");
        assert_eq!(noto_code("🇫🇷"), "1f1eb_1f1f7");
        assert_eq!(openmoji_code("❤️"), "2764");
        assert_eq!(openmoji_code("👁️‍🗨️"), "1F441-FE0F-200D-1F5E8-FE0F");
        for (n, e) in NAMES {
            assert!(resolve(e).is_some(), "{n}");
        }
    }

    /// Live: `cargo test -p filmcraft-automation sticker -- --ignored`.
    #[test]
    #[ignore = "needs the network"]
    fn live_sticker() {
        let dir = std::env::temp_dir().join(format!("filmcraft-sticker-{}", std::process::id()));
        let d = download("skull", &dir).unwrap();
        assert!(std::fs::read(&d.path).unwrap().starts_with(b"\x89PNG"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
