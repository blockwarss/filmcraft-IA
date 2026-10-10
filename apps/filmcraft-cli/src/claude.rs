//! `filmcraft-cli claude …`: connect FilmCraft to Claude (Claude Desktop and Claude Code) in one
//! step, so people use it with their Claude subscription — no API key.
//!
//! Both clients start `filmcraft-cli mcp --bridge 127.0.0.1:<port> --launch`: the MCP server
//! drives the running desktop app and opens it when it isn't running.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// Name of the MCP server in the clients' configuration.
pub const SERVER: &str = "filmcraft";
pub const DEFAULT_PORT: u16 = 9876;

/// The arguments the clients run `filmcraft-cli` with.
pub fn server_args(port: u16) -> Vec<String> {
    vec!["mcp".into(), "--bridge".into(), format!("127.0.0.1:{port}"), "--launch".into()]
}

/// Claude Desktop's configuration file on this platform.
pub fn desktop_config_path() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support/Claude/claude_desktop_config.json"))
    } else if cfg!(windows) {
        env("APPDATA").map(|a| a.join("Claude").join("claude_desktop_config.json"))
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config"))).map(|c| c.join("Claude").join("claude_desktop_config.json"))
    }
}

/// `config` with the FilmCraft server added (or replaced), every other setting kept. Fails on a
/// file that isn't a JSON object rather than overwriting someone's configuration.
pub fn merge_desktop_config(config: Option<&str>, exe: &Path, port: u16) -> Result<Value, String> {
    let mut v: Value = match config.map(str::trim).filter(|c| !c.is_empty()) {
        None => json!({}),
        Some(c) => serde_json::from_str(c).map_err(|e| format!("the Claude Desktop configuration is not valid JSON ({e}); fix or remove it first"))?,
    };
    let obj = v.as_object_mut().ok_or("the Claude Desktop configuration is not a JSON object")?;
    let servers = obj.entry("mcpServers").or_insert_with(|| json!({}));
    let servers = servers.as_object_mut().ok_or("`mcpServers` in the Claude Desktop configuration is not an object")?;
    servers.insert(SERVER.into(), json!({"command": exe.to_string_lossy(), "args": server_args(port)}));
    Ok(v)
}

/// `config` without the FilmCraft server (`None` when it wasn't there).
pub fn remove_from_desktop_config(config: &str) -> Option<Value> {
    let mut v: Value = serde_json::from_str(config).ok()?;
    v.get_mut("mcpServers")?.as_object_mut()?.remove(SERVER)?;
    Some(v)
}

/// The desktop app next to this executable, installed in the usual places, or on PATH.
/// `FILMCRAFT_APP` overrides.
pub fn find_app() -> Option<PathBuf> {
    let exe_name = if cfg!(windows) { "filmcraft.exe" } else { "filmcraft" };
    if let Some(p) = std::env::var_os("FILMCRAFT_APP").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join(exe_name));
    }
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from("/Applications/FilmCraft.app/Contents/MacOS/filmcraft"));
        if let Some(h) = env("HOME") {
            candidates.push(h.join("Applications/FilmCraft.app/Contents/MacOS/filmcraft"));
        }
    } else if cfg!(windows) {
        for k in ["LOCALAPPDATA", "ProgramFiles"] {
            if let Some(d) = env(k) {
                candidates.push(d.join("Programs").join("FilmCraft").join(exe_name));
                candidates.push(d.join("FilmCraft").join(exe_name));
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join(exe_name)));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Find `claude` (Claude Code) on PATH.
pub fn find_claude_code() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["claude.exe", "claude.cmd"] } else { &["claude"] };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

/// The `claude mcp add` command line for Claude Code.
pub fn code_add_args(exe: &Path, port: u16) -> Vec<String> {
    let mut a = vec!["mcp".into(), "add".into(), "--scope".into(), "user".into(), SERVER.into(), "--".into(), exe.to_string_lossy().into_owned()];
    a.extend(server_args(port));
    a
}

