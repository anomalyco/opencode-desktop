//! Built-in Chromium 156 Browser panel registered on `crate::panels::PANELS`, with `:port` URL
//! resolution, `@eN` visual element picker into `s.composer`, and local agent HTTP API.

pub mod agent_api;
#[cfg(test)]
mod tests;
pub mod view;

use std::collections::HashMap;

use browser_pane::{BrowserHost, EngineStatus, bare_url, host_of};
use rune::{Ime, Key, KeyEvent, TextEdit, Tx, Waker};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::panels::{
    ClosedPanelTab, ClosedTabInfo, PanelLauncherDesc, PanelRegistration, PanelSessionCtx,
    PanelTabDesc, SessionPane, ShortcutChord, panel_key, split_panel_key,
};

pub struct BrowserPane {
    pub address: TextEdit,
    pub host: BrowserHost,
    pub api: Option<agent_api::AgentServer>,
    pub session_tabs: HashMap<String, Vec<u64>>,
    last_synced_url: String,
    last_synced_tab: Option<u64>,
}

impl Default for BrowserPane {
    fn default() -> Self {
        Self {
            address: TextEdit::new(""),
            host: BrowserHost::new(),
            api: None,
            session_tabs: HashMap::new(),
            last_synced_url: String::new(),
            last_synced_tab: None,
        }
    }
}

rune_register::register!(crate::panels::PANELS: PanelRegistration = [
    PanelRegistration::new("browser", list_tabs, view::render_panel)
        .order(15)
        .command("Open browser", ShortcutChord::ctrl_cmd('b', "⌃⌘B"))
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
        shortcut_hint: "⌃⌘B",
        order: 15,
        opened: false,
    })
}

fn open_from_launcher(s: &mut App, pane: SessionPane, _tx: &mut Tx) {
    s.open_new_browser_tab_in("about:blank", Some(pane));
    s.composer.focused = false;
    s.browser.address.focused = true;
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
        if let Some(api) = &mut s.browser.api {
            api.drop_tab_refs(id);
        }
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
        if is_blank {
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
    if crate::shortcuts::action(ev).is_some() {
        return true;
    }
    let m = ev.mods;
    // ⌃R (rename session) and ⌃B (move blocking shell to background).
    if m.ctrl && !m.cmd && !m.alt && matches!(&ev.key, Key::Command('r' | 'b') | Key::Text(_)) {
        if let Key::Text(s) = &ev.key {
            if s.eq_ignore_ascii_case("r") || s.eq_ignore_ascii_case("b") || s == "`" {
                return true;
            }
        } else {
            return true;
        }
    }
    match &ev.key {
        Key::Command(c) => {
            let lo = c.to_ascii_lowercase();
            // Shell, side-region, picker, knobs, storybook, and dictation chords:
            // Note: ⌘A/C/V/X/Z/R (unshifted, without alt/ctrl) stay with the web page!
            if !m.alt && !m.ctrl && !m.shift && matches!(lo, 'a' | 'c' | 'v' | 'x' | 'z' | 'r') {
                return false;
            }
            if !m.alt && !m.ctrl && m.shift && lo == 'z' {
                return false;
            }
            matches!(
                lo,
                'k' | 'l' | 'b' | 'j' | 'r' | 'p' | 'o' | 'c' | 'd' | 'm' | '\'' | '.' | ',' | '[' | ']' | '{' | '}'
            )
        }
        _ => false,
    }
}

impl App {
    pub fn setup_browser(&mut self, waker: Waker) {
        self.browser.host.set_waker(waker.clone());
        self.browser.host.set_shell_shortcut(claims_app_shortcut);
        if self.live() && !crate::headless() && !cfg!(test) {
            self.browser.api = agent_api::AgentServer::start(waker);
        }
    }

    fn use_mock_browser(&self) -> bool {
        (crate::headless() || cfg!(test) || self.stage.is_some())
            && std::env::var("OPENCODE_BROWSER_LIVE_TEST").as_deref() != Ok("1")
    }

    /// Zero-allocation lookup of the active browser tab ID in the routed session.
    pub fn active_browser_tab_id(&self) -> Option<u64> {
        let sid = self.active_session_id()?;
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
        let id = if self.use_mock_browser() {
            let resolved = browser_pane::resolve_address(url);
            let title = if resolved == "about:blank" { "New tab" } else { &resolved };
            self.browser.host.insert_mock_tab(&resolved, title, EngineStatus::Ready)
        } else {
            self.browser.host.open_tab(url)
        };
        if let Some(sid) = self.active_session_id().map(str::to_owned) {
            self.browser.session_tabs.entry(sid.clone()).or_default().push(id);
            let key = panel_key("browser", &id.to_string());
            self.open_side_tab_for_session(&sid, &key, target_pane, false, false);
        }
        self.sync_browser_address(true);
        id
    }

    pub fn register_existing_browser_tab(&mut self, id: u64) {
        if let Some(sid) = self.active_session_id().map(str::to_owned) {
            let vec = self.browser.session_tabs.entry(sid.clone()).or_default();
            if !vec.contains(&id) {
                vec.push(id);
            }
            let key = panel_key("browser", &id.to_string());
            self.open_side_tab_for_session(&sid, &key, None, false, false);
        }
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
        if !self.side_open() {
            return false;
        }
        // ⌘L focuses the URL bar on an open browser tab, or opens a browser tab if on New tab.
        if ev.is_command('l') && !ev.mods.alt && !ev.mods.ctrl && !ev.mods.shift {
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
        if self.side.focused
            && ev.mods.cmd
            && !ev.mods.alt
            && !ev.mods.ctrl
            && !ev.mods.shift
            && matches!(&ev.key, Key::Command('r' | 'R'))
        {
            self.browser.host.reload(active_id);
            return true;
        }

        // ⇧⌘C toggles the @eN element picker (`browser.inspect`).
        if self.side.focused
            && ev.mods.cmd
            && ev.mods.shift
            && !ev.mods.alt
            && !ev.mods.ctrl
            && matches!(&ev.key, Key::Command('c' | 'C'))
        {
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
        let mut changed = self.browser.host.poll();
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
        if agent_api::poll(self) {
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
