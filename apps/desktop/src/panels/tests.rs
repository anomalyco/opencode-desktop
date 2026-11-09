use rune::{El, Env, Key, KeyEvent, Mods, Runtime, col, text};
use rune_theme_opencode::icons as ic;

use super::closing::State as Closing;
use super::{
    PanelRegistration, PanelSessionCtx, PanelTabDesc, PanelViewCtx, SideState,
    panel_key,
};
use crate::palette::Pal;
use crate::view::UI;
use crate::{App, FIXTURE, Route, open_scene, spec};

fn test_runtime() -> Runtime<App> {
    let s = spec(|| {
        let mut a = App::empty();
        a.load_fixture(FIXTURE);
        open_scene(&mut a);
        a.finish_stream();
        crate::plugin_host::setup(&mut a, std::sync::Arc::new(|| {}));
        a
    });
    let mut rt = Runtime::new((s.init)(), s.view, s.size, Env::new(true, 2.0));
    rt.poll = s.poll;
    rt.on_key = s.on_key;
    rt.frame(0.0);
    rt
}

fn mock_notes_list(_s: &App, ctx: &PanelSessionCtx) -> Vec<PanelTabDesc> {
    ctx.open_ids
        .iter()
        .map(|id| PanelTabDesc::new(id.clone(), format!("Note {id}")).icon(&ic::EDIT).fallback())
        .collect()
}

fn mock_notes_render(_s: &App, p: &Pal, ctx: &PanelViewCtx) -> El {
    col()
        .key("mock-notes-body")
        .w_full()
        .grow(1.0)
        .padding(16.0)
        .child(text(format!("Scratchpad: {}", ctx.tab.title)).font(UI).color(p.text))
}

