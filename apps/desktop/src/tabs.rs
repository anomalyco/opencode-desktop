//! Pinned tabs, from every server the app keeps live, in one list per window, as OpenCode
//! Desktop keeps them (`shell/tabs/schema.ts`): each tab carries its server, so a window mixes
//! sessions of the app's own server, servers added by address and plugins' servers. The list,
//! the saved titles and the closed tabs live in the window's own file (`windows::store`,
//! `opencode.window.<id>.dat`'s `tabs`, `tabs.info` and `tabs.closed`), never on a server.
//!
//! A profile's first window with nothing saved reads Desktop Beta's list (read-only, through the
//! `sqlite3` CLI) so the two show the same tabs; without Beta it starts with none.

use std::path::PathBuf;
use std::process::Command;

use opencode_kit::servers::tabs as doc;
use opencode_kit::servers::{ServerKey, TabKey};
use serde_json::{Value, json};

use crate::Route;

/// What a tab holds: a session, or Desktop's "New session" draft (`TabStorage.Draft`), which
/// becomes a session tab in its place when its first prompt goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TabKind {
    Session,
    Draft,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Tab {
    /// A session's id, or a draft's own `draft_…` id.
    pub id: String,
    pub kind: TabKind,
    /// The server its session is on, or where a draft's session will start.
    pub server: ServerKey,
    /// The title and directory when the tab was saved, for a session outside the loaded list.
    /// A draft's directory is where its session will start; its title is unused.
    pub title: String,
    pub directory: String,
}

impl Tab {
    /// A tab for session `id` on the server the window works against now.
    pub fn session(id: impl Into<String>, title: impl Into<String>, directory: impl Into<String>) -> Tab {
        Tab { id: id.into(), kind: TabKind::Session, server: crate::servers::runtime::current(), title: title.into(), directory: directory.into() }
    }

    /// A draft tab whose session will start on the server the window works against now.
    pub fn draft(id: impl Into<String>, directory: impl Into<String>) -> Tab {
        Tab { id: id.into(), kind: TabKind::Draft, server: crate::servers::runtime::current(), title: String::new(), directory: directory.into() }
    }

    /// The tab on `server` instead.
    pub fn on(mut self, server: ServerKey) -> Tab {
        self.server = server;
        self
    }

    pub fn is_draft(&self) -> bool {
        self.kind == TabKind::Draft
    }

    /// Whether it is this session's tab on `server`, or this draft's.
    pub fn is(&self, server: &ServerKey, id: &str) -> bool {
        self.id == id && (self.is_draft() || self.server == *server)
    }

    /// The page this tab shows.
    pub fn route(&self) -> Route {
        match self.kind {
            TabKind::Session => Route::session(self.server.clone(), self.id.clone()),
            TabKind::Draft => Route::Draft(self.id.clone()),
        }
    }

    /// Its identity in the window's file (`tabKey`).
    pub fn key(&self) -> TabKey {
        match self.kind {
            TabKind::Session => TabKey::session(self.server.clone(), self.id.clone()),
            TabKind::Draft => TabKey::draft(self.id.clone()),
        }
    }

    /// The tab as the window's file keeps it, with what `tabs.info` saves of it.
    pub fn to_doc(&self) -> (doc::Tab, doc::TabInfo) {
        let text = |s: &str| (!s.is_empty()).then(|| s.to_owned());
        match self.kind {
            TabKind::Session => (doc::Tab::Session(doc::SessionTab::new(self.server.clone(), &self.id)), doc::TabInfo { title: text(&self.title), directory: text(&self.directory), prompted: None }),
            TabKind::Draft => (doc::Tab::Draft(doc::DraftTab::new(&self.id, self.server.clone(), &self.directory)), doc::TabInfo::default()),
        }
    }

