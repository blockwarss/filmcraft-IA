---
description: Turn clips into a vertical TikTok / Reels / Shorts edit (hook, jump cuts, captions, SFX, music)
argument-hint: "[clip paths or folder] [style: gaming|funny|hype|vlog|chill] [notes]"
---
Use the filmcraft-editor skill. Make a vertical short-form edit in FilmCraft from: $ARGUMENTS

If no files are given, use what is already in the open project (or ask once for the folder).
Follow the short-form section of the playbook: `studio.format {preset: "tiktok"}`, cold-open hook
with a hook title, tight `studio.cutSilences`, a change every 2–3 s, `studio.captions`, SFX on the
accents, a music bed under the voice, then review frames and export with
`studio.exportSettings {platform: "tiktok"}`. Report in my language.
