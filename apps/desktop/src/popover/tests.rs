use rune::{Key, KeyEvent, Mods, Runtime};

use super::*;
use crate::fixture_runtime as runtime;
use crate::pickers::Kind;

fn rect(rt: &Runtime<App>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap_or_else(|| panic!("no {key}")).target
}

fn left_of_text(rt: &Runtime<App>, inside: &str) -> f32 {
    // The first text element under `inside`, by its key path.
    let els = rt.elements();
    let host = els.iter().position(|e| e.key.as_deref() == Some(inside)).unwrap_or_else(|| panic!("no {inside}"));
    els[host..].iter().find(|e| e.text.is_some()).unwrap().target[0]
}

#[test]
fn search_headers_and_labels_share_one_edge() {
    let mut rt = runtime();
    rt.state.open_picker(Kind::Model);
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    let menu = rect(&rt, "picker-model");
    let query = rect(&rt, "picker-search")[0] - menu[0];
    let header = left_of_text(&rt, "group-Anthropic") - menu[0];
    let label = left_of_text(&rt, "option-anthropic:claude-opus-5-5") - menu[0];
    assert!((query - LIST.inset - LIST.label_x()).abs() < 0.5, "query at {query}");
    assert!((header - query).abs() < 0.5, "header at {header}, query at {query}");
    assert!((label - query).abs() < 0.5, "label at {label}, query at {query}");
    // The query is one line tall, on the glass's centre line.
    let field = rect(&rt, "picker-search");
    let glass = rect(&rt, "picker-search-row")[1] + LIST.inset + LIST.row_h / 2.0;
    assert!((field[3] - UI.line_height).abs() < 0.5, "field {} tall", field[3]);
    assert!((field[1] + field[3] / 2.0 - glass).abs() < 0.5, "field {field:?}, glass centre {glass}");
    // The highlight stays 6 pt inside the surface.
    let row = rect(&rt, "option-anthropic:claude-opus-5-5");
    assert!((row[0] - menu[0] - LIST.inset).abs() < 0.5);
}

#[test]
fn keys_mean_the_same_in_every_popover() {
    let ev = |key: Key, mods: Mods| KeyEvent { key, mods };
    let ctrl = Mods { ctrl: true, ..Mods::default() };
    assert_eq!(nav(&ev(Key::Down, Mods::default())), Some(Nav::Step(1)));
    assert_eq!(nav(&ev(Key::Text("p".into()), ctrl)), Some(Nav::Step(-1)));
    assert_eq!(nav(&ev(Key::Enter, Mods::default())), Some(Nav::Choose));
    assert_eq!(nav(&ev(Key::Escape, Mods::default())), Some(Nav::Close));
    assert_eq!(nav(&ev(Key::Tab, Mods::default())), Some(Nav::Leave));
    assert_eq!(nav(&ev(Key::Text("x".into()), Mods::default())), None);
    assert_eq!(cycle(3, Some(0), -1), 2);
}

fn surface_el(rt: &Runtime<App>) -> Option<rune::ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("picker-model"))
}

#[test]
fn popovers_open_and_close_at_once_by_default() {
    let k = rune_knobs::Knobs::default();
    assert!(!motion::opens(&k) && !motion::closes(&k));
    assert_eq!(motion::transition(&k, Side::Top, Align::Start), Transition::none());
}

#[test]
fn grow_and_shrink_pivot_on_the_trigger() {
    let mut k = rune_knobs::Knobs::default();
    k.set(&motion::OPEN, 2.0);
    k.set(&motion::CLOSE, 2.0);
    let t = motion::transition(&k, Side::Top, Align::Start);
    assert_eq!((t.insertion.anchor.x, t.insertion.anchor.y), (0.0, 1.0), "above its trigger, it grows from its bottom-left");
    assert_eq!(t.removal.anchor, t.insertion.anchor);
    assert!(t.insertion.scale < 1.0 && t.insertion.opacity == 0.0);
    let t = motion::transition(&k, Side::Bottom, Align::End);
    assert_eq!((t.insertion.anchor.x, t.insertion.anchor.y), (1.0, 0.0));
}

#[test]
fn a_fade_plays_in_and_a_shrink_plays_out() {
    let mut rt = runtime();
    rt.state.knobs.set(&motion::OPEN, 1.0);
    rt.state.knobs.set(&motion::CLOSE, 2.0);
    rt.frame(1.0 / 60.0);
    rt.update(|s, tx| {
        s.open_picker(Kind::Model);
        motion::opening(&s.knobs, tx);
    });
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    let mid = surface_el(&rt).expect("open").opacity;
    assert!(mid > 0.0 && mid < 1.0, "fading in, opacity {mid}");
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(surface_el(&rt).unwrap().opacity, 1.0);
    rt.update(|s, tx| {
        s.close_picker(true);
        motion::closing(&s.knobs, tx);
    });
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    let out = surface_el(&rt).expect("still leaving");
    assert!(out.exiting && out.scale < 1.0, "shrinking, scale {}", out.scale);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert!(surface_el(&rt).is_none());
}
