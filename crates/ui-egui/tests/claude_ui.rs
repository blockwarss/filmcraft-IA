//! Headless UI test of the Claude panel's message box: a long message grows the box up to about
//! eight lines, then it scrolls inside, so what you type never slides out of the panel.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

const H: f32 = 980.0;

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
