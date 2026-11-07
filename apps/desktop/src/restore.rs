//! What survives a relaunch beyond the tabs: the open page, every composer draft, the closed
//! tabs ⇧⌘T reopens and the inbox's disclosures. The auto-rebuild restarts the app on each
//! merge, so live mode writes this on every change, to `ui.json` in the app's data folder
//! (`crate::dirs`), one entry per server.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::{App, Route};

/// The draft key for a route: a session's id, or a draft tab's.
pub fn draft_key(route: &Route) -> Option<String> {
    match route {
        Route::Session(id) | Route::Draft(id) => Some(id.clone()),
        Route::Home => None,
    }
}

/// The key the one new-session page kept its words under before drafts were tabs.
const LEGACY_DRAFT: &str = "new";

/// The inbox's disclosures, as Desktop's `sidebar.inbox` (`inbox.tsx`): Working open, Recent
/// showing its first ten.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InboxUi {
    pub working_expanded: bool,
    pub recent_expanded: bool,
}

impl Default for InboxUi {
    fn default() -> Self {
        InboxUi { working_expanded: true, recent_expanded: false }
    }
}

impl InboxUi {
    fn to_json(self) -> Value {
        json!({ "workingExpanded": self.working_expanded, "recentExpanded": self.recent_expanded })
    }

    fn from_json(v: &Value) -> InboxUi {
        let d = InboxUi::default();
        InboxUi {
            working_expanded: v["workingExpanded"].as_bool().unwrap_or(d.working_expanded),
            recent_expanded: v["recentExpanded"].as_bool().unwrap_or(d.recent_expanded),
        }
    }
}

#[derive(Default)]
pub struct Restore {
    /// Unsent composer text per draft key; the open one lives in `App::composer`.
    pub drafts: HashMap<String, String>,
    /// Which draft `App::composer` holds.
    pub shown: Option<String>,
    pub inbox: InboxUi,
    saved: Option<Value>,
    stamp: Option<u64>,
}

fn file() -> Option<PathBuf> {
    crate::dirs::data().map(|d| d.join("ui.json"))
}

fn read() -> Value {
    file().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}))
}

fn route_json(route: &Route) -> Value {
    match route {
        Route::Session(id) => json!({ "session": id }),
        Route::Draft(id) => json!({ "draft": id }),
        Route::Home => json!("home"),
    }
}

/// The page as Desktop's last active URL keeps it: Settings, while open, is the page
/// (`/settings?tab=`), whatever it was opened over.
fn page_json(app: &App) -> Value {
    if app.settings.open {
        return json!({ "settings": settings_tab(&app.settings_pages.page) });
    }
    route_json(&app.route)
}

/// Desktop's `?tab=` for a settings page.
fn settings_tab(page: &crate::settings::pages::Page) -> String {
    page.id().to_owned()
}

fn settings_page(tab: &str) -> crate::settings::pages::Page {
    crate::settings::pages::Page::parse(tab)
}

/// A saved route.
#[derive(Debug, PartialEq)]
enum Saved {
    Route(Route),
    /// The new-session page, saved before drafts were tabs: it comes back as a draft tab.
    LegacyDraft,
    /// Settings on this tab, which closes to Home (Desktop's `source` without a saved one).
    Settings(String),
}

fn route_from(v: &Value) -> Option<Saved> {
    if let Some(id) = v["session"].as_str() {
        return Some(Saved::Route(Route::Session(id.to_owned())));
    }
    if let Some(id) = v["draft"].as_str() {
        return Some(Saved::Route(Route::Draft(id.to_owned())));
    }
    if let Some(tab) = v["settings"].as_str() {
        return Some(Saved::Settings(tab.to_owned()));
    }
    match v.as_str()? {
        "new" => Some(Saved::LegacyDraft),
        "home" => Some(Saved::Route(Route::Home)),
        _ => None,
    }
}

impl App {
    /// Puts back the saved route and drafts for `server` (live mode, before the first frame).
    pub fn restore_ui(&mut self, server: &str) {
        let v = &read()[server];
        self.restore.drafts =
            v["drafts"].as_object().into_iter().flatten().filter_map(|(k, t)| Some((k.clone(), t.as_str()?.to_owned()))).collect();
        self.closed = crate::tabs::Closed::from_json(&v["closedTabs"]);
        self.restore.inbox = InboxUi::from_json(&v["inbox"]);
        let saved = match route_from(&v["route"]) {
            Some(Saved::LegacyDraft) => Some(self.migrate_legacy_draft(server)),
            Some(Saved::Route(route)) => Some(route),
            Some(Saved::Settings(tab)) => {
                self.settings.open = true;
                self.settings_pages.page = settings_page(&tab);
                Some(Route::Home)
            }
            None => None,
        };
        if let Some(route) = saved {
            let known = |id: &str| self.launch.waiting || self.tabs.iter().any(|t| t.id == id) || self.sessions.iter().any(|s| s.id == id);
            let lost = match &route {
                Route::Session(id) => !known(id),
                Route::Draft(id) => !self.tabs.iter().any(|t| t.id == *id && t.is_draft()),
                Route::Home => false,
            };
            if !lost {
                if self.launch.waiting
                    && let Route::Session(id) = &route
                    && !self.transcripts.contains_key(id)
                    && let Some(messages) = crate::session_pages::load_cached_latest(server, id)
                {
                    self.transcripts.insert(id.clone(), opencode_kit::model::Transcript { session: id.clone(), messages });
                    self.launch.cold_transcript = Some(id.clone());
                }
                self.route = route;
            }
        }
        if let Some(sidebar) = v["sidebar"].as_bool() {
            self.sidebar_open = sidebar;
        }
        if let Some(w) = v["sidebarWidth"].as_f64() {
            self.sidebar_w = (w as f32).clamp(crate::view::SIDEBAR_MIN, crate::view::SIDEBAR_MAX);
        }
        self.side.restore_json(&v["side"]);
        self.load_persisted_terminals(server);
        self.show_draft();
        self.restore.saved = Some(self.ui_json());
        self.restore.stamp = Some(self.restore_stamp());
    }

