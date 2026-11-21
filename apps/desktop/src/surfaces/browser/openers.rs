//! Chrome-style opener-chain tab placement and initial `"New tab"` title hold (`UP-118`,
//! `anomalyco/opencode` `#65126`, `80d7c17d8e`: `tab-openers.ts`, `pane-machine.ts`).
//!
//! When a user opens a link in a new tab (Cmd/Ctrl-click, middle-click, `target="_blank"`, or
//! context menu "Open link/image in new tab"), the new tab inserts beside its opener (or after
//! the contiguous row of tabs that opener already opened in background order) and keeps `"New tab"`
//! until its document names itself or finishes loading.

use std::collections::{HashMap, HashSet};

use browser_pane::BrowserHost;

use crate::panels::{panel_key, tabs::split_panel_key};
use crate::{App, SessionPane};

use super::PLUGIN;

#[derive(Clone, Debug)]
struct UnnamedSnap {
    url: String,
    title: String,
    loading: bool,
    had_error: bool,
}

#[derive(Default)]
pub struct TabOpeners {
    /// Child tab id -> opener tab id (`tab-openers.ts`).
    openers: HashMap<u64, u64>,
    /// Tabs opened from a link whose first document has not named itself yet.
    unnamed: HashMap<u64, UnnamedSnap>,
    /// Last active browser tab per session for `openers_after_select`.
    last_selected: HashMap<String, u64>,
}

impl TabOpeners {
    /// Tab id after which a new tab opened from `opener` stands (`DetermineInsertionIndex`).
    pub fn placed_after(&self, strip_tabs: &[u64], opener: u64, front: bool) -> u64 {
        if front {
            return opener;
        }
        let Some(pos) = strip_tabs.iter().position(|&t| t == opener) else {
            return opener;
        };
        let mut family = HashSet::from([opener]);
        let mut last = opener;
        for &tab in &strip_tabs[pos + 1..] {
            let Some(&from) = self.openers.get(&tab) else { break };
            if !family.contains(&from) {
                break;
            }
            family.insert(tab);
            last = tab;
        }
        last
    }

    /// Updates the opener map after `opener` opens `tab` (`openersAfterOpen`).
    pub fn record_open(&mut self, tab: u64, opener: u64, front: bool, snap: Option<(String, String, bool, bool)>) {
        if front {
            self.openers.clear();
        }
        self.openers.insert(tab, opener);
        let (url, title, loading, had_error) = snap.unwrap_or_else(|| (String::new(), String::new(), true, false));
        self.unnamed.insert(tab, UnnamedSnap { url, title, loading, had_error });
    }

    /// Updates openers after switching selection from `from` to `to` (`openersAfterSelect`).
    pub fn on_select(&mut self, from: Option<u64>, to: u64) {
        let Some(from) = from else { return };
        if self.openers.is_empty() || from == to {
            return;
        }
        let next = self.openers.get(&to).copied();
        let previous = self.openers.get(&from).copied();
        if !(next == previous || next == Some(from) || previous == Some(to)) {
            self.openers.clear();
        }
    }

    /// Reparents children of closed `tab` to `tab`'s own opener (`openersAfterClose` / `FixOpeners`).
    pub fn on_close(&mut self, tab: u64) {
        self.unnamed.remove(&tab);
        if self.openers.is_empty() {
            return;
        }
        let parent = self.openers.remove(&tab);
        let children: Vec<u64> = self
            .openers
            .iter()
            .filter_map(|(&child, &op)| (op == tab).then_some(child))
            .collect();
        for child in children {
            match parent {
                Some(p) => { self.openers.insert(child, p); }
                None => { self.openers.remove(&child); }
            }
        }
    }

    /// Clears openers when the user manually addresses a tab, except a fresh blank tab at the end
    /// (`pane-machine.ts` `tab.addressed`).
    pub fn on_addressed(&mut self, is_blank_last: bool) {
        if !self.openers.is_empty() && !is_blank_last {
            self.openers.clear();
        }
    }

