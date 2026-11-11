//! What survives a relaunch beyond the tabs: the open page, every composer draft, the closed
//! tabs ⇧⌘T reopens and the inbox's disclosures. The auto-rebuild restarts the app on each
//! merge, so live mode writes this on every change: the window's own (its page, drafts, panes
//! and closed tabs, from every server) to its file in Desktop's shape (`windows::store`,
//! `lastActiveUrl`, `tabs.recent`, `drafts`, `side`, `tabs.closed`), and what every window
//! shares (the sidebar, the inbox's and the organized sidebar's state) to `ui.json` in the app's
//! data folder (`crate::dirs`), under the app's own server.

mod doc;
mod parked;

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;

use opencode_kit::servers::TabKey;
use opencode_kit::servers::key::Route as Page;
use opencode_kit::servers::tabs::WindowDoc;
use serde_json::{Value, json};

use crate::{App, Route};

pub use doc::read_window;
pub use parked::Parked;

/// The draft key for a route: a session's id, or a draft tab's.
pub fn draft_key(route: &Route) -> Option<String> {
    match route {
        Route::Session { id, .. } | Route::Draft(id) => Some(id.clone()),
        Route::Home => None,
    }
}

/// The inbox's disclosures, as Desktop's `sidebar.inbox` (`inbox.tsx`): Working open, Recent
/// showing its first ten; and Home's automation folds, shut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InboxUi {
    pub working_expanded: bool,
    pub recent_expanded: bool,
    pub home_automations: bool,
}

impl Default for InboxUi {
    fn default() -> Self {
        InboxUi { working_expanded: true, recent_expanded: false, home_automations: false }
    }
}

impl InboxUi {
    fn to_json(self) -> Value {
        json!({ "workingExpanded": self.working_expanded, "recentExpanded": self.recent_expanded, "homeAutomationsExpanded": self.home_automations })
    }

    fn from_json(v: &Value) -> InboxUi {
        let d = InboxUi::default();
        InboxUi {
            working_expanded: v["workingExpanded"].as_bool().unwrap_or(d.working_expanded),
            recent_expanded: v["recentExpanded"].as_bool().unwrap_or(d.recent_expanded),
            home_automations: v["homeAutomationsExpanded"].as_bool().unwrap_or(d.home_automations),
        }
    }
}

