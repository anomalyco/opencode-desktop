//! Built-in Chromium 156 Browser panel registered on `crate::panels::PANELS`, with `:port` URL
//! resolution and `@eN` visual element picker into `s.composer`.

pub mod prefetch;
#[cfg(test)]
mod tests;
pub mod view;

use std::collections::HashMap;

use browser_pane::{BrowserHost, EngineStatus, bare_url, host_of};
use rune::{Ime, Key, KeyEvent, TextEdit, Tx, Waker};
use rune_theme_opencode::icons as ic;

use crate::keymap::Bind;
use crate::App;
use crate::panels::{
    ClosedPanelTab, ClosedTabInfo, PanelLauncherDesc, PanelRegistration, PanelSessionCtx,
    PanelTabDesc, SessionPane, panel_key, split_panel_key,
};

pub struct BrowserPane {
    pub address: TextEdit,
    pub host: BrowserHost,
    pub session_tabs: HashMap<String, Vec<u64>>,
    pub ref_nodes: HashMap<(u64, String), i64>,
    pub next_ref: u64,
    pub prefetch: prefetch::IdleGate,
    last_synced_url: String,
    last_synced_tab: Option<u64>,
}

impl Default for BrowserPane {
    fn default() -> Self {
        Self {
            address: TextEdit::new(""),
            host: BrowserHost::new(),
            session_tabs: HashMap::new(),
            ref_nodes: HashMap::new(),
            next_ref: 1,
            prefetch: Default::default(),
            last_synced_url: String::new(),
            last_synced_tab: None,
        }
    }
}

rune_register::register!(crate::panels::PANELS: PanelRegistration = [
    PanelRegistration::new("browser", list_tabs, view::render_panel)
        .order(15)
        .command("Open browser", Bind::BrowserOpen)
        .launcher(launcher_desc, open_from_launcher)
        .on_close(on_close_tab)
        .closed(closed_tab_info)
        .on_restore(on_restore_tab)
        .on_focus(on_focus_tab)
        .on_enter(on_enter_tab)
        .poll(poll_panel)
        .on_key(key_panel)
        .on_ime(ime_panel),
]);

fn launcher_desc(_s: &App, _ctx: &PanelSessionCtx) -> Option<PanelLauncherDesc> {
    Some(PanelLauncherDesc {
        id: "open",
        title: "Browser".into(),
        description: Some("Open a local port or web page".into()),
        icon: &ic::GLOBE,
        shortcut: &[Bind::BrowserOpen],
        order: 15,
        opened: false,
    })
}

fn open_from_launcher(s: &mut App, pane: SessionPane, _tx: &mut Tx) {
    s.open_new_browser_tab_in("about:blank", Some(pane));
    if !s.browser_unsupported() {
        s.composer.focused = false;
        s.browser.address.focused = true;
    }
}

fn list_tabs(s: &App, ctx: &PanelSessionCtx) -> Vec<PanelTabDesc> {
    let mut out = Vec::new();
    for raw_id in ctx.open_ids {
        let Ok(id) = raw_id.parse::<u64>() else { continue };
        let Some(tab) = s.browser.host.tab(id) else {
            // Keep placeholder for a restored tab id if not yet materialized.
            out.push(
                PanelTabDesc::new(raw_id.clone(), "Browser")
                    .icon(&ic::GLOBE)
                    .group("browser")
                    .fallback(),
            );
            continue;
        };
        let label = tab.label();
        out.push(
            PanelTabDesc::new(id.to_string(), label)
                .icon(&ic::GLOBE)
                .group("browser")
                .fallback(),
        );
    }
    out
}

fn on_close_tab(s: &mut App, session_id: &str, tab_id: &str, _tx: &mut Tx) {
    if let Ok(id) = tab_id.parse::<u64>() {
        if let Some(vec) = s.browser.session_tabs.get_mut(session_id) {
            vec.retain(|&x| x != id);
        }
        s.browser.ref_nodes.retain(|(tid, _), _| *tid != id);
        s.browser.host.close_tab(id);
        s.sync_browser_address(true);
    }
}

