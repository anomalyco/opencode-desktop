//! What survives a relaunch beyond the tabs: the open page, every composer draft, the closed
//! tabs ⇧⌘T reopens and the inbox's disclosures. The auto-rebuild restarts the app on each
//! merge, so live mode keeps this current on every change, in the app's database
//! (`crate::storage`), one row per thing, written behind: the window's own (its page, panes and
//! closed tabs as `lastActiveUrl`, `tabs.recent`, `side`, `tabs.closed` of
//! `opencode.window.<id>.dat`, each draft its own document, `windows::store`), and what every
//! window shares in Desktop's global keys (`layout`'s sidebar, `sidebar.inbox`,
//! `sidebar.organize`). A keystroke writes only its draft's document; a sidebar drag only
//! `layout`.

mod copies;
mod doc;
mod draft_doc;
mod parked;
mod preview;

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

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
    /// What each row was last written as, so a change writes only the rows it changed.
    written: Written,
    stamp: Option<u64>,
    /// Whether the window has what the boot didn't hold: its closed tabs and other drafts.
    hydration: Hydration,
    previews: preview::Previews,
    copies: copies::Copies,
    /// Which blob holds each draft file's bytes (`draft_doc`).
    blobs: draft_doc::Blobs,
    /// Each draft's own editor while another shows: its undo history comes back with it.
    edits: HashMap<String, rune::TextEdit>,
}

/// The rows this window last wrote (or read): its keys and each draft's document by tab key.
#[derive(Default)]
struct Written {
    keys: HashMap<&'static str, Value>,
    drafts: HashMap<String, String>,
}

/// The window's state beyond what the first frame needed. The boot packet holds the shown draft
/// and no closed tabs; once the storage is ready the rest comes in, and only then may the
/// window write its closed tabs or remove a draft it never saw.
#[derive(Debug, Default)]
pub enum Hydration {
    /// Built from the boot packet.
    #[default]
    Boot,
    /// Some drafts are being read from disk; `read` turns true once they're in memory.
    Loading { read: std::sync::Arc<std::sync::atomic::AtomicBool> },
    /// Everything is in.
    Done,
}

/// The app-wide keys Desktop keeps the sidebar's and the inbox's state in.
const GLOBAL: &str = "opencode.global.dat";

