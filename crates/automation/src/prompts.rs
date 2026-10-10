//! MCP prompts: one-click editing jobs a person picks in their MCP client (Claude Desktop's "+"
//! menu, Claude Code's `/mcp__filmcraft__…` commands). Each expands to a brief that walks the
//! agent through a professional workflow with FilmCraft's tools.

use rmcp::model::{GetPromptResult, JsonObject, Prompt, PromptArgument, PromptMessage, Role};
use serde_json::Value;

struct Spec {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    args: &'static [(&'static str, &'static str, bool)],
    body: &'static str,
}

const COMMON: &str = "Work in FilmCraft through its MCP tools. Call `studio_overview` once and follow the playbook in the resource `filmcraft://guide`. Look at the result after every significant step (`render_preview`, `ui_screenshot {panel: \"Timeline\"}`) and fix what is off before moving on. Talk to me in my language with short progress lines, then finish with: what you did (3–6 bullets), where the export is, and 1–2 ideas to make it better.";

const SPECS: &[Spec] = &[
    Spec {
        name: "tiktok_edit",
        title: "TikTok / Reels / Shorts edit",
        description: "Vertical short-form edit: hook, jump cuts, captions, sound design, music, export.",
        args: &[
            ("files", "Absolute paths of the clips to use (one per line); empty = what is already in the project", false),
            ("style", "gaming, funny, hype, vlog, chill, cinematic…", false),
            ("length", "Target length in seconds (default: as short as the story allows, 15–45 s)", false),
            ("notes", "Anything else: the hook, what to keep, the text to show, the platform", false),
        ],
        body: "Make a vertical short (TikTok / Reels / Shorts) from {files}.\nStyle: {style}. Target length: {length}. Notes: {notes}.\n\nSteps: import → `studio.format {preset: \"tiktok\"}` → find the strongest moment and open on it (cold open) with a hook title (`studio.title {style: \"hook\"}`) → transcribe if there is speech → `studio.cutSilences` (tight: minSeconds 0.35) and `transcript.removeFillers` → pacing: something changes at least every 2–3 s (punch zooms, cuts, text) → captions (`studio.captions`, style matching the vibe) → sound design (SFX on every accent, music bed under the voice, ducking) → review the whole edit frame by frame at the key moments → export with `studio.exportSettings {platform: \"tiktok\"}` next to the source files.",
    },
    Spec {
        name: "gaming_montage",
        title: "Gaming montage (Minecraft, FPS…)",
        description: "High-energy gaming edit cut to the beat: phonk/trap music, zooms, shakes, impacts, game SFX.",
        args: &[
            ("files", "Absolute paths of the gameplay recordings (one per line)", false),
            ("game", "The game (Minecraft, Fortnite, Valorant…) — sets the sound and font flavour", false),
            ("music", "phonk (default), trap, edm, chiptune, or a file / search term from my library", false),
            ("format", "tiktok (vertical, default) or youtube (16:9)", false),
        ],
        body: "Make a gaming montage from {files} (game: {game}). Format: {format}. Music: {music}.\n\nSteps: import and watch the footage (render frames every few seconds) to list the highlights (kills, clutches, rare finds, funny fails, builds) with their times → format → music bed: generate it with `studio.music.generate` (the requested style, durationSeconds = the montage length; the result gives the beats and the drop) or take it from my library → build the montage on the beats (`studio.beatMontage` or place highlights so each cut lands on a beat; the best moment on the drop) → accents: punch zoom + boom/808 on kills, shake + explosion on blasts, xp/level-up sounds on rewards, record scratch on fails, block sounds for Minecraft — one strong accent per moment → a short hook title in the first second, pixel or bold gaming font (`fonts_install`) → review → export.",
    },
    Spec {
        name: "youtube_edit",
        title: "YouTube video edit",
        description: "16:9 edit: cold open, jump cuts, B-roll, lower thirds, chapters, music under voice, captions.",
        args: &[
            ("files", "Absolute paths of the footage (one per line)", false),
            ("topic", "What the video is about / the title", false),
            ("length", "Target length (e.g. 8 min)", false),
        ],
        body: "Edit a YouTube video about {topic} from {files}. Target length: {length}.\n\nSteps: import → 1920×1080 (`studio.format {preset: \"youtube\"}` if needed) → transcribe → structure: cold open (best 5–10 s) → title → sections → outro with 15 s of room for end-screen elements → `studio.cutSilences {minSeconds: 0.5}` and `transcript.removeFillers` → B-roll over explanations, punch zooms on key sentences (subtle: 110–115 %) → lower thirds and chapter markers (`markers.add`) at section changes → chill/lofi/cinematic music bed under the voice, ducked, changing with sections → captions or an SRT sidecar → review → export with `studio.exportSettings {platform: \"youtube\"}`; list the chapter timestamps for the description.",
    },
    Spec {
        name: "trend_sound_edit",
        title: "Edit to a trending sound",
        description: "Cut the video to a trending sound used as a guide track, exported without it so the official sound can be added in the app.",
        args: &[("files", "Absolute paths of the clips", false), ("sound", "The sound: a file path, or a search term for a file in my library", true)],
        body: "Edit {files} to the sound {sound}, TikTok-style.\n\nThe sound is only a guide: never download platform sounds. Find it in my library (`studio.library.list {query}`) or use the path I gave, place it with `studio.library.add {…, guide: true}` at 0 s → `studio.beats` (uses the Guide clip) → format vertical → cut every shot on the beats (`studio.beatCut` / `studio.beatMontage`), the biggest moment on the strongest beat or drop, transitions and zooms on beats → captions/text timed to the sound → before export `studio.guide {mute: true}` → export. Tell me the exact second the sound must start at when I add it in the app.",
    },
    Spec {
        name: "find_viral_clips",
        title: "Find the best clips for shorts",
        description: "Scan long footage (stream, podcast, gameplay) and cut the best moments into ready-to-post shorts.",
        args: &[("files", "Absolute paths of the long recordings", true), ("count", "How many shorts (default 3)", false)],
        body: "Find the {count} best moments for shorts in {files}.\n\nTranscribe, read the transcript and look at frames to find self-contained moments (15–60 s) with a strong hook in the first seconds: a surprising line, a funny reaction, a clutch play, a reveal. For each: set In/Out around it (`markers.markIn` / `markers.markOut` with `seconds`) and `sequence.makeSubsequence {name}` (it opens as a new sequence), then the full short-form treatment (vertical format, hook title, tight jump cuts, captions, SFX, music) and export each one. Give me a table: title idea, timestamp in the source, why it works.",
    },
    Spec {
        name: "captions",
        title: "Add styled captions",
        description: "Transcribe and add readable, styled captions (TikTok, gaming, YouTube, minimal, bold).",
        args: &[
            ("style", "tiktok (default), gaming, youtube, minimal or bold", false),
            ("font", "A font family (installed from Google Fonts if needed)", false),
        ],
        body: "Add {style} captions to the active sequence (font: {font}).\n\nTranscribe if needed (`transcript.generate`), fix obvious mis-recognitions (names, game terms) before creating captions, install the font with `fonts_install` when it isn't there, then `studio.captions {style, font}`. Check 3 frames: readable, inside the safe zone (not in the bottom 20 % on vertical video), not covering faces or the action. Export with `burnCaptions: true` for short-form.",
    },
    Spec {
        name: "review_edit",
        title: "Review my edit like a senior editor",
        description: "Critique the current timeline (pacing, hook, audio, text, mistakes) and offer to fix each point.",
        args: &[("platform", "Where it will be posted (tiktok, youtube…)", false)],
        body: "Review the active sequence as a senior editor would for {platform}.\n\nInspect it (`doc_inspect`, timeline screenshot, frames at the start, every cut and the end; `studio.silences` for dead air). Check: hook in the first 2 s, pacing (shots too long, dead air), flash frames (clips under 3 frames), jump-cut smoothness, audio levels (music vs voice, clipping), captions (typos, safe zones), text on screen, ending/loop, export settings. Give a prioritised list with the time code of each issue and the fix, then ask which ones to apply — and apply them when I say so.",
    },
];

