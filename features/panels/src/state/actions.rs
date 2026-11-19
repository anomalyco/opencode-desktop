//! Side, split, and bottom pane state mutations (open, close, select, move, toggle, reopen).

use std::collections::HashSet;

use rune::Tx;

use crate::closing::Event as ClosingEvent;
use crate::host::PanelsHost;
use crate::keys::run_closing;
use crate::tabs::{
    ClosedSideTab, SessionTabs, close_session_tab, move_session_tab, open_session_tab, place_session_tabs,
    preview_session_tab, push_closed_tab, session_tab_pane, split_panel_key, take_closed_tab,
};
use crate::{ClosedTabInfo, SessionPane};

use super::{
    FocusMode, NewTabEntry, SideMenu, Visibility, all_panels, find_panel, panes_shown, resolve_side_region,
    session_side_mut, shown_new_tab, side_open, side_stored_open,
};

/// Saved keyboard focus state for [`begin_keeping_keyboard`] / [`end_keeping_keyboard`].
#[derive(Clone, Copy, Debug)]
pub struct SavedKeyboardFocus {
    pub composer_focused: bool,
    pub side_focus: FocusMode,
    pub active_pane_focus: SessionPane,
}

/// Begins a background open (`background_open = true`) and snapshots current keyboard focus.
pub fn begin_keeping_keyboard(host: &mut dyn PanelsHost) -> SavedKeyboardFocus {
    let composer_focused = host.composer_focused();
    let side = host.side_mut();
    let saved = SavedKeyboardFocus {
        composer_focused,
        side_focus: side.focus.mode(),
        active_pane_focus: side.active_pane_focus,
    };
    side.background_open = true;
    saved
}

/// Restores the focus snapshot saved by [`begin_keeping_keyboard`] and clears `background_open`.
pub fn end_keeping_keyboard(host: &mut dyn PanelsHost, saved: SavedKeyboardFocus) {
    {
        let side = host.side_mut();
        side.background_open = false;
        side.focus.restore(saved.side_focus);
        side.active_pane_focus = saved.active_pane_focus;
    }
    host.set_composer_focused(saved.composer_focused);
}

/// Runs `open`, which shows a tab for someone other than the user (an agent's tool), leaving
/// the keyboard where it was: now, and when the tab's panel hears at the next poll that it
/// was selected, as a restored tab's does.
pub fn keeping_keyboard<H: PanelsHost, R>(host: &mut H, open: impl FnOnce(&mut H) -> R) -> R {
    let saved = begin_keeping_keyboard(host);
    let out = open(host);
    end_keeping_keyboard(host, saved);
    out
}

/// Marks `tx` instant if showing a pane tab did not change which panes are visible.
pub fn finish_show_pane_tab(host: &dyn PanelsHost, was: (bool, bool), tx: &mut Tx) {
    if panes_shown(host) == was {
        tx.instant();
    }
}

/// Lands a tab shown with `show` at once when its pane was already on screen, as switching
/// tabs does; a pane it opens flies on `SIDE_SPRING`, as the region's toggle does.
pub fn show_pane_tab<H: PanelsHost>(host: &mut H, tx: &mut Tx, show: impl FnOnce(&mut H)) {
    let was = panes_shown(host);
    show(host);
    finish_show_pane_tab(host, was, tx);
}

pub fn open_side_new_tab(host: &mut dyn PanelsHost, pane: SessionPane) -> String {
    host.wake_panel_launchers();
    let Some(sid) = host.active_session_id().map(str::to_owned) else {
        return String::new();
    };
    let over = resolve_side_region(host).and_then(|reg| {
        let empty = SessionTabs::default();
        let stored = host.side().sessions.get(&sid).map_or(&empty, |s| &s.tab_state.tabs);
        reg.pane_active(pane, stored)
    });
    let id = {
        let side = host.side_mut();
        let id = format!("new:{}", side.next_new_serial);
        side.next_new_serial += 1;
        side.adding.push(NewTabEntry {
            id: id.clone(),
            pane,
            session_id: sid.clone(),
            over,
        });
        side.chosen_new_tab.insert(pane, id.clone());
        side.focus.focus();
        side.active_pane_focus = pane;
        let st = side.sessions.entry(sid).or_default();
        if pane == SessionPane::Bottom {
            st.bottom_open = true;
        } else {
            st.visibility.step(true);
        }
        id
    };
    host.set_composer_focused(false);
    id
}

