//! The navigation prototype's pinned sessions above the side tabs (`pinned-area.tsx`'s
//! `PinnedArea`, vertical). The order is Recent's `sidebar.recent` document; the close action
//! only changes the tab store. Pin groups (`shell/tabs/pins.ts`) are not here yet.
use super::*;
use crate::session_menu::{Spot, Target, preferences};
use opencode_kit::servers::ServerKey;
use rune_kit::shell::{TabItem, TabStrip};

pub(crate) fn muted_mark(p: &Pal) -> El {
    // `MutedIcon`, tab-nav.tsx.
    static BELL: Icon = Icon { name: "sidebar-muted", view: 16.0, body: r#"<path d="M11.89 4.13C11.24 2.61 9.74 1.56 8 1.56C5.67 1.56 3.78 3.45 3.78 5.78V9.56C3.78 10.54 2.98 11.33 2 11.33H4.67M14.22 1.78L1.78 14.22M8.44 11.33H14C13.02 11.33 12.22 10.54 12.22 9.56V7.55M9.33 13.68C9.07 14.14 8.57 14.44 8 14.44C7.43 14.44 6.93 14.14 6.67 13.68" stroke="currentColor" fill="none"/>"# };
    icon(&BELL, 14.0).key("muted").color(p.icon_faint).help(t!("sidebar.recent.muted"))
}

/// The kit's decorated element wraps the entire tab. Reserve a column beside the
/// pill so its title and close button keep their own space.
pub(crate) fn marked_slot(slot: El, mark: El) -> El {
    slot.pr(22.0).child(el().pin(None, Some(0.0), Some(4.0), Some(0.0)).w(16.0).center().child(mark))
}

pub(crate) fn section(s: &App, p: &Pal) -> (Option<El>, Vec<usize>) {
    let mut visible = vec![];
    let mut items = vec![];
    let mut recent: Vec<_> = (0..s.tabs.len()).collect();
    for pin in &s.project_state.recent.pins {
        let Some((server, id)) = super::state::split(pin) else { continue };
        let server = ServerKey::new(server);
        if preferences::hidden(&server, id) { continue; }
        let tab = s.tabs.iter().position(|tab| !tab.is_draft() && tab.is(&server, id));
        let info = (&server == crate::servers::runtime::active(s)).then(|| s.session(id)).flatten();
        if info.is_some_and(|i| i.archived.is_some()) { continue; }
        let closed = s.gestures.closed.entries().find(|c| c.tab.is(&server, id));
        let name = info.map(|i| i.title.as_str()).or_else(|| tab.map(|i| s.tabs[i].title.as_str())).or_else(|| closed.map(|c| c.tab.title.as_str()));
        let Some(name) = name else { continue };
        let target = Target::on(&server, id, if tab.is_some() { Spot::Tab } else { Spot::Row });
        recent.retain(|i| Some(*i) != tab);
        visible.push(pin.clone());
        let muted = preferences::muted(&server, id);
        let item = TabItem::new(pin.clone(), crate::tabs::tab_title(name)).closable(tab.is_some())
            .avatar(crate::icons::badge(s, p, info.and_then(|i| s.project_for_session(i)), name))
            .decorate(move |slot, _| {
                let slot = if muted { marked_slot(slot, muted_mark(p)) } else { slot };
                crate::session_menu::on_dialog(slot, target)
            });
        items.push(item);
    }
    if visible.is_empty() { return (None, recent); }
    let count = visible.len();
    let active = s.current_tab().and_then(|id| s.tab(&id)).map(|tab| super::state::key(tab.server.as_str(), &tab.id));
    let strip = TabStrip::new("project-pins", TabOrientation::Vertical).tabs(items).words(super::super::tab_strip::tab_words())
        .active(active.as_deref()).close_icon(&ic::XMARK_SMALL).fill(p.bg_deep).label(t!("sidebar.recent.pinned"))
        .on_select(|s: &mut App, key, tx| { if let Some((key, id)) = super::state::split(key) { crate::servers::runtime::switch(s, &ServerKey::new(key)); s.open(id, tx); } })
        .on_close(|s: &mut App, key, tx| { if let Some((key, id)) = super::state::split(key) { s.close_tab(&crate::tabs::session_uid(&ServerKey::new(key), id), tx); } })
        .drag_state(&s.gestures.tab_drag, |s: &mut App| &mut s.gestures.tab_drag)
        .on_reorder(move |s: &mut App, r, _| s.project_state.recent.reorder(&visible, r.from, r.to))
        .build(crate::palette::kit(p));
    let h = rune::memo::layout_with("project-pins-list", |r| r[3]).unwrap_or(count as f32 * 30.0);
    (Some(col().w_full().child(super::rows::section_title(p, "project-pins-title", t!("sidebar.recent.pinned"))).child(col().w_full().h(h).child(strip))), recent)
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    // Consolidated's Tabs: two sessions pinned, the second muted and its tab closed.
    ("sidebar-consolidated", |s| {
        crate::scenes::open_scene(s);
        s.finish_stream();
        crate::scenes::side_tabs(s);
        s.settings.sidebar_organize = crate::settings::Organize::Consolidated;
        let server = crate::servers::runtime::active(s).clone();
        let pins: Vec<_> = s.sessions.iter().filter(|i| i.parent.is_none()).take(2).map(|i| i.id.clone()).collect();
        for id in &pins { s.project_state.recent.pin(super::state::key(server.as_str(), id)); }
        if let Some(id) = pins.last() {
            preferences::scene_muted(&server, id);
            s.tabs.retain(|tab| !tab.is(&server, id));
        }
    }),
]);
