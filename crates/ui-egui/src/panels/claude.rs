//! Claude panel: ask Claude to edit, in a chat next to the timeline. The host runs the user's own
//! Claude Code (`claude -p`, signed in with their Claude plan — no API key) with FilmCraft's MCP
//! server bridged to this window, so every change happens live here and is undoable.
//!
//! The panel only draws the conversation and forwards messages; the host (the desktop app)
//! implements [`ClaudeHost`]. Without a host (the web) the panel explains how to connect Claude
//! Desktop or Claude Code instead.
//!
//! Automation ids: `claude.input`, `claude.send`, `claude.stop`, `claude.new`, `claude.retry`,
//! `claude.listen`, `claude.quick.<id>`, `claude.item.<n>`, `claude.model` (options
//! `claude.model.option.<alias>`, `default`), `claude.effort` (`claude.effort.option.<level>`).

use egui::{Align2, Color32, Key, KeyboardShortcut, Modifiers, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::FilmcraftApp;
use crate::theme::Tokens;

/// Tallest the message box grows (about 8 lines) before it scrolls.
const INPUT_MAX_H: f32 = 150.0;

/// What the host reports while Claude works.
#[derive(Clone, Debug, PartialEq)]
pub enum ClaudeEvent {
    /// The conversation id (to continue it with the next message).
    Session(String),
    /// Text Claude wrote.
    Text(String),
    /// Claude called a tool (`name` without the `mcp__filmcraft__` prefix; `detail`: what it asked).
    Tool { name: String, detail: String },
    /// The last tool call finished.
    ToolDone { ok: bool, detail: String },
    /// The turn ended (with an error message when it failed).
    Done { error: Option<String> },
}

/// Models the panel offers: (Claude Code `--model` alias, label); `""` is Claude Code's default.
pub const MODELS: &[(&str, &str)] = &[("", "Default"), ("opus", "Opus"), ("sonnet", "Sonnet"), ("haiku", "Haiku")];

/// Effort levels the panel offers: (Claude Code `--effort` level, label); `""` is the default.
pub const EFFORTS: &[(&str, &str)] = &[("", "Default"), ("low", "Low"), ("medium", "Medium"), ("high", "High"), ("xhigh", "Extra High"), ("max", "Max")];

/// How Claude runs a turn: the model and effort chosen in the panel (`None`: Claude Code's own).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClaudeOptions {
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl ClaudeOptions {
    /// Only values the panel offers come through (anything else, say from an edited settings
    /// file, means the default): nothing else is ever passed to Claude Code.
    pub fn new(model: &str, effort: &str) -> Self {
        let pick = |list: &[(&str, &str)], v: &str| list.iter().find(|(id, _)| !id.is_empty() && *id == v).map(|(id, _)| (*id).to_string());
        ClaudeOptions { model: pick(MODELS, model), effort: pick(EFFORTS, effort) }
    }
}

/// Runs Claude for the panel.
pub trait ClaudeHost {
    /// `Ok(version)` when Claude Code is installed, else what to do about it.
    fn check(&mut self) -> Result<String, String>;
    /// Start a turn: `text` in the conversation `session` (a new one when `None`), with
    /// FilmCraft's MCP server bridged to `port`, on the model and effort of `opts`.
    fn send(&mut self, text: &str, session: Option<&str>, port: u16, opts: &ClaudeOptions) -> Result<(), String>;
    /// Events since the last poll.
    fn poll(&mut self) -> Vec<ClaudeEvent>;
    /// Stop the running turn.
    fn stop(&mut self);
}

/// One line of the conversation.
#[derive(Clone, Debug, PartialEq)]
pub enum ChatItem {
    User(String),
    Claude(String),
    /// A tool call: name, detail, outcome (`None` while it runs).
    Tool(String, String, Option<bool>),
    Error(String),
}

/// The conversation (not saved with the UI state).
#[derive(Default)]
pub struct ChatState {
    pub input: String,
    pub items: Vec<ChatItem>,
    pub session: Option<String>,
    pub busy: bool,
    /// Result of the last [`ClaudeHost::check`].
    pub checked: Option<Result<String, String>>,
    /// The control port the conversation's MCP server talks to.
    pub port: u16,
    /// `claude.json`'s `listen`, read once.
    pub listen: Option<bool>,
    /// The model and effort for the next message (`claude.json`'s, read once: see [`prefs`]).
    pub options: Option<(String, String)>,
}

/// The chosen (model, effort), loaded from `claude.json` the first time.
pub fn prefs(app: &mut FilmcraftApp) -> (String, String) {
    app.claude
        .options
        .get_or_insert_with(|| {
            let c = filmcraft_engine::studio::claude_settings();
            let o = ClaudeOptions::new(&c.model, &c.effort);
            (o.model.unwrap_or_default(), o.effort.unwrap_or_default())
        })
        .clone()
}

/// Choose the model and effort for the next messages (unknown values mean the default) and
/// remember them in `claude.json`.
pub fn set_prefs(app: &mut FilmcraftApp, model: &str, effort: &str) {
    let o = ClaudeOptions::new(model, effort);
    let (model, effort) = (o.model.unwrap_or_default(), o.effort.unwrap_or_default());
    app.claude.options = Some((model.clone(), effort.clone()));
    let saved = filmcraft_engine::studio::save_claude_settings(&filmcraft_engine::studio::ClaudeSettings {
        model,
        effort,
        ..filmcraft_engine::studio::claude_settings()
    });
    if let Err(e) = saved {
        app.claude.items.push(ChatItem::Error(e));
    }
}

/// One-click requests: (id, button label, request sent to Claude).
pub const QUICK: &[(&str, &str, &str)] = &[
    (
        "tiktok",
        "Make it a TikTok",
        "Turn the active sequence into a vertical TikTok / Reels / Shorts edit, following the editor playbook: vertical format, a hook in the first 2 seconds, tight jump cuts, captions, sound effects on the accents and a music bed. Only ask me something if you are blocked.",
    ),
    (
        "montage",
        "Gaming montage",
        "Make a high-energy gaming montage from the clips in this project: find the highlights, generate a phonk beat, cut on the beats with the best moment on the drop, and add punch zooms, shakes and impact sounds on the highlights.",
    ),
    ("silences", "Cut silences", "Cut the dead air out of the active sequence (jump cuts) and tell me how much time was removed."),
    ("captions", "Add captions", "Add TikTok-style karaoke captions to the active sequence (the spoken word lights up), transcribing it first if needed."),
    ("music", "Add music", "Add a music bed that fits the mood under the whole sequence, ducked under the voice, ending cleanly."),
    (
        "review",
        "Review my edit",
        "Review the active sequence like a senior editor: list the issues with time codes and the fix for each, then ask which ones to apply.",
    ),
];

/// The language Claude should answer in: the user's system language when the host knows it.
fn reply_language(app: &FilmcraftApp) -> &'static str {
    let tag = app.hooks.system_languages.as_ref().and_then(|f| f().into_iter().next()).unwrap_or_default().to_ascii_lowercase();
    match tag.get(..2).unwrap_or("") {
        "fr" => "French",
        "es" => "Spanish",
        "de" => "German",
        "it" => "Italian",
        "pt" => "Portuguese",
        "ja" => "Japanese",
        "zh" => "Chinese",
        "ko" => "Korean",
        "nl" => "Dutch",
        "pl" => "Polish",
        "tr" => "Turkish",
        "ar" => "Arabic",
        "ru" => "Russian",
        _ => match app.ui.language {
            crate::i18n::Language::Es => "Spanish",
            crate::i18n::Language::Ja => "Japanese",
            crate::i18n::Language::PtBr => "Portuguese",
            _ => "English",
        },
    }
}

