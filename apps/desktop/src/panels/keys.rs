//! Panel polling, IME forwarding, and side/dock region keyboard shortcuts.

use std::collections::HashSet;

use rune::{Ime, Key, KeyEvent, Tx};

use crate::keymap::Bind;

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

    pub fn poll_panels(&mut self, tx: &mut Tx) -> bool {
        let mut changed = false;
        let has_stored_tabs = self
            .active_session_id()
            .and_then(|sid| self.side.sessions.get(sid))
            .is_some_and(|st| !st.tab_state.tabs.all.is_empty());
        if has_stored_tabs
            && let Some(sid) = self.active_session_id().map(str::to_owned)
        {
            let panels = self.all_panels();
            if let Some(st) = self.side.sessions.get_mut(&sid) {
                for prov in &panels {
                    for &(old, new_id) in prov.legacy {
                        for k in &mut st.tab_state.tabs.all {
                            if k == old {
                                *k = panel_key(prov.plugin, new_id);
                                changed = true;
                            }
                        }
                    }
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
        // Static slice iteration on idle frames: 0 heap allocations.
        for p in super::registered_panels() {
            if let Some(poll) = p.poll {
                changed |= poll(self, tx);
            }
        }
        #[cfg(test)]
        if !self.side.extra_panels.is_empty() {
            let extras = self.side.extra_panels.clone();
            for p in extras {
                if let Some(poll) = p.poll {
                    changed |= poll(self, tx);
                }
            }
        }
        changed
    }

    pub fn panels_ime(&mut self, ime: &Ime) -> bool {
        let any_open = self.side_open() || self.bottom_open();
        if !any_open || self.composer.focused {
            return false;
        }
        for p in self.all_panels() {
            if let Some(on_ime) = p.on_ime
                && on_ime(self, ime)
            {
                return true;
            }
        }
        false
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

        // 2. Registered panels' launcher commands (Browser, Review, Files, Terminal; chords in `keymap`).
        for p in self.all_panels() {
            if let Some(sc) = p.shortcut
                && sc.matches_tx(ev, tx)
            {
                if let Some(run) = p.run {
                    run(self, tx);
                    return true;
                }
                if let Some(open_fn) = p.open_launcher {
                    open_fn(self, p.default_pane, tx);
                    return true;
                }
            }
            for &(sc, handler) in p.extra_shortcuts {
                if sc.matches_tx(ev, tx) {
                    handler(self, tx);
                    return true;
                }
            }
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

        // 4. Active panels' own scoped key handlers (e.g. Browser's ⌘L, ⌘R, ⇧⌘C, or Terminal's keys).
        for p in self.all_panels() {
            if let Some(on_key) = p.on_key
                && on_key(self, ev, tx)
            {
                return true;
            }
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