fn closed_tab_info(s: &App, _session_id: &str, tab: &PanelTabDesc) -> ClosedTabInfo {
    let Ok(id) = tab.id.parse::<u64>() else {
        return ClosedTabInfo::Deny;
    };
    let Some(btab) = s.browser.host.tab(id) else {
        return ClosedTabInfo::Deny;
    };
    if btab.url.is_empty() || btab.url == "about:blank" {
        return ClosedTabInfo::Deny;
    }
    ClosedTabInfo::Allow(ClosedPanelTab {
        title: Some(tab.title.clone()),
        icon_name: Some("globe".into()),
        detail: Some(host_of(&btab.url)),
        data: Some(btab.url.clone()),
    })
}

fn on_restore_tab(s: &mut App, _session_id: &str, _tab_id: &str, data: Option<&str>, _tx: &mut Tx) {
    let url = data.unwrap_or("about:blank");
    s.open_new_browser_tab(url);
}

fn on_focus_tab(s: &mut App, _session_id: &str, tab_id: &str, _restored: bool) {
    if let Ok(id) = tab_id.parse::<u64>() {
        s.browser.host.focus_tab(id);
        s.sync_browser_address(true);
    }
}

fn on_enter_tab(s: &mut App, _session_id: &str, tab_id: &str) {
    if let Ok(id) = tab_id.parse::<u64>() {
        s.browser.host.focus_tab(id);
        let is_blank = s
            .browser
            .host
            .tab(id)
            .is_some_and(|t| t.url.is_empty() || t.url == "about:blank");
        if is_blank && !s.browser_unsupported() {
            s.composer.focused = false;
            s.browser.address.focused = true;
        }
        s.sync_browser_address(true);
    }
}

fn poll_panel(s: &mut App, tx: &mut Tx) -> bool {
    s.poll_browser(tx)
}

fn key_panel(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    s.browser_scoped_key(ev, tx)
}

fn ime_panel(s: &mut App, ime: &Ime) -> bool {
    s.browser_ime(ime)
}


/// App-level shortcuts that a focused web page must yield to OpenCode (Rune) rather than swallow.
pub fn claims_app_shortcut(ev: &KeyEvent) -> bool {
    // The page's own editing keys: select all, copy, paste, cut, undo, redo, and reload. Off
    // macOS Ctrl+R is also Rename, which leaves a focused page alone, as Desktop's does.
    if PAGE_KEYS.iter().any(|c| c.matches(ev)) {
        return false;
    }
    if crate::shortcuts::action(ev).is_some() {
        return true;
    }
    // Rename, Move to background and the terminal toggle, whose chords name Ctrl.
    if [Bind::Rename, Bind::Background, Bind::DockToggle].iter().any(|b| b.matches(ev)) {
        return true;
    }
    // Shell, side-region, picker, knobs, storybook, and dictation chords: any with the primary key.
    matches!(&ev.key, Key::Command(c) if matches!(
        c.to_ascii_lowercase(),
        'k' | 'l' | 'b' | 'j' | 'r' | 'p' | 'o' | 'c' | 'd' | 'm' | '\'' | '.' | ',' | '[' | ']' | '{' | '}'
    ))
}

static PAGE_KEYS: std::sync::LazyLock<[rune::Chord; 7]> = std::sync::LazyLock::new(|| {
    ["mod+a", "mod+c", "mod+v", "mod+x", "mod+z", "mod+r", "mod+shift+z"].map(|c| rune::Chord::parse(c).expect("chord"))
});

impl App {
    pub fn setup_browser(&mut self, waker: Waker) {
        self.browser.host.set_waker(waker);
        self.browser.host.set_shell_shortcut(claims_app_shortcut);
        if self.persists()
            && let Some(dir) = crate::dirs::data()
        {
            self.browser.host.remember_site_zoom(dir.join("browser-zoom.json"));
        }
    }

    /// No engine drives live tabs in this build: they show the unsupported card and have no address
    /// bar to focus. Mock tabs (headless, tests, stories) still work.
    pub(crate) fn browser_unsupported(&self) -> bool {
        !self.use_mock_browser() && self.browser.host.status() == EngineStatus::Unsupported
    }

    pub(crate) fn use_mock_browser(&self) -> bool {
        (crate::headless() || cfg!(test) || self.stage.is_some())
            && !self.browser.host.has_socket_override()
            && std::env::var_os("OPENCODE_BROWSER_LIVE_TEST").as_deref() != Some(std::ffi::OsStr::new("1"))
    }