/// Send `text` to Claude (also what the quick buttons do).
pub fn send(app: &mut FilmcraftApp, text: &str, quick: bool) {
    let text = text.trim().to_string();
    if text.is_empty() || app.claude.busy {
        return;
    }
    let port = ensure_listening(app);
    let lang = reply_language(app);
    let ask = if quick { format!("{text}\n\nReply in {lang}.") } else { text.clone() };
    let session = app.claude.session.clone();
    let (model, effort) = prefs(app);
    let opts = ClaudeOptions::new(&model, &effort);
    let Some(host) = app.hooks.claude.as_mut() else {
        app.claude.items.push(ChatItem::Error(tl!("Claude is not available in this version of FilmCraft.").into()));
        return;
    };
    app.claude.items.push(ChatItem::User(text));
    match host.send(&ask, session.as_deref(), port, &opts) {
        Ok(()) => app.claude.busy = true,
        Err(e) => app.claude.items.push(ChatItem::Error(e)),
    }
}

/// Make sure this window answers on the control port the MCP server will connect to; returns it.
pub fn ensure_listening(app: &mut FilmcraftApp) -> u16 {
    if app.claude.port == 0 {
        app.claude.port = filmcraft_engine::studio::claude_settings().port;
    }
    if !app.has_control() {
        let port = app.claude.port;
        if let Some(start) = app.hooks.start_control.as_mut()
            && let Some(rx) = start(port)
        {
            app.set_control(rx);
        }
    }
    app.claude.port
}