    /// Settings reopened on its page loads what the page reads, once the server is the source.
    pub fn resume_settings_page(&mut self) {
        if self.settings.open {
            let page = std::mem::take(&mut self.settings_pages.page);
            self.open_settings_page(page, &mut rune::Tx::default());
        }
    }

    /// The new-session page saved before drafts were tabs comes back as a draft tab with its words.
    fn migrate_legacy_draft(&mut self, server: &str) -> Route {
        let id = crate::tabs::draft_id();
        if let Some(text) = self.restore.drafts.remove(LEGACY_DRAFT) {
            self.restore.drafts.insert(id.clone(), text);
        }
        self.tabs.push(crate::tabs::Tab::draft(id.clone(), self.default_dir.clone()));
        // The source isn't live yet, so `save_tabs` would skip it: written here, once, unless
        // the tabs still come from Beta.
        if !crate::headless() && !cfg!(test) && !self.launch.offline && !self.launch.beta_tabs {
            crate::tabs::save(server, &self.tabs);
        }
        Route::Draft(id)
    }

    /// When the window closes: the newest draft goes out, and every write waits to land.
    pub fn save_on_close(&mut self) {
        self.close_rune_mcp();
        if self.persists() {
            self.poll_restore();
            self.queue_terminal_save();
        }
        crate::disk::flush();
    }

    /// Swaps the composer's text when the route moves to another draft.
    pub(crate) fn show_draft(&mut self) {
        let key = draft_key(&self.route);
        if key == self.restore.shown {
            return;
        }
        if let Some(old) = self.restore.shown.take() {
            let text = std::mem::take(&mut self.composer.text);
            if text.trim().is_empty() {
                self.restore.drafts.remove(&old);
            } else {
                self.restore.drafts.insert(old, text);
            }
        }
        let text = key.as_ref().and_then(|k| self.restore.drafts.get(k).cloned()).unwrap_or_default();
        let focused = self.composer.focused;
        self.composer = rune::TextEdit::new(&text);
        self.composer.focused = focused;
        self.restore.shown = key;
    }

    fn ui_json(&self) -> Value {
        let mut drafts = self.restore.drafts.clone();
        if let Some(k) = &self.restore.shown {
            if self.composer.text.trim().is_empty() {
                drafts.remove(k);
            } else {
                drafts.insert(k.clone(), self.composer.text.clone());
            }
        }
        json!({
            "route": page_json(self),
            "drafts": drafts,
            "sidebar": self.sidebar_open,
            "sidebarWidth": self.sidebar_w,
            "side": self.side.to_json(),
            "closedTabs": self.closed.to_json(),
            "inbox": self.restore.inbox.to_json(),
        })
    }

    fn restore_stamp(&self) -> u64 {
        let mut h = DefaultHasher::new();
        (&self.route, self.sidebar_open, self.sidebar_w.to_bits(), &self.composer.text, &self.closed, self.restore.inbox).hash(&mut h);
        self.settings.open.then(|| settings_tab(&self.settings_pages.page)).hash(&mut h);
        self.side.stamp_hash(&mut h);
        h.finish()
    }

    /// Called every poll: keeps drafts per route and, on any change, writes the file off the UI
    /// thread (it's a few hundred bytes, renamed into place).
    pub fn poll_restore(&mut self) {
        self.show_draft();
        let stamp = self.restore_stamp();
        if self.restore.stamp == Some(stamp) {
            return;
        }
        self.restore.stamp = Some(stamp);
        let crate::Source::Live { server, .. } = &self.source else { return };
        let v = self.ui_json();
        if self.restore.saved.as_ref() == Some(&v) {
            return;
        }
        let Some(path) = file() else { return };
        let server = server.base.clone();
        self.restore.saved = Some(v.clone());
        crate::disk::later("ui", move || {
            let mut all = read();
            all[server] = v;
            crate::disk::replace(&path, all.to_string().as_bytes());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_round_trip() {
        for r in [Route::Home, Route::Draft("draft_AbC".into()), Route::Session("ses_AbC".into())] {
            assert_eq!(route_from(&route_json(&r)), Some(Saved::Route(r)));
        }
    }

    #[test]
    fn each_session_keeps_its_own_draft() {
        let mut a = App::empty();
        a.route = Route::Session("ses_a".into());
        a.show_draft();
        a.composer = rune::TextEdit::new("half a thought");
        a.route = Route::Session("ses_b".into());
        a.show_draft();
        assert_eq!(a.composer.text, "");
        a.composer = rune::TextEdit::new("other");
        a.route = Route::Session("ses_a".into());
        a.show_draft();
        assert_eq!(a.composer.text, "half a thought");
        assert_eq!(a.ui_json()["drafts"]["ses_b"], "other");
    }
}