/// Install into Claude Desktop: returns what was done.
pub fn install_desktop(exe: &Path, port: u16, dry_run: bool) -> Result<String, String> {
    let path = desktop_config_path().ok_or("cannot find the Claude Desktop configuration folder (no HOME / APPDATA)")?;
    let old = std::fs::read_to_string(&path).ok();
    let merged = merge_desktop_config(old.as_deref(), exe, port)?;
    let text = serde_json::to_string_pretty(&merged).map_err(|e| e.to_string())?;
    if dry_run {
        return Ok(format!("would write {}:\n{text}", path.display()));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if let Some(o) = &old {
        let bak = path.with_extension("json.bak");
        std::fs::write(&bak, o).map_err(|e| format!("{}: {e}", bak.display()))?;
    }
    std::fs::write(&path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    listen(port)?;
    Ok(format!("Claude Desktop: added the \"{SERVER}\" server to {} — quit and reopen Claude Desktop", path.display()))
}

/// Make the desktop app open its control port for Claude whenever it starts (so Claude reaches a
/// FilmCraft the user opened by hand instead of starting a second one).
pub fn listen(port: u16) -> Result<(), String> {
    filmcraft_engine::studio::save_claude_settings(&filmcraft_engine::studio::ClaudeSettings { listen: true, port }).map(|_| ())
}

pub fn uninstall_desktop() -> Result<String, String> {
    let path = desktop_config_path().ok_or("no Claude Desktop configuration folder")?;
    let Ok(old) = std::fs::read_to_string(&path) else { return Ok("Claude Desktop: nothing to remove".into()) };
    match remove_from_desktop_config(&old) {
        None => Ok("Claude Desktop: FilmCraft was not configured".into()),
        Some(v) => {
            std::fs::write(&path, serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(format!("Claude Desktop: removed FilmCraft from {}", path.display()))
        }
    }
}

/// Install into Claude Code with `claude mcp add` (replacing an older registration).
pub fn install_code(exe: &Path, port: u16, dry_run: bool) -> Result<String, String> {
    let args = code_add_args(exe, port);
    let Some(claude) = find_claude_code() else {
        return Err(format!("Claude Code (`claude`) is not on PATH; install it, or run:\n  claude {}", args.join(" ")));
    };
    if dry_run {
        return Ok(format!("would run: {} {}", claude.display(), args.join(" ")));
    }
    let _ = std::process::Command::new(&claude).args(["mcp", "remove", "--scope", "user", SERVER]).output();
    let out = std::process::Command::new(&claude).args(&args).output().map_err(|e| format!("{}: {e}", claude.display()))?;
    if !out.status.success() {
        return Err(format!("`claude mcp add` failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    listen(port)?;
    Ok("Claude Code: added the \"filmcraft\" MCP server (user scope). For the editor skill and the /filmcraft:… commands, in Claude Code run:\n  /plugin install filmcraft --marketplace blockwarss/filmcraft-IA".into())
}

pub fn uninstall_code() -> Result<String, String> {
    let claude = find_claude_code().ok_or("Claude Code (`claude`) is not on PATH")?;
    let out = std::process::Command::new(&claude).args(["mcp", "remove", "--scope", "user", SERVER]).output().map_err(|e| e.to_string())?;
    Ok(if out.status.success() { "Claude Code: removed FilmCraft".into() } else { "Claude Code: FilmCraft was not configured".into() })
}

/// What is set up, as JSON.
pub fn status(port: u16) -> Value {
    let desktop = desktop_config_path();
    let configured = desktop
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|v| v["mcpServers"][SERVER].clone())
        .filter(|v| !v.is_null());
    let reachable = std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port)), std::time::Duration::from_millis(400)).is_ok();
    json!({
        "claudeDesktop": {"config": desktop.map(|p| p.to_string_lossy().into_owned()), "filmcraft": configured},
        "claudeCode": {"cli": find_claude_code().map(|p| p.to_string_lossy().into_owned())},
        "app": find_app().map(|p| p.to_string_lossy().into_owned()),
        "appListening": reachable,
        "port": port,
        "appListensAtStartup": filmcraft_engine::studio::claude_settings().listen,
        "studioFolder": filmcraft_engine::studio::library_dir().map(|p| p.to_string_lossy().into_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_config_is_merged_never_clobbered() {
        let exe = Path::new("/opt/filmcraft/filmcraft-cli");
        let v = merge_desktop_config(None, exe, 9876).unwrap();
        assert_eq!(v["mcpServers"]["filmcraft"]["args"], json!(["mcp", "--bridge", "127.0.0.1:9876", "--launch"]));
        let old = r#"{"theme": "dark", "mcpServers": {"other": {"command": "x"}, "filmcraft": {"command": "old"}}}"#;
        let v = merge_desktop_config(Some(old), exe, 7000).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert_eq!(v["mcpServers"]["filmcraft"]["command"], "/opt/filmcraft/filmcraft-cli");
        assert_eq!(v["mcpServers"]["filmcraft"]["args"][2], "127.0.0.1:7000");
        assert!(merge_desktop_config(Some("{broken"), exe, 1).is_err());
        assert!(merge_desktop_config(Some("[1, 2]"), exe, 1).is_err());
        assert!(merge_desktop_config(Some(r#"{"mcpServers": 3}"#), exe, 1).is_err());
        assert_eq!(merge_desktop_config(Some("  "), exe, 1).unwrap()["mcpServers"]["filmcraft"]["command"], "/opt/filmcraft/filmcraft-cli");
        let removed = remove_from_desktop_config(old).unwrap();
        assert!(removed["mcpServers"].get("filmcraft").is_none() && removed["mcpServers"]["other"].is_object());
        assert!(remove_from_desktop_config(r#"{"mcpServers": {}}"#).is_none());
    }

    #[test]
    fn claude_code_command_line() {
        let a = code_add_args(Path::new("/x/filmcraft-cli"), 9876);
        assert_eq!(a.join(" "), "mcp add --scope user filmcraft -- /x/filmcraft-cli mcp --bridge 127.0.0.1:9876 --launch");
    }
}