/// Apply what the host reported since the last frame.
pub fn pump(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(host) = app.hooks.claude.as_mut() else { return };
    let events = host.poll();
    let st = &mut app.claude;
    for e in events {
        match e {
            ClaudeEvent::Session(id) => st.session = Some(id),
            ClaudeEvent::Text(t) => match st.items.last_mut() {
                Some(ChatItem::Claude(prev)) => {
                    prev.push_str("\n\n");
                    prev.push_str(&t);
                }
                _ => st.items.push(ChatItem::Claude(t)),
            },
            ClaudeEvent::Tool { name, detail } => st.items.push(ChatItem::Tool(name, detail, None)),
            ClaudeEvent::ToolDone { ok, detail } => {
                if let Some(ChatItem::Tool(_, d, state)) = st.items.iter_mut().rev().find(|i| matches!(i, ChatItem::Tool(_, _, None))) {
                    *state = Some(ok);
                    if !ok && !detail.is_empty() {
                        d.push_str(" — ");
                        d.push_str(&detail);
                    }
                }
            }
            ClaudeEvent::Done { error } => {
                st.busy = false;
                for it in st.items.iter_mut() {
                    if let ChatItem::Tool(_, _, s @ None) = it {
                        *s = Some(error.is_none());
                    }
                }
                if let Some(e) = error {
                    st.items.push(ChatItem::Error(e));
                }
            }
        }
    }
    if app.claude.busy {
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }
}

