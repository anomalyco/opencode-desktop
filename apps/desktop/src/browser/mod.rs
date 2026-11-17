//! The window's embedded Chromium 156 browser (`browser_pane::BrowserHost`): its pages, the
//! session each belongs to, the engine's background download, and the DevTools calls in flight.
//! The browser plugin (`plugins/browser`) draws the pane, its address bar and its menus over
//! this through the `Browser` service and `ui.browserSurface` (`plugin_host::native::browser`).

mod scene;
pub mod view;

use std::collections::HashMap;
use rune::time::Instant;

use browser_pane::{BrowserHost, EngineStatus, bare_url};
use rune::{Key, KeyEvent, Tx, Waker};

use crate::App;
use crate::keymap::Bind;
use crate::panels::{SessionPane, panel_key, split_panel_key};

/// The browser plugin's id: its tabs key as `browser:<page id>`.
pub const PLUGIN: &str = "browser";

#[derive(Default)]
pub struct BrowserPane {
    pub host: BrowserHost,
    /// Each session's pages, oldest first.
    pub session_tabs: HashMap<String, Vec<u64>>,
    /// DevTools calls a plugin waits on, by the engine's request id: who to answer and when to
    /// give up.
    pub devtools: HashMap<u64, (rune_luau::Completer, Instant)>,
}

/// App-level shortcuts that a focused web page must yield to OpenCode (Rune) rather than swallow,
/// matching Desktop's `packages/gui-plugins/src/browser/keys.ts` (`pageShortcut`): in-page chords
/// such as ⌘K, ⌘B, ⌘F and ⌘G stay with the page.
pub fn claims_app_shortcut(ev: &KeyEvent) -> bool {
    if PAGE_KEYS.iter().any(|c| c.matches(ev)) {
        return false;
    }
    if matches!(
        crate::shortcuts::action(ev),
        Some(a) if !matches!(a, crate::shortcuts::Action::ZoomIn | crate::shortcuts::Action::ZoomOut | crate::shortcuts::Action::ZoomReset)
    ) {
        return true;
    }
    if [Bind::Rename, Bind::Background].iter().any(|b| b.matches(ev)) || crate::keymap::TERMINAL_TOGGLE.matches(ev) {
        return true;
    }
    let Key::Command(c) = ev.key else { return false };
    let ch = c.to_ascii_lowercase();
    if ev.mods.alt {
        return !ev.mods.shift && matches!(ch, 'r' | 'b' | 'p');
    }
    if ev.mods.shift {
        if ch == 'm' && matches!(crate::updater::build_channel(), "dev" | "local") { return true; }
        return matches!(ch, 't' | '[' | ']' | '{' | '}' | 'r' | 'p' | 'l' | 'e' | 'o' | 's' | 'c');
    }
    matches!(ch, 'l' | 't' | 'n' | 'w' | ',' | '\\' | '[' | ']')
}

static PAGE_KEYS: std::sync::LazyLock<[rune::Chord; 7]> = std::sync::LazyLock::new(|| {
    ["mod+a", "mod+c", "mod+v", "mod+x", "mod+z", "mod+r", "mod+shift+z"].map(|c| rune::Chord::parse(c).expect("chord"))
});

