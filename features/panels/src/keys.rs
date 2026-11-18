//! Panel polling, IME forwarding, and side/dock region keyboard shortcuts.

use std::collections::HashSet;

use opencode_ui::keymap::Bind;
use rune::{Ime, Key, KeyEvent, Tx};

use crate::SessionPane;
use crate::closing::{Effect, Event};
use crate::host::PanelsHost;
use crate::state::{
    ResolvedEntry, ResolvedRegion, all_panels, bottom_open, close_side_tab, dismiss_side_new_tab, open_side_new_tab,
    reopen_closed_side_tab, resolve_side_region, select_side_tab, shown_new_tab, side_open, sync_empty_side,
    toggle_bottom_region, toggle_side_region,
};
use crate::tabs::{SessionTabs, panel_key};

pub fn select_adjacent_side_tab(host: &mut dyn PanelsHost, offset: i32, tx: &mut Tx) -> bool {
    let Some(reg) = resolve_side_region(host) else { return false };
    let pane = host.side().active_pane_focus;
    let keys: Vec<&str> = reg.pane_entries(pane).into_iter().map(|e| e.key.as_str()).collect();
    if keys.len() <= 1 {
        return false;
    }
    let empty = SessionTabs::default();
    let stored = host
        .side()
        .sessions
        .get(&reg.session_id)
        .map_or(&empty, |s| &s.tab_state.tabs);
    let cur = reg.pane_active(pane, stored);
    if let Some(next) = adjacent(&keys, cur.as_deref(), offset) {
        select_side_tab(host, &next, tx);
        return true;
    }
    false
}

/// The last tab a close is sliding shut, which `pane` draws until the region lands, so the
/// flight shows what it closes rather than an empty pane.
/// Input runs after the frame's poll, so a tab shown since then already hides it.
pub fn closing_side_entry<'a>(
    host: &'a dyn PanelsHost,
    reg: &ResolvedRegion,
    pane: SessionPane,
) -> Option<&'a ResolvedEntry> {
    let held = host.side().closing.held(&reg.session_id, pane)?;
    side_region_shut(host, &reg.session_id, Some(reg)).then_some(held)
}

/// `sid`'s side region is shut with nothing in it: no side tab, and no New tab on its way.
fn side_region_shut(host: &dyn PanelsHost, sid: &str, reg: Option<&ResolvedRegion>) -> bool {
    let resolved;
    let reg = match reg {
        Some(r) => Some(r),
        None => {
            resolved = resolve_side_region(host);
            resolved.as_deref()
        }
    };
    host.active_session_id() == Some(sid)
        && !side_open(host)
        && reg.is_none_or(|r| r.strip.iter().all(|e| e.pane == SessionPane::Bottom))
        && !host.side().adding.iter().any(|n| n.session_id == sid && n.pane != SessionPane::Bottom)
}

pub(crate) fn run_closing(effects: Vec<Effect>, tx: &mut Tx) -> bool {
    let mut changed = false;
    for effect in effects {
        match effect {
            Effect::WakeAfter(after) => tx.wake_after(after),
            Effect::Redraw => changed = true,
        }
    }
    changed
}

