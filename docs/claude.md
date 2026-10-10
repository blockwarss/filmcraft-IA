# Claude in FilmCraft

Let Claude edit your videos like a professional editor — TikTok / Reels / Shorts, YouTube videos,
gaming and Minecraft montages — using **your Claude plan (Pro or Max). No API key, no extra cost.**

Claude drives the real FilmCraft: every cut, sound, title and export happens in the window you see,
and every change can be undone (Edit ▸ Undo, one step per recipe).

There are three ways to talk to Claude. Use whichever you like; they all drive the same FilmCraft.

| Where | Best for | Setup |
|---|---|---|
| **Claude panel** inside FilmCraft (Window ▸ Claude) | editing while you watch, quick one-click requests | Install [Claude Code](https://claude.com/code) and sign in once (`claude` in a terminal). Nothing else. |
| **Claude Desktop** | chatting with Claude on the side, dropping files in the conversation | `filmcraft-cli claude install --desktop`, or double-click `filmcraft-<version>.mcpb` |
| **Claude Code** | long autonomous jobs, batches, scripts | `/plugin install filmcraft --marketplace blockwarss/filmcraft-IA` (or `filmcraft-cli claude install --code`) |

## Setup

### Claude panel

1. Install Claude Code and run `claude` once in a terminal to sign in with your Claude account.
2. In FilmCraft: **Window ▸ Claude**. The panel shows `Claude Code <version> · your Claude plan`.
3. Type what you want, or click a quick action (*Make it a TikTok*, *Gaming montage*, *Cut silences*,
   *Add captions*, *Add music*, *Review my edit*). Enter sends, Shift+Enter adds a line, **New Chat**
   starts over, **Stop** interrupts.
4. **Model** and **Effort** (above the quick actions) choose how Claude works on the next message:
   *Opus* for a whole edit, *Sonnet* fast and capable, *Haiku* quickest for small requests; a higher
   effort thinks longer before editing (better on big edits, slower, and it uses your plan's limits
   faster). *Default* keeps Claude Code's own setting. The choice is remembered and the
   conversation carries on.

The panel runs `claude -p` with FilmCraft's MCP server bridged to the window (see
`apps/filmcraft/src/claude_host.rs`). Claude may only use FilmCraft's tools and web search there.
The **Desktop** checkbox keeps FilmCraft listening for Claude Desktop / Claude Code whenever it is
open (stored in `<data dir>/claude.json`, with the model and effort).

### Claude Desktop

```sh
filmcraft-cli claude install --desktop     # adds "filmcraft" to claude_desktop_config.json
```

Quit and reopen Claude Desktop. The existing configuration is merged (a `.bak` copy is kept), never
overwritten. Alternatively build the one-click extension with `packaging/claude-desktop/pack.sh`
and double-click `dist/filmcraft-<version>.mcpb` (Settings ▸ Extensions shows its port and app-path
options).

In Claude Desktop, the **+** menu lists FilmCraft's prompts: *TikTok / Reels / Shorts edit*, *Gaming
montage*, *YouTube video edit*, *Edit to a trending sound*, *Find the best clips for shorts*, *Add
styled captions*, *Review my edit like a senior editor*.

### Claude Code

```text
/plugin install filmcraft --marketplace blockwarss/filmcraft-IA
```

The plugin brings the editor skill (`plugins/filmcraft/skills/filmcraft-editor/SKILL.md`), slash
commands (`/filmcraft:short`, `/filmcraft:montage`, `/filmcraft:youtube`, `/filmcraft:trend`,
`/filmcraft:captions`, `/filmcraft:review`, `/filmcraft:clips`, `/filmcraft:setup`) and the MCP
server (`filmcraft-cli` must be on PATH; Linux packages install it in `/usr/bin`). Without the
plugin, `filmcraft-cli claude install --code` registers the MCP server with `claude mcp add`.

### What `claude install` sets up

`filmcraft-cli claude install` (both clients by default) registers
`filmcraft-cli mcp --bridge 127.0.0.1:9876 --launch`: the MCP server drives the running app and
**starts FilmCraft when it isn't open** (`--launch`; `--app <path>` or `FILMCRAFT_APP` if it lives
somewhere unusual). It also writes `<data dir>/claude.json` so the app opens its control port for
Claude whenever you start it yourself. `filmcraft-cli claude status` shows what is configured;
`claude uninstall` removes it; `claude config` prints the MCP configuration for other clients.

## What to ask

Talk to Claude like you would brief an editor:

- "Make a TikTok from the clips in ~/Videos/clutch, hype style, yellow captions, under 30 seconds."
- "Cut a Minecraft montage on a phonk beat, zoom on every kill, explosion sound on the TNT."
- "Remove the silences and filler words, add a lofi bed under my voice."
- "Turn this 40-minute stream into 3 shorts with the best moments."
- "Review my edit for YouTube and fix the pacing."

