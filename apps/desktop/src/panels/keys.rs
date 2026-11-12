//! Panel polling, IME forwarding, and side/dock region keyboard shortcuts.

use std::collections::HashSet;

use rune::{Ime, Key, KeyEvent, Tx};

use crate::keymap::Bind;

use super::closing::{Effect, Event};
use super::state::{ResolvedEntry, ResolvedRegion};
use super::tabs::{SessionTabs, panel_key};
use super::SessionPane;
use crate::App;

impl App {
    pub fn select_adjacent_side_tab(&mut self, offset: i32, tx: &mut Tx) -> bool {
        let Some(reg) = self.resolve_side_region() else { return false };
        let pane = self.side.active_pane_focus;
        let keys: Vec<&str> = reg.pane_entries(pane).into_iter().map(|e| e.key.as_str()).collect();
        if keys.len() <= 1 {
            return false;
        }
        let empty = SessionTabs::default();
        let stored = self
            .side
            .sessions
            .get(&reg.session_id)
            .map_or(&empty, |s| &s.tab_state.tabs);
        let cur = reg.pane_active(pane, stored);
        if let Some(next) = crate::shortcuts::adjacent(&keys, cur.as_deref(), offset, |_| true) {
            self.select_side_tab(&next, tx);
            return true;
        }
        false
    }

    /// The last tab a close is sliding shut, which `pane` draws until the region lands, so the
    /// flight shows what it closes rather than an empty pane.
    /// Input runs after the frame's poll, so a tab shown since then already hides it.
    pub fn closing_side_entry(&self, reg: &ResolvedRegion, pane: SessionPane) -> Option<&ResolvedEntry> {
        let held = self.side.closing.held(&reg.session_id, pane)?;
        self.side_region_shut(&reg.session_id, Some(reg)).then_some(held)
    }

    /// `sid`'s side region is shut with nothing in it: no side tab, and no New tab on its way.
    fn side_region_shut(&self, sid: &str, reg: Option<&ResolvedRegion>) -> bool {
        let resolved;
        let reg = match reg {
            Some(r) => Some(r),
            None => {
                resolved = self.resolve_side_region();
                resolved.as_ref()
            }
        };
        self.active_session_id() == Some(sid)
            && !self.side_open()
            && reg.is_none_or(|r| r.strip.iter().all(|e| e.pane == SessionPane::Bottom))
            && !self.side.adding.iter().any(|n| n.session_id == sid && n.pane != SessionPane::Bottom)
    }