    pub fn is_unnamed(&self, tab: u64) -> bool {
        self.unnamed.contains_key(&tab)
    }

    pub fn unnamed_ids_sorted(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self.unnamed.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Advances unnamed link-opened tabs against current engine snapshots (`pane-machine.ts` `named`).
    pub fn sync_unnamed(&mut self, host: &BrowserHost) -> bool {
        if self.unnamed.is_empty() {
            return false;
        }
        let ids: Vec<u64> = self.unnamed.keys().copied().collect();
        let mut changed = false;
        for id in ids {
            let Some(tab) = host.tab(id) else {
                self.unnamed.remove(&id);
                changed = true;
                continue;
            };
            let Some(before) = self.unnamed.get(&id).cloned() else { continue };
            let blank = tab.url.is_empty() || tab.url == "about:blank";
            let is_named = if !tab.loading {
                (before.loading || before.had_error) && (blank || !tab.title.is_empty())
            } else {
                !blank && !tab.title.is_empty() && tab.title != before.title && tab.url == before.url
            };
            if is_named {
                self.unnamed.remove(&id);
                changed = true;
            } else {
                let now_err = tab.load_error.is_some();
                if before.url != tab.url || before.title != tab.title || before.loading != tab.loading || before.had_error != now_err {
                    self.unnamed.insert(
                        id,
                        UnnamedSnap {
                            url: tab.url.clone(),
                            title: tab.title.clone(),
                            loading: tab.loading,
                            had_error: now_err,
                        },
                    );
                }
            }
        }
        changed
    }
}

impl App {
    /// Opens a link's new tab beside `opener_id` in session `sid` (`UP-118`, `tab-openers.ts`).
    pub(crate) fn open_new_browser_tab_beside(
        &mut self,
        sid: Option<&str>,
        url: &str,
        opener_id: u64,
        front: bool,
        context: browser_pane::ContextKind,
        driver: Option<browser_pane::TabDriver>,
    ) -> u64 {
        let agent_driven = driver.is_some();
        let candidate = browser_pane::resolve_address(url);
        match self.resolve_preview_target(sid, &candidate, agent_driven) {
            super::preview::PreviewTarget::Preview { display_url, spec } => {
                return self.open_resolved_preview_tab(sid, &display_url, spec, None, front, driver);
            }
            super::preview::PreviewTarget::Refused(code) => {
                return self.open_refused_browser_tab(sid, code, None, front);
            }
            super::preview::PreviewTarget::NotPreview => {}
        }
        let id = if self.use_mock_browser() {
            let refused = browser_pane::refusal(&candidate, &[]);
            let resolved = if refused.is_some() { "about:blank".into() } else { candidate };
            let title = if resolved == "about:blank" { String::new() } else { resolved.clone() };
            let id = self.workspace_panes.browser.host.insert_mock_tab(&resolved, &title, browser_pane::EngineStatus::Ready);
            self.workspace_panes.browser.host.set_mock_history(id, false, false);
            if let Some(why) = refused {
                self.workspace_panes.browser.host.set_mock_refusal(id, why.code());
            }
            id
        } else {
            self.workspace_panes.browser.host.open_tab_with_context(url, Vec::new(), context, false, agent_driven)
        };
        if let Some(d) = driver {
            self.workspace_panes.browser.host.set_tab_driver(id, d);
        }
        let Some(sid) = sid else { return id };
        if agent_driven {
            self.attach_browser_tab_to_session(Some(sid), id, None, front);
            return id;
        }
        self.attach_browser_tab_beside(sid, id, opener_id, front);
        id
    }