pub fn dismiss_side_new_tab(host: &mut dyn PanelsHost, new_id: &str) {
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    host.side_mut().adding.retain(|n| n.id != new_id);
    // Desktop's `dismiss` empties the region only when the strip holds no tab in any pane and
    // no New tab is left (`new-tab-machine.ts`); with a bottom tab the side region stays open.
    let strip_empty = resolve_side_region(host).is_none_or(|r| r.strip.is_empty());
    let any_new = host.side().adding.iter().any(|n| n.session_id == sid);
    if strip_empty && !any_new {
        let side = host.side_mut();
        side.sessions.entry(sid).or_default().visibility.step(false);
        side.focus.blur();
    }
}

/// Toggles the right-hand side region on its smooth spring (`⌥⌘B`, `⇧⌘R`, or header toggle).
pub fn toggle_side_region(host: &mut dyn PanelsHost, tx: &mut Tx) {
    host.wake_panel_launchers();
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    host.side_mut().menu = SideMenu::Closed;
    let was_open = side_stored_open(host);
    if was_open {
        // Closing gives focus back to the prompt when it was in the region (`panels.tsx`
        // `toggle`'s `giveBackFocus`), rather than to nothing.
        if host.side().focus.is_focused() {
            tx.return_focus("prompt", tx.focus_visible());
        }
        {
            let side = host.side_mut();
            side.sessions.entry(sid).or_default().visibility.step(false);
            side.focus.blur();
        }
        host.blur_panel_fields();
        return;
    }

    {
        let side = host.side_mut();
        side.sessions.entry(sid.clone()).or_default().visibility.step(true);
        side.focus.focus();
        side.active_pane_focus = SessionPane::Side;
    }
    // Opening moves focus into it (`panels.tsx` `toggle`): the prompt gives up the keyboard
    // below, and without this nothing had it, so a reader heard nothing.
    tx.focus_when_shown("side-panel", tx.focus_visible());

    // An empty strip shows a New tab at once rather than on the next frame's poll.
    if sync_empty_side(host) {
        host.set_composer_focused(false);
        return;
    }
    let entered = resolve_side_region(host).and_then(|r| {
        let act_key = r.active.as_deref()?;
        let entry = r.entry(act_key)?;
        Some((entry.provider, entry.tab.id.clone()))
    });
    if let Some((provider, tab_id)) = entered {
        provider.enter(host, &sid, &tab_id);
    }
}

/// Desktop's New tab `sync` (`new-tab-machine.ts`): a side region open with nothing in its
/// strip, hidden tabs included, shows a New tab once per such span. That covers opening it
/// empty and its last tab leaving without closing it (a terminal that exits). A side region
/// whose only tabs are in the bottom pane shows the empty pane, as Desktop's does.
pub fn sync_empty_side(host: &mut dyn PanelsHost) -> bool {
    let Some(id) = host.active_session_id().map(str::to_owned) else { return false };
    let stored_empty = host.side().sessions.get(&id).is_none_or(|st| st.tab_state.tabs.all.is_empty());
    let candidate = side_stored_open(host) && stored_empty;
    if candidate && host.side().empty_span.within(&id) {
        return false;
    }
    // Pinned tabs fill the strip without being stored.
    let open_empty = candidate && resolve_side_region(host).is_none_or(|r| r.strip.is_empty());
    if !host.side().empty_span.moves(&id, open_empty) || !host.side_mut().empty_span.step(&id, open_empty) {
        return false;
    }
    let woke = host.wake_panel_launchers();
    // One left from an earlier opening shows again rather than being joined by another.
    if shown_new_tab(host, &id, SessionPane::Side, None).is_some() {
        return woke;
    }
    // Focus stays where it is, as Desktop's does: a terminal exiting doesn't take it.
    let side = host.side_mut();
    let new = NewTabEntry {
        id: format!("new:{}", side.next_new_serial),
        pane: SessionPane::Side,
        session_id: id,
        over: None,
    };
    side.next_new_serial += 1;
    side.chosen_new_tab.insert(SessionPane::Side, new.id.clone());
    side.adding.push(new);
    true
}

