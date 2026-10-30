use rune::{Env, Key, KeyEvent, Mods, Runtime};
use rune_kit::diff::DiffMode;

use super::{ChangeMode, REVIEW_KEY};
use crate::panels::state::SIDE_WIDE_W;
use crate::{App, FIXTURE, open_scene, spec};

fn test_runtime() -> Runtime<App> {
    let s = spec(|| {
        let mut a = App::empty();
        a.load_fixture(FIXTURE);
        open_scene(&mut a);
        a.finish_stream();
        a
    });
    let mut rt = Runtime::new((s.init)(), s.view, (1440.0, 900.0), Env::new(true, 2.0));
    rt.poll = s.poll;
    rt.on_key = s.on_key;
    rt.frame(0.0);
    rt
}

#[test]
fn review_tab_pins_first_and_supports_split_wide_layout_and_tree_toggle() {
    let mut rt = test_runtime();
    // ⇧⌘R opens the side region onto pinned `review:changes` ("Files Changed 4").
    rt.key(KeyEvent {
        key: Key::Command('r'),
        mods: Mods { cmd: true, shift: true, logo: true, ..Default::default() },
    });
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    let reg = rt.state.resolve_side_region().expect("side region");
    assert_eq!(reg.active.as_deref(), Some(REVIEW_KEY));
    let tab = &reg.entry(REVIEW_KEY).expect("review entry").tab;
    assert!(tab.pinned);
    assert!(tab.title.starts_with("Files Changed "), "got title: {}", tab.title);
    assert!(rt.find("review-sidebar").is_some());
    assert!(rt.find("review-style-split").is_some());

    let unified_w = rt.presented("side-panel").map(|(r, _)| r[2]).unwrap_or(0.0);

    // Switch to Split diff: `wide` kicks in (`>= SIDE_WIDE_W` when window has room).
    rt.update(|s, _| s.set_review_diff_mode(DiffMode::Split));
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.state.review.diff_mode, DiffMode::Split);
    let split_w = rt.presented("side-panel").map(|(r, _)| r[2]).unwrap_or(0.0);
    assert!(
        split_w >= SIDE_WIDE_W - 1.0 && split_w > unified_w,
        "split diff widens side region: unified={unified_w} split={split_w}"
    );

    // `⌘\` toggles the file tree sidebar instantly.
    rt.key(KeyEvent {
        key: Key::Command('\\'),
        mods: Mods { cmd: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 120.0);
    assert!(!rt.state.review.sidebar_open);
    assert!(rt.find("review-sidebar").is_none());
}

#[test]
fn jumping_files_and_hunks_and_filtering_tree_syncs_selection() {
    let mut rt = test_runtime();
    rt.update(|s, tx| s.toggle_side_region(tx));
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    let sid = rt.state.active_session_id().unwrap().to_owned();
    let first_path = rt.state.review.sessions[&sid]
        .active_path
        .clone()
        .expect("initial active path");

    // `→` jumps to the next changed file and syncs `active_path`.
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    let second_path = rt.state.review.sessions[&sid]
        .active_path
        .clone()
        .expect("second active path");
    assert_ne!(first_path, second_path);

    // `[` jumps back to the first file.
    rt.key(KeyEvent { key: Key::Text("[".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(
        rt.state.review.sessions[&sid].active_path.as_deref(),
        Some(first_path.as_str())
    );

    // `j` jumps forward through hunks.
    rt.key(KeyEvent { key: Key::Text("j".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);

    // Filter files in `review-filter` and press Enter to jump to the matched file.
    rt.update(|s, _| {
        s.review.filter = rune::TextEdit::new("store.ts");
        s.review.filter.focused = true;
    });
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(
        rt.state.review.sessions[&sid].active_path.as_deref(),
        Some("src/sidebar/store.ts")
    );

    // Switching ChangeMode to `Turn` reloads turn-specific diffs.
    rt.update(|s, _| {
        s.review.filter = rune::TextEdit::new("");
        s.set_review_change_mode(ChangeMode::Turn);
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.review.sessions[&sid].mode, ChangeMode::Turn);
    assert!(!rt.state.review.sessions[&sid].meta.is_empty());
}
