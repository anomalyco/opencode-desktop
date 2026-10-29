use super::*;
use crate::{Route, fixture_runtime as runtime};
use rune::{Mods, Runtime};

fn app() -> App {
    // Never App::new: tests must not probe a server.
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    app
}

fn press(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 60.0);
}

fn key(rt: &mut Runtime<App>, k: Key) {
    press(rt, k, Mods::default());
}

fn cmd_k(rt: &mut Runtime<App>) {
    press(rt, Key::Command('k'), Mods { cmd: true, ..Default::default() });
}

fn keys(a: &App) -> Vec<String> {
    a.switcher_entries().into_iter().map(|e| e.key).collect()
}

fn query(a: &mut App, q: &str) {
    a.switcher.search = TextEdit::new(q);
}

#[test]
fn empty_lists_commands_then_the_newest_sessions() {
    let mut a = app();
    a.open_switcher();
    let e = a.switcher_entries();
    let groups: Vec<&str> = e.iter().map(|e| e.group).collect();
    assert_eq!(groups, [["Commands"; 4].as_slice(), ["Recent sessions"; 5].as_slice()].concat());
    assert_eq!(e[0].title, "New session");
    // Roots only, newest first: the subagent session never lists.
    assert_eq!(e[4].key, "session:ses_fx0001aaaaaaaaaaaaaaaaaa");
    assert_eq!(e[5].key, "session:ses_fx0003aaaaaaaaaaaaaaaaaa");
    assert!(!keys(&a).iter().any(|k| k.contains("fx0002")));
    assert_eq!(e[4].description, "lumen");
}

#[test]
fn a_query_ranks_open_tabs_then_the_newest_then_commands() {
    let mut a = app();
    a.open_switcher();
    // Only the oldest match is open as a tab.
    a.tabs = vec![crate::tabs::Tab { id: "ses_fx0012aaaaaaaaaaaaaaaaaa".into(), title: String::new(), directory: String::new() }];
    query(&mut a, "THE");
    let k = keys(&a);
    assert_eq!(k[0], "session:ses_fx0012aaaaaaaaaaaaaaaaaa");
    assert_eq!(k[1], "session:ses_fx0001aaaaaaaaaaaaaaaaaa");
    assert!(a.switcher_entries().iter().all(|e| e.group == "Session"));
    // Sessions lead commands; a command matches by title.
    query(&mut a, "sidebar");
    assert_eq!(keys(&a), ["session:ses_fx0003aaaaaaaaaaaaaaaaaa", "command:sidebar"]);
    // Project names and ids match; the "Session" group name doesn't.
    query(&mut a, "harbor");
    assert_eq!(keys(&a), ["session:ses_fx0006aaaaaaaaaaaaaaaaaa", "session:ses_fx0007aaaaaaaaaaaaaaaaaa"]);
    query(&mut a, "fx0009");
    assert_eq!(keys(&a), ["session:ses_fx0009aaaaaaaaaaaaaaaaaa"]);
    query(&mut a, "session");
    assert!(keys(&a).iter().all(|k| k == "command:new" || a.switcher_entries().iter().any(|e| &e.key == k && e.title.to_lowercase().contains("session"))));
    query(&mut a, "zzz-nothing");
    assert!(keys(&a).is_empty());
}

#[test]
fn server_finds_join_the_matches() {
    let mut a = app();
    a.open_switcher();
    let mut far = a.sessions[0].clone();
    far.id = "ses_far000000000000000000".into();
    far.title = "An old reconnect fix".into();
    far.updated = 1;
    a.switcher.loaded.push(far);
    query(&mut a, "reconnect");
    assert_eq!(keys(&a).last().map(String::as_str), Some("session:ses_far000000000000000000"));
    a.switcher_choose("session:ses_far000000000000000000", &mut Tx::default());
    assert_eq!(a.route, Route::Session("ses_far000000000000000000".into()));
    assert!(a.tabs.iter().any(|t| t.id == "ses_far000000000000000000" && t.title == "An old reconnect fix"));
}

