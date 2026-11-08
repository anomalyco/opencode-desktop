//! Pinned session tabs. OpenCode Desktop keeps its tab list on the client (Electron's
//! `drafts.sqlite`), never on the server, so this example keeps its own in
//! `<data dir>/rune-opencode/tabs.json`, one list per server.
//!
//! Until you change a tab here, each launch reads Desktop Beta's list (read-only, through the
//! `sqlite3` CLI) so the two show the same tabs; without Beta it starts from recent sessions.

use std::path::PathBuf;
use std::process::Command;

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
    /// The title and directory when the tab was saved, for a session outside the loaded list.
    /// A draft's directory is where its session will start; its title is unused.
    pub title: String,
    pub directory: String,
}

impl Tab {
    pub fn session(id: impl Into<String>, title: impl Into<String>, directory: impl Into<String>) -> Tab {
        Tab { id: id.into(), kind: TabKind::Session, title: title.into(), directory: directory.into() }
    }

    pub fn draft(id: impl Into<String>, directory: impl Into<String>) -> Tab {
        Tab { id: id.into(), kind: TabKind::Draft, title: String::new(), directory: directory.into() }
    }

    pub fn is_draft(&self) -> bool {
        self.kind == TabKind::Draft
    }

    /// The page this tab shows.
    pub fn route(&self) -> Route {
        match self.kind {
            TabKind::Session => Route::Session(self.id.clone()),
            TabKind::Draft => Route::Draft(self.id.clone()),
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
/// its latest close. Drafts are never recorded; their state goes with them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Closed(Vec<ClosedTab>);

impl Closed {
    pub fn push(&mut self, entry: ClosedTab) {
        if entry.tab.is_draft() {
            return;
        }
        self.0.retain(|e| e.tab.id != entry.tab.id);
        self.0.push(entry);
        let over = self.0.len().saturating_sub(CLOSED_LIMIT);
        self.0.drain(..over);
    }

    /// Takes the newest closed tab that isn't open again (or `target`'s), dropping stale ones
    /// passed on the way (`takeClosedTab`).
    pub fn take(&mut self, open: &[Tab], target: Option<&str>) -> Option<ClosedTab> {
        let is_open = |e: &ClosedTab| open.iter().any(|t| t.kind == TabKind::Session && t.id == e.tab.id);
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
        self.0.iter().rev().filter(|e| !open.iter().any(|t| t.kind == TabKind::Session && t.id == e.tab.id))
    }

    /// Sessions that are gone for good leave the list (`removeClosedTabs`).
    pub fn remove(&mut self, gone: impl Fn(&str) -> bool) {
        self.0.retain(|e| !gone(&e.tab.id));
    }

    pub fn to_json(&self) -> Value {
        self.0
            .iter()
            .map(|e| json!({ "index": e.index, "id": e.tab.id, "title": e.tab.title, "directory": e.tab.directory, "prompted": e.prompted }))
            .collect()
    }

    pub fn from_json(v: &Value) -> Closed {
        let list = v.as_array().into_iter().flatten().filter_map(|e| {
            let tab = Tab::session(e["id"].as_str()?, e["title"].as_str().unwrap_or(""), e["directory"].as_str().unwrap_or(""));
            Some(ClosedTab { tab, index: e["index"].as_u64().unwrap_or(0) as usize, prompted: e["prompted"].as_bool().unwrap_or(false) })
        });
        let mut closed = Closed::default();
        list.for_each(|e| closed.push(e));
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

fn parse(list: &Value) -> Vec<Tab> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let id = t["id"].as_str()?;
            let directory = t["directory"].as_str().unwrap_or("");
            // Lists saved before drafts were tabs have no kind: they're all sessions.
            Some(match t["kind"].as_str() {
                Some("draft") => Tab::draft(id, directory),
                _ => Tab::session(id, t["title"].as_str().unwrap_or(""), directory),
            })
        })
        .collect()
}

/// Window `w`'s saved list for `server`, if you've changed tabs there before.
pub fn load(w: &crate::windows::Window, server: &str) -> Option<Vec<Tab>> {
    if w.id.is_empty() {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(file()?).ok()?).ok()?;
        return saved(&v, server);
    }
    let v = crate::windows::store::load(w, server);
    v.get("tabs").filter(|t| t.is_array()).map(parse)
}

/// `server`'s list in the file's JSON; none when only other keys (such as `zoom`) are there.
fn saved(v: &Value, server: &str) -> Option<Vec<Tab>> {
    v.get(server)?.as_array()?;
    Some(parse(&v[server]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_with_only_zoom_has_no_saved_tabs() {
        let v = json!({ "zoom": 1.2 });
        assert_eq!(saved(&v, "http://x"), None);
        let v = json!({ "zoom": 1.2, "http://x": [{ "id": "a", "title": "A", "directory": "/d" }] });
        assert_eq!(saved(&v, "http://x").map(|t| t.len()), Some(1));
        assert_eq!(saved(&json!({ "http://x": [] }), "http://x"), Some(vec![]));
    }
}

/// Saves window `w`'s list for `server`: in its own file, or `tabs.json` for a window without one.
pub fn save(w: &crate::windows::Window, server: &str, tabs: &[Tab]) {
    // Tests record what they would save in `SAVED`; they never touch the saved file.
    if cfg!(test) {
        return;
    }
    let list: Value = tabs
        .iter()
        .map(|t| {
            let kind = if t.is_draft() { "draft" } else { "session" };
            json!({ "id": t.id, "title": t.title, "directory": t.directory, "kind": kind })
        })
        .collect();
    if w.id.is_empty() {
        update("tabs", server.to_owned(), list);
    } else {
        crate::windows::store::save(w, server, "tabs", list);
    }
}

/// Sets `key` in the tabs file, off the UI thread. Tabs and zoom share the file, so each read
/// and write happens on the saves thread, in order.
fn update(slot: &'static str, key: String, value: Value) {
    let Some(path) = file() else { return };
    crate::disk::later(slot, move || {
        let mut v: Value = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
        v[key] = value;
        crate::disk::replace(&path, v.to_string().as_bytes());
    });
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
    let (tabs, info) = (get("tabs"), get("tabs.info"));
    let list: Vec<Tab> = tabs
        .as_array()?
        .iter()
        .filter(|t| t["type"] == "session" && t["server"] == "sidecar")
        .filter_map(|t| {
            let id = t["sessionId"].as_str()?.to_owned();
            // tabs.info is keyed "<server>\n/server/<base64 server>/session/<id>".
            let meta = info.as_object().and_then(|m| m.iter().find(|(k, _)| k.ends_with(&format!("/session/{id}"))).map(|e| e.1));
            let field = |k: &str| meta.and_then(|m| m[k].as_str()).unwrap_or("").to_owned();
            Some(Tab::session(id, field("title"), field("directory")))
        })
        .collect();
    Some(list)
}