#[derive(Default)]
pub struct Restore {
    /// Unsent composer text per draft key; the open one lives in `App::composer`.
    pub drafts: HashMap<String, String>,
    /// The rest of each draft that isn't shown: attachments, quotes, pills (`parked`).
    pub parked: HashMap<String, Parked>,
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

/// The page as Desktop's last active URL keeps it (`route-storage.ts`): Settings, while open, is
/// the page (`/settings?tab=`), whatever it was opened over.
fn page_href(app: &App) -> String {
    if app.settings.open {
        return Page::Settings(Some(app.settings_pages.page.id().to_owned())).href();
    }
    route_href(&app.route)
}

fn route_href(route: &Route) -> String {
    match route {
        Route::Home => Page::Home,
        Route::Draft(id) => Page::Draft(id.clone()),
        Route::Session { server, id } => Page::Session { server: server.clone(), session_id: id.clone() },
    }
    .href()
}

/// A saved page.
#[derive(Debug, PartialEq)]
enum Saved {
    Route(Route),
    /// Settings on this tab, which closes to Home (Desktop's `source` without a saved one).
    Settings(String),
}

fn route_from(href: &str) -> Option<Saved> {
    Some(match Page::parse(href)? {
        Page::Home | Page::Settings(None) => Saved::Route(Route::Home),
        Page::Settings(Some(tab)) => Saved::Settings(tab),
        Page::Draft(id) => Saved::Route(Route::Draft(id)),
        Page::Session { server, session_id } => Saved::Route(Route::session(server, session_id)),
    })
}

impl App {
    /// Puts back the window's saved page, drafts, closed tabs and panes, and what every window
    /// shares under the app's own server `server` (live mode, before the first frame).
    pub fn restore_ui(&mut self, doc: &WindowDoc, server: &str) {
        let global = &read()[server];
        // A draft is its text, or `{ text, mode }` for one in shell mode, as Desktop keeps the
        // mode in the draft it persists (`ComposerPersistedState.mode`).
        self.restore.drafts = HashMap::new();
        for (key, d) in &doc.drafts {
            let k = match key {
                TabKey::Draft(id) => id.clone(),
                TabKey::Session { session_id, .. } => session_id.clone(),
            };
            let text = d.as_str().or(d["text"].as_str()).unwrap_or_default();
            if !text.is_empty() {
                self.restore.drafts.insert(k.clone(), text.to_owned());
            }
            if d["mode"].as_str() == Some("shell") {
                self.add.park(k, session_view::composer::mode::Mode::Shell);
            }
        }
        self.closed = crate::tabs::Closed::from_doc(&doc.closed);
        self.restore.inbox = InboxUi::from_json(&global["inbox"]);
        self.restore_organize(&global["organize"]);
        let saved = match doc.last_active_url.as_deref().and_then(route_from) {
            Some(Saved::Route(route)) => Some(route),
            Some(Saved::Settings(tab)) => {
                self.settings.open = true;
                self.settings_pages.page = crate::settings::pages::Page::parse(&tab);
                Some(Route::Home)
            }
            None => None,
        };
        if let Some(route) = saved {
            // The window starts on the app's own server (`App::load_live`).
            let own = opencode_kit::servers::ServerKey::sidecar();
            let waiting = self.launch.phase(own.as_str()).is_some_and(crate::launch::Phase::waiting);
            let lost = match &route {
                // A session of another server keeps its page while its tab does: that server
                // answers after the window opens.
                Route::Session { server, id } => !(waiting && *server == own) && !self.tabs.iter().any(|t| t.id == *id) && !(*server == own && self.sessions.iter().any(|s| s.id == *id)),
                Route::Draft(id) => !self.tabs.iter().any(|t| t.id == *id && t.is_draft()),
                Route::Home => false,
            };
            if !lost {
                if waiting
                    && let Route::Session { server: on, id } = &route
                    && *on == own
                    && !self.transcripts.contains_key(id)
                    && let Some(messages) = crate::session_pages::load_cached_latest(server, id)
                {
                    self.transcripts.insert(id.clone(), opencode_kit::model::Transcript { session: id.clone(), messages });
                    self.launch.cold_transcript = Some(id.clone());
                }
                self.route = route;
            }
        }
        if let Some(sidebar) = global["sidebar"].as_bool() {
            self.sidebar_open = sidebar;
        }
        if let Some(w) = global["sidebarWidth"].as_f64() {
            self.sidebar_w = (w as f32).clamp(crate::view::SIDEBAR_MIN, crate::view::SIDEBAR_MAX);
        }
        self.side.restore_json(doc.side.as_ref().unwrap_or(&Value::Null));
        self.load_persisted_terminals(server);
        let _ = self.show_draft();
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

    /// When the window closes: the newest draft goes out, and every write waits to land.
    pub fn save_on_close(&mut self) {
        // A tab on its way out takes its window back, and a gap it opened in another closes.
        self.tear_back();
        self.close_rune_mcp();
        if self.persists() {
            self.poll_restore();
            self.queue_terminal_save();
        }
        self.window_closed();
        crate::servers::runtime::leave_all(self);
        crate::disk::flush();
    }

    /// When the app starts to quit (`App::on_quit`, every window): what the window keeps goes
    /// out and lands before the process may end, the window list and its frame with it.
    pub fn save_on_quit(&mut self) {
        if self.persists() {
            self.poll_restore();
            self.queue_terminal_save();
        }
        self.window_quitting();
        crate::disk::flush();
    }

    /// Swaps the composer's draft when the route moves to another: its words, pills, attachments,
    /// context and quotes go with the draft they were made in. A story's composer is its own.
    pub(crate) fn show_draft(&mut self) -> bool {
        let key = draft_key(&self.route);
        if key == self.restore.shown || self.stage.is_some() {
            return false;
        }
        // Before any draft showed, what a scene put in the composer is the page's draft.
        if self.restore.shown.is_none() && (!self.composer.text.is_empty() || self.attach.any() || self.quote.any()) {
            self.restore.shown = key;
            return false;
        }
        self.leave_queue_edits();
        self.add.swap_draft(self.restore.shown.as_deref(), key.as_deref());
        let (text, parked) = self.take_composer_draft();
        if let Some(old) = self.restore.shown.take() {
            self.park_draft(old, text, parked);
        }
        // Another draft replaces this one: no list or recalled prompt applies to it (`history.reset`).
        self.suggest_drop();
        self.put_composer_draft(key.as_deref());
        self.restore.shown = key;
        true
    }

    fn ui_json(&self) -> Value {
        let recent = self.current_tab().and_then(|uid| self.tab(&uid)).map(|t| json!({ "key": t.key().encode() }));
        json!({
            "lastActiveUrl": page_href(self),
            "tabs.recent": recent,
            "drafts": self.drafts_json(),
            "sidebar": self.sidebar_open,
            "sidebarWidth": self.sidebar_w,
            "side": self.side.to_json(),
            "tabs.closed": self.closed.to_doc().iter().map(opencode_kit::servers::tabs::ClosedTab::to_json).collect::<Value>(),
            "inbox": self.restore.inbox.to_json(),
            "organize": self.organize_json(),
        })
    }

    /// Each draft's text under its tab's key (`draft:<id>`, or the session's server and route),
    /// and its mode when it is shell (`{ text, mode: "shell" }`), so a draft left in shell mode
    /// comes back in it, empty or not.
    fn drafts_json(&self) -> Value {
        use session_view::composer::mode::Mode;
        let mut drafts: HashMap<String, (String, Mode)> = self.restore.drafts.iter().map(|(k, t)| (k.clone(), (t.clone(), Mode::Normal))).collect();
        for (k, mode) in self.add.parked() {
            drafts.entry(k.to_owned()).or_default().1 = mode;
        }
        if let Some(k) = &self.restore.shown {
            drafts.insert(k.clone(), (self.composer.text.clone(), self.add.mode));
        }
        let out: serde_json::Map<String, Value> = drafts
            .into_iter()
            .filter_map(|(k, (text, mode))| {
                let v = match mode {
                    Mode::Shell => json!({ "text": text, "mode": "shell" }),
                    Mode::Normal if text.trim().is_empty() => return None,
                    Mode::Normal => json!(text),
                };
                Some((self.draft_tab_key(&k).encode(), v))
            })
            .collect();
        Value::Object(out)
    }

    /// The tab key draft `k` is kept under: a draft tab's, or its session's on its server.
    fn draft_tab_key(&self, k: &str) -> TabKey {
        if let Some(t) = self.tabs.iter().find(|t| t.id == k) {
            return t.key();
        }
        if k.starts_with("draft_") {
            return TabKey::draft(k);
        }
        let server = match &self.route {
            Route::Session { server, id } if id == k => server.clone(),
            _ => crate::servers::runtime::current(),
        };
        TabKey::session(server, k)
    }

    fn restore_stamp(&self) -> u64 {
        let mut h = DefaultHasher::new();
        (&self.route, self.sidebar_open, self.sidebar_w.to_bits(), &self.composer.text, self.add.mode, &self.closed, self.restore.inbox).hash(&mut h);
        self.settings.open.then(|| self.settings_pages.page.id().to_owned()).hash(&mut h);
        self.side.stamp_hash(&mut h);
        let st = &self.project_state;
        (st.organize_collapsed, organize_expanded(st), &st.organize_order).hash(&mut h);
        h.finish()
    }

    /// Called every poll: keeps drafts per route and, on any change, writes the files off the UI
    /// thread (a few hundred bytes, renamed into place).
    pub fn poll_restore(&mut self) {
        let _ = self.show_draft();
        self.freeze_organize_order();
        let stamp = self.restore_stamp();
        if self.restore.stamp == Some(stamp) {
            return;
        }
        self.restore.stamp = Some(stamp);
        if !self.live() {
            return;
        }
        let v = self.ui_json();
        if self.restore.saved.as_ref() == Some(&v) {
            return;
        }
        let Some(path) = file() else { return };
        let Some(server) = crate::servers::runtime::own_server().map(|s| s.base) else { return };
        self.restore.saved = Some(v.clone());
        // The page, drafts, panes and closed tabs are the window's own (Desktop's
        // `opencode.window.<id>.dat`); the sidebar, the inbox's disclosures and the organized
        // sidebar's are every window's.
        for key in ["lastActiveUrl", "tabs.recent", "drafts", "side", "tabs.closed"] {
            crate::windows::store::save(&self.window, key, v[key].clone());
        }
        crate::disk::later("ui", move || {
            let mut all = read();
            if !all[&server].is_object() {
                all[&server] = json!({});
            }
            for key in ["sidebar", "sidebarWidth", "inbox", "organize"] {
                all[&server][key] = v[key].clone();
            }
            crate::disk::replace(&path, all.to_string().as_bytes());
        });
    }
}

/// The organized sidebar's open projects, sorted, so the file and its stamp don't depend on
/// the set's order.
fn organize_expanded(st: &crate::projects::ProjectsState) -> Vec<&String> {
    let mut ids: Vec<&String> = st.organize_expanded.iter().collect();
    ids.sort();
    ids
}

impl App {
    /// The organized sidebar's state as Desktop's `sidebar.organize` keeps it: Projects shut,
    /// the projects open to their sessions and the projects' order.
    fn organize_json(&self) -> Value {
        let st = &self.project_state;
        // `expanded`: `Record<"<server>:<project>", boolean>`; `order`: `Record<server, id[]>`.
        let expanded: serde_json::Map<String, Value> = organize_expanded(st).into_iter().map(|k| (k.clone(), Value::Bool(true))).collect();
        json!({ "projectsCollapsed": st.organize_collapsed, "expanded": expanded, "order": st.organize_order })
    }

    fn restore_organize(&mut self, v: &Value) {
        let strings = |v: &Value| v.as_array().into_iter().flatten().filter_map(|s| Some(s.as_str()?.to_owned())).collect::<Vec<_>>();
        // Saved before servers mixed: plain lists, of the app's own server's projects.
        let own = opencode_kit::servers::ServerKey::SIDECAR;
        let st = &mut self.project_state;
        st.organize_collapsed = v["projectsCollapsed"].as_bool().unwrap_or(false);
        st.organize_expanded = match &v["expanded"] {
            Value::Object(m) => m.iter().filter(|(_, on)| on.as_bool() == Some(true)).map(|(k, _)| k.clone()).collect(),
            list => strings(list).into_iter().map(|id| format!("{own}:{id}")).collect(),
        };
        st.organize_order = match &v["order"] {
            Value::Object(m) => m.iter().map(|(k, ids)| (k.clone(), strings(ids))).collect(),
            Value::Array(_) => std::iter::once((own.to_owned(), strings(&v["order"]))).collect(),
            _ => Default::default(),
        };
    }
}