fn args_of(s: &Spec) -> Vec<PromptArgument> {
    s.args.iter().map(|(n, d, req)| PromptArgument::new(*n).with_description(*d).with_required(*req)).collect()
}

/// Every prompt.
pub fn list() -> Vec<Prompt> {
    SPECS.iter().map(|s| Prompt::new(s.name, Some(s.description), Some(args_of(s))).with_title(s.title)).collect()
}

/// The brief of prompt `name` with its arguments filled in (`None`: unknown prompt).
pub fn get(name: &str, args: Option<&JsonObject>) -> Option<GetPromptResult> {
    let s = SPECS.iter().find(|s| s.name == name)?;
    let mut body = s.body.to_string();
    for (n, _, _) in s.args {
        let v = args.and_then(|a| a.get(*n)).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
        let fill = match (*n, v) {
            (_, Some(v)) => v.chars().take(4000).collect::<String>(),
            ("files", None) => "the media already in the project".into(),
            ("count", None) => "3".into(),
            ("format", None) => "tiktok".into(),
            ("music", None) => "phonk".into(),
            ("style", None) => "the vibe that fits the footage".into(),
            _ => "(not given — decide like a professional editor)".into(),
        };
        body = body.replace(&format!("{{{n}}}"), &fill);
    }
    let text = format!("{body}\n\n{COMMON}");
    Some(GetPromptResult::new(vec![PromptMessage::new_text(Role::User, text)]).with_description(s.description))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_fill_every_placeholder() {
        assert_eq!(list().len(), SPECS.len());
        for s in SPECS {
            let r = get(s.name, None).unwrap();
            let t = serde_json::to_string(&r).unwrap();
            assert!(!t.contains("{files}") && !t.contains("{style}"), "{}: {t}", s.name);
            for (n, _, _) in s.args {
                assert!(!t.contains(&format!("{{{n}}}")), "{} leaves {{{n}}}", s.name);
            }
        }
        let mut a = JsonObject::new();
        a.insert("files".into(), Value::from("/clips/a.mp4\n/clips/b.mp4"));
        a.insert("game".into(), Value::from(42));
        let t = serde_json::to_string(&get("gaming_montage", Some(&a)).unwrap()).unwrap();
        assert!(t.contains("/clips/a.mp4") && t.contains("not given"), "{t}");
        assert!(get("nope", None).is_none());
    }
}