/// `fresh`: the view ran or an earlier poll changed something since the last poll, so what
/// derives from the app's state (tab keys, transient tabs, plugins' watched signals) may
/// have moved. Otherwise those checks are skipped: nothing they read changed.
pub fn poll_panels(host: &mut dyn PanelsHost, fresh: bool, tx: &mut Tx) -> bool {
    let mut changed = false;
    if let Some(sid) = host.side().closing.session().map(str::to_owned) {
        let shut = side_region_shut(host, &sid, None);
        let clock = host.clock();
        let effects = host.side_mut().closing.step(Event::Frame { clock, shut });
        changed |= run_closing(effects, tx);
    }
    let has_stored_tabs = fresh
        && host
            .active_session_id()
            .and_then(|sid| host.side().sessions.get(sid))
            .is_some_and(|st| !st.tab_state.tabs.all.is_empty());
    if has_stored_tabs
        && let Some(sid) = host.active_session_id().map(str::to_owned)
    {
        let panels = all_panels(host);
        let needs_rekey = host.side().sessions.get(&sid).is_some_and(|st| {
            panels
                .iter()
                .any(|prov| needs_rekey_luau(&st.tab_state.tabs, prov.plugin))
        });
        if needs_rekey && let Some(st) = host.side_mut().sessions.get_mut(&sid) {
            for prov in &panels {
                changed |= rekey_luau(&mut st.tab_state.tabs, prov.plugin);
            }
        }
        if panels.iter().any(|p| p.transient)
            && let Some(reg) = resolve_side_region(host)
        {
            let listed: HashSet<&str> = reg.entries.iter().map(|e| e.key.as_str()).collect();
            let transient_prefixes: Vec<String> = panels
                .iter()
                .filter(|p| p.transient)
                .map(|p| format!("{}:", p.plugin))
                .collect();
            let needs_prune = host.side().sessions.get(&sid).is_some_and(|st| {
                st.tab_state.tabs.all.iter().any(|k| {
                    !listed.contains(k.as_str())
                        && transient_prefixes.iter().any(|pref| k.starts_with(pref))
                })
            });
            if needs_prune && let Some(st) = host.side_mut().sessions.get_mut(&sid) {
                let before = st.tab_state.tabs.all.len();
                st.tab_state.tabs.all.retain(|k| {
                    listed.contains(k.as_str())
                        || !transient_prefixes.iter().any(|pref| k.starts_with(pref))
                });
                if st.tab_state.tabs.all.len() != before {
                    changed = true;
                }
            }
        }
    }
    // A strip its last tab left (a terminal that exited) shows a New tab, as Desktop's does.
    changed |= sync_empty_side(host);
    // The native views Luau panels place (Review's diffs).
    changed |= host.poll_native_panels(tx);
    // The plugins: what came due, finished or arrived for them. At rest it enters no VM.
    changed |= host.poll_plugins(fresh || changed, tx);
    changed
}

pub fn panels_ime(host: &mut dyn PanelsHost, ime: &Ime) -> bool {
    let any_open = side_open(host) || bottom_open(host);
    if !any_open || host.composer_focused() {
        return false;
    }
    host.handle_panel_ime(ime)
}

