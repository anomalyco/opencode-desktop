use rune::{Env, Runtime};

use super::REVIEW_KEY;
use crate::{App, FIXTURE, open_scene, spec};

fn test_runtime() -> Runtime<App> {
    let s = spec(|| {
        let mut a = App::empty();
        a.load_fixture(FIXTURE);
        open_scene(&mut a);
        a.finish_stream();
        crate::plugin_host::setup(&mut a, std::sync::Arc::new(|| {}));
        a
    });
    let mut rt = Runtime::new((s.init)(), s.view, (1440.0, 900.0), Env::new(true, 2.0));
    rt.poll = s.poll;
    rt.on_key = s.on_key;
    rt.frame(0.0);
    rt
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
    let before = rt.state.review.sessions[&sid].load.loaded_revision();
    rt.update(|s, _| {
        s.note_review_event("filesystem.changed");
    });
    rt.frame(1.0 / 60.0);
    let sr = &rt.state.review.sessions[&sid];
    assert_ne!(sr.load.loaded_revision(), before, "the change refetched the diffs");
    assert!(!sr.tree.is_expanded("src/net"), "the folder Kit closed stays closed");
    assert!(sr.tree.is_expanded("src/sidebar"), "other folders stay open");
}

#[test]
fn the_changes_tab_closes_as_any_tab() {
    let mut rt = test_runtime();
    open_review(&mut rt);
    rt.update(|s, _| s.side.tab_menu = Some((REVIEW_KEY.to_owned(), 100.0, 100.0)));
    rt.frame(1.0 / 60.0);
    assert!(rt.find("side-tab-menu").is_some());
    let closes = rt.state.resolve_side_region().and_then(|r| r.entry(REVIEW_KEY).map(|e| e.tab.can_close()));
    assert_eq!(closes, Some(true), "Desktop's review tab isn't pinned: its menu's Close tab closes it");
}