#[test]
fn moves_wrap_and_jump_sections() {
    let s = ["a", "a", "a", "b", "b"];
    assert_eq!(target(&s, 4, Move::Next), Some(0));
    assert_eq!(target(&s, 0, Move::Previous), Some(4));
    assert_eq!(target(&s, 1, Move::NextSection), Some(3));
    assert_eq!(target(&s, 3, Move::NextSection), Some(0));
    assert_eq!(target(&s, 4, Move::PreviousSection), Some(3));
    assert_eq!(target(&s, 3, Move::PreviousSection), Some(0));
    assert_eq!(target(&s, 0, Move::PreviousSection), Some(3));
    assert_eq!(target(&s, 2, Move::Last), Some(4));
    assert_eq!(target(&[], 0, Move::Next), None);
    let ev = |key, mods| KeyEvent { key, mods };
    assert_eq!(move_for(&ev(Key::Down, Mods { alt: true, ..Default::default() })), Some(Move::NextSection));
    assert_eq!(move_for(&ev(Key::Up, Mods { shift: true, ..Default::default() })), None);
}

#[test]
fn cmd_k_arrows_enter_and_escape() {
    let mut rt = runtime();
    let before = rt.state.route.clone();
    cmd_k(&mut rt);
    assert!(rt.state.switcher.open);
    assert!(rt.state.switcher.search.focused);
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("switcher")));
    // First row active; Down steps, Up wraps from the top to the last row.
    let active = |rt: &Runtime<App>| rt.state.switcher_active(&rt.state.switcher_entries());
    assert_eq!(active(&rt).as_deref(), Some("command:new"));
    key(&mut rt, Key::Down);
    assert_eq!(active(&rt).as_deref(), Some("command:home"));
    key(&mut rt, Key::Up);
    key(&mut rt, Key::Up);
    assert_eq!(active(&rt), rt.state.switcher_entries().last().map(|e| e.key.clone()));
    // Typing starts again from the first match.
    key(&mut rt, Key::Text("footnote".into()));
    assert_eq!(active(&rt).as_deref(), Some("session:ses_fx0010aaaaaaaaaaaaaaaaaa"));
    key(&mut rt, Key::Enter);
    assert!(!rt.state.switcher.open);
    assert_eq!(rt.state.route, Route::Session("ses_fx0010aaaaaaaaaaaaaaaaaa".into()));
    assert_ne!(rt.state.route, before);
    // ⌘K toggles; Escape closes and gives the composer its focus back.
    rt.update(|s, _| s.composer.focused = true);
    rt.frame(1.0 / 60.0);
    cmd_k(&mut rt);
    assert!(rt.state.switcher.open && !rt.state.composer.focused);
    assert_eq!(rt.state.switcher.search.text, "", "it opens empty");
    key(&mut rt, Key::Escape);
    assert!(!rt.state.switcher.open);
    assert!(rt.state.composer.focused);
    cmd_k(&mut rt);
    cmd_k(&mut rt);
    assert!(!rt.state.switcher.open);
    assert_eq!(rt.state.route, Route::Session("ses_fx0010aaaaaaaaaaaaaaaaaa".into()));
}

/// Each letter filters the drawn rows in the frame it lands, already in place and opaque, and
/// Enter shows the picked session's page in its own frame, with no frame of the old one.
#[test]
fn filtering_and_picking_land_in_the_keys_frame() {
    let mut rt = runtime();
    cmd_k(&mut rt);
    for c in "footnote".chars() {
        key(&mut rt, Key::Text(c.to_string()));
        let want: Vec<String> = keys(&rt.state).into_iter().map(|k| format!("sw-{k}")).collect();
        let rows: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("sw-") && !k.starts_with("sw-section-"))).collect();
        assert!(!rows.is_empty(), "after {c}: rows show");
        for e in rows {
            let k = e.key.clone().unwrap();
            let moved = e.rect.iter().zip(e.target).any(|(a, b)| (a - b).abs() > 0.01);
            assert!(want.contains(&k) && !e.exiting, "after {c}: {k} isn't a match but draws");
            assert!(!moved && (e.opacity - 1.0).abs() < 1e-3, "{k} after {c}: {:?} → {:?}, opacity {}", e.rect, e.target, e.opacity);
        }
    }
    let picked = "ses_fx0010aaaaaaaaaaaaaaaaaa";
    let title = rt.state.session(picked).unwrap().title.clone();
    let old = rt.state.session(crate::fixture_ids::MAIN).unwrap().title.clone();
    let header = |rt: &Runtime<App>, t: &str| rt.elements().into_iter().any(|e| {
        !e.exiting && e.opacity > 0.99 && e.path.contains("header") && (e.text.as_deref() == Some(t) || e.label.as_deref() == Some(t))
    });
    assert!(header(&rt, &old), "the open session's title heads the page");
    key(&mut rt, Key::Enter);
    assert!(header(&rt, &title), "{title:?} heads the page in Enter's frame");
    assert!(!header(&rt, &old), "no frame of {old:?}");
}

