//! The Claude panel's host: runs the user's own Claude Code (`claude -p … --output-format
//! stream-json`, signed in with their Claude plan — no API key) with FilmCraft's MCP server
//! (`filmcraft-cli mcp --bridge`) pointed at this window's control port, and turns its stream into
//! panel events. Each message is one `claude -p` run; `--resume` continues the conversation.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};

use filmcraft_ui_egui::panels::claude::{ClaudeEvent, ClaudeHost};
use serde_json::{Value, json};

/// What Claude is told about where it runs (added to Claude Code's own system prompt).
const SYSTEM: &str = "You are the video editor working inside FilmCraft's Claude panel. The user watches the FilmCraft window; your edits happen live in it and every change can be undone. Use the filmcraft MCP tools: in a new conversation call studio_overview first and follow the editor playbook (resource filmcraft://guide). Look at your work (render_preview / ui_screenshot) before saying it is done. Keep replies short and friendly: what you did, what you suggest next. Reply in the user's language.";

pub struct ClaudeCode {
    child: Option<Arc<Mutex<Child>>>,
    rx: Option<Receiver<ClaudeEvent>>,
}

impl ClaudeCode {
    pub fn new() -> Self {
        ClaudeCode { child: None, rx: None }
    }
}

/// Where `claude` usually lives: PATH first, then the installers' folders (apps started from the
/// Dock or Start menu don't get the shell's PATH).
fn find_claude() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["claude.exe", "claude.cmd"] } else { &["claude"] };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend(extra_dirs());
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

fn extra_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    let mut v: Vec<PathBuf> = Vec::new();
    if let Some(h) = &home {
        v.push(h.join(".claude/local"));
        v.push(h.join(".local/bin"));
        v.push(h.join(".npm-global/bin"));
        v.push(h.join(".bun/bin"));
    }
    if cfg!(windows) {
        if let Some(a) = std::env::var_os("APPDATA") {
            v.push(PathBuf::from(a).join("npm"));
        }
    } else {
        v.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from));
    }
    v
}

/// PATH for the child: the user's plus the installers' folders (Claude Code's launcher may need
/// `node` from them).
fn child_path() -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for d in extra_dirs() {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

/// `filmcraft-cli` next to this program.
fn find_cli() -> Option<PathBuf> {
    let name = if cfg!(windows) { "filmcraft-cli.exe" } else { "filmcraft-cli" };
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let p = dir.join(name);
    p.is_file().then_some(p)
}

/// The folder Claude Code runs in (so it doesn't pick up an unrelated project's settings).
fn work_dir() -> PathBuf {
    let d = filmcraft_engine::studio::library_dir().map(|d| d.join("claude")).unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Arguments of one `claude -p` run (the message itself goes to stdin).
pub fn claude_args(cli: &Path, port: u16, session: Option<&str>) -> Vec<String> {
    let mcp = json!({"mcpServers": {"filmcraft": {"command": cli.to_string_lossy(), "args": ["mcp", "--bridge", format!("127.0.0.1:{port}")]}}});
    let mut a: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--mcp-config".into(),
        mcp.to_string(),
        "--strict-mcp-config".into(),
        "--allowedTools".into(),
        "mcp__filmcraft".into(),
        "WebSearch".into(),
        "--append-system-prompt".into(),
        SYSTEM.into(),
    ];
    // a session id is a UUID: never something that could read as an option
    if let Some(s) = session.filter(|s| (8..=64).contains(&s.len()) && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')) {
        a.push("--resume".into());
        a.push(s.into());
    }
    a
}

fn short(v: &Value, max: usize) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let s = s.replace('\n', " ");
    if s.chars().count() > max { format!("{}…", s.chars().take(max).collect::<String>()) } else { s }
}

/// What a tool call is about, in a few words.
fn tool_detail(name: &str, input: &Value) -> String {
    match name {
        "command_run" => {
            let p = input.get("params").filter(|p| p.as_object().is_some_and(|o| !o.is_empty()));
            format!("{} {}", short(&input["id"], 60), p.map(|p| short(p, 70)).unwrap_or_default()).trim().to_string()
        }
        "command_batch" => {
            let steps: Vec<String> = input["steps"].as_array().into_iter().flatten().filter_map(|s| s["id"].as_str().map(str::to_string)).collect();
            format!("{} steps: {}", steps.len(), short(&Value::String(steps.join(", ")), 80))
        }
        _ => short(input, 80),
    }
}

/// Reads Claude Code's `stream-json` output; remembers the tool calls it hides so their results
/// are hidden too.
#[derive(Default)]
pub struct Parser {
    hidden: std::collections::HashSet<String>,
}

/// The events of one line of Claude Code's `stream-json` output (stateless; tests).
#[cfg(test)]
pub fn parse_line(line: &str) -> Vec<ClaudeEvent> {
    Parser::default().parse(line)
}