/// Light Markdown for chat text: `**bold**` markers removed, list bullets as "•", headings plain.
pub fn plain(md: &str) -> String {
    md.lines()
        .map(|l| {
            let t = l.trim_start();
            let indent = l.get(..l.len() - t.len()).unwrap_or("");
            let t = t.trim_start_matches('#').trim_start();
            let t = if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) { format!("• {rest}") } else { t.to_string() };
            format!("{indent}{}", t.replace("**", "").replace('`', ""))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    if app.claude.checked.is_none()
        && let Some(h) = app.hooks.claude.as_mut()
    {
        app.claude.checked = Some(h.check());
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 8.0))));
    let ui = &mut child;
    let mut elems: Vec<(String, Rect, String)> = Vec::new();
    let mut to_send: Option<(String, bool)> = None;
    let mut new_chat = false;
    let mut stop = false;
    let mut recheck = false;
    let mut listen_toggle: Option<bool> = None;
    let (mut model, mut effort) = prefs(app);
    let chosen = (model.clone(), effort.clone());

    // header: status + New chat
    ui.horizontal(|ui| {
        let (dot, line) = match (&app.hooks.claude, &app.claude.checked) {
            (None, _) => (t.text_faint, tl!("Connect Claude Desktop or Claude Code to FilmCraft").to_string()),
            (Some(_), Some(Ok(v))) => (Color32::from_rgb(0x4c, 0xc3, 0x6b), tlf!("Claude Code {version} · your Claude plan, no API key", version = v)),
            (Some(_), Some(Err(_))) => (Color32::from_rgb(0xe8, 0xb3, 0x3c), tl!("Claude Code is not installed").to_string()),
            (Some(_), None) => (t.text_faint, tl!("Checking Claude Code…").to_string()),
        };
        let (r, _) = ui.allocate_exact_size(vec2(10.0, 16.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 4.0, dot);
        ui.label(RichText::new(line).color(t.text_dim).size(11.5));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add_enabled(!app.claude.items.is_empty() && !app.claude.busy, egui::Button::new(tl!("New Chat")));
            elems.push(("claude.new".into(), r.rect, "New Chat".into()));
            new_chat = r.clicked();
        });
    });
    if let Some(Err(help)) = &app.claude.checked {
        egui::Frame::new().fill(t.field_bg).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
            ui.label(RichText::new(help).color(t.text).size(12.0));
            let r = ui.button(tl!("Check Again"));
            elems.push(("claude.retry".into(), r.rect, "Check Again".into()));
            recheck = r.clicked();
        });
    }
    // model and effort for the next message
    ui.add_enabled_ui(app.hooks.claude.is_some(), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let label = |list: &[(&str, &str)], v: &str| crate::i18n::t(list.iter().find(|(id, _)| *id == v).map_or("Default", |(_, l)| *l)).to_string();
            ui.label(RichText::new(tl!("Model")).color(t.text_dim).size(11.5));
            let shown = label(MODELS, &model);
            let r = egui::ComboBox::from_id_salt("claude.model").selected_text(&shown).width(92.0).show_ui(ui, |ui| {
                for (id, l) in MODELS {
                    let o = ui.selectable_value(&mut model, (*id).to_string(), crate::i18n::t(l));
                    elems.push((format!("claude.model.option.{}", if id.is_empty() { "default" } else { id }), o.rect, (*l).into()));
                }
            });
            r.response
                .clone()
                .on_hover_text(tl!("Opus: the most capable, best for a whole edit. Sonnet: fast and capable. Haiku: quickest, for small requests."));
            elems.push(("claude.model".into(), r.response.rect, shown));
            ui.add_space(6.0);
            ui.label(RichText::new(tl!("Effort")).color(t.text_dim).size(11.5));
            let shown = label(EFFORTS, &effort);
            let r = egui::ComboBox::from_id_salt("claude.effort").selected_text(&shown).width(92.0).show_ui(ui, |ui| {
                for (id, l) in EFFORTS {
                    let o = ui.selectable_value(&mut effort, (*id).to_string(), crate::i18n::t(l));
                    elems.push((format!("claude.effort.option.{}", if id.is_empty() { "default" } else { id }), o.rect, (*l).into()));
                }
            });
            r.response.clone().on_hover_text(tl!(
                "Higher effort: Claude thinks longer before editing. Better results on big edits, but slower, and it uses your plan's limits faster."
            ));
            elems.push(("claude.effort".into(), r.response.rect, shown));
        });
    });
    ui.add_space(4.0);

    // quick actions
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for (id, label, prompt) in QUICK {
            let b = egui::Button::new(RichText::new(crate::i18n::t(label)).size(11.5))
                .corner_radius(10.0)
                .fill(t.field_bg)
                .stroke(Stroke::new(1.0, t.field_border));
            let r = ui.add_enabled(!app.claude.busy && app.hooks.claude.is_some(), b);
            elems.push((format!("claude.quick.{id}"), r.rect, (*label).into()));
            if r.clicked() {
                to_send = Some(((*prompt).into(), true));
            }
        }
    });
    ui.add_space(6.0);

    // conversation: everything above the input, whose height (measured last frame) grows with
    // the message up to INPUT_MAX_H
    let input_key = egui::Id::new("claude.input.height");
    let input_h = ui.data(|d| d.get_temp::<f32>(input_key)).unwrap_or(86.0).clamp(60.0, INPUT_MAX_H + 40.0);
    let list_h = (ui.available_height() - input_h).max(40.0);
    let width = ui.available_width();
    egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        if app.claude.items.is_empty() {
            ui.add_space(8.0);
            let hello = if app.hooks.claude.is_some() {
                tl!("Tell Claude what you want, like a brief to an editor:\n• “Make a TikTok from these clips, hype style, captions in yellow”\n• “Cut a Minecraft montage on a phonk beat, zoom on every kill”\n• “Remove the silences and add a lofi bed under my voice”\n\nClaude edits live in this window. Every change can be undone.")
            } else {
                tl!("In Claude Desktop or Claude Code, connect FilmCraft with:\n  filmcraft-cli claude install\nthen ask Claude to edit. It drives this window live.")
            };
            ui.label(RichText::new(hello).color(t.text_dim).size(12.5));
        }
        for (n, item) in app.claude.items.iter().enumerate() {
            let id = format!("claude.item.{n}");
            match item {
                ChatItem::User(text) => {
                    // top-down + right-aligned (a right-to-left layout would not wrap the text)
                    ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                        let r = egui::Frame::new().fill(t.accent).corner_radius(10.0).inner_margin(vec2(10.0, 6.0)).show(ui, |ui| {
                            ui.set_max_width(width * 0.8);
                            ui.add(egui::Label::new(RichText::new(text).color(Color32::WHITE).size(12.5)).wrap());
                        });
                        elems.push((id, r.response.rect, text.clone()));
                    });
                }
                ChatItem::Claude(text) => {
                    let r = egui::Frame::new().fill(t.field_bg).corner_radius(10.0).inner_margin(vec2(10.0, 7.0)).show(ui, |ui| {
                        ui.set_max_width(width * 0.92);
                        ui.add(egui::Label::new(RichText::new(plain(text)).color(t.text).size(12.5)).wrap());
                    });
                    elems.push((id, r.response.rect, text.clone()));
                }
                ChatItem::Tool(name, detail, state) => {
                    let (r, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
                    let col = match state {
                        None => t.accent,
                        Some(true) => Color32::from_rgb(0x4c, 0xc3, 0x6b),
                        Some(false) => Color32::from_rgb(0xe0, 0x4f, 0x4f),
                    };
                    ui.painter().circle_filled(pos2(r.min.x + 7.0, r.center().y), 3.5, col);
                    let text = if detail.is_empty() { name.clone() } else { format!("{name}  {detail}") };
                    let clip = Rect::from_min_max(pos2(r.min.x + 16.0, r.min.y), r.max);
                    ui.painter().with_clip_rect(clip).text(pos2(clip.min.x, r.center().y), Align2::LEFT_CENTER, &text, Tokens::mono(11.0), t.text_dim);
                    elems.push((id, r, text));
                }
                ChatItem::Error(e) => {
                    let r = ui.add(egui::Label::new(RichText::new(e).color(Color32::from_rgb(0xe0, 0x4f, 0x4f)).size(12.0)).wrap());
                    elems.push((id, r.rect, e.clone()));
                }
            }
            ui.add_space(5.0);
        }
        if app.claude.busy {
            let dots = ".".repeat(1 + (ui.input(|i| i.time) * 2.5) as usize % 3);
            ui.label(RichText::new(format!("{}{dots}", tl!("Claude is editing"))).color(t.text_faint).size(11.5));
        }
    });

    // input: grows with the message, then scrolls (the cursor stays in view)
    ui.add_space(4.0);
    let row = ui.horizontal(|ui| {
        let w = (ui.available_width() - 74.0).max(60.0);
        let enabled = app.hooks.claude.is_some();
        let input = &mut app.claude.input;
        let out = egui::ScrollArea::vertical().id_salt("claude.input.scroll").max_height(INPUT_MAX_H).max_width(w).auto_shrink([false, true]).show(ui, |ui| {
            let edit = egui::TextEdit::multiline(input)
                .hint_text(tl!("Ask Claude to edit… (Enter to send, Shift+Enter for a new line)"))
                .desired_rows(3)
                .desired_width(ui.available_width())
                .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter));
            ui.add_enabled(enabled, edit)
        });
        let r = out.inner;
        // the visible part of the box (the text inside can be taller and scroll)
        elems.push(("claude.input".into(), out.inner_rect.intersect(r.rect), "Message".into()));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
        ui.vertical(|ui| {
            if app.claude.busy {
                let b = ui.add_sized(vec2(66.0, 28.0), egui::Button::new(tl!("Stop")));
                elems.push(("claude.stop".into(), b.rect, "Stop".into()));
                stop = b.clicked();
            } else {
                let b = ui.add_enabled(
                    !app.claude.input.trim().is_empty() && app.hooks.claude.is_some(),
                    egui::Button::new(RichText::new(tl!("Send")).color(Color32::WHITE)).fill(t.accent).min_size(vec2(66.0, 28.0)),
                );
                elems.push(("claude.send".into(), b.rect, "Send".into()));
                if b.clicked() || enter {
                    to_send = Some((app.claude.input.clone(), false));
                }
            }
            let mut on = *app.claude.listen.get_or_insert_with(|| filmcraft_engine::studio::claude_settings().listen);
            let c = ui
                .checkbox(&mut on, RichText::new(tl!("Desktop")).size(10.5))
                .on_hover_text(tl!("Let Claude Desktop and Claude Code connect whenever FilmCraft is open"));
            elems.push(("claude.listen".into(), c.rect, "Desktop".into()));
            if c.changed() {
                listen_toggle = Some(on);
            }
        });
    });
    let used = row.response.rect.height() + 4.0;
    if (used - input_h).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(input_key, used));
        ui.ctx().request_repaint();
    }

    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if (model.clone(), effort.clone()) != chosen {
        set_prefs(app, &model, &effort);
    }
    if recheck {
        app.claude.checked = app.hooks.claude.as_mut().map(|h| h.check());
    }
    if new_chat {
        app.claude.items.clear();
        app.claude.session = None;
    }
    if stop && let Some(h) = app.hooks.claude.as_mut() {
        h.stop();
    }
    if let Some(on) = listen_toggle {
        app.claude.listen = Some(on);
        let port = ensure_listening(app);
        let saved = filmcraft_engine::studio::save_claude_settings(&filmcraft_engine::studio::ClaudeSettings {
            listen: on,
            port,
            ..filmcraft_engine::studio::claude_settings()
        });
        if let Err(e) = saved {
            app.claude.items.push(ChatItem::Error(e));
        }
    }
    if let Some((text, quick)) = to_send {
        if !quick {
            app.claude.input.clear();
        }
        send(app, &text, quick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A host that records what it was asked and replays scripted events.
    struct Fake {
        sent: Arc<Mutex<Vec<(String, Option<String>, u16, ClaudeOptions)>>>,
        events: Vec<ClaudeEvent>,
    }

    impl ClaudeHost for Fake {
        fn check(&mut self) -> Result<String, String> {
            Ok("2.1.0".into())
        }
        fn send(&mut self, text: &str, session: Option<&str>, port: u16, opts: &ClaudeOptions) -> Result<(), String> {
            self.sent.lock().unwrap().push((text.into(), session.map(str::to_string), port, opts.clone()));
            Ok(())
        }
        fn poll(&mut self) -> Vec<ClaudeEvent> {
            std::mem::take(&mut self.events)
        }
        fn stop(&mut self) {}
    }

    #[test]
    fn a_turn_streams_text_and_tool_calls_and_keeps_the_session() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let sent = Arc::new(Mutex::new(Vec::new()));
        app.hooks.claude = Some(Box::new(Fake {
            sent: sent.clone(),
            events: vec![
                ClaudeEvent::Session("abc".into()),
                ClaudeEvent::Text("Cutting silences.".into()),
                ClaudeEvent::Tool { name: "command_run".into(), detail: "studio.cutSilences".into() },
                ClaudeEvent::ToolDone { ok: true, detail: String::new() },
                ClaudeEvent::Text("Done: −12 s.".into()),
                ClaudeEvent::Done { error: None },
            ],
        }));
        app.claude.port = 9999;
        app.claude.options = Some((String::new(), String::new()));
        send(&mut app, "  cut the silences ", false);
        assert!(app.claude.busy);
        pump(&mut app, &egui::Context::default());
        assert!(!app.claude.busy);
        assert_eq!(app.claude.session.as_deref(), Some("abc"));
        assert_eq!(
            app.claude.items,
            vec![
                ChatItem::User("cut the silences".into()),
                ChatItem::Claude("Cutting silences.".into()),
                ChatItem::Tool("command_run".into(), "studio.cutSilences".into(), Some(true)),
                ChatItem::Claude("Done: −12 s.".into()),
            ]
        );
        // the next message continues the conversation; quick requests ask for a reply language
        send(&mut app, QUICK[2].2, true);
        let s = sent.lock().unwrap();
        assert_eq!(s[0], ("cut the silences".into(), None, 9999, ClaudeOptions::default()));
        assert_eq!(s[1].1.as_deref(), Some("abc"));
        assert!(s[1].0.contains("Reply in"), "{}", s[1].0);
    }

    #[test]
    fn the_chosen_model_and_effort_go_with_the_message() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let sent = Arc::new(Mutex::new(Vec::new()));
        app.hooks.claude = Some(Box::new(Fake { sent: sent.clone(), events: vec![] }));
        app.claude.port = 9999;
        app.claude.options = Some(("opus".into(), "xhigh".into()));
        send(&mut app, "make it a TikTok", false);
        let s = sent.lock().unwrap();
        assert_eq!(s[0].3, ClaudeOptions { model: Some("opus".into()), effort: Some("xhigh".into()) });
        // only what the panel offers: anything else (an edited settings file) is the default
        assert_eq!(ClaudeOptions::new("--dangerously-skip-permissions", "ultra"), ClaudeOptions::default());
        assert_eq!(ClaudeOptions::new("", ""), ClaudeOptions::default());
        assert!(MODELS.iter().chain(EFFORTS).all(|(id, _)| !id.starts_with('-') && id.chars().all(|c| c.is_ascii_lowercase())));
    }

    #[test]
    fn errors_and_empty_messages() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        send(&mut app, "hello", false);
        assert!(matches!(app.claude.items.last(), Some(ChatItem::Error(_))), "no host: an explanation, no panic");
        let n = app.claude.items.len();
        send(&mut app, "   ", false);
        assert_eq!(app.claude.items.len(), n);
        assert_eq!(plain("## Done\n- **12** cuts\n  * `studio.title`"), "Done\n• 12 cuts\n  • studio.title");
    }
}
