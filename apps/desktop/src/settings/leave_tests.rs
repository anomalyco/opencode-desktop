//! Leaving Settings: the title bar's back button, and every page the sidebar opens.

use rune::Runtime;

use crate::fixture_ids::THIRD;
use crate::{App, Route, fixture_runtime};

pub(crate) fn in_settings(mut rt: Runtime<App>) -> Runtime<App> {
    rt.state.open_settings();
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert!(rt.find("settings").is_some());
    rt
}

/// The fixture with side tabs: since Top tabs became the default (42172690) the sidebar holds
/// no tabs.
fn side_tabs() -> Runtime<App> {
    let mut rt = fixture_runtime();
    rt.update(|s, tx| s.set_tab_layout(crate::layout_state::TabLayout::Vertical, tx));
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    rt
}

pub(crate) fn tap(rt: &mut Runtime<App>, target: &str) {
    let (x, y) = rt.find(target).unwrap_or_else(|| panic!("{target} is on screen"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

/// Settings is Desktop's two-pane screen (d5c7a898): Back to app leads its rail, under the
/// titlebar's row, whether the sidebar is open or not.
#[test]
fn the_back_button_leads_the_settings_rail() {
    for open in [true, false] {
        let mut rt = fixture_runtime();
        rt.state.sidebar_open = open;
        let mut rt = in_settings(rt);
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let rect = |rt: &Runtime<App>, key: &str| rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}")).rect;
        let (back, bar) = (rect(&rt, "settings-back"), rect(&rt, "titlebar"));
        assert!(back[1] >= bar[1] + bar[3] - 0.5, "under the titlebar: {back:?} {bar:?}");
        tap(&mut rt, "settings-back");
        assert!(!rt.state.settings.open);
    }
}

#[test]
fn home_in_the_sidebar_leaves_settings() {
    let mut rt = in_settings(fixture_runtime());
    tap(&mut rt, "home");
    assert!(!rt.state.settings.open);
    assert_eq!(rt.state.route, Route::Home);
}

#[test]
fn search_in_the_sidebar_leaves_settings() {
    let mut rt = in_settings(fixture_runtime());
    tap(&mut rt, "search");
    assert!(!rt.state.settings.open);
    assert!(rt.state.switcher.open);
}

#[test]
fn new_session_in_the_sidebar_leaves_settings() {
    let mut rt = in_settings(fixture_runtime());
    tap(&mut rt, "new");
    assert!(!rt.state.settings.open);
    assert!(matches!(rt.state.route, Route::Draft(_)));
}

#[test]
fn a_session_in_the_sidebar_leaves_settings_for_it() {
    let mut rt = in_settings(fixture_runtime());
    tap(&mut rt, &format!("t-{THIRD}"));
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert!(!rt.state.settings.open);
    assert_eq!(rt.state.route, Route::here(THIRD.into()));
}

#[test]
fn no_sidebar_row_reads_as_selected_in_settings_and_it_comes_back() {
    // The side tabs' rows; Settings' rail lights its own page.
    let selected = |rt: &Runtime<App>| rt.elements().into_iter().filter(|e| e.item_selected && !e.exiting && e.key.as_deref().is_some_and(|k| k.starts_with("t-"))).count();
    let rt = side_tabs();
    assert!(matches!(rt.state.route, Route::Session { .. }));
    let route = rt.state.route.clone();
    assert!(selected(&rt) > 0, "the open session is highlighted");
    let mut rt = in_settings(rt);
    assert_eq!(selected(&rt), 0, "nothing is highlighted in Settings");
    assert_eq!(rt.state.route, route, "Settings still returns to the same session");
    tap(&mut rt, "settings-back");
    assert!(selected(&rt) > 0, "the highlight is back");
    assert_eq!(rt.state.route, route);
}