impl Parser {
    pub fn parse(&mut self, line: &str) -> Vec<ClaudeEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return vec![] };
        let mut out = Vec::new();
        match v["type"].as_str() {
            Some("system") if v["subtype"] == "init" => {
                if let Some(id) = v["session_id"].as_str() {
                    out.push(ClaudeEvent::Session(id.into()));
                }
            }
            Some("assistant") => {
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => {
                            if let Some(t) = b["text"].as_str().filter(|t| !t.trim().is_empty()) {
                                out.push(ClaudeEvent::Text(t.trim().into()));
                            }
                        }
                        // Claude Code's own plumbing (loading tool schemas, to-do lists) isn't editing
                        Some("tool_use") if matches!(b["name"].as_str(), Some("ToolSearch" | "TodoWrite")) => {
                            if let Some(id) = b["id"].as_str().filter(|_| self.hidden.len() < 10_000) {
                                self.hidden.insert(id.to_string());
                            }
                        }
                        Some("tool_use") => {
                            let full = b["name"].as_str().unwrap_or("tool");
                            let name = full.strip_prefix("mcp__filmcraft__").unwrap_or(full);
                            out.push(ClaudeEvent::Tool { name: name.into(), detail: tool_detail(name, &b["input"]) });
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for b in v["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    if b["tool_use_id"].as_str().is_some_and(|id| self.hidden.remove(id)) {
                        continue;
                    }
                    let failed = b["is_error"].as_bool() == Some(true);
                    let text = match &b["content"] {
                        Value::Array(a) => a.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join(" "),
                        other => short(other, 400),
                    };
                    out.push(ClaudeEvent::ToolDone { ok: !failed, detail: if failed { short(&Value::String(text), 140) } else { String::new() } });
                }
            }
            Some("result") => {
                let failed = v["is_error"].as_bool() == Some(true) || v["subtype"].as_str().is_some_and(|s| s != "success");
                let error = failed.then(|| {
                    let msg =
                        v["result"].as_str().map(str::to_string).unwrap_or_else(|| format!("Claude stopped ({})", v["subtype"].as_str().unwrap_or("error")));
                    hint(&msg)
                });
                out.push(ClaudeEvent::Done { error });
            }
            _ => {}
        }
        out
    }
}

/// Add what to do to the errors people hit first.
fn hint(msg: &str) -> String {
    let l = msg.to_ascii_lowercase();
    if l.contains("login") || l.contains("api key") || l.contains("authenticat") || l.contains("not logged") {
        format!("{msg}\nOpen a terminal, run `claude` once and sign in with your Claude account (Pro or Max plan), then try again.")
    } else if l.contains("rate limit") || l.contains("usage limit") {
        format!("{msg}\nYour Claude plan's usage limit was reached; it resets after a while.")
    } else {
        msg.to_string()
    }
}

fn run(child: Arc<Mutex<Child>>, stdout: impl Read, stderr: impl Read + Send + 'static, tx: Sender<ClaudeEvent>) {
    let err_tail: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let tail = err_tail.clone();
    let err_thread = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let mut t = tail.lock().unwrap_or_else(PoisonError::into_inner);
            t.push(line);
            if t.len() > 20 {
                t.remove(0);
            }
        }
    });
    let mut done = false;
    let mut parser = Parser::default();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        for e in parser.parse(&line) {
            done |= matches!(e, ClaudeEvent::Done { .. });
            if tx.send(e).is_err() {
                return;
            }
        }
    }
    let _ = err_thread.join();
    let status = child.lock().unwrap_or_else(PoisonError::into_inner).wait();
    if !done {
        let tail = err_tail.lock().unwrap_or_else(PoisonError::into_inner).join("\n");
        let msg = match status {
            Ok(s) if s.success() => "Claude Code ended without an answer".to_string(),
            Ok(_) | Err(_) if !tail.trim().is_empty() => hint(tail.trim()),
            _ => "Claude Code stopped".to_string(),
        };
        let _ = tx.send(ClaudeEvent::Done { error: Some(msg) });
    }
}

impl ClaudeHost for ClaudeCode {
    fn check(&mut self) -> Result<String, String> {
        let claude = find_claude().ok_or_else(|| {
            "Claude Code is not installed. Install it from claude.com/code (it works with your Claude Pro or Max plan, no API key), run `claude` once in a terminal to sign in, then click Check Again.".to_string()
        })?;
        let out =
            Command::new(&claude).arg("--version").env("PATH", child_path()).stdin(Stdio::null()).output().map_err(|e| format!("{}: {e}", claude.display()))?;
        let v = String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or("").to_string();
        if find_cli().is_none() {
            return Err("filmcraft-cli was not found next to FilmCraft; reinstall FilmCraft to use the Claude panel.".into());
        }
        Ok(v)
    }

