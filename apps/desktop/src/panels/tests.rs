use rune::{El, Env, Key, KeyEvent, Mods, Runtime, col, text};
use rune_theme_opencode::icons as ic;

use super::{
    DockPlacement, PanelRegistration, PanelSessionCtx, PanelTabDesc, PanelViewCtx, SideState,
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

fn mock_dock_list(_s: &App, _ctx: &PanelSessionCtx) -> Vec<PanelTabDesc> {
    vec![PanelTabDesc::new("main", "Terminal").icon(&ic::CONSOLE)]
}

fn mock_dock_render(_s: &App, p: &Pal, ctx: &PanelViewCtx) -> El {
    col()
        .key(if ctx.reserve_top_right { "mock-dock-reserved" } else { "mock-dock-stacked" })
        .w_full()
        .grow(1.0)
        .padding(12.0)
        .child(text("zsh · ~/project").font(UI).color(p.text))
}

#[test]
fn side_region_toggles_on_spring_and_launches_browser() {
    let mut rt = test_runtime();
    assert!(!rt.state.side_open());

    // ⌥⌘B opens the side region on its spring onto the pinned Review tab; `+` opens New tab.
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

    // ⌘3 jumps straight back to the third side tab (`notes:1`; ⌘1 is pinned Review, ⌘2 is Browser).
    rt.key(KeyEvent {
        key: Key::Command('3'),
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

    // Second ⌘W closes `browser:1`, falling back to pinned `review:changes`.
    rt.key(KeyEvent {
        key: Key::Command('w'),
        mods: Mods { cmd: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.side_open());
    assert_eq!(
        rt.state.resolve_side_region().and_then(|r| r.active).as_deref(),
        Some("review:changes")
    );

    // Third ⌘W on pinned `review:changes` collapses the side region and arms `emptied_guard`.
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
fn dock_region_stacks_under_side_tabs_and_supports_bottom_placement() {
    let mut rt = test_runtime();
    rt.state
        .side
        .extra_panels
        .push(PanelRegistration::dock("terminal", mock_dock_list, mock_dock_render).order(20));

    // 1. With side tabs closed, ⌘J opens the dock filling the right column (`reserve_top_right = true`).
    rt.key(KeyEvent {
        key: Key::Command('j'),
        mods: Mods { cmd: true, logo: true, ..Default::default() },
    });
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    assert!(!rt.state.side_open());
    assert!(rt.state.dock_side_open());
    assert!(rt.state.right_column_open());
    assert!(rt.find("mock-dock-reserved").is_some());

    // 2. Opening a Browser tab stacks `side-panel` over an 8px `dock-side-resize` handle over `dock-panel`.
    rt.update(|s, _| {
        s.open_new_browser_tab("http://localhost:5173/");
    });
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.state.side_open() && rt.state.dock_side_open());
    assert!(rt.presented("side-panel").is_some());
    assert!(rt.presented("dock-panel").is_some());
    let (resize_rect, _) = rt.presented("dock-side-resize").expect("8px vertical resize handle");
    assert!((resize_rect[3] - 8.0).abs() < 0.5, "expected 8px vertical resize handle, got {resize_rect:?}");
    assert!(rt.find("mock-dock-stacked").is_some());

    // 3. `terminalPlacement = "bottom"` moves the dock across the bottom of the session screen.
    rt.update(|s, tx| {
        tx.instant();
        s.settings.terminal_placement = DockPlacement::Bottom;
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.dock_bottom_open());
    assert!(rt.presented("dock-bottom-panel").is_some());
    assert!(rt.presented("dock-bottom-resize").is_some());
}

#[test]
fn per_session_persistence_roundtrips_and_isolates_sessions() {
    let mut rt = test_runtime();
    let s1 = rt.state.active_session_id().unwrap().to_owned();
    let s2 = rt.state.tabs.iter().map(|t| t.id.clone()).find(|id| id != &s1).unwrap();

    rt.update(|s, _| {
        s.side.resized = true;
        s.side.width = 540.0;
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
    assert_eq!(restored.width, 540.0);
    assert!(restored.resized);
    assert!(restored.sessions.get(&s1).is_some_and(|st| st.open));
}
