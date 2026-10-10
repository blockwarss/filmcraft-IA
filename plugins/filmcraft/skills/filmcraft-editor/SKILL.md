---
name: filmcraft-editor
description: Edit videos in FilmCraft like a professional editor — TikTok / Reels / Shorts, YouTube videos, gaming and Minecraft montages, vlogs. Use whenever the user asks to edit, cut, montage, caption, add music or sound effects, reframe for vertical, sync to a beat or export a video with FilmCraft (MCP tools `command_run`, `doc_inspect`, `render_preview`, `studio_overview`…).
---

# FilmCraft editor playbook

You drive FilmCraft, a full non-linear editor, through its MCP server. You are the editor: make
decisions, execute them, look at the result, fix it. Talk to the user in their language and keep
them in the loop with short progress lines ("Silences cut: −41 s. Adding captions…").

## 0. Tools in one minute

- `studio_overview` — start here once per session: every recipe, sound, music style, caption and
  title style, plus this playbook's short version.
- `doc_inspect` — the project (bins, items with ids) and the active sequence (tracks, clips with
  ids, `start`/`end` in ticks, markers, playhead). **254 016 000 000 ticks = 1 s**; commands also
  take `seconds`.
- `command_run {id, params}` / `command_batch {steps}` — every edit. `command_list {filter}` finds
  ids (`filter: "studio"` shows the recipes).
- `render_preview {seconds}` — look at a frame. In bridge mode `ui_screenshot {panel: "Timeline"}`
  shows the timeline. **Look after every significant step**; never claim a result you haven't
  checked.
- `library_search` / `library_download` — openly licensed music and SFX online (CC0 / CC BY).
- `fonts_search` / `fonts_install` — Google Fonts for titles and captions.
- Undo: `command_run {id: "edit.undo"}`. Every studio recipe is one undo step.

## 1. The loop

1. **Understand the brief**: platform (TikTok / Shorts / Reels vertical, YouTube 16:9), length
   target, tone (hype, funny, chill, cinematic), the hook, what must be kept. If the user gave no
   brief, infer it from the footage and say what you assumed — don't block on questions an editor
   would answer themselves.
2. **Ingest**: `media_import` (absolute paths), then `doc_inspect`. For talking footage run
   `transcript.generate` (when available) — the transcript drives cuts, captions and hook choice.
3. **Rough cut**: structure first (hook → body → payoff → outro/CTA). Remove dead air with
   `studio.cutSilences`, filler words with `transcript.removeFillers`.
4. **Pacing pass**: punch zooms, B-roll, cuts on the beat.
5. **Sound design**: music bed, SFX accents, ducking, loudness.
6. **Text**: hook title, captions, callouts.
7. **Review**: render frames at key moments (hook, every transition, the end), check the
   timeline screenshot, fix. Then export.

## 2. Short-form (TikTok / Reels / Shorts)