pub fn toggle_bottom_region(host: &mut dyn PanelsHost) {
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    let side = host.side_mut();
    let st = side.sessions.entry(sid).or_default();
    st.bottom_open = !st.bottom_open;
    if st.bottom_open {
        side.focus.focus();
        side.active_pane_focus = SessionPane::Bottom;
    } else {
        side.focus.blur();
    }
}

/// Opens or focuses `key` (`"{plugin}:{tab_id}"`) in `session_id` (defaults to active session).
pub fn open_side_tab_for_session(
    host: &mut dyn PanelsHost,
    session_id: &str,
    key: &str,
    target_pane: Option<SessionPane>,
    preview: bool,
    first: bool,
) {
    let was = host
        .side()
        .sessions
        .get(session_id)
        .map_or((Visibility::Unseen, false), |st| (st.visibility, st.bottom_open));
    let transient_launchers: HashSet<String> = resolve_side_region(host)
        .map(|r| {
            r.entries
                .iter()
                .filter(|e| e.tab.transient)
                .map(|e| e.key.clone())
                .collect()
        })
        .unwrap_or_default();

    let default_pane = split_panel_key(key)
        .and_then(|(pl, _)| all_panels(host).into_iter().find(|p| p.plugin == pl))
        .map(|p| p.default_pane);
    let pane_choice = target_pane.or(default_pane);

    {
        let side = host.side_mut();
        let st = side.sessions.entry(session_id.to_owned()).or_default();
        st.tab_state = if preview {
            preview_session_tab(&st.tab_state, key, &transient_launchers, pane_choice)
        } else {
            open_session_tab(&st.tab_state, key, &transient_launchers, first, pane_choice)
        };
        let actual_pane = session_tab_pane(&st.tab_state.tabs, key);
        if actual_pane == SessionPane::Bottom {
            st.bottom_open = true;
        } else {
            st.visibility.step(true);
        }
        side.adding.retain(|n| !(n.session_id == session_id && n.pane == actual_pane));
        side.focus.focus();
        side.active_pane_focus = actual_pane;
    }

    if let Some((plugin, tab_id)) = split_panel_key(key)
        && let Some(prov) = find_panel(host, plugin)
    {
        prov.focus(host, session_id, tab_id, false);
    }
    host.place_narrow(session_id, key, was);
}

/// Hides the pane showing `key` (Desktop's `layout.hide`): the bottom pane, or the side region.
pub fn hide_side_tab(host: &mut dyn PanelsHost, key: &str, tx: &mut Tx) {
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    let is_bottom = host
        .side()
        .sessions
        .get(&sid)
        .is_some_and(|st| session_tab_pane(&st.tab_state.tabs, key) == SessionPane::Bottom);
    if is_bottom {
        let side = host.side_mut();
        side.sessions.entry(sid).or_default().bottom_open = false;
        side.focus.blur();
        return;
    }
    if side_stored_open(host) {
        toggle_side_region(host, tx);
    }
}

