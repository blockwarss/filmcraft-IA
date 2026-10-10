//! Headless UI test of the Claude panel's message box: a long message grows the box up to about
//! eight lines, then it scrolls inside, so what you type never slides out of the panel.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use filmcraft_ui_egui::panels::claude::{ClaudeEvent, ClaudeHost, ClaudeOptions};
use serde_json::{Value, json};

const H: f32 = 980.0;

/// A Claude Code that is installed and never answers (the menus are only live with a host).
struct Idle;

impl ClaudeHost for Idle {
    fn check(&mut self) -> Result<String, String> {
        Ok("2.1.0".into())
    }
    fn send(&mut self, _: &str, _: Option<&str>, _: u16, _: &ClaudeOptions) -> Result<(), String> {
        Ok(())
    }
    fn poll(&mut self) -> Vec<ClaudeEvent> {
        Vec::new()
    }
    fn stop(&mut self) {}
}

fn frames(h: &mut Harness<'static, FilmcraftApp>, n: usize) {
    for _ in 0..n {
        let ctx = h.ctx.clone();
        let mut raw = std::mem::take(h.input_mut());
        eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut raw);
        *h.input_mut() = raw;
        h.step();
    }
}

fn call(h: &mut Harness<'static, FilmcraftApp>, tx: &Sender<ControlRequest>, method: &str, params: Value) -> Value {
    let (req, reply) = ControlRequest::new(method, params.clone());
    tx.send(req).unwrap();
    for _ in 0..600 {
        frames(h, 1);
        if let Ok(v) = reply.try_recv() {
            assert_eq!(v["ok"], json!(true), "{method} {params}: {v}");
            return v["result"].clone();
        }
    }
    panic!("no reply to {method}");
}

fn rect(h: &mut Harness<'static, FilmcraftApp>, tx: &Sender<ControlRequest>, id: &str) -> [f32; 4] {
    let v = call(h, tx, "ui.elements", json!({"prefix": id}));
    let e = v.as_array().and_then(|a| a.iter().find(|e| e["id"] == id).cloned()).unwrap_or_else(|| panic!("no {id} in {v}"));
    let r = &e["rect"];
    [0, 1, 2, 3].map(|i| r[i].as_f64().unwrap() as f32)
}

#[test]
fn a_long_message_scrolls_inside_the_box() {
    let mut session = Session::default();
    session.execute("file.openDemoProject", json!({})).unwrap();
    let (tx, rx) = channel();
    let app = FilmcraftApp::new(session).with_control(rx);
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, H)).with_max_steps(10_000).build_eframe(move |_cc| app);
    frames(&mut h, 4);
    call(&mut h, &tx, "ui.panel.show", json!({"panel": "Claude"}));
    frames(&mut h, 3);
    let short = rect(&mut h, &tx, "claude.input");
    let send = rect(&mut h, &tx, "claude.send");
    h.state_mut().claude.input = (1..=15).map(|i| format!("line {i} of a long brief")).collect::<Vec<_>>().join("\n");
    frames(&mut h, 4);
    let long = rect(&mut h, &tx, "claude.input");
    // [x, y, w, h] in points: the box grew, but only to about eight lines, and stays on screen
    // with the Send button beside it
    assert!(long[3] > short[3], "the box grows with the message: {short:?} → {long:?}");
    assert!(long[3] <= 160.0, "then it scrolls instead of growing: {long:?}");
    assert!(long[1] + long[3] <= H, "the box stays in the window: {long:?}");
    let send2 = rect(&mut h, &tx, "claude.send");
    assert!(send2[1] + send2[3] <= H && (send2[0] - send[0]).abs() < 1.0, "{send:?} → {send2:?}");
}

#[test]
fn model_and_effort_are_chosen_in_the_panel_and_remembered() {
    // claude.json must go to a scratch data folder, not the user's: the test runs itself again in
    // a child process with FILMCRAFT_DATA_DIR set (setting it here would need `unsafe`)
    const CHILD: &str = "FILMCRAFT_CLAUDE_UI_CHILD";
    let Some(data) = std::env::var_os(CHILD).map(std::path::PathBuf::from) else {
        let data = std::env::temp_dir().join(format!("filmcraft-claude-ui-{}", std::process::id()));
        std::fs::create_dir_all(&data).unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["model_and_effort_are_chosen_in_the_panel_and_remembered", "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD, &data)
            .env("FILMCRAFT_DATA_DIR", &data)
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&data);
        assert!(out.status.success(), "{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 passed"), "the child ran the test");
        return;
    };
    let mut session = Session::default();
    session.execute("file.openDemoProject", json!({})).unwrap();
    let (tx, rx) = channel();
    let mut app = FilmcraftApp::new(session).with_control(rx);
    app.hooks.claude = Some(Box::new(Idle));
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, H)).with_max_steps(10_000).build_eframe(move |_cc| app);
    frames(&mut h, 4);
    call(&mut h, &tx, "ui.panel.show", json!({"panel": "Claude"}));
    frames(&mut h, 3);
    assert_eq!(filmcraft_ui_egui::panels::claude::prefs(h.state_mut()), (String::new(), String::new()), "Claude Code's defaults at first");
    for (combo, option) in [("claude.model", "claude.model.option.opus"), ("claude.effort", "claude.effort.option.high")] {
        call(&mut h, &tx, "ui.click", json!({"id": combo}));
        frames(&mut h, 3);
        call(&mut h, &tx, "ui.click", json!({"id": option}));
        frames(&mut h, 3);
    }
    assert_eq!(h.state_mut().claude.options, Some(("opus".into(), "high".into())));
    let saved: Value = serde_json::from_slice(&std::fs::read(data.join("claude.json")).unwrap()).unwrap();
    assert_eq!((saved["model"].as_str(), saved["effort"].as_str()), (Some("opus"), Some("high")), "{saved}");
}