/// The page as Desktop's last active URL keeps it (`route-storage.ts`): Settings, while open, is
/// the page (`/settings?tab=`), whatever it was opened over.
fn page_href(app: &App) -> String {
    if app.settings.open {
        return Page::Settings(Some(app.settings_pages.page.id().to_owned())).href();
    }
    route_href(&*app.route)
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
    /// shares (live mode, before the first frame). `server` is the app's own server.
    pub fn restore_ui(&mut self, doc: &WindowDoc, server: &str) {
        self.restore.drafts = HashMap::new();
        self.take_drafts(&doc.drafts);
        *self.closed = crate::tabs::Closed::from_doc(&doc.closed);
        let global = |key: &str| crate::storage::json(GLOBAL, key).unwrap_or(Value::Null);
        self.restore.inbox = InboxUi::from_json(&global("sidebar.inbox"));
        self.restore_organize(&global("sidebar.organize"));
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
                    && self.take_boot_preview(on, id)
                {
                    self.launch.cold_transcript = Some(id.clone());
                }
                *self.route = route;
            }
        }
        let sidebar = &global("layout")["sidebar"];
        if let Some(open) = sidebar["opened"].as_bool() {
            *self.sidebar_open = open;
        }
        if let Some(w) = sidebar["width"].as_f64() {
            *self.sidebar_w = (w as f32).clamp(crate::view::SIDEBAR_MIN, crate::view::SIDEBAR_MAX);
        }
        self.side.restore_json(doc.side.as_ref().unwrap_or(&Value::Null));
        self.load_persisted_terminals(server);
        let _ = self.show_draft();
        // What was read is what's stored: nothing is written until something changes.
        self.restore.written = Written::default();
        self.note_written();
        self.restore.stamp = Some(self.restore_stamp());
        if crate::storage::is_ready() {
            self.hydrate_restore();
        }
    }

    /// Says where the window's state is when it isn't simply the saved state (`storage::Notice`),
    /// from the first poll on: a held file's stand-in shows with its notice. True when it did.
    pub fn tell_storage_notice(&mut self) -> bool {
        use crate::storage::Notice;
        use opencode_i18n::t;
        match crate::storage::take_notice() {
            Some(Notice::Memory(why)) => crate::status_toast::fail(self, t!("rune.storage.memoryOnly.title"), t!("rune.storage.memoryOnly.description", why = crate::status_toast::plain(&why))),
            Some(Notice::Checkpoint) => crate::status_toast::fail(self, t!("rune.storage.memoryOnly.title"), t!("rune.storage.checkpoint.description")),
            Some(Notice::FirstRun) => crate::status_toast::fail(self, t!("rune.storage.memoryOnly.title"), t!("rune.storage.firstRun.description")),
            Some(Notice::Saving) => crate::status_toast::succeed(self, t!("rune.storage.saving.title"), ""),
            None => return false,
        }
        true
    }

    /// Takes saved drafts into the window: a draft is its text, or `{ text, mode }` for one in
    /// shell mode, as Desktop keeps the mode in the draft it persists
    /// (`ComposerPersistedState.mode`). A draft the window already holds keeps its own.
    fn take_drafts(&mut self, drafts: &[(TabKey, Value)]) {
        for (key, d) in drafts {
            let k = match key {
                TabKey::Draft(id) => id.clone(),
                TabKey::Session { session_id, .. } => session_id.clone(),
            };
            self.restore.written.drafts.insert(key.encode(), d.to_string());
            if self.restore.drafts.contains_key(&k) || self.restore.parked.contains_key(&k) || self.restore.shown.as_deref() == Some(k.as_str()) {
                continue;
            }
            let draft = draft_doc::decode(d);
            if !draft.text.is_empty() {
                self.restore.drafts.insert(k.clone(), draft.text.clone());
            }
            if draft.parked.worth_keeping(&draft.text) {
                self.restore.parked.insert(k.clone(), draft.parked);
            }
            self.read_draft_files(&k, draft.stored);
            if draft.shell {
                self.add.park(k, session_view::composer::mode::Mode::Shell);
            }
        }
    }

    /// The storage is ready (or the drafts it was reading came in): the window takes its closed
    /// tabs and the drafts the boot packet didn't hold. Steps [`Hydration`]; true when it did
    /// (the window has more to draw).
    pub fn hydrate_restore(&mut self) -> bool {
        match &self.restore.hydration {
            Hydration::Done => return false,
            Hydration::Loading { read } if !read.load(std::sync::atomic::Ordering::Acquire) => return false,
            _ if !crate::storage::is_ready() => return false,
            _ => {}
        }
        let mut expired = Vec::new();
        if matches!(self.restore.hydration, Hydration::Boot) {
            let raw = crate::windows::store::load(&self.window);
            let doc = WindowDoc::from_json(&raw);
            // Tabs closed since launch stay newest.
            let mut closed = crate::tabs::Closed::from_doc(&doc.closed);
            expired = closed.take_drafts_json(&raw["rune.closedDrafts"], *self.now_ms);
            for e in self.closed.entries().cloned().collect::<Vec<_>>() {
                if e.tab.is_draft() { closed.push_draft(e, *self.now_ms) } else { closed.push(e) }
            }
            *self.closed = closed;
            self.restore.written.keys.insert("tabs.closed", closed_json(&self.closed));
            self.restore.written.keys.insert("rune.closedDrafts", self.closed.drafts_json());
        }
        let (ready, pending) = crate::windows::store::drafts(&self.window);
        self.take_drafts(&ready);
        // A draft closed more than a week ago goes with its words.
        for id in expired {
            self.restore.drafts.remove(&id);
            self.restore.parked.remove(&id);
        }
        // A draft that still can't be read after its load stays where it is, unshown.
        if pending.is_empty() || matches!(self.restore.hydration, Hydration::Loading { .. }) {
            self.restore.hydration = Hydration::Done;
            return true;
        }
        let read = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.restore.hydration = Hydration::Loading { read: read.clone() };
        let keys: Vec<&str> = pending.iter().map(String::as_str).collect();
        let waker = (*self.waker).clone();
        crate::storage::load_docs(&keys, Box::new(move || {
            read.store(true, std::sync::atomic::Ordering::Release);
            if let Some(w) = waker {
                w();
            }
        }));
        true
    }

    /// Whether the window has everything it saved (`Hydration::Done`).
    pub fn hydrated(&self) -> bool {
        matches!(self.restore.hydration, Hydration::Done)
    }

    /// Settings reopened on its page loads what the page reads, once the server is the source.
    pub fn resume_settings_page(&mut self) {
        if self.settings.open {
            let page = std::mem::take(&mut self.settings_pages.page);
            self.open_settings_page(page, &mut rune::Tx::default());
        }
    }

    /// When the window closes: the newest draft goes out to the storage, which lands it at once.
    pub fn save_on_close(&mut self) {
        // A tab on its way out takes its window back, and a gap it opened in another closes.
        self.tear_back();
        self.close_rune_mcp();
        if self.persists() {
            self.poll_restore();
            self.poll_preview(true);
            self.queue_terminal_save();
            crate::terminal::persist::flush();
        }
        self.window_closed();
        crate::servers::runtime::leave_all(self);
        crate::storage::flush(opencode_storage::FlushReason::Close, false);
    }

    /// When the app starts to quit (`App::on_quit`, every window): what the window keeps is
    /// queued, the window list and its frame with it; the last window to quit lands every
    /// window's rows in one transaction before the process may end (`windows::quit_saved`).
    pub fn save_on_quit(&mut self) {
        if self.persists() {
            self.poll_restore();
            self.poll_preview(true);
            self.queue_terminal_save();
            crate::terminal::persist::flush();
        }
        crate::notifications::quitting(self);
        self.window_quitting();
        crate::windows::quit_saved(&self.window.id);
    }
    /// Swaps the composer's draft when the route moves to another: its words, pills, attachments,
    /// context and quotes go with the draft they were made in. A story's composer is its own.
    pub(crate) fn show_draft(&mut self) -> bool {
        let key = draft_key(&*self.route);
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
        // The field itself (its undo history) waits for the draft to show again.
        let edit = (*self.composer).clone();
        let (text, parked) = self.take_composer_draft();
        if let Some(old) = self.restore.shown.take() {
            self.restore.edits.insert(old.clone(), edit);
            self.park_draft(old, text, parked);
        }
        // Another draft replaces this one: no list or recalled prompt applies to it (`history.reset`).
        self.suggest_drop();
        self.put_composer_draft(key.as_deref());
        self.restore.shown = key;
        true
    }

    /// The window's own rows (Desktop's `opencode.window.<id>.dat` keys) as they stand.
    fn window_keys(&self) -> [(&'static str, Value); 5] {
        let recent = self.current_tab().and_then(|uid| self.tab(&uid)).map(|t| json!({ "key": t.key().encode() }));
        [("lastActiveUrl", json!(page_href(self))), ("tabs.recent", recent.unwrap_or(Value::Null)), ("side", self.side.to_json()), ("tabs.closed", closed_json(&self.closed)), ("rune.closedDrafts", self.closed.drafts_json())]
    }

    /// Every window's rows this one shares (Desktop's global `layout` sidebar, `sidebar.inbox`,
    /// `sidebar.organize`).
    fn global_keys(&self) -> [(&'static str, Value); 3] {
        [("layout.sidebar", json!({ "opened": self.sidebar_open, "width": self.sidebar_w })), ("sidebar.inbox", self.restore.inbox.to_json()), ("sidebar.organize", self.organize_json())]
    }

    /// Each draft's document under its tab's key (`draft:<id>`, or the session's server and
    /// route): its text, its mode when it is shell, and the rest of it (`draft_doc`), so a draft
    /// comes back as it was left, empty or not. A draft with nothing in it has none.
    fn drafts_now(&self) -> HashMap<String, String> {
        use session_view::composer::mode::Mode;
        let mut drafts: HashMap<&str, (&str, Mode)> = self.restore.drafts.iter().map(|(k, t)| (k.as_str(), (t.as_str(), Mode::Normal))).collect();
        for k in self.restore.parked.keys().chain(self.restore.blobs_pending()) {
            drafts.entry(k.as_str()).or_insert(("", Mode::Normal));
        }
        for (k, mode) in self.add.parked() {
            drafts.entry(k).or_insert(("", Mode::Normal)).1 = mode;
        }
        if let Some(k) = &self.restore.shown {
            drafts.insert(k.as_str(), (self.composer.text.as_str(), self.add.mode));
        }
        drafts
            .into_iter()
            .filter_map(|(k, (text, mode))| {
                let parts = if self.restore.shown.as_deref() == Some(k) { self.composer_parts() } else { self.restore.parked.get(k).map(draft_doc::Parts::of) };
                let v = draft_doc::encode(text, mode == Mode::Shell, parts, &self.restore.blobs, self.pending_files(k))?;
                Some((self.draft_tab_key(k).encode(), v))
            })
            .collect()
    }

    /// Every draft's document as it stands, by tab key: what a closed window keeps (`windows::reopen`).
    pub(crate) fn draft_documents(&self) -> HashMap<String, String> {
        self.drafts_now()
    }

    /// Records what stands now as written: the rows read at launch.
    fn note_written(&mut self) {
        let keys: HashMap<&'static str, Value> = self.window_keys().into_iter().chain(self.global_keys()).collect();
        self.restore.written.keys = keys;
        let drafts = self.drafts_now();
        self.restore.written.drafts.extend(drafts);
    }

    /// The document draft `k`'s words are kept in (none in a window without an id, which keeps
    /// no drafts): the outbox clears it in the same transaction as the prompt's first row.
    pub(crate) fn draft_document_of(&self, k: &str) -> Option<String> {
        (!self.window.id.is_empty()).then(|| crate::windows::store::draft_key(&self.window, &self.draft_tab_key(k)))
    }

    /// The tab key draft `k` is kept under: a draft tab's, or its session's on its server.
    fn draft_tab_key(&self, k: &str) -> TabKey {
        if let Some(t) = self.tabs.iter().find(|t| t.id == k) {
            return t.key();
        }
        if k.starts_with("draft_") {
            return TabKey::draft(k);
        }
        let server = match &*self.route {
            Route::Session { server, id } if id == k => server.clone(),
            _ => crate::servers::runtime::current(),
        };
        TabKey::session(server, k)
    }

    fn restore_stamp(&self) -> u64 {
        let mut h = DefaultHasher::new();
        (&*self.route, *self.sidebar_open, self.sidebar_w.to_bits(), &self.composer.text, self.add.mode, &self.closed, self.restore.inbox).hash(&mut h);
        (self.restore.drafts.len(), self.restore.shown.as_deref()).hash(&mut h);
        // The rest of the shown draft and what the parked ones hold (`draft_doc`).
        (self.composer.range(), self.attach.files.len(), self.attach.paths.len(), self.attach.context.len(), self.quote.revision()).hash(&mut h);
        (self.restore.parked.len(), self.restore.blobs.revision).hash(&mut h);
        self.settings.open.then(|| self.settings_pages.page.id().to_owned()).hash(&mut h);
        self.side.stamp_hash(&mut h);
        let st = &self.project_state;
        (st.organize_collapsed, organize_expanded(st), &st.organize_order).hash(&mut h);
        h.finish()
    }

    /// Called every poll: keeps drafts per route and, on any change, hands the rows that changed
    /// to the storage (memory now, the database behind). A keystroke changes one draft's
    /// document; a sidebar drag step the shared `layout`.
    pub fn poll_restore(&mut self) {
        let _ = self.show_draft();
        if self.persists() {
            self.poll_draft_files();
        }
        self.freeze_organize_order();
        self.hydrate_restore();
        let stamp = self.restore_stamp();
        if self.restore.stamp == Some(stamp) {
            return;
        }
        self.restore.stamp = Some(stamp);
        if !self.persists() {
            return;
        }
        let hydrated = self.hydrated();
        for (key, value) in self.window_keys() {
            // The closed tabs on disk aren't in memory until the window is hydrated.
            if (matches!(key, "tabs.closed" | "rune.closedDrafts") && !hydrated) || self.restore.written.keys.get(key) == Some(&value) {
                continue;
            }
            crate::windows::store::save(&self.window, key, &value);
            self.restore.written.keys.insert(key, value);
        }
        for (key, value) in self.global_keys() {
            if self.restore.written.keys.get(key) == Some(&value) {
                continue;
            }
            match key.split_once('.') {
                Some(("layout", sub)) => crate::storage::update(GLOBAL, "layout", |m| {
                    m.insert(sub.to_owned(), value.clone());
                }),
                _ => crate::storage::set_json(GLOBAL, key, &value),
            }
            self.restore.written.keys.insert(key, value);
        }
        let drafts = self.drafts_now();
        let gone: Vec<String> = self.restore.written.drafts.keys().filter(|k| !drafts.contains_key(*k)).cloned().collect();
        for key in gone {
            self.restore.written.drafts.remove(&key);
            if let Some(tab) = TabKey::parse(&key) {
                crate::windows::store::save_draft(&self.window, &tab, None);
            }
        }
        for (key, draft) in drafts {
            if self.restore.written.drafts.get(&key) == Some(&draft) {
                continue;
            }
            if let Some(tab) = TabKey::parse(&key) {
                crate::windows::store::save_draft(&self.window, &tab, Some(draft.clone()));
            }
            self.restore.written.drafts.insert(key, draft);
        }
    }
}

/// `tabs.closed`, as the window keeps it.
fn closed_json(closed: &crate::tabs::Closed) -> Value {
    closed.to_doc().iter().map(opencode_kit::servers::tabs::ClosedTab::to_json).collect()
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
        let st = &mut *self.project_state;
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