pub fn panels_key(host: &mut dyn PanelsHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let is_close_key = Bind::TabClose.matches_tx(ev, tx);

    if host.side().emptied_guard {
        if is_close_key {
            return true;
        }
        host.side_mut().emptied_guard = false;
    }

    if ev.key == Key::Escape && (host.side().tab_menu.is_some() || host.side().add_menu.is_some()) {
        tx.instant();
        let side = host.side_mut();
        side.tab_menu = None;
        side.add_menu = None;
        return true;
    }

    let on_session = host.active_session_id().is_some() && !host.settings_open();
    if !on_session {
        return false;
    }

    // 1. Toggle side region: ⌥⌘B or Desktop's ⇧⌘R (`review.toggle`).
    if Bind::SideToggle.matches_tx(ev, tx) {
        toggle_side_region(host, tx);
        return true;
    }

    // 2. The panels' commands (Toggle terminal, New terminal, Open review; chords in `keymap`).
    if host.handle_panel_command_key(ev, tx) {
        return true;
    }

    // 3. ⌘J toggles `SessionPane::Bottom` when bottom tabs exist.
    if Bind::BottomToggle.matches_tx(ev, tx)
        && resolve_side_region(host).is_some_and(|r| r.strip.iter().any(|e| e.pane == SessionPane::Bottom))
    {
        toggle_bottom_region(host);
        return true;
    }

    let region_visible = side_open(host) || bottom_open(host);
    if !region_visible {
        return false;
    }

    // 4. Active panels' own scoped key handlers (e.g. Browser's ⌘L, ⌘R, ⇧⌘C, or Terminal's keys),
    // and the native views Luau panels place.
    if host.handle_panel_key(ev, tx) {
        return true;
    }

    if !host.side().focused {
        return false;
    }

    // 5. Side-region tab strip shortcuts when focus is in the side/bottom tab strip.
    if is_close_key {
        let Some(sid) = host.active_session_id().map(str::to_owned) else { return false };
        let pane = host.side().active_pane_focus;
        let pane_act = resolve_side_region(host).and_then(|reg| {
            let empty = SessionTabs::default();
            let stored = host.side().sessions.get(&sid).map_or(&empty, |s| &s.tab_state.tabs);
            reg.pane_active(pane, stored)
        });
        if let Some(nt) = shown_new_tab(host, &sid, pane, pane_act.as_deref()).cloned() {
            tx.instant();
            dismiss_side_new_tab(host, &nt.id);
            if !side_open(host) {
                host.side_mut().emptied_guard = true;
            }
            return true;
        }
        if let Some(act_key) = pane_act {
            let is_pinned = resolve_side_region(host)
                .and_then(|r| r.entry(&act_key).map(|e| !e.tab.can_close()))
                .unwrap_or(false);
            if is_pinned && pane != SessionPane::Bottom {
                toggle_side_region(host, tx);
                host.side_mut().emptied_guard = true;
                return true;
            }
            close_side_tab(host, &act_key, tx);
            if !side_open(host) && pane != SessionPane::Bottom {
                host.side_mut().emptied_guard = true;
            }
            return true;
        }
        toggle_side_region(host, tx);
        host.side_mut().emptied_guard = true;
        return true;
    }

    if Bind::PaneTabNew.matches_tx(ev, tx) {
        tx.instant();
        let pane = host.side().active_pane_focus;
        open_side_new_tab(host, pane);
        return true;
    }

    if Bind::TabReopen.matches_tx(ev, tx) {
        return reopen_closed_side_tab(host, None, tx);
    }

    if let Some(idx) = Bind::tab_at(ev, tx)
        && let Some(reg) = resolve_side_region(host)
    {
        let keys: Vec<String> = reg
            .pane_entries(host.side().active_pane_focus)
            .into_iter()
            .map(|e| e.key.clone())
            .collect();
        let target = if idx == 8 { keys.last() } else { keys.get(idx) };
        if let Some(k) = target {
            let k = k.clone();
            select_side_tab(host, &k, tx);
            return true;
        }
    }

    if Bind::PaneTabPrev.matches_tx(ev, tx) || Bind::TabPrev.matches_tx(ev, tx) {
        return select_adjacent_side_tab(host, -1, tx);
    }
    if Bind::PaneTabNext.matches_tx(ev, tx) || Bind::TabNext.matches_tx(ev, tx) {
        return select_adjacent_side_tab(host, 1, tx);
    }

    false
}

/// The nearest other tab in `offset`'s direction, wrapping (`adjacentTabKey`).
fn adjacent(order: &[&str], current: Option<&str>, offset: i32) -> Option<String> {
    let n = order.len() as i32;
    let Some(index) = current.and_then(|c| order.iter().position(|k| *k == c)) else {
        return if offset < 0 { order.last() } else { order.first() }.map(|s| (*s).to_owned());
    };
    (1..n)
        .map(|step| order[(index as i32 + offset * step).rem_euclid(n) as usize])
        .next()
        .map(str::to_owned)
}

fn needs_rekey_luau(tabs: &SessionTabs, plugin: &str) -> bool {
    let old = format!("plugin:{plugin}.");
    tabs.all.iter().any(|k| k.starts_with(&old))
}

/// Moves Luau plugin `plugin`'s tabs stored under the host's old `plugin` panel
/// (`plugin:<plugin>.<panel>`, `plugin:<plugin>.<panel>/<instance>`) to its own namespace.
fn rekey_luau(tabs: &mut SessionTabs, plugin: &str) -> bool {
    let old = format!("plugin:{plugin}.");
    let new_key = |k: &str| -> Option<String> {
        let rest = k.strip_prefix(&old)?;
        Some(panel_key(plugin, rest.split_once('/').map_or(rest, |(_, instance)| instance)))
    };
    if !tabs.all.iter().any(|k| k.starts_with(&old)) {
        return false;
    }
    for k in tabs.all.iter_mut().chain(tabs.active.iter_mut()).chain(tabs.selected.values_mut()) {
        if let Some(n) = new_key(k) {
            *k = n;
        }
    }
    tabs.panes = std::mem::take(&mut tabs.panes)
        .into_iter()
        .map(|(k, p)| (new_key(&k).unwrap_or(k), p))
        .collect();
    true
}
