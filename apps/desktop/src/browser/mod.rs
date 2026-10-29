//! Resizable right-hand Chromium browser split pane (`crates/browser-pane` + on-demand CEF sidecar),
//! `@eN` visual element picker into the session composer, and local agent browser API.

pub mod agent_api;
#[cfg(test)]
mod tests;
pub mod view;

use browser_pane::{BrowserHost, EngineStatus};
use rune::{Ime, Key, KeyEvent, TextEdit, Tx, Waker};

use crate::App;

pub const BROWSER_DEFAULT_W: f32 = 520.0;
pub const BROWSER_MIN_W: f32 = 320.0;
pub const BROWSER_MAX_W: f32 = 960.0;

pub struct BrowserPane {
    pub open: bool,
    pub width: f32,
    pub drag_origin: Option<f32>,
    pub address: TextEdit,
    pub host: BrowserHost,
    pub api: Option<agent_api::AgentServer>,
    last_synced_url: String,
    last_synced_tab: Option<u64>,
}

impl Default for BrowserPane {
    fn default() -> Self {
        Self {
            open: false,
            width: BROWSER_DEFAULT_W,
            drag_origin: None,
            address: TextEdit::new(""),
            host: BrowserHost::new(),
            api: None,
            last_synced_url: String::new(),
            last_synced_tab: None,
        }
    }
}

impl App {
    pub fn setup_browser(&mut self, waker: Waker) {
        self.browser.host.set_waker(waker.clone());
        if self.live() && !crate::headless() && !cfg!(test) {
            self.browser.api = agent_api::AgentServer::start(waker);
        }
    }

    pub fn toggle_browser(&mut self, tx: &mut Tx) {
        self.browser.open = !self.browser.open;
        if self.browser.open && self.browser.host.tabs().is_empty() {
            self.open_new_browser_tab("about:blank");
        }
        tx.settle();
    }

    pub fn open_browser_url(&mut self, url: &str) {
        self.browser.open = true;
        if let Some(tab_id) = self.browser.host.focused_tab_id() {
            self.browser.host.navigate(tab_id, url);
        } else {
            self.open_new_browser_tab(url);
        }
        self.sync_browser_address(true);
    }

    pub fn open_new_browser_tab(&mut self, url: &str) -> u64 {
        let use_mock = (crate::headless() || cfg!(test) || self.stage.is_some())
            && std::env::var("OPENCODE_BROWSER_LIVE_TEST").as_deref() != Ok("1");
        let id = if use_mock {
            let resolved = browser_pane::resolve_address(url);
            let title = if resolved == "about:blank" { "New tab" } else { &resolved };
            self.browser.host.insert_mock_tab(&resolved, title, EngineStatus::Ready)
        } else {
            self.browser.host.open_tab(url)
        };
        self.sync_browser_address(true);
        id
    }

    pub fn submit_browser_address(&mut self) {
        let raw = self.browser.address.text.trim().to_owned();
        self.browser.address.focused = false;
        if raw.is_empty() {
            return;
        }
        let resolved = browser_pane::resolve_address(&raw);
        let use_mock = (crate::headless() || cfg!(test) || self.stage.is_some())
            && std::env::var("OPENCODE_BROWSER_LIVE_TEST").as_deref() != Ok("1");
        if use_mock {
            if let Some(id) = self.browser.host.focused_tab_id() {
                self.browser.host.close_tab(id);
            }
            self.browser.host.insert_mock_tab(&resolved, &resolved, EngineStatus::Ready);
        } else if let Some(id) = self.browser.host.focused_tab_id() {
            self.browser.host.navigate(id, &resolved);
        } else {
            self.browser.host.open_tab(&resolved);
        }
        self.sync_browser_address(true);
    }

    pub fn browser_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        // ⌥⌘B toggles the browser split pane from anywhere in the window.
        if ev.mods.cmd
            && ev.mods.alt
            && !ev.mods.ctrl
            && !ev.mods.shift
            && matches!(&ev.key, Key::Command('b' | 'B'))
        {
            self.toggle_browser(tx);
            return true;
        }
        if !self.browser.open {
            return false;
        }
        // ⌘L focuses and selects the browser URL bar.
        if ev.is_command('l') && !ev.mods.alt && !ev.mods.ctrl && !ev.mods.shift {
            self.composer.focused = false;
            self.browser.address.focused = true;
            let len = self.browser.address.text.len();
            self.browser.address.set_selection(0, len);
            return true;
        }
        if self.browser.address.focused {
            match ev.key {
                Key::Enter if !ev.mods.shift => {
                    self.submit_browser_address();
                    return true;
                }
                Key::Escape => {
                    self.browser.address.focused = false;
                    self.sync_browser_address(true);
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    pub fn browser_ime(&mut self, ime: &Ime) -> bool {
        if !self.browser.open || self.composer.focused || self.browser.address.focused {
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
        let (tab_id, url) = match self.browser.host.focused_tab() {
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