/// `url` when the default browser can open it: an `http:` or `https:` page, not `about:blank`,
/// `chrome:` or `data:`.
pub fn external_url(url: &str) -> Option<&str> {
    let scheme = url.split_once("://")?.0;
    (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")).then_some(url)
}

impl App {
    /// The page browser tab `id` shows, when the default browser can open it.
    pub fn browser_tab_external_url(&self, id: u64) -> Option<String> {
        external_url(&self.browser.host.tab(id)?.url).map(str::to_owned)
    }

    /// Opens browser tab `id`'s page in the default browser.
    pub fn open_browser_tab_externally(&mut self, id: u64, tx: &mut Tx) {
        if let Some(url) = self.browser_tab_external_url(id) {
            crate::open_in::open_external(tx, &url);
        }
    }

    pub fn setup_browser(&mut self, waker: Waker) {
        self.browser.host.set_waker(waker);
        self.browser.host.set_shell_shortcut(claims_app_shortcut);
        // The levels live in the app's state (`plugin.browser`, Desktop's browser plugin namespace),
        // in memory once the storage is up; a change goes behind like any other.
        if self.persists() {
            self.browser.host.remember_site_zoom(browser_pane::ZoomStore {
                read: Box::new(|| crate::storage::text("plugin.browser", "rune.siteZoom").map(|t| t.to_string())),
                write: Box::new(|levels| crate::storage::set("plugin.browser", "rune.siteZoom", levels)),
            });
        }
    }

    /// No engine drives live tabs in this build: they show the unsupported card and have no address
    /// bar to focus. Mock tabs (headless, tests, stories) still work.
    pub(crate) fn browser_unsupported(&self) -> bool {
        !self.use_mock_browser() && self.browser.host.status() == EngineStatus::Unsupported
    }

    pub(crate) fn use_mock_browser(&self) -> bool {
        (crate::headless() || self.stage.is_some())
            && !self.browser.host.has_socket_override()
            && std::env::var_os("OPENCODE_BROWSER_LIVE_TEST").as_deref() != Some(std::ffi::OsStr::new("1"))
    }

    /// A link to `path`, inside the session's folder, opens as a page in the browser pane rather
    /// than as a file tab, as Desktop's file plugin decides (`renderer.tsx` `fileOrPage`): an
    /// HTML file, the browser on and able to run, the app's own server (which shares this disk:
    /// the pane loads `file://` inside the workspace only) and a wide window.
    pub fn opens_as_page(&self, path: &str) -> bool {
        let html = std::path::Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"));
        html && !self.browser_unsupported()
            && !self.narrow()
            && crate::servers::runtime::active(self).is_sidecar()
            && self.plugins.list.iter().any(|p| p.manifest.id == PLUGIN && p.running())
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
        if plugin != PLUGIN {
            return None;
        }
        let pane = crate::panels::tabs::session_tab_pane(&st.tab_state.tabs, act);
        if self.side.adding.iter().any(|n| n.session_id == sid && n.pane == pane && n.over.as_deref() == Some(act)) {
            return None;
        }
        tid.parse::<u64>().ok()
    }

    /// Best default browser tab for `sid`: its active browser tab, else its most recent live tab,
    /// else the host's focused tab.
    pub fn default_browser_tab_for_session(&self, sid: &str) -> Option<u64> {
        self.active_browser_tab_for_session(sid)
            .or_else(|| self.browser.session_tabs.get(sid)?.iter().rev().copied().find(|&id| self.browser.host.tab(id).is_some()))
            .or_else(|| self.browser.host.focused_tab_id())
    }

    /// The pane of the routed session's selected browser tab while the keyboard is in it: its
    /// page, or a field of the browser plugin (its address field) when `field` says one has it.
    pub fn focused_browser_pane(&self, field: bool) -> Option<SessionPane> {
        if self.composer.focused {
            return None;
        }
        let sid = self.active_session_id()?;
        let key = panel_key(PLUGIN, &self.active_browser_tab_id()?.to_string());
        let st = self.side.sessions.get(sid)?;
        let pane = crate::panels::tabs::session_tab_pane(&st.tab_state.tabs, &key);
        let in_pane = self.side.focused && self.side.active_pane_focus == pane;
        (field || in_pane).then_some(pane)
    }

    /// A press on page `id`: the keyboard is in its pane now, as a click inside Desktop's side
    /// region makes it, so ⌘W, ⌘L and ⌘T act on the browser rather than the session's tab.
    pub fn focus_browser_page(&mut self, id: u64) {
        let Some(sid) = self.browser_tab_session(id).map(str::to_owned) else { return };
        let key = panel_key(PLUGIN, &id.to_string());
        let Some(st) = self.side.sessions.get(&sid) else { return };
        self.side.active_pane_focus = crate::panels::tabs::session_tab_pane(&st.tab_state.tabs, &key);
        self.side.focused = true;
        self.composer.focused = false;
    }

    /// Workspace directories whose `file://` documents may open in session `sid` (`pane.ts:162`):
    /// the session's folder when the active server shares this disk (`is_sidecar`), else empty.
    pub fn browser_file_roots(&self, sid: Option<&str>) -> Vec<String> {
        if !crate::servers::runtime::active(self).is_sidecar() {
            return Vec::new();
        }
        let dir = sid
            .or_else(|| self.active_session_id())
            .map(|id| self.link_directory(id))
            .unwrap_or_else(|| self.draft_dir().to_owned());
        if dir.is_empty() { Vec::new() } else { vec![dir] }
    }

    pub fn open_new_browser_tab(&mut self, url: &str) -> u64 {
        self.open_new_browser_tab_in(url, None)
    }

    pub fn open_new_browser_tab_in(&mut self, url: &str, target_pane: Option<SessionPane>) -> u64 {
        let sid = self.active_session_id().map(str::to_owned);
        self.open_new_browser_tab_for_session(sid.as_deref(), url, target_pane, true)
    }

    pub fn open_new_browser_tab_for_session(&mut self, sid: Option<&str>, url: &str, target_pane: Option<SessionPane>, steal_focus: bool) -> u64 {
        self.open_new_browser_tab_for_session_with(sid, url, target_pane, steal_focus, false)
    }

    pub fn open_new_browser_tab_for_session_with(
        &mut self,
        sid: Option<&str>,
        url: &str,
        target_pane: Option<SessionPane>,
        steal_focus: bool,
        from_address: bool,
    ) -> u64 {
        let roots = self.browser_file_roots(sid);
        let id = if self.use_mock_browser() {
            let candidate = if from_address { browser_pane::resolve_address(url) } else { browser_pane::with_scheme(url) };
            let refused = browser_pane::refusal(&candidate, &roots);
            let resolved = if refused.is_some() { "about:blank".into() } else { candidate };
            let title = if resolved == "about:blank" { String::new() } else { bare_url(&resolved) };
            let id = self.browser.host.insert_mock_tab(&resolved, &title, EngineStatus::Ready);
            self.browser.host.set_mock_history(id, false, false);
            if let Some(why) = refused {
                self.browser.host.set_mock_refusal(id, why.code());
            }
            id
        } else {
            self.browser.host.open_tab_with_roots(url, roots, from_address)
        };
        if let Some(sid) = sid {
            self.browser.session_tabs.entry(sid.to_owned()).or_default().push(id);
            let key = panel_key(PLUGIN, &id.to_string());
            if steal_focus {
                self.open_side_tab_for_session(sid, &key, target_pane, false, false);
            } else {
                self.keeping_keyboard(|s| s.open_side_tab_for_session(sid, &key, target_pane, false, false));
            }
        }
        id
    }

    /// Loads `url` (or a `:port`) in page `id`. A mock page shows it loaded at once.
    pub fn navigate_browser_tab(&mut self, id: u64, url: &str) {
        let sid = self.browser_tab_session(id).map(str::to_owned);
        let roots = self.browser_file_roots(sid.as_deref());
        let resolved = browser_pane::resolve_address(url);
        if self.use_mock_browser() {
            if let Some(why) = browser_pane::refusal(&resolved, &roots) {
                self.browser.host.set_mock_refusal(id, why.code());
                return;
            }
            let title = if resolved == "about:blank" { String::new() } else { bare_url(&resolved) };
            self.browser.host.set_mock_page(id, &resolved, &title);
            return;
        }
        self.browser.host.set_tab_policy(id, roots);
        self.browser.host.navigate_with_policy(id, &resolved, false);
    }

    pub fn register_existing_browser_tab(&mut self, id: u64) {
        if let Some(sid) = self.active_session_id().map(str::to_owned) {
            self.register_existing_browser_tab_for_session(&sid, id, true);
        }
    }

    pub fn register_existing_browser_tab_for_session(&mut self, sid: &str, id: u64, steal_focus: bool) {
        let vec = self.browser.session_tabs.entry(sid.to_owned()).or_default();
        if !vec.contains(&id) {
            vec.push(id);
        }
        let key = panel_key(PLUGIN, &id.to_string());
        if steal_focus {
            self.open_side_tab_for_session(sid, &key, None, false, false);
        } else {
            self.keeping_keyboard(|s| s.open_side_tab_for_session(sid, &key, None, false, false));
        }
    }

    /// The session page `id` belongs to.
    pub fn browser_tab_session(&self, id: u64) -> Option<&str> {
        self.browser.session_tabs.iter().find(|(_, ids)| ids.contains(&id)).map(|(sid, _)| sid.as_str())
    }

    pub fn close_browser_tab_for_session(&mut self, sid: &str, id: u64, tx: &mut Tx) {
        let (composer, side) = (self.composer.focused, self.side.focused);
        let key = panel_key(PLUGIN, &id.to_string());
        let routed = self.active_session_id() == Some(sid);
        if routed {
            self.close_side_tab(&key, tx);
        } else {
            let st = self.session_side_mut(sid);
            st.tab_state = crate::panels::tabs::close_session_tab(&st.tab_state, &key);
            if st.tab_state.tabs.all.is_empty() {
                st.visibility.step(false);
            }
        }
        self.forget_browser_tab(id);
        // The keyboard stays where it was, unless it was in the panel this just closed: then it
        // leaves with the panel, and the next ⌘W is held as after the strip's last tab closes,
        // so one press too many doesn't close the session.
        self.composer.focused = composer;
        if routed && side && !self.side_open() {
            self.side.focused = false;
            self.side.emptied_guard = true;
        } else {
            self.side.focused = side;
        }
    }

    /// Page `id`'s tab left its strip: the page closes.
    pub fn forget_browser_tab(&mut self, id: u64) {
        for ids in self.browser.session_tabs.values_mut() {
            ids.retain(|&x| x != id);
        }
        if self.browser.host.tab(id).is_some() {
            self.browser.host.close_tab(id);
        }
    }

    pub fn start_browser_install(&mut self) {
        self.browser.host.start_install();
    }

    /// The engine's messages and the page the engine paints: the
    /// routed session's selected one. True when what the pane shows changed.
    pub fn poll_browser(&mut self) -> bool {
        // Every poll comes here: the pane is written only when the engine has something.
        let mut changed = !self.browser.host.quiet() && self.browser.host.poll();
        // The engine paints only its focused tab, so the page on screen must be that tab (after a
        // session switch, say, which shows another session's page without selecting it).
        if self.side_open()
            && let Some(id) = self.active_browser_tab_id()
            && self.browser.host.focused_tab_id() != Some(id)
        {
            self.browser.host.focus_tab(id);
            changed = true;
        }
        changed
    }
}