    pub(super) fn run_closing(&mut self, effects: Vec<Effect>, tx: &mut Tx) -> bool {
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
    pub fn poll_panels(&mut self, fresh: bool, tx: &mut Tx) -> bool {
        let mut changed = false;
        if let Some(sid) = self.side.closing.session().map(str::to_owned) {
            let shut = self.side_region_shut(&sid, None);
            let effects = self.side.closing.step(Event::Frame { clock: *self.clock, shut });
            changed |= self.run_closing(effects, tx);
        }
        let has_stored_tabs = fresh && self
            .active_session_id()
            .and_then(|sid| self.side.sessions.get(sid))
            .is_some_and(|st| !st.tab_state.tabs.all.is_empty());
        if has_stored_tabs
            && let Some(sid) = self.active_session_id().map(str::to_owned)
        {
            let panels = self.all_panels();
            if let Some(st) = self.side.sessions.get_mut(&sid) {
                for prov in &panels {
                    changed |= rekey_luau(&mut st.tab_state.tabs, prov.plugin);
                }
            }
            if panels.iter().any(|p| p.transient)
                && let Some(reg) = self.resolve_side_region()
            {
                let listed: HashSet<&str> = reg.entries.iter().map(|e| e.key.as_str()).collect();
                let transient_prefixes: Vec<String> = panels
                    .iter()
                    .filter(|p| p.transient)
                    .map(|p| format!("{}:", p.plugin))
                    .collect();
                if let Some(st) = self.side.sessions.get_mut(&sid) {
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
        changed |= self.sync_empty_side();
        // The native views Luau panels place (Review's diffs).
        changed |= crate::plugin_host::native::poll(self, tx);
        // The plugins: what came due, finished or arrived for them. At rest it enters no VM.
        changed |= crate::plugin_host::poll(self, fresh || changed, tx);
        changed
    }

    pub fn panels_ime(&mut self, ime: &Ime) -> bool {
        let any_open = self.side_open() || self.bottom_open();
        if !any_open || self.composer.focused {
            return false;
        }
        crate::plugin_host::native::on_ime(self, ime)
    }

    pub fn panels_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let is_close_key = Bind::TabClose.matches_tx(ev, tx);

        if self.side.emptied_guard {
            if is_close_key {
                return true;
            }
            self.side.emptied_guard = false;
        }

        if ev.key == Key::Escape && (self.side.tab_menu.is_some() || self.side.add_menu.is_some()) {
            tx.instant();
            self.side.tab_menu = None;
            self.side.add_menu = None;
            return true;
        }

        let on_session = self.active_session_id().is_some() && !self.settings.open;
        if !on_session {
            return false;
        }

        // 1. Toggle side region: ⌥⌘B or Desktop's ⇧⌘R (`review.toggle`).
        if Bind::SideToggle.matches_tx(ev, tx) {
            self.toggle_side_region(tx);
            return true;
        }

        // 2. The panels' commands (Toggle terminal, New terminal, Open review; chords in `keymap`).
        if crate::plugin_host::commands::panel_key(self, ev, tx) {
            return true;
        }

        // 3. ⌘J toggles `SessionPane::Bottom` when bottom tabs exist.
        if Bind::BottomToggle.matches_tx(ev, tx) {
            if self
                .resolve_side_region()
                .is_some_and(|r| r.strip.iter().any(|e| e.pane == SessionPane::Bottom))
            {
                self.toggle_bottom_region();
                return true;
            }
        }

        let region_visible = self.side_open() || self.bottom_open();
        if !region_visible {
            return false;
        }

        // 4. Active panels' own scoped key handlers (e.g. Browser's ⌘L, ⌘R, ⇧⌘C, or Terminal's keys),
        // and the native views Luau panels place.
        if crate::plugin_host::native::on_key(self, ev, tx) {
            return true;
        }

        if !self.side.focused {
            return false;
        }

        // 5. Side-region tab strip shortcuts when focus is in the side/bottom tab strip.
        if is_close_key {
            let sid = self.active_session_id().unwrap().to_owned();
            let pane = self.side.active_pane_focus;
            let pane_act = self.resolve_side_region().and_then(|reg| {
                let empty = SessionTabs::default();
                let stored = self.side.sessions.get(&sid).map_or(&empty, |s| &s.tab_state.tabs);
                reg.pane_active(pane, stored)
            });
            if let Some(nt) = self.shown_new_tab(&sid, pane, pane_act.as_deref()).cloned() {
                tx.instant();
                self.dismiss_side_new_tab(&nt.id);
                if !self.side_open() {
                    self.side.emptied_guard = true;
                }
                return true;
            }
            if let Some(act_key) = pane_act {
                let is_pinned = self
                    .resolve_side_region()
                    .and_then(|r| r.entry(&act_key).map(|e| !e.tab.can_close()))
                    .unwrap_or(false);
                if is_pinned && pane != SessionPane::Bottom {
                    self.toggle_side_region(tx);
                    self.side.emptied_guard = true;
                    return true;
                }
                self.close_side_tab(&act_key, tx);
                if !self.side_open() && pane != SessionPane::Bottom {
                    self.side.emptied_guard = true;
                }
                return true;
            }
            self.toggle_side_region(tx);
            self.side.emptied_guard = true;
            return true;
        }

        if Bind::PaneTabNew.matches_tx(ev, tx) {
            tx.instant();
            self.open_side_new_tab(self.side.active_pane_focus);
            return true;
        }

        if Bind::TabReopen.matches_tx(ev, tx) {
            return self.reopen_closed_side_tab(None, tx);
        }

        if let Some(idx) = Bind::tab_at(ev, tx) {
            if let Some(reg) = self.resolve_side_region() {
                let keys: Vec<String> = reg
                    .pane_entries(self.side.active_pane_focus)
                    .into_iter()
                    .map(|e| e.key.clone())
                    .collect();
                let target = if idx == 8 { keys.last() } else { keys.get(idx) };
                if let Some(k) = target {
                    let k = k.clone();
                    self.select_side_tab(&k, tx);
                    return true;
                }
            }
        }

        if Bind::PaneTabPrev.matches_tx(ev, tx) || Bind::TabPrev.matches_tx(ev, tx) {
            return self.select_adjacent_side_tab(-1, tx);
        }
        if Bind::PaneTabNext.matches_tx(ev, tx) || Bind::TabNext.matches_tx(ev, tx) {
            return self.select_adjacent_side_tab(1, tx);
        }

        false
    }
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
    tabs.panes = std::mem::take(&mut tabs.panes).into_iter().map(|(k, p)| (new_key(&k).unwrap_or(k), p)).collect();
    true
}