- `studio.format {preset: "tiktok"}` first (1080×1920, clips fill the frame).
- **Hook in the first 1–2 s**: start on the most surprising/funny/impressive moment (cold open),
  big text with `studio.title {style: "hook"}` saying *why to keep watching* ("I found diamonds
  at Y=-59 in 2 minutes"). Never start with an intro, a logo or "hey guys".
- **Pace**: no shot longer than ~3 s without a change (cut, zoom, text, B-roll). Remove every
  breath gap: `studio.cutSilences {minSeconds: 0.35, padSeconds: 0.08}` for talking, 0.5 / 0.12
  for gameplay commentary.
- **Captions always** (most people watch muted): `studio.captions {style: "tiktok"}` or
  `"gaming"`; 1 line, 2–4 words, centred low but above the bottom 20 % (app UI). Fix
  mis-transcribed words with `transcript.set` / caption edits.
- **Safe zones**: keep text out of the bottom 20 % and the right 15 % (buttons).
- Length: 15–45 s performs best unless the story needs more. End on the payoff, loop-friendly
  (the last frame should flow into the first).
- Export: `studio.exportSettings {platform: "tiktok"}` gives the `file.exportMedia` params.

## 3. Gaming & Minecraft edits

- **Montage energy**: phonk or trap bed (`studio.music.generate {style: "phonk"}`), cuts on beats
  (`studio.beatCut {every: 2}` or `studio.beatMontage`), hardest moment on the drop
  (`drop` in the result).
- **Accents on every highlight**: kill/clutch → `studio.punchZoom {scale: 135}` + `boom` or
  `bass_808`; explosion/TNT → `studio.shake` + `explosion`; rare find (diamonds, loot) →
  `xp_pickup` / `level_up` + zoom on it with `focus`; fail → `record_scratch` + freeze or
  `sad_trombone`; building timelapse → speed-up (`clip.speedDuration {speed: 400}`) with `whoosh` at the transitions.
- Block-game flavour: `block_break`, `block_place`, `xp_pickup`, `bow`, `sword`, `portal`,
  `footsteps`, chiptune bed for funny/retro edits, pixel fonts (`fonts_install {family:
  "Pixelify Sans"}` or `"Press Start 2P"`) for labels.
- Keep game audio under voice: game audio ≈ −18 LUFS, voice −14, music −22 under voice / −16
  alone. Use `essentialSound.setType` (dialogue / music / sfx) then
  `essentialSound.generateDucking`.
- Don't stack effects: one strong accent per moment. A zoom + shake + boom + flash on every
  kill is noise; save the full combo for the best moment.

## 4. YouTube (16:9)

- `studio.format {preset: "youtube"}` when the sequence isn't 1920×1080 already.
- Cold open (best 5–10 s), title card, then content. Jump cuts on silences (`minSeconds: 0.5`),
  B-roll over explanations, lower thirds for names (`studio.title {style: "lowerThird"}`),
  chapters as markers (`markers.add {name}`).
- Music: chill / lofi / cinematic under voice at −24…−28 LUFS-ish (gain −10…−14 dB on a
  generated bed), ducked. Change music at section changes.
- Captions: `studio.captions {style: "youtube"}` or export an SRT sidecar
  (`captionSidecar: "srt"`).
- End screen: the last 10–20 s need space for YouTube's end-screen elements.

## 5. Sound design rules

- `studio.sfx.list` shows ~60 original sounds (free to use anywhere). Use `variant` 1, 2, 3… when
  the same sound repeats so it doesn't sound copy-pasted.
- Place SFX **slightly before** the visual (1–2 frames) for impacts; whooshes start ~0.2 s before
  the cut so the peak lands on it.
- Risers end exactly on the drop / reveal; downlifters start on it.
- Gain: SFX −6…−12 dB under voice; impacts can peak, UI pops stay subtle (−12 dB).
- Music ends on a musical boundary: generated music already does (pass `durationSeconds` = the
  sequence length) — otherwise fade the last 1–2 s.

## 6. Music sources (in this order)

1. **Generated** (`studio.music.generate`): original, never claimed by Content ID, exact beat grid.
   Styles: phonk, trap, edm, chiptune, lofi, chill, cinematic. Change `seed` for another tune,
   `bpm`/`key` to taste.
2. **The user's library** (`studio.library.list {query}` / `studio.library.add`): their own packs
   and tracks. Their responsibility for rights.
3. **Online openly licensed** (`library_search {kind: "music"}` → `library_download`): CC0 / CC BY.
   When `needsCredit` is true, give the user the `attribution` line for the description.
4. **Trending platform sounds** (TikTok etc.): never download them. Workflow: the user puts the
   sound file they have in their library (or skips it), you place it with
   `studio.library.add {…, guide: true}` → it lands on the **Guide** track; cut to its beats
   (`studio.beatCut`, `studio.markBeats` use the Guide clip by default); before export run
   `studio.guide {mute: true}`; tell the user to add the official sound in the app when posting,
   starting at the same point. You can search the web for what's trending if you have a web tool.

## 7. Text & fonts

- Titles: `studio.title {text, style}` — `hook`, `gaming`, `meme`, `subscribe`, `lowerThird`,
  `neon`, `clean`; they pop in (scale keyframes). Max ~6 words on screen; UPPERCASE for hooks.
- Fonts: `fonts_search {query: "gaming"}`, `fonts_install {family: "Bebas Neue"}`, then pass
  `font` to `studio.title` / `studio.captions`. Good defaults: Montserrat (captions), Bebas Neue /
  Anton (hooks), Bangers / Luckiest Guy (funny gaming), Pixelify Sans (block games).

## 8. Quality bar before export

- [ ] Hook in the first 2 s; no dead air anywhere (scrub the timeline screenshot for gaps).
- [ ] Every cut is intentional; no flash frames (1-frame leftovers) — check `doc_inspect` for
      clips shorter than 3 frames and remove them.
- [ ] Captions readable, inside safe zones, no typos.
- [ ] Audio: no clipping, voice clear over music, music fades/ends cleanly.
- [ ] Guide track muted (if any). Export at the platform settings; `burnCaptions: true` for
      short-form.
- [ ] Tell the user what you did in 3–6 bullet points, the file path, and 1–2 ideas to make it
      even better.

## 9. Things that go wrong

- "not available right now": the command needs a selection or a sequence — pass explicit
  `clips` / `clip` ids, or `timeline.select` first.
- Bridge errors: the app isn't running; the MCP server started with `--launch` opens it, otherwise
  ask the user to open FilmCraft.
- `studio.captions` needs a transcript: `transcript.generate` (speech-to-text build) or ask the
  user for the words and use `transcript.set`.
- Long exports: `file.exportMedia {…, wait: true}` reports progress; don't start a second one.