#[test]
fn a_click_opens_a_row_and_outside_closes() {
    let mut rt = runtime();
    cmd_k(&mut rt);
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find("sw-session:ses_fx0003aaaaaaaaaaaaaaaaaa").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.route, Route::Session(crate::fixture_ids::THIRD.into()));
    assert!(!rt.state.switcher.open);
    cmd_k(&mut rt);
    rt.pointer_down(10.0, 400.0);
    rt.pointer_up(10.0, 400.0);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.switcher.open);
    assert_eq!(rt.state.route, Route::Session(crate::fixture_ids::THIRD.into()));
}

#[test]
fn the_hover_highlight_jumps_between_rows() {
    let mut rt = runtime();
    cmd_k(&mut rt);
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    let fill = |rt: &Runtime<App>, k: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k)).unwrap().fill.a;
    let (a, b) = ("sw-session:ses_fx0003aaaaaaaaaaaaaaaaaa", "sw-session:ses_fx0004aaaaaaaaaaaaaaaaaa");
    let (x, y) = rt.find(a).unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    let (x, y) = rt.find(b).unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    let first = fill(&rt, b);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert!(fill(&rt, a) < 1e-3, "the old row clears at once: {}", fill(&rt, a));
    assert!(first > 0.0 && (first - fill(&rt, b)).abs() < 1e-3, "the new row lights at once: {first} then {}", fill(&rt, b));
}

#[test]
fn it_opens_and_closes_instantly() {
    let mut rt = runtime();
    let info = |rt: &Runtime<App>, k: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k));
    rt.frame(1.0 / 60.0);
    cmd_k(&mut rt);
    rt.frame(1.0 / 60.0);
    let d = info(&rt, "switcher").unwrap();
    assert!((d.scale - 1.0).abs() < 1e-3 && (d.opacity - 1.0).abs() < 1e-3, "first frame is final: scale {} opacity {}", d.scale, d.opacity);
    assert!((info(&rt, "switcher-scrim").unwrap().opacity - 1.0).abs() < 1e-3);
    for part in ["switcher-field", "switcher-input", "switcher-results", "sw-section-Commands"] {
        let e = info(&rt, part).unwrap();
        assert!((e.opacity - 1.0).abs() < 1e-3, "{part} is there at once: {}", e.opacity);
    }
    key(&mut rt, Key::Escape);
    rt.frame(1.0 / 60.0);
    assert!(info(&rt, "switcher").is_none() && info(&rt, "switcher-scrim").is_none());
}

#[test]
fn queries_encode_for_the_url() {
    assert_eq!(encode("a b&c/é"), "a%20b%26c%2F%C3%A9");
}

#[test]
fn a_failed_search_says_so() {
    let mut a = app();
    a.open_switcher();
    query(&mut a, "zzz");
    a.switcher.loading = Some("zzz".into());
    let _ = a.switcher.found_tx.send(Found { query: "zzz".into(), sessions: vec![], failed: true });
    a.switcher.searched = "zzz".into();
    assert!(a.poll_switcher(&mut Tx::default()));
    assert_eq!(a.switcher.failed.as_deref(), Some("zzz"));
    assert_eq!(a.switcher.loading, None);
}