#[test]
fn side_region_toggles_on_spring_and_launches_browser() {
    let mut rt = test_runtime();
    assert!(!rt.state.side_open());

    // ⌥⌘B opens the side region on its spring onto New tab; `+` opens New tab.
    rt.key(KeyEvent {
        key: Key::Command('b'),
        mods: Mods { cmd: true, alt: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 120.0);
    assert!(rt.state.side_open());
    let mid_w = rt.presented("side-region").map(|(r, _)| r[2]).unwrap_or(0.0);
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    let settled_w = rt.presented("side-region").map(|(r, _)| r[2]).unwrap_or(0.0);
    assert!(mid_w < settled_w && settled_w > 350.0, "expected spring travel: mid={mid_w} settled={settled_w}");
    rt.update(|s, _| {
        s.open_side_new_tab(super::SessionPane::Side);
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.find("side-launch-browser-open").is_some(), "Browser launcher card shown in New tab");

    // ⇧⌘R (Desktop's review.toggle) closes and reopens the region.
    rt.key(KeyEvent {
        key: Key::Command('r'),
        mods: Mods { cmd: true, shift: true, logo: true, ..Default::default() },
    });
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    assert!(!rt.state.side_open());

    // ⌃⌘B (Desktop's browser.open) opens a Browser tab directly in the side region.
    rt.key(KeyEvent {
        key: Key::Command('b'),
        mods: Mods { cmd: true, ctrl: true, logo: true, ..Default::default() },
    });
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.state.side_open());
    assert!(rt.state.active_browser_tab_id().is_some());
    assert!(rt.find("browser-address").is_some());
}

#[test]
fn instant_tab_switch_close_emptied_guard_and_reopen_closed() {
    let mut rt = test_runtime();
    rt.state
        .side
        .extra_panels
        .push(PanelRegistration::new("notes", mock_notes_list, mock_notes_render).order(25));

    let b_id = rt.state.open_new_browser_tab("http://localhost:3000/app");
    let b_key = panel_key("browser", &b_id.to_string());
    rt.update(|s, tx| s.open_side_tab("notes:1", None, false, false, tx));
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.find("mock-notes-body").is_some());

    // Switching tabs with ⇧⌘[ is instant (lands in a single frame).
    rt.key(KeyEvent {
        key: Key::Command('['),
        mods: Mods { cmd: true, shift: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.resolve_side_region().and_then(|r| r.active), Some(b_key.clone()));
    assert!(rt.find("browser-address").is_some());

    // ⌘2 jumps straight back to the second side tab (`notes:1`; ⌘1 is Browser: Changes isn't
    // pinned, as in Desktop).
    rt.key(KeyEvent {
        key: Key::Command('2'),
        mods: Mods { cmd: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 120.0);
    assert_eq!(
        rt.state.resolve_side_region().and_then(|r| r.active).as_deref(),
        Some("notes:1")
    );

    let session_tabs_before = rt.state.tabs.len();

    // First ⌘W closes `notes:1`, leaving `browser:1` active.
    rt.key(KeyEvent {
        key: Key::Command('w'),
        mods: Mods { cmd: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.side_open());
    assert_eq!(rt.state.resolve_side_region().and_then(|r| r.active), Some(b_key));

    // Second ⌘W closes the last tab, `browser:1`: the side region collapses and arms `emptied_guard`.
    rt.key(KeyEvent {
        key: Key::Command('w'),
        mods: Mods { cmd: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.side_open());

    // An extra immediate ⌘W is swallowed by `emptied_guard` instead of closing the session tab!
    rt.key(KeyEvent {
        key: Key::Command('w'),
        mods: Mods { cmd: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.tabs.len(), session_tabs_before, "extra ⌘W must not close session tab");

    // Reopen side region and click `+` (`New tab`) -> Recently closed lists both closed tabs; ⌘⇧T restores the newest.
    rt.update(|s, tx| {
        s.toggle_side_region(tx);
        s.open_side_new_tab(super::SessionPane::Side);
    });
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.find("side-recently-closed").is_some());

    rt.key(KeyEvent {
        key: Key::Command('t'),
        mods: Mods { cmd: true, shift: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 60.0);
    let restored_url = rt
        .state
        .active_browser_tab_id()
        .and_then(|id| rt.state.browser.host.tab(id))
        .map(|t| t.url.clone());
    assert_eq!(restored_url.as_deref(), Some("http://localhost:3000/app"));
}

#[test]
fn closing_last_side_tab_keeps_its_contents_until_the_region_lands() {
    let mut rt = test_runtime();
    rt.update(|s, tx| s.open_side_tab("review:changes", None, false, false, tx));
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.find("panel-review:changes").is_some());

    rt.update(|s, tx| s.close_side_tab("review:changes", tx));
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    assert!(!rt.state.side_open());
    let w = rt.presented("side-region").map_or(0.0, |(r, _)| r[2]);
    assert!(w > 50.0, "still mid-flight: {w}");
    assert!(rt.find("panel-review:changes").is_some(), "the closing tab stays drawn while the region slides shut");
    assert!(rt.find("side-tab-review-changes").is_some(), "and so does its tab");
    assert!(rt.find("side-empty").is_none(), "no empty pane flashes mid-flight");

    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert!(matches!(rt.state.side.closing, Closing::Idle));

    // Reopening shows New tab, not the tab just closed.
    rt.update(|s, tx| s.toggle_side_region(tx));
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.find("panel-review:changes").is_none());
    let launchers: Vec<String> = rt.state.resolve_side_region().map_or_else(Vec::new, |r| r.launchers.iter().map(|(p, d, _)| format!("{p}-{} opened={}", d.id, d.opened)).collect());
    assert!(rt.find("side-launch-review-open").is_some(), "Changes is a New tab row again: {launchers:?}");
}

#[test]
fn reopening_mid_slide_lets_go_of_the_closing_tab_at_once() {
    let mut rt = test_runtime();
    rt.update(|s, tx| s.open_side_tab("review:changes", None, false, false, tx));
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    rt.update(|s, tx| s.close_side_tab("review:changes", tx));
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    assert!(matches!(rt.state.side.closing, Closing::Sliding { .. }));

    // Input runs after a frame's poll, so the reopened region holds nothing before the next one.
    rt.update(|s, tx| s.toggle_side_region(tx));
    let reg = rt.state.resolve_side_region().unwrap();
    assert!(rt.state.closing_side_entry(&reg, super::SessionPane::Side).is_none());
    rt.frame(1.0 / 60.0);
    assert!(matches!(rt.state.side.closing, Closing::Idle));
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.find("panel-review:changes").is_none());
    let launchers: Vec<String> = rt.state.resolve_side_region().map_or_else(Vec::new, |r| r.launchers.iter().map(|(p, d, _)| format!("{p}-{} opened={}", d.id, d.opened)).collect());
    assert!(rt.find("side-launch-review-open").is_some(), "Changes is a New tab row again: {launchers:?}");
}

#[test]
fn per_session_persistence_roundtrips_and_isolates_sessions() {
    let mut rt = test_runtime();
    let s1 = rt.state.active_session_id().unwrap().to_owned();
    let s2 = rt.state.tabs.iter().map(|t| t.id.clone()).find(|id| id != &s1).unwrap();

    rt.update(|s, _| {
        s.open_new_browser_tab("http://localhost:4000/");
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.side_open());

    // Switching to another session isolates side-region state.
    rt.update(|s, tx| s.open(&s2, tx));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.route, Route::Session(s2));
    assert!(!rt.state.side_open());

    // Switching back restores s1's open side region.
    rt.update(|s, tx| s.open(&s1, tx));
    rt.frame(1.0 / 60.0);
    assert!(rt.state.side_open());

    // JSON persistence roundtrip.
    let saved = rt.state.side.to_json();
    let mut restored = SideState::default();
    restored.restore_json(&saved);
    assert!(restored.sessions.get(&s1).is_some_and(|st| st.open));
}