    pub fn from_doc(tab: &doc::Tab, info: Option<&doc::TabInfo>) -> Tab {
        let field = |f: fn(&doc::TabInfo) -> &Option<String>| info.and_then(|i| f(i).clone()).unwrap_or_default();
        match tab {
            doc::Tab::Session(t) => Tab { id: t.session_id.clone(), kind: TabKind::Session, server: t.server.clone(), title: field(|i| &i.title), directory: field(|i| &i.directory) },
            doc::Tab::Draft(d) => Tab { id: d.draft_id.clone(), kind: TabKind::Draft, server: d.server.clone(), title: String::new(), directory: d.directory.clone() },
        }
    }
}

/// A tab's label: drafts, sessions still starting and untitled sessions all read "Session", as
/// a missing or timestamped fallback title does (`sessionTabTitle`).
pub fn tab_title(title: &str) -> String {
    let fallback = ["New session - ", "Child session - "].iter().any(|p| title.strip_prefix(p).is_some_and(is_iso_stamp));
    if title.is_empty() || fallback { opencode_i18n::t!("session.tab.session").into() } else { title.to_owned() }
}

/// `2026-07-11T09:20:00.000Z`, as JavaScript's `toISOString` writes it.
fn is_iso_stamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 24
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            19 => *c == b'.',
            23 => *c == b'Z',
            _ => c.is_ascii_digit(),
        })
}

/// A fresh draft id, unique as a session id is (Desktop uses a UUID).
pub fn draft_id() -> String {
    let id = opencode_kit::client::session_id();
    format!("draft_{}", id.strip_prefix("ses_").unwrap_or(&id))
}

/// How many closed tabs Desktop keeps for reopening (`CLOSED_TAB_LIMIT`).
pub const CLOSED_LIMIT: usize = 100;

/// A closed session tab and the place it had (`TabStorage.ClosedTab`). `prompted`: its session
/// had a prompt, so "Recently closed" lists it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClosedTab {
    pub tab: Tab,
    pub index: usize,
    pub prompted: bool,
}

/// Closed session tabs, oldest first, as Desktop's `tabs.closed`: a session appears once, at
/// its latest close, keyed by its server and id (`shell/tabs/closed.ts`). Drafts are never
/// recorded; their state goes with them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Closed(Vec<ClosedTab>);

impl Closed {
    pub fn push(&mut self, entry: ClosedTab) {
        if entry.tab.is_draft() {
            return;
        }
        self.0.retain(|e| !e.tab.is(&entry.tab.server, &entry.tab.id));
        self.0.push(entry);
        let over = self.0.len().saturating_sub(CLOSED_LIMIT);
        self.0.drain(..over);
    }

    /// Takes the newest closed tab that isn't open again (or `target`'s), dropping stale ones
    /// passed on the way (`takeClosedTab`).
    pub fn take(&mut self, open: &[Tab], target: Option<&str>) -> Option<ClosedTab> {
        let is_open = |e: &ClosedTab| open.iter().any(|t| t.kind == TabKind::Session && t.is(&e.tab.server, &e.tab.id));
        if let Some(id) = target {
            let at = self.0.iter().rposition(|e| e.tab.id == id)?;
            return (!is_open(&self.0[at])).then(|| self.0.remove(at));
        }
        while let Some(e) = self.0.pop() {
            if !is_open(&e) {
                return Some(e);
            }
        }
        None
    }

    /// Newest first, without tabs open again (`listClosedTabs`).
    pub fn list<'a>(&'a self, open: &'a [Tab]) -> impl Iterator<Item = &'a ClosedTab> + 'a {
        self.0.iter().rev().filter(|e| !open.iter().any(|t| t.kind == TabKind::Session && t.is(&e.tab.server, &e.tab.id)))
    }

    /// Sessions of the server the window works against now that are gone for good leave the
    /// list (`removeClosedTabs`).
    pub fn remove(&mut self, gone: impl Fn(&str) -> bool) {
        let server = crate::servers::runtime::current();
        self.0.retain(|e| e.tab.server != server || !gone(&e.tab.id));
    }

    /// `tabs.closed`, as the window's file keeps it.
    pub fn to_doc(&self) -> Vec<doc::ClosedTab> {
        self.0
            .iter()
            .map(|e| {
                let (_, mut info) = e.tab.to_doc();
                info.prompted = e.prompted.then_some(true);
                doc::ClosedTab { tab: doc::SessionTab::new(e.tab.server.clone(), &e.tab.id), index: e.index as u64, info: (!info.is_empty()).then_some(info) }
            })
            .collect()
    }

    pub fn from_doc(list: &[doc::ClosedTab]) -> Closed {
        let mut closed = Closed::default();
        for c in list {
            let tab = Tab::from_doc(&doc::Tab::Session(c.tab.clone()), c.info.as_ref());
            closed.push(ClosedTab { tab, index: c.index as usize, prompted: c.info.as_ref().and_then(|i| i.prompted).unwrap_or(false) });
        }
        closed
    }
}