    /// Zero-allocation lookup of the active browser tab ID in the routed session.
    pub fn active_browser_tab_id(&self) -> Option<u64> {
        self.active_browser_tab_for_session(self.active_session_id()?)
    }

    /// Active browser tab ID in `sid`'s side panel, if a browser tab is currently active there.
    pub fn active_browser_tab_for_session(&self, sid: &str) -> Option<u64> {
        let st = self.side.sessions.get(sid)?;
        let act = st.tab_state.tabs.active.as_deref()?;
        let (plugin, tid) = split_panel_key(act)?;
        if plugin != "browser" {
            return None;
        }
        let pane = crate::panels::tabs::session_tab_pane(&st.tab_state.tabs, act);
        if self
            .side
            .adding
            .iter()
            .any(|n| n.session_id == sid && n.pane == pane && n.over.as_deref() == Some(act))
        {
            return None;
        }
        tid.parse::<u64>().ok()
    }

    /// Best default browser tab for `sid`: its active browser tab, else its most recent live tab,
    /// else the host's focused tab.
    pub fn default_browser_tab_for_session(&self, sid: &str) -> Option<u64> {
        self.active_browser_tab_for_session(sid)
            .or_else(|| {
                self.browser
                    .session_tabs
                    .get(sid)?
                    .iter()
                    .rev()
                    .copied()
                    .find(|&id| self.browser.host.tab(id).is_some())
            })
            .or_else(|| self.browser.host.focused_tab_id())
    }

    /// The pane of the active browser tab when focus is in it: its page or the address bar.
    fn focused_browser_pane(&self) -> Option<SessionPane> {
        if self.composer.focused {
            return None;
        }
        let sid = self.active_session_id()?;
        let key = panel_key("browser", &self.active_browser_tab_id()?.to_string());
        let st = self.side.sessions.get(sid)?;
        let pane = crate::panels::tabs::session_tab_pane(&st.tab_state.tabs, &key);
        let in_pane = self.side.focused && !self.side.dock_focused && self.side.active_pane_focus == pane;
        (self.browser.address.focused || in_pane).then_some(pane)
    }

    pub fn open_browser_url(&mut self, url: &str) {
        if let Some(id) = self.active_browser_tab_id() {
            let resolved = browser_pane::resolve_address(url);
            self.browser.host.navigate(id, &resolved);
            if let Some(sid) = self.active_session_id().map(str::to_owned) {
                self.session_side_mut(&sid).open = true;
                self.side.mounted = true;
            }
            self.sync_browser_address(true);
        } else {
            self.open_new_browser_tab(url);
        }
    }

    pub fn open_new_browser_tab(&mut self, url: &str) -> u64 {
        self.open_new_browser_tab_in(url, None)
    }

    pub fn open_new_browser_tab_in(&mut self, url: &str, target_pane: Option<SessionPane>) -> u64 {
        let sid = self.active_session_id().map(str::to_owned);
        self.open_new_browser_tab_for_session(sid.as_deref(), url, target_pane, true)
    }

    pub fn open_new_browser_tab_for_session(
        &mut self,
        sid: Option<&str>,
        url: &str,
        target_pane: Option<SessionPane>,
        steal_focus: bool,
    ) -> u64 {
        let saved_focus = (self.composer.focused, self.side.focused, self.side.dock_focused);
        let id = if self.use_mock_browser() {
            let resolved = browser_pane::resolve_address(url);
            let title = if resolved == "about:blank" { "New tab" } else { &resolved };
            self.browser.host.insert_mock_tab(&resolved, title, EngineStatus::Ready)
        } else {
            self.browser.host.open_tab(url)
        };
        if let Some(sid) = sid {
            self.browser.session_tabs.entry(sid.to_owned()).or_default().push(id);
            let key = panel_key("browser", &id.to_string());
            self.open_side_tab_for_session(sid, &key, target_pane, false, false);
        }
        if !steal_focus {
            (self.composer.focused, self.side.focused, self.side.dock_focused) = saved_focus;
        }
        self.sync_browser_address(true);
        id
    }

    pub fn register_existing_browser_tab(&mut self, id: u64) {
        if let Some(sid) = self.active_session_id().map(str::to_owned) {
            self.register_existing_browser_tab_for_session(&sid, id, true);
        } else {
            self.sync_browser_address(true);
        }
    }

