//! Motion-gate scenario for the model list's efforts flyout (p169): the pointer rests on a row
//! and its efforts open, moves to the next row and they follow at once, crosses a row on its
//! way into them, wheels the list, then the arrow keys close them.

use crate::pickers::PickerHost as _;
use rune::{Key, KeyEvent, Mods, Runtime};

use super::Run;
use crate::App;
use crate::pickers::Kind;

/// The model rows wholly in the list's view, top to bottom.
fn rows(rt: &Runtime<App>) -> Vec<[f32; 4]> {
    let Some(view) = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("picker-scroll")).map(|e| e.rect) else { return vec![] };
    let mut rows: Vec<_> = rt.elements().into_iter()
        .filter(|e| !e.exiting && e.key.as_deref().is_some_and(|k| k.starts_with("option-")))
        .map(|e| e.rect)
        .filter(|r| r[1] >= view[1] && r[1] + r[3] <= view[1] + view[3])
        .collect();
    rows.sort_by(|a, b| a[1].total_cmp(&b[1]));
    rows
}

fn on(r: [f32; 4]) -> (f32, f32) {
    (r[0] + 40.0, r[1] + r[3] / 2.0)
}

pub fn run(r: &mut Run) {
    let efforts = serde_json::json!([{"id": "high"}, {"id": "xhigh"}]);
    let model = |id: &str, provider: &str| serde_json::json!({"id": id, "providerID": provider, "name": id, "variants": efforts, "cost": [{"input": 3.0}]});
    let mut models = vec![model("opus", "opencode"), model("sonnet", "opencode")];
    for p in ["github-copilot", "openai", "xai", "mistral", "groq", "deepseek"] {
        models.push(model(&format!("m-{p}"), p));
    }
    r.rt.state.pickers.load_live(&serde_json::Value::Array(models), &serde_json::json!([{"id": "build"}]));
    r.rt.state.pickers.prefs = Default::default();
    r.act("open the model list", |rt| rt.update(|s, _| s.open_picker(Kind::Model)));
    r.play(0.4);
    let rows = rows(&r.rt);
    assert!(rows.len() >= 4, "the list shows rows: {rows:?}");
    let (x, y) = on(rows[1]);
    r.act("rest on a row", |rt| rt.pointer_move(x, y));
    r.play(0.3);
    assert!(r.rt.state.pickers.flyout.is_some(), "its efforts open");
    let (x2, y2) = on(rows[2]);
    r.act("the next row", |rt| rt.pointer_move(x2, y2));
    r.play(0.2);
    let fly = r.rt.elements().into_iter().find(|e| e.key.as_deref() == Some("effort-flyout")).map(|e| e.rect).expect("the efforts");
    // Straight into the efforts' far half, over the rows below on the way.
    let to = (if fly[0] > x2 { fly[0] + 20.0 } else { fly[0] + fly[2] - 20.0 }, fly[1] + fly[3] - 16.0);
    for t in [0.25f32, 0.5, 0.75, 1.0] {
        let (x, y) = (x2 + (to.0 - x2) * t, y2 + (to.1 - y2) * t);
        r.act("toward the efforts", |rt| rt.pointer_move(x, y));
    }
    assert!(r.rt.state.pickers.flyout.is_some(), "the efforts stay open on the way in");
    r.play(0.3);
    r.act("back on the row", |rt| rt.pointer_move(x2, y2));
    r.play(0.2);
    for _ in 0..4 {
        r.act("wheel the list", |rt| rt.wheel_xy(x2, y2, 0.0, -12.0));
        r.play(0.1);
    }
    r.play(0.3);
    r.act("↓", |rt| rt.key(KeyEvent { key: Key::Down, mods: Mods::default() }));
    r.play(0.4);
    assert!(r.rt.state.pickers.flyout.is_none(), "the keys close the efforts");
    r.act("Esc", |rt| rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }));
    r.play(0.5);
}