Claude follows a professional playbook (`filmcraft://guide`, the same text as the skill): hook in
the first two seconds, a change every 2–3 s on short-form, captions inside the safe zones, one
strong accent per highlight, music ducked under voice, and it checks frames before saying it's done.

## The studio recipes

High-level engine commands (`crates/engine/src/studio.rs`), usable from the panel, MCP
(`command_run`), the CLI (`filmcraft-cli exec studio.sfx.add sound=boom seconds=3`) and the control
channel. Each edit is one undo step.

| Command | What it does |
|---|---|
| `studio.sfx.list` / `studio.sfx.add` | ~100 built-in sound effects, synthesised from scratch: transitions (whoosh, swish, whip, zoom in, riser, bass riser, downlifter, glitch, glitch stutter, tape stop, rewind…), impacts (bass boom, cinematic impact, braam, sub drop, 808, gong, taiko, thunder, glass break, dramatic sting…), UI (pop, bubble pop, water drop, ding, like, subscribe bell, cash register, message send, phone vibrate, camera focus, typing, countdown…), meme (record scratch, bonk, honk, boing, rimshot, sad trombone, air horn, applause, crowd cheer, kiss, magic wand, crickets…), game (coin, level up, power up, explosion, gunshot, reload, hitmarker, combo, respawn, shield, XP pickup, block break / place, bow, sword, portal…). `variant` gives other takes. |
| `studio.music.styles` / `studio.music.generate` | Original music beds: phonk, trap, EDM, chiptune, lo-fi, chill, cinematic, at any tempo, key and length, with intro, break and drop. Returns the **exact beat grid** (beats, downbeats, drop) in sequence time. |
| `studio.silences` / `studio.cutSilences` | Find / ripple-delete dead air in the recorded sound (jump cuts); automatic threshold between noise floor and voice. |
| `studio.beats`, `studio.markBeats`, `studio.beatCut`, `studio.beatMontage` | Beat grid of a music clip (exact for generated music, measured otherwise), markers on beats, cuts on beats, a montage with one shot every N beats. |
| `studio.punchZoom` / `studio.shake` | Keyframed punch-in zoom (optionally about a point) and camera shake. |
| `studio.format` | TikTok / Reels / Shorts (1080×1920), portrait, square, YouTube 1080p / 4K, clips filling the frame. |
| `studio.captions` | Captions from the transcript in a style: `tiktok`, `gaming`, `youtube`, `minimal`, `bold`. |
| `studio.title` | Animated titles sized to fit the frame: `hook`, `gaming`, `meme`, `subscribe`, `lowerThird`, `neon`, `clean`, TikTok's own looks (`boxed`, `boxedDark`, `highlight`, `reply` comment bubble, `part2`), `news`, `warning`, `quote`, `cinematic`, `countdown`, `pixel`, `horror`, `pastel`, `fire`. Entrances: `pop`, `bounce`, `zoom`, `slide`, `drop`, `fade`, `wipe` (typed-on reveal), `shake`, `none`. |
| `studio.karaoke` | Karaoke captions: 2–3 words per line, the word being said lights up and grows, timed from the transcript (or from `words` you pass with their times). Styles `yellow`, `green`, `red`, `cyan`, `boxed`, `dark`; `mode: "word"` shows one big word at a time. |
| `studio.overlay` | A picture, sticker or video over the shot on its own track: named spots inside TikTok's safe zone (or `[x, y]`), width as a fraction of the frame, rotation, `pop` / `bounce` / `fade` / `slide` / `spin` entrance and a quick exit. |
| `studio.broll` | A full-frame cut-in above the main shot for N seconds (pictures get a slow push-in; long agency footage starts at its liveliest moment, past title slates). |
| `studio.blurFill` | Landscape footage in a vertical frame: the whole shot over a blurred, darkened, frame-filling copy of itself, instead of black bars. |
| `studio.look` | Colour looks through Lumetri: `vibrant`, `cyberpunk`, `vaporwave`, `vintage`, `sepia`, `noir`, `dreamy`, `moody`, `sunset`, `matrix`, `cinematic` (generated `.cube` LUTs) and the built-in `teal_orange`, `warm`, `cold`, `bleach`, `faded`, `bw`, `golden`, `night`; `intensity` 0–200. |
| `studio.fx` | Effect presets: `vhs`, `grain`, `vignette`, `letterbox`, `blur`, `pixelate`, `mirror`, `pushin`, `fadein`, `fadeout`, `blurin`, and at a moment `flash`, `glitch`, `zoompulse`. |
| `studio.transition` | At the nearest cut: `zoom`, `whip` (wrap-around pan with motion blur, never a black frame), `flash`, `glitch`, `spin`, `shake`, each with its sound effect. `studio.fx.list` lists effects, looks, transitions, positions and animations. |
| `studio.guide` | The **Guide** audio track (see trending sounds below); `mute: true` before export. |
| `studio.library.list` / `.add` / `.addFolder` | Your own sounds, music and clips: `<data dir>/studio/my-library/` plus any folder you add. Search by file and folder names. |
| `studio.fonts.install` / `studio.fonts.list` | Install a .ttf/.otf for titles and captions (kept in `<data dir>/studio/fonts/`, loaded at start-up). |
| `studio.exportSettings` | `file.exportMedia` parameters for TikTok, Reels, Shorts, YouTube or X, with a warning when the Guide track would end up in the file. |

