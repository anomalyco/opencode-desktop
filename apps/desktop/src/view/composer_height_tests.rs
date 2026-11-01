//! An empty composer reserves one line, as Desktop's does: 16 + 20 + 8 + 44 = 88 pt, and each
//! further line grows it by 20.
use rune::{Env, Key, KeyEvent, Mods, Runtime};

use crate::App;

fn session(zoom: f32) -> Runtime<App> {
    let mut app = App::fixture_session();
    app.zoom = zoom;
    let mut rt = Runtime::new(app, super::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
}

fn rect(rt: &Runtime<App>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}")).rect
}

/// (composer height, gap from the prompt's bottom to the toolbar's top), in layout points.
fn measure(rt: &Runtime<App>) -> (f32, f32) {
    let (composer, prompt, toolbar) = (rect(rt, "composer"), rect(rt, "prompt"), rect(rt, "toolbar"));
    (composer[3], toolbar[1] - (prompt[1] + prompt[3]))
}

#[test]
fn an_empty_composer_is_one_line_tall() {
    for zoom in [1.0, 1.6] {
        let rt = session(zoom);
        let (height, gap) = measure(&rt);
        assert!((height - 88.0 * zoom).abs() < 1.0 && (gap - 8.0 * zoom).abs() < 1.0, "zoom {zoom}: {height} {gap}");
    }
}

#[test]
fn a_second_line_grows_it_by_twenty() {
    let mut rt = session(1.0);
    let (rt_h, _) = measure(&rt);
    let (x, y) = rt.find("prompt").unwrap();
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    for t in ["a", "\n", "b"] {
        rt.key(KeyEvent { key: Key::Text(t.into()), mods: Mods::default() });
        rt.frame(1.0 / 60.0);
    }
    settle(&mut rt);
    let (two, gap) = measure(&rt);
    assert!((rt_h - 88.0).abs() < 0.5 && (two - 108.0).abs() < 0.5 && (gap - 8.0).abs() < 0.5, "{rt_h} {two} {gap}");
}