    pub fn register_existing_browser_tab_for_session(&mut self, sid: &str, id: u64, steal_focus: bool) {
        let saved_focus = (self.composer.focused, self.side.focused, self.side.dock_focused);
        let vec = self.browser.session_tabs.entry(sid.to_owned()).or_default();
        if !vec.contains(&id) {
            vec.push(id);
        }
        let key = panel_key("browser", &id.to_string());
        self.open_side_tab_for_session(sid, &key, None, false, false);
        if !steal_focus {
            (self.composer.focused, self.side.focused, self.side.dock_focused) = saved_focus;
        }
        self.sync_browser_address(true);
    }

    pub fn close_browser_tab_for_session(&mut self, sid: &str, id: u64, tx: &mut Tx) {
        let saved_focus = (self.composer.focused, self.side.focused, self.side.dock_focused);
        let key = panel_key("browser", &id.to_string());
        if self.active_session_id() == Some(sid) {
            self.close_side_tab(&key, tx);
        } else {
            for vec in self.browser.session_tabs.values_mut() {
                vec.retain(|&x| x != id);
            }
            self.browser.ref_nodes.retain(|(tid, _), _| *tid != id);
            let st = self.session_side_mut(sid);
            st.tab_state = crate::panels::tabs::close_session_tab(&st.tab_state, &key);
            if st.tab_state.tabs.all.is_empty() {
                st.open = false;
            }
            self.browser.host.close_tab(id);
        }
        for vec in self.browser.session_tabs.values_mut() {
            vec.retain(|&x| x != id);
        }
        self.browser.ref_nodes.retain(|(tid, _), _| *tid != id);
        if self.browser.host.tab(id).is_some() {
            self.browser.host.close_tab(id);
        }
        (self.composer.focused, self.side.focused, self.side.dock_focused) = saved_focus;
        self.sync_browser_address(true);
    }

    pub fn submit_browser_address(&mut self) {
        let raw = self.browser.address.text.trim().to_owned();
        self.browser.address.focused = false;
        if raw.is_empty() {
            return;
        }
        let resolved = browser_pane::resolve_address(&raw);
        if self.use_mock_browser() {
            if let Some(old_id) = self.active_browser_tab_id() {
                let new_id = self.browser.host.insert_mock_tab(
                    &resolved,
                    &bare_url(&resolved),
                    EngineStatus::Ready,
                );
                if let Some(sid) = self.active_session_id().map(str::to_owned) {
                    let old_key = panel_key("browser", &old_id.to_string());
                    let new_key = panel_key("browser", &new_id.to_string());
                    let st = self.session_side_mut(&sid);
                    for k in &mut st.tab_state.tabs.all {
                        if *k == old_key {
                            *k = new_key.clone();
                        }
                    }
                    if st.tab_state.tabs.active.as_deref() == Some(old_key.as_str()) {
                        st.tab_state.tabs.active = Some(new_key.clone());
                    }
                    for v in st.tab_state.tabs.selected.values_mut() {
                        if *v == old_key {
                            *v = new_key.clone();
                        }
                    }
                    if let Some(vec) = self.browser.session_tabs.get_mut(&sid) {
                        for slot in vec {
                            if *slot == old_id {
                                *slot = new_id;
                            }
                        }
                    }
                }
                self.browser.host.close_tab(old_id);
                self.browser.host.focus_tab(new_id);
            } else {
                self.open_new_browser_tab(&resolved);
            }
        } else if let Some(id) = self.active_browser_tab_id() {
            self.browser.host.navigate(id, &resolved);
        } else {
            self.open_new_browser_tab(&resolved);
        }
        self.sync_browser_address(true);
    }

    pub fn start_browser_install(&mut self) {
        if !self.browser.address.text.trim().is_empty() {
            self.submit_browser_address();
        }
        self.browser.host.start_install();
    }

