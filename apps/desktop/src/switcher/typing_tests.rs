//! Typing in the palette while its file search answers later, as a server's does: the list never
//! blanks or says "Loading" between keys, a stale answer never lands, and "No results found"
//! waits for the search to finish.

use super::shown::{Effect, Empty, Event, LOADING_AFTER, Shown};
use super::*;
use crate::fixture_runtime as runtime;
use rune::{Mods, Runtime};

/// Files only the fixture's server knows, so the folder's known files can't match "ios" first.
const FOUND: [&str; 3] = ["apps/note-web/src/ios_thread.rs", "apps/desktop/src/motion_gate/scenarios.rs", "docs/ios-release.md"];
const FRAME: f64 = 1.0 / 60.0;

fn slow_find() -> Runtime<App> {
    let mut rt = runtime();
    rt.state.fixture["find_ms"] = serde_json::json!(180);
    rt.state.fixture["find_files"] = serde_json::json!(FOUND);
    rt.key(KeyEvent { key: Key::Command('k'), mods: Mods { cmd: true, ..Default::default() } });
    rt.frame(FRAME);
    rt
}

/// One frame of the palette as drawn.
#[derive(Debug)]
struct Seen {
    query: String,
    rows: Vec<String>,
    loading: bool,
    no_results: bool,
    searching: bool,
}

fn seen(rt: &Runtime<App>) -> Seen {
    let els: Vec<_> = rt.elements().into_iter().filter(|e| !e.exiting).collect();
    let said = |t: &str| els.iter().any(|e| e.text.as_deref() == Some(t));
    Seen {
        query: rt.state.switcher.query().to_owned(),
        rows: els.iter().filter_map(|e| e.key.clone()).filter(|k| k.starts_with("sw-") && !k.starts_with("sw-section-")).collect(),
        loading: said("Loading"),
        no_results: said("No results found"),
        searching: rt.state.switcher_searching(),
    }
}

/// Types `keys` 120 ms apart, then waits `rest` seconds, and returns every frame.
fn type_keys(rt: &mut Runtime<App>, keys: &[Key], rest: f64) -> Vec<Seen> {
    let mut frames = Vec::new();
    for (i, k) in keys.iter().enumerate() {
        rt.key(KeyEvent { key: k.clone(), mods: Mods::default() });
        let wait = if i + 1 == keys.len() { rest } else { 0.12 };
        for _ in 0..(wait / FRAME).round() as usize {
            rt.frame(FRAME);
            frames.push(seen(rt));
        }
    }
    frames
}

fn letters(s: &str) -> Vec<Key> {
    s.chars().map(|c| Key::Text(c.to_string())).collect()
}

#[test]
fn typing_ios_then_gibberish_never_blanks_the_list() {
    let mut rt = slow_find();
    let mut keys = letters("ios");
    let ios = type_keys(&mut rt, &keys, 0.5);
    assert!(FOUND.iter().all(|f| ios.last().unwrap().rows.contains(&format!("sw-file:{f}"))), "the search's files join: {:?}", ios.last());
    keys = vec![Key::Backspace; 3];
    keys.extend(letters("ioeuaoe"));
    let gibberish = type_keys(&mut rt, &keys, 0.5);
    let mut settled_empty: Vec<String> = Vec::new();
    for f in ios.iter().chain(&gibberish) {
        assert!(!f.loading, "no frame says Loading between keys: {f:?}");
        assert!(!f.rows.is_empty() || f.no_results, "no frame is blank: {f:?}");
        if f.rows.is_empty() && !f.searching {
            settled_empty.push(f.query.clone());
        }
        // "No results found" only for a query whose search finished empty, or one that extends it.
        assert!(!f.no_results || settled_empty.iter().any(|q| f.query.starts_with(q.as_str())), "early No results: {f:?}");
    }
    assert_eq!(gibberish.last().map(|f| (f.query.as_str(), f.no_results)), Some(("ioeuaoe", true)));
}

