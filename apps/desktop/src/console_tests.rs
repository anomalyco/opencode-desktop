//! An open shell's console (t1461): its sections sit inside its ring, so no fill covers the
//! ring's corners, and each scrolls on its own past its cap with its text inset all round.

use rune::{ElementInfo, Runtime};

use crate::App;

const DT: f64 = 1.0 / 60.0;
/// The output's cap (`tools::console::MAX_H`).
const MAX_H: f32 = 240.0;

fn scene(name: &str) -> Runtime<App> {
    let mut app = App::fixture_session();
    let (_, setup) = crate::scenes::all().into_iter().find(|s| s.0 == name).expect("the scene");
    setup(&mut app);
    let mut rt = app.into_runtime_sized((1100.0, 1400.0));
    rt.frame(0.0);
    for _ in 0..60 {
        rt.frame(DT);
    }
    rt
}

/// The rect of the tour shell's element keyed `key`, under an element keyed `within`.
fn rect(els: &[ElementInfo], within: &str, key: &str) -> [f32; 4] {
    els.iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key) && e.path.contains("call_shell_tour") && e.path.contains(within))
        .unwrap_or_else(|| panic!("no {key} in {within}"))
        .rect
}

#[test]
fn a_long_shell_console_keeps_its_ring_and_insets() {
    let rt = scene("tools-shell-long");
    let els = rt.elements();
    let card = rect(&els, "", "console");
    let command = rect(&els, "console", "command");
    let result = rect(&els, "console", "result");
    let near = |a: f32, b: f32| (a - b).abs() < 0.01;
    for (name, r) in [("command", command), ("result", result)] {
        assert!(near(r[0] - card[0], 0.5) && near(card[0] + card[2] - r[0] - r[2], 0.5), "{name} sits 0.5 inside the ring's sides: {r:?} in {card:?}");
    }
    assert!(near(command[1] - card[1], 0.5), "the command sits 0.5 under the ring's top: {command:?} in {card:?}");
    assert!(near(card[1] + card[3] - result[1] - result[3], 0.5), "the result sits 0.5 over the ring's bottom: {result:?} in {card:?}");
    assert!(near(result[3], MAX_H), "the long output stops at its cap: {result:?}");
    // The viewport fills the section, so its text keeps its 12 pt inset at the bottom, the
    // cut text feathering out over it.
    let scroll = rect(&els, "result", "scroll");
    assert!(near(scroll[1] + scroll[3], result[1] + result[3]), "the viewport fills the result: {scroll:?} in {result:?}");
}