    pub fn browser_scoped_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if !self.side_open() || self.browser_unsupported() {
            return false;
        }
        // ⌘L focuses the URL bar on an open browser tab, or opens a browser tab if on New tab.
        // Off macOS Ctrl+L is also Focus input, so there the address bar takes it only from the pane.
        if Bind::BrowserAddress.matches_tx(ev, tx) && (self.side.focused || !Bind::FocusInput.matches_tx(ev, tx)) {
            tx.instant();
            if self.active_browser_tab_id().is_none() {
                self.open_new_browser_tab("about:blank");
            }
            self.composer.focused = false;
            self.side.focused = true;
            self.browser.address.focused = true;
            let len = self.browser.address.text.len();
            self.browser.address.set_selection(0, len);
            return true;
        }

        // ⌘T in a browser tab's pane (page or address bar) opens another browser tab, like Brave.
        if Bind::BrowserTab.matches_tx(ev, tx)
            && let Some(pane) = self.focused_browser_pane()
        {
            tx.instant();
            self.open_new_browser_tab_in("about:blank", Some(pane));
            self.composer.focused = false;
            self.side.focused = true;
            self.side.active_pane_focus = pane;
            self.browser.address.focused = true;
            self.browser.address.set_selection(0, self.browser.address.text.len());
            return true;
        }

        if self.browser.address.focused {
            match ev.key {
                Key::Enter if !ev.mods.shift => {
                    tx.instant();
                    self.submit_browser_address();
                    return true;
                }
                Key::Escape => {
                    tx.instant();
                    self.browser.address.focused = false;
                    self.sync_browser_address(true);
                    return true;
                }
                _ => {}
            }
        }

        let Some(active_id) = self.active_browser_tab_id() else {
            return false;
        };

        // ⌘R reloads the active browser tab when focus is in the side region.
        if self.side.focused && Bind::BrowserReload.matches_tx(ev, tx) {
            self.browser.host.reload(active_id);
            return true;
        }

        // ⇧⌘C toggles the @eN element picker (`browser.inspect`).
        if self.side.focused && Bind::BrowserInspect.matches_tx(ev, tx) {
            tx.instant();
            self.browser.host.toggle_inspect(active_id);
            return true;
        }

        false
    }

    pub fn browser_ime(&mut self, ime: &Ime) -> bool {
        if !self.side_open()
            || self.composer.focused
            || self.browser.address.focused
            || self.active_browser_tab_id().is_none()
        {
            return false;
        }
        self.browser.host.ime(ime)
    }

    pub fn poll_browser(&mut self, tx: &mut Tx) -> bool {
        self.poll_browser_prefetch(tx);
        let mut changed = self.browser.host.poll();
        // The engine paints only its focused tab, so the page on screen must be that tab (after a
        // session switch, say, which shows another session's page without selecting it).
        if self.side_open()
            && let Some(id) = self.active_browser_tab_id()
            && self.browser.host.focused_tab_id() != Some(id)
        {
            self.browser.host.focus_tab(id);
            changed = true;
        }
        if self.sync_browser_address(false) {
            changed = true;
        }
        for picked in self.browser.host.drain_picked_elements() {
            let label = format!("{} ({})", picked.eref, picked.selector);
            if !self.composer.text.is_empty() && !self.composer.text.ends_with(char::is_whitespace) {
                self.composer.insert(" ");
            }
            self.composer
                .insert_pill(format!("browser:{}", picked.eref), &label);
            self.composer.insert(" ");
            self.composer.focused = true;
            self.side.focused = false;
            self.browser.address.focused = false;
            tx.instant();
            changed = true;
        }
        changed
    }

    pub(crate) fn sync_browser_address_now(&mut self) {
        let _ = self.sync_browser_address(true);
    }

    pub(crate) fn sync_browser_address(&mut self, force: bool) -> bool {
        if self.browser.address.focused && !force {
            return false;
        }
        let target_tab = self
            .active_browser_tab_id()
            .and_then(|id| self.browser.host.tab(id))
            .or_else(|| self.browser.host.focused_tab());
        let (tab_id, url) = match target_tab {
            Some(t) => (Some(t.id), if t.url == "about:blank" { "" } else { t.url.as_str() }),
            None => (None, ""),
        };
        if !force
            && self.browser.last_synced_tab == tab_id
            && self.browser.last_synced_url == url
        {
            return false;
        }
        self.browser.last_synced_tab = tab_id;
        self.browser.last_synced_url = url.to_owned();
        if self.browser.address.text != url {
            self.browser.address = TextEdit::new(url);
            return true;
        }
        false
    }
}