/// The rows a key brings are the query's own or, while nothing of its own is in, the last
/// query's, never a mix, and they swap in one frame.
#[test]
fn a_late_answer_swaps_the_rows_in_one_frame() {
    let mut rt = slow_find();
    rt.state.fixture["find_files"] = serde_json::json!(["web/xyzzy.rs"]);
    let before = seen(&rt).rows;
    // A pasted query nothing local matches: only the file search can find it.
    rt.key(KeyEvent { key: Key::Text("xyzzy".into()), mods: Mods::default() });
    rt.frame(FRAME);
    assert_eq!(seen(&rt).rows, before, "the last rows hold while xyzzy is searched");
    let mut frames = 0;
    while seen(&rt).rows == before {
        rt.frame(FRAME);
        frames += 1;
        assert!(frames < 30, "the answer lands");
    }
    assert_eq!(seen(&rt).rows, ["sw-file:web/xyzzy.rs"]);
}

#[test]
fn a_stale_file_answer_is_dropped() {
    let mut rt = slow_find();
    type_keys(&mut rt, &letters("ios"), 0.5);
    let ios = seen(&rt).rows;
    // An answer to an older query that lands after a newer one was asked never shows.
    rt.update(|s, _| {
        s.switcher.files_asked = Some("iosx".into());
        s.switcher.files_fetch = rune::Latest::default();
        s.switcher.files_fetch.request((), |()| ("io".to_owned(), vec!["stale/io.rs".to_owned()]));
    });
    while rt.state.switcher.files_fetch.is_pending() {
        std::thread::yield_now();
    }
    rt.frame(FRAME);
    assert_ne!(rt.state.switcher.files.0, "io");
    assert!(!seen(&rt).rows.iter().any(|k| k.contains("stale/io.rs")));
    assert!(ios.iter().any(|k| k.starts_with("sw-file:")));
}

#[test]
fn files_found_before_hold_their_places_while_the_next_query_is_searched() {
    let mut rt = slow_find();
    type_keys(&mut rt, &letters("ios"), 0.5);
    rt.key(KeyEvent { key: Key::Text("r".into()), mods: Mods::default() });
    rt.frame(FRAME);
    // "iosr": the server's fuzzy search keeps a path whose letters come in order.
    let rows = seen(&rt).rows;
    assert!(rows.contains(&"sw-file:apps/desktop/src/motion_gate/scenarios.rs".to_owned()), "{rows:?}");
}

#[test]
fn loading_waits_and_no_results_needs_a_finished_search() {
    let mut s = Shown::default();
    assert_eq!(s.step(Event::Opened { now: 1.0 }), Effect::WakeAt(1.0 + LOADING_AFTER));
    let seen = |q, rows, searching, now| Event::Seen { query: q, rows, searching, failed: false, now };
    s.step(seen("", false, true, 1.0));
    assert_eq!(s.empty("", true, false), Empty::Blank);
    assert_eq!(s.step(Event::Tick { now: 1.1 }), Effect::WakeAt(1.0 + LOADING_AFTER));
    assert_eq!(s.step(Event::Tick { now: 1.0 + LOADING_AFTER }), Effect::Redraw);
    assert_eq!(s.empty("", true, false), Empty::Loading);
    // Rows hold while a newer query is searched with nothing of its own.
    s.step(seen("io", true, false, 2.0));
    s.step(seen("ioq", false, true, 2.1));
    assert_eq!(s.held("ioq"), Some("io"));
    s.step(seen("ioq", false, false, 2.3));
    assert_eq!(s.empty("ioq", false, false), Empty::NoResults);
    // A longer query can't find more: it says so at once; a shorter one waits for its answer.
    s.step(seen("ioqz", false, true, 2.4));
    assert_eq!(s.empty("ioqz", true, false), Empty::NoResults);
    assert_eq!(s.step(seen("io", false, true, 2.5)), Effect::WakeAt(2.5 + LOADING_AFTER));
    assert_eq!(s.empty("io", true, false), Empty::Blank);
    assert_eq!(s.empty("io", false, true), Empty::Failed);
}

#[test]
fn loading_is_a_quiet_line_not_a_placeholder() {
    let mut rt = slow_find();
    rt.update(|s, _| s.switcher.shown = Shown::Loading { query: "zzz".into() });
    rt.state.switcher.search = TextEdit::new("zzz");
    rt.state.switcher.search.focused = true;
    rt.frame(FRAME);
    let els = rt.elements();
    let line = els.iter().find(|e| e.key.as_deref() == Some("switcher-loading")).expect("the Loading line");
    assert!(line.rect[3] <= 28.5, "one row tall: {:?}", line.rect);
    assert!(!els.iter().any(|e| e.key.as_deref() == Some("switcher-empty")));
}