#[cfg(test)]
thread_local! {
    /// The tab ids the last `App::save_tabs` would write, in order.
    pub static SAVED: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

fn file() -> Option<PathBuf> {
    crate::dirs::data().map(|d| d.join("tabs.json"))
}

/// Sets `key` in the tabs file, off the UI thread. The zoom and the projects used share the
/// file, so each read and write happens on the saves thread, in order.
fn update(slot: &'static str, key: String, value: Value) {
    let Some(path) = file() else { return };
    crate::disk::later(slot, move || {
        let mut v: Value = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
        v[key] = value;
        crate::disk::replace(&path, v.to_string().as_bytes());
    });
}

/// `tabs.json` and `ui.json`, the files every window read before windows had their own.
pub fn legacy_files() -> (Option<Value>, Option<Value>) {
    let read = |name: &str| crate::dirs::data().and_then(|d| std::fs::read_to_string(d.join(name)).ok()).and_then(|s| serde_json::from_str(&s).ok());
    (read("tabs.json"), read("ui.json"))
}

/// The saved UI zoom (1.0 is 100%), kept in the same file under its own key.
pub fn load_zoom() -> Option<f32> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(file()?).ok()?).ok()?;
    v["zoom"].as_f64().map(|z| z as f32)
}

pub fn save_zoom(zoom: f32) {
    update("zoom", "zoom".into(), json!(f64::from((zoom * 100.0).round()) / 100.0));
}

/// Project IDs this client has used on `server`, most recent first.
pub fn load_projects(server: &str) -> Option<Vec<String>> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(file()?).ok()?).ok()?;
    let list = v.get(format!("projects:{server}"))?.as_array()?;
    Some(list.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
}

pub fn save_projects(server: &str, ids: &[String]) {
    if cfg!(test) {
        return;
    }
    update("projects", format!("projects:{server}"), json!(ids));
}

/// Desktop Beta's tabs for its own sidecar server (the local `:4096` service), most recently
/// written window first. Read-only: `mode=ro` and `-readonly`.
pub fn from_beta() -> Option<Vec<Tab>> {
    from_beta_db(&crate::dirs::beta_drafts()?)
}

pub(crate) fn from_beta_db(db: &std::path::Path) -> Option<Vec<Tab>> {
    if !db.exists() {
        return None;
    }
    let uri = format!("file:{}?mode=ro", db.display());
    let sql = "select key, value from state where name = (select name from state where key = 'tabs' \
               and name like 'opencode.window.%' order by updated_at desc limit 1) and key in ('tabs', 'tabs.info')";
    let out = Command::new("sqlite3").args(["-readonly", "-json", &uri, sql]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let rows: Value = serde_json::from_slice(&out.stdout).ok()?;
    let get = |k: &str| -> Value {
        rows.as_array()
            .into_iter()
            .flatten()
            .find(|r| r["key"] == k)
            .and_then(|r| serde_json::from_str(r["value"].as_str()?).ok())
            .unwrap_or(Value::Null)
    };
    // Beta's rows are already in the mixed-server shape; its own sidecar's tabs are this app's.
    let beta = opencode_kit::servers::migration::window::from_beta(&get("tabs"), &get("tabs.info"));
    let list = beta.tabs.iter().flatten().filter(|t| matches!(t, doc::Tab::Session(s) if s.server.is_sidecar())).map(|t| Tab::from_doc(t, beta.info(&t.key()))).collect();
    Some(list)
}
