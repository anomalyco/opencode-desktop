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