    fn send(&mut self, text: &str, session: Option<&str>, port: u16) -> Result<(), String> {
        if self.child.is_some() && self.rx.is_some() {
            return Err("Claude is still working on the previous message".into());
        }
        let claude = find_claude().ok_or("Claude Code is not installed (claude.com/code)")?;
        let cli = find_cli().ok_or("filmcraft-cli was not found next to FilmCraft")?;
        let mut child = Command::new(&claude)
            .args(claude_args(&cli, port, session))
            .current_dir(work_dir())
            .env("PATH", child_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start Claude Code ({}): {e}", claude.display()))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes()).map_err(|e| format!("Claude Code: {e}"))?;
        }
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            return Err("Claude Code: no output stream".into());
        };
        let child = Arc::new(Mutex::new(child));
        let (tx, rx) = channel();
        let c = child.clone();
        let fallback = tx.clone();
        std::thread::Builder::new()
            .name("claude-panel".into())
            .spawn(move || {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(c, stdout, stderr, tx))).is_err() {
                    let _ = fallback.send(ClaudeEvent::Done { error: Some("internal error while reading Claude Code".into()) });
                }
            })
            .map_err(|e| e.to_string())?;
        self.child = Some(child);
        self.rx = Some(rx);
        Ok(())
    }

    fn poll(&mut self) -> Vec<ClaudeEvent> {
        let Some(rx) = &self.rx else { return vec![] };
        let mut out = Vec::new();
        let mut finished = false;
        loop {
            match rx.try_recv() {
                Ok(e) => {
                    finished |= matches!(e, ClaudeEvent::Done { .. });
                    out.push(e);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if !finished {
                        out.push(ClaudeEvent::Done { error: Some("Claude Code stopped".into()) });
                    }
                    finished = true;
                    break;
                }
            }
        }
        if finished {
            self.rx = None;
            self.child = None;
        }
        out
    }

    fn stop(&mut self) {
        if let Some(c) = &self.child {
            let _ = c.lock().unwrap_or_else(PoisonError::into_inner).kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_json_becomes_panel_events() {
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"5f1c-9a"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"On coupe les silences."},{"type":"tool_use","name":"mcp__filmcraft__command_run","input":{"id":"studio.cutSilences","params":{"minSeconds":0.4}}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","is_error":false,"content":[{"type":"text","text":"{}"}]}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","is_error":true,"content":"no sequence"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"ok"}"#,
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key · Please run /login"}"#,
            "not json",
            r#"{"type":"assistant","message":{"content":"weird"}}"#,
        ];
        let ev: Vec<ClaudeEvent> = lines.iter().flat_map(|l| parse_line(l)).collect();
        assert_eq!(ev[0], ClaudeEvent::Session("5f1c-9a".into()));
        assert_eq!(ev[1], ClaudeEvent::Text("On coupe les silences.".into()));
        assert_eq!(ev[2], ClaudeEvent::Tool { name: "command_run".into(), detail: r#"studio.cutSilences {"minSeconds":0.4}"#.into() });
        assert_eq!(ev[3], ClaudeEvent::ToolDone { ok: true, detail: String::new() });
        assert_eq!(ev[4], ClaudeEvent::ToolDone { ok: false, detail: "no sequence".into() });
        assert_eq!(ev[5], ClaudeEvent::Done { error: None });
        assert!(matches!(&ev[6], ClaudeEvent::Done { error: Some(e) } if e.contains("sign in")));
        assert_eq!(ev.len(), 7);
        // Claude Code's tool loading is hidden, and so is its result
        let mut p = Parser::default();
        let hidden = p.parse(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"ToolSearch","input":{}}]}}"#);
        let res = p.parse(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"x"}]}}"#);
        assert!(hidden.is_empty() && res.is_empty());
    }

    #[test]
    fn arguments_bridge_the_window_and_resume_safely() {
        let a = claude_args(Path::new("/x/filmcraft-cli"), 9876, Some("5f1c9a2e-77d0-4b1e"));
        let mcp: Value = serde_json::from_str(&a[a.iter().position(|x| x == "--mcp-config").unwrap() + 1]).unwrap();
        assert_eq!(mcp["mcpServers"]["filmcraft"]["args"], json!(["mcp", "--bridge", "127.0.0.1:9876"]));
        assert_eq!(a.last().map(String::as_str), Some("5f1c9a2e-77d0-4b1e"));
        // a session id that isn't one is never passed on
        for bad in ["--dangerously-skip-permissions", "-p", "abc", "deadbeef; rm -rf /", ""] {
            assert!(!claude_args(Path::new("/x"), 1, Some(bad)).iter().any(|x| x == "--resume"), "{bad}");
        }
        assert!(a.contains(&"--strict-mcp-config".to_string()) && !a.iter().any(|x| x.contains("dangerously")));
    }
}