/// Desktop's `layout.open(key, session, { tab: "select" })`: adds `key` at the strip's end
/// when it isn't there, without replacing a preview tab, selects it, and shows its pane. A
/// new tab lands in `target`, else beside its plugin's last tab, else in its panel's default
/// pane.
pub fn show_side_tab(host: &mut dyn PanelsHost, session_id: &str, key: &str, target: Option<SessionPane>) {
    let was = host
        .side()
        .sessions
        .get(session_id)
        .map_or((Visibility::Unseen, false), |st| (st.visibility, st.bottom_open));
    let fallback = split_panel_key(key)
        .and_then(|(pl, _)| all_panels(host).into_iter().find(|p| p.plugin == pl))
        .map(|p| p.home_pane(host));
    let prefix = split_panel_key(key).map(|(pl, _)| format!("{pl}:")).unwrap_or_default();

    {
        let side = host.side_mut();
        let st = side.sessions.entry(session_id.to_owned()).or_default();
        let sibling = st.tab_state.tabs.all.iter().any(|k| k.starts_with(&prefix));
        let mut next = st.tab_state.tabs.clone();
        if !next.all.iter().any(|k| k == key) {
            next.all.push(key.to_owned());
        }
        next.active = Some(key.to_owned());
        let target = target.or(if sibling { None } else { fallback });
        st.tab_state.tabs = place_session_tabs(&st.tab_state.tabs, next, target);
        let pane = session_tab_pane(&st.tab_state.tabs, key);
        if pane == SessionPane::Bottom {
            st.bottom_open = true;
        } else {
            st.visibility.step(true);
        }
        side.adding.retain(|n| !(n.session_id == session_id && n.pane == pane));
    }

    if let Some((plugin, tab_id)) = split_panel_key(key)
        && let Some(prov) = find_panel(host, plugin)
    {
        prov.focus(host, session_id, tab_id, false);
    }
    host.place_narrow(session_id, key, was);
}

pub fn open_side_tab(
    host: &mut dyn PanelsHost,
    key: &str,
    target_pane: Option<SessionPane>,
    preview: bool,
    first: bool,
    _tx: &mut Tx,
) {
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    open_side_tab_for_session(host, &sid, key, target_pane, preview, first);
}

pub fn select_side_tab(host: &mut dyn PanelsHost, key: &str, tx: &mut Tx) {
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    // Switching tabs is always instant (rules.md).
    tx.instant();
    host.side_mut().menu.close_tab_menu();
    // A plugin's field in the tab left behind gives up the keyboard, as the app's own do.
    host.blur_panel_fields();
    host.on_side_tab_selected(key);
    let pane = {
        let side = host.side_mut();
        let st = side.sessions.entry(sid.clone()).or_default();
        let p = session_tab_pane(&st.tab_state.tabs, key);
        st.tab_state.tabs.active = Some(key.to_owned());
        st.tab_state.tabs.selected.insert(p, key.to_owned());
        side.adding.retain(|n| !(n.session_id == sid && n.pane == p));
        side.focus.focus();
        side.active_pane_focus = p;
        p
    };
    // A tab picked past the strip's edge, by a key or a click on its visible part, scrolls
    // into view, as Desktop's focused trigger does.
    tx.scroll_into_view_in(
        format!("side-tab-scroll-{}", pane.name()),
        format!("side-tab-{}", crate::view::safe_id(key)),
    );

    if let Some((plugin, tab_id)) = split_panel_key(key)
        && let Some(prov) = find_panel(host, plugin)
    {
        prov.focus(host, &sid, tab_id, false);
    }
}

pub fn move_side_tab(host: &mut dyn PanelsHost, key: &str, pane: SessionPane, before: Option<&str>, tx: &mut Tx) {
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    tx.instant();
    let st = session_side_mut(host, &sid);
    st.tab_state.tabs = move_session_tab(&st.tab_state.tabs, key, pane, before);
    if pane == SessionPane::Bottom {
        st.bottom_open = true;
    } else {
        st.visibility.step(true);
    }
}

pub fn promote_preview_tab(host: &mut dyn PanelsHost, key: &str) {
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    if host
        .side()
        .sessions
        .get(&sid)
        .is_none_or(|st| st.tab_state.preview.as_deref() != Some(key))
    {
        return;
    }
    session_side_mut(host, &sid).tab_state.preview = None;
}

