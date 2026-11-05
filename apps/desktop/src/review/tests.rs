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

fn open_review(rt: &mut Runtime<App>) -> String {
    rt.update(|s, tx| s.toggle_side_region(tx));
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    rt.state.active_session_id().unwrap().to_owned()
}

#[test]
fn a_collapsed_folder_stays_collapsed_across_a_refetch() {
    let mut rt = test_runtime();
    let sid = open_review(&mut rt);
    let tree = &rt.state.review.sessions[&sid].tree;
    assert!(tree.is_expanded("src/net") && tree.is_expanded("src/sidebar"));

    rt.update(|s, _| s.review.sessions.get_mut(&sid).unwrap().tree.toggle("src/net"));
    let before = rt.state.review.sessions[&sid].loaded_revision;
    rt.update(|s, _| {
        s.note_review_event("filesystem.changed");
    });
    rt.frame(1.0 / 60.0);
    let sr = &rt.state.review.sessions[&sid];
    assert_ne!(sr.loaded_revision, before, "the change refetched the diffs");
    assert!(!sr.tree.is_expanded("src/net"), "the folder Kit closed stays closed");
    assert!(sr.tree.is_expanded("src/sidebar"), "other folders stay open");
}

#[test]
fn scrolling_the_diff_does_not_reopen_a_collapsed_folder() {
    let mut rt = test_runtime();
    let sid = open_review(&mut rt);
    rt.update(|s, tx| {
        let sr = s.review.sessions.get_mut(&sid).unwrap();
        sr.tree.toggle("src/net");
        sr.active_path = Some("src/sidebar/store.ts".to_owned());
        s.sync_review_tree_to_diff(&sid, tx);
    });
    let sr = &rt.state.review.sessions[&sid];
    assert!(sr.active_path.as_deref().is_some_and(|p| p.starts_with("src/net/")));
    assert!(!sr.tree.is_expanded("src/net"));
}

#[test]
fn the_files_changed_tab_menu_offers_no_close() {
    let mut rt = test_runtime();
    open_review(&mut rt);
    rt.update(|s, _| s.side.tab_menu = Some((REVIEW_KEY.to_owned(), 100.0, 100.0)));
    rt.frame(1.0 / 60.0);
    assert!(rt.find("side-tab-menu").is_some());
    assert!(rt.find("side-tab-menu-close").is_none(), "Files Changed can't close, so no Close tab");
    assert!(rt.find("side-tab-menu-close-others").is_none(), "no other tab to close");
}