    fn attach_browser_tab_beside(&mut self, sid: &str, id: u64, opener_id: u64, front: bool) {
        let opener_key = panel_key(PLUGIN, &opener_id.to_string());
        let (strip_browser_ids, opener_pane) = {
            let st = self.session_side_mut(sid);
            let ids: Vec<u64> = st
                .tab_state
                .tabs
                .all
                .iter()
                .filter_map(|k| {
                    let (plug, local) = split_panel_key(k)?;
                    (plug == PLUGIN).then(|| local.parse::<u64>().ok()).flatten()
                })
                .collect();
            let pane = st.tab_state.tabs.all.contains(&opener_key)
                .then(|| crate::panels::tabs::session_tab_pane(&st.tab_state.tabs, &opener_key));
            (ids, pane)
        };
        let Some(target_pane) = opener_pane else {
            self.attach_browser_tab_to_session(Some(sid), id, None, front);
            return;
        };
        let after_id = self.workspace_panes.browser.openers.placed_after(&strip_browser_ids, opener_id, front);
        let after_key = panel_key(PLUGIN, &after_id.to_string());
        let snap = self.workspace_panes.browser.host.tab(id).map(|t| {
            (t.url.clone(), t.title.clone(), t.loading, t.load_error.is_some())
        });
        self.workspace_panes.browser.openers.record_open(id, opener_id, front, snap);

        let vec = self.workspace_panes.browser.session_tabs.entry(sid.to_owned()).or_default();
        if let Some(pos) = vec.iter().position(|&x| x == after_id) {
            vec.insert(pos + 1, id);
        } else {
            vec.push(id);
        }

        let key = panel_key(PLUGIN, &id.to_string());
        if front {
            let focused_tab = self.workspace_panes.browser.host.focused_tab_id();
            let shared = self.settings.shared_side_panel;
            let side = self.kernel.slots.get_mut(&crate::panels::SIDE);
            let st = side.sessions.entry(sid.to_owned()).or_default();
            st.tab_state = crate::panels::tabs::open_session_tab_after(
                &st.tab_state,
                &key,
                &HashSet::new(),
                Some(&after_key),
                Some(target_pane),
            );
            if target_pane == SessionPane::Bottom {
                st.bottom_open = true;
            } else {
                st.visibility.step(true);
                if shared {
                    side.shared_open = true;
                }
            }
            side.focus.focus();
            side.active_pane_focus = target_pane;
            if focused_tab != Some(id) {
                self.workspace_panes.browser.host.focus_tab(id);
            }
            self.workspace_panes.browser.openers.last_selected.insert(sid.to_owned(), id);
        } else {
            // Background tab stays behind the current tab without changing selection or focus.
            let prev_focused = self.workspace_panes.browser.host.focused_tab_id();
            let st = self.session_side_mut(sid);
            st.tab_state.tabs = crate::panels::tabs::append_session_tab(&st.tab_state.tabs, &key, Some(&after_key));
            if let Some(prev) = prev_focused.filter(|&p| p != id) {
                self.workspace_panes.browser.host.focus_tab(prev);
            }
        }
    }

    pub(crate) fn note_browser_addressed(&mut self, id: u64) {
        let blank = self
            .workspace_panes
            .browser
            .host
            .tab(id)
            .is_some_and(|t| t.url.is_empty() || t.url == "about:blank");
        let last = self.browser_tab_session(id).is_some_and(|sid| {
            self.workspace_panes
                .browser
                .session_tabs
                .get(sid)
                .and_then(|v| v.last().copied())
                == Some(id)
        });
        self.workspace_panes.browser.openers.on_addressed(blank && last);
    }

    pub(crate) fn sync_browser_openers(&mut self) -> bool {
        if let Some(sid) = self.active_session_id().map(str::to_owned)
            && let Some(active) = self.active_browser_tab_id()
        {
            let prev = self.workspace_panes.browser.openers.last_selected.insert(sid, active);
            if prev != Some(active) {
                self.workspace_panes.browser.openers.on_select(prev, active);
            }
        }
        self.workspace_panes.browser.openers.sync_unnamed(&self.workspace_panes.browser.host)
    }

    pub fn browser_tab_unnamed(&self, id: u64) -> bool {
        self.workspace_panes.browser.openers.is_unnamed(id)
    }
}