pub fn close_side_tab(host: &mut dyn PanelsHost, key: &str, tx: &mut Tx) {
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    host.side_mut().menu.close_tab_menu();
    let reg = resolve_side_region(host);
    let entry = reg.as_ref().and_then(|r| r.entry(key)).cloned();
    if entry.as_ref().is_some_and(|e| !e.tab.can_close()) {
        return;
    }

    // Capture ClosedSideTab before removing the tab so the provider can still read its URL/state.
    let now_ms = host.now_ms();
    let closed_record = entry.as_ref().and_then(|e| {
        let info = e.provider.closed_info(host, &sid, &e.tab);
        match info {
            ClosedTabInfo::Deny => None,
            ClosedTabInfo::Default if e.provider.transient || e.tab.transient => None,
            ClosedTabInfo::Default => Some(ClosedSideTab {
                key: key.to_owned(),
                title: e.tab.title.clone(),
                icon_name: None,
                file: e.tab.file.clone(),
                detail: None,
                data: None,
                closed_ms: now_ms,
            }),
            ClosedTabInfo::Allow(custom) => Some(ClosedSideTab {
                key: key.to_owned(),
                title: custom.title.unwrap_or_else(|| e.tab.title.clone()),
                icon_name: custom.icon_name,
                file: e.tab.file.clone(),
                detail: custom.detail,
                data: custom.data,
                closed_ms: now_ms,
            }),
        }
    });

    let was_pane = {
        let st = session_side_mut(host, &sid);
        if let Some(rec) = closed_record {
            push_closed_tab(&mut st.closed, rec);
        }
        let p = session_tab_pane(&st.tab_state.tabs, key);
        st.tab_state = close_session_tab(&st.tab_state, key);
        p
    };

    if let Some(e) = &entry {
        host.on_side_tab_closing(&sid, key);
        e.provider.close(host, &sid, &e.tab.id, tx);
    }

    // As Desktop's region `close`: the bottom pane shuts with its last tab, and the side region
    // only once the whole strip is empty with no New tab left in its side pane. A side pane
    // whose last tab closed while the bottom pane holds one stays open on the empty pane.
    let next_reg = resolve_side_region(host);
    let strip_empty = next_reg.as_ref().is_none_or(|r| r.strip.is_empty());
    let side_new = host.side().adding.iter().any(|n| n.session_id == sid && n.pane != SessionPane::Bottom);
    let any_bottom = next_reg.as_ref().is_some_and(|r| r.strip.iter().any(|e| e.pane == SessionPane::Bottom));
    if was_pane == SessionPane::Bottom && !any_bottom {
        let side = host.side_mut();
        side.sessions.entry(sid.clone()).or_default().bottom_open = false;
        side.adding.retain(|n| !(n.session_id == sid && n.pane == SessionPane::Bottom));
        side.focus.blur();
    }
    if strip_empty && !side_new && side_stored_open(host) {
        // The side region slides shut on its spring; a side tab it closed stays on screen
        // until it lands.
        let at = host.clock();
        let effects = {
            let side = host.side_mut();
            side.sessions.entry(sid.clone()).or_default().visibility.step(false);
            side.focus.blur();
            entry
                .filter(|_| was_pane != SessionPane::Bottom)
                .map(|entry| side.closing.step(ClosingEvent::LastTabClosed { session_id: sid, entry: Box::new(entry), at }))
        };
        if let Some(effects) = effects {
            run_closing(effects, tx);
        }
        return;
    }
    if was_pane == SessionPane::Bottom {
        return;
    }
    // Remaining tabs close the gap at once (docs/kit-design.md: immediate).
    tx.instant();
    let next_focus = next_reg
        .as_ref()
        .and_then(|r| r.active.as_deref())
        .and_then(|k| split_panel_key(k).map(|(pl, tid)| (pl.to_owned(), tid.to_owned())));
    if let Some((pl, tid)) = next_focus
        && let Some(prov) = find_panel(host, &pl)
    {
        prov.focus(host, &sid, &tid, false);
    }
}

pub fn reopen_closed_side_tab(host: &mut dyn PanelsHost, key: Option<&str>, tx: &mut Tx) -> bool {
    let Some(sid) = host.active_session_id().map(str::to_owned) else {
        return false;
    };
    if host.side().sessions.get(&sid).is_none_or(|st| st.closed.is_empty()) {
        return false;
    }
    let Some(closed) = take_closed_tab(&mut session_side_mut(host, &sid).closed, key) else {
        return false;
    };
    if side_open(host) {
        tx.instant();
    }
    if let Some((plugin, tab_id)) = split_panel_key(&closed.key)
        && let Some(prov) = find_panel(host, plugin)
        && prov.restore(host, &sid, tab_id, closed.data.as_deref(), tx)
    {
        return true;
    }
    open_side_tab_for_session(host, &sid, &closed.key, None, false, false);
    true
}