MCP-only tools (`crates/automation`):

| Tool | What it does |
|---|---|
| `studio_overview` | Everything above in one call. |
| `library_search` | Music and sound effects: Kevin MacLeod's Incompetech catalogue (~1 500 tracks, searchable by mood, genre, BPM and length; CC BY), Openverse (Freesound, Jamendo, ccMixter, Wikimedia; CC0 / CC BY / CC BY-SA) and Freesound directly with your key. |
| `media_search` | B-roll footage, photos, illustrations and stickers: Pexels and Pixabay (with your free keys), Openverse images, NASA, Wikimedia Commons video. Vertical (9:16) results first. |
| `library_download` | Downloads any result with its `.attribution` file and places it: sounds on a free audio track, footage as B-roll (`as: broll`) or an overlay (`as: overlay`). Returns the credit line when one is needed. |
| `library_packs` | Kenney's CC0 sound packs: impacts, interface, UI, retro / digital, sci-fi, RPG, casino, music jingles and announcer voices ("fight", "combo", "game over", "level up"…). Installed packs are searchable with `studio.library.list`. |
| `sticker_add` | Emoji stickers (💀 🔥 😭 👀 🗿 🤡 💯… or names like `skull`, `fire`) with a pop entrance, placed in the safe zone. |
| `library_keys` | Your own free Pexels / Pixabay / Freesound API keys: see which are set, store one (kept in `<data dir>/studio/api-keys.json`, owner-only on macOS / Linux) or forget it. Environment variables `PEXELS_API_KEY`, `PIXABAY_API_KEY`, `FREESOUND_API_KEY` work too. |
| `fonts_search` / `fonts_install` | Google Fonts for titles and captions. |

## Music and sounds: what is safe where

| Source | Use it on | Notes |
|---|---|---|
| Built-in sound effects and generated music | everywhere, monetised included | original work generated on your machine; never matched by Content ID |
| Kenney packs (CC0), Openverse / Freesound CC0, NASA | everywhere | no credit needed (NASA: "Credit: NASA" is customary; partner footage is credited) |
| Incompetech, Openverse / Freesound CC BY, Noto / OpenMoji stickers | everywhere | put the credit line in the description (`needsCredit: true`) |
| Pexels / Pixabay | everywhere | no credit needed; don't resell the clips as they are |
| Your library | your responsibility | packs you bought, sounds you recorded… |
| Trending platform sounds | in the platform's app only | FilmCraft never downloads them (their licence covers use inside the app) |

**Editing to a trending TikTok sound.** Put the sound file you have in your library (or skip it).
Claude places it on the **Guide** track (`studio.library.add {guide: true}`), cuts everything to its
beats, mutes the Guide before export (`studio.guide {mute: true}`), and tells you where the sound
starts. Add the official sound in TikTok when you post: perfect sync, and the video counts towards
the trend.

## Security and privacy

- The control port listens on **127.0.0.1 only**, refuses anything that isn't a JSON request line
  (so a web page can't send it commands), and has no password: it is on only when you start
  FilmCraft for Claude or tick **Desktop** (`docs/control-protocol.md`).
- The panel passes Claude only FilmCraft's tools and web search (`--strict-mcp-config`,
  `--allowedTools`); session ids are checked before being passed on, and the model and effort are
  passed only when they are one of the panel's choices (`--model`, `--effort`).
- Network access happens only when Claude (or you) asks for an online library or a font: HTTPS only,
  size limits, file names built by FilmCraft, licences checked again on download. Sound packs are
  unpacked with path, count and size limits (audio and licence files only). API keys are stored
  owner-only, sent only to their own service and never shown back, not even in error messages.
- Your conversations go to Claude through your own Claude Code / Claude Desktop, under your account.

## Troubleshooting

| Problem | Fix |
|---|---|
| Panel: "Claude Code is not installed" | Install it from claude.com/code, run `claude` once to sign in, click **Check Again**. |
| "Please run /login" / authentication errors | Run `claude` in a terminal and sign in with your Claude account. |
| Claude Desktop doesn't list FilmCraft | `filmcraft-cli claude status`, then quit and reopen Claude Desktop. |
| "cannot connect to 127.0.0.1:9876" | Open FilmCraft (or let `--launch` open it); tick **Desktop** in the Claude panel so it always listens. |
| Captions fail | They need a transcript: `transcript.generate` (speech-to-text build) or give Claude the words. |
| Usage limit reached | Your Claude plan's limit resets after a while. |
