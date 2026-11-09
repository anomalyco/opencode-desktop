//! A window's per-server entries to one mixed-server [`WindowDoc`], and back.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::{Context, is_server_url};
use crate::servers::key::{Route, ServerKey, TabKey};
use crate::servers::tabs::{ClosedTab, DraftTab, SessionTab, Tab, TabInfo, WindowDoc};

/// What one window read before: its own file and, for the profile's first window, the shared
/// files it fell back to for a server its own file had nothing for (`windows/store.rs` `load`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Legacy<'a> {
    /// `opencode.window.<id>.dat`.
    pub file: Option<&'a Value>,
    /// The profile's first window, which reads `tabs.json` and `ui.json`.
    pub first: bool,
    pub tabs_json: Option<&'a Value>,
    pub ui_json: Option<&'a Value>,
}

/// One old tab as the old app read it (`tabs.rs` `parse`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OldTab {
    pub id: String,
    pub draft: bool,
    pub title: String,
    pub directory: String,
}

/// One old closed tab as the old app read it (`tabs.rs` `Closed::from_json`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OldClosed {
    pub id: String,
    pub title: String,
    pub directory: String,
    pub index: u64,
    pub prompted: bool,
}

/// One server's old window entry, as the old app read it: what [`restore`] must give back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub tabs: Option<Vec<OldTab>>,
    pub route: Option<Value>,
    pub drafts: Option<BTreeMap<String, Value>>,
    pub side: Option<Value>,
    pub closed: Option<Vec<OldClosed>>,
    pub rest: Map<String, Value>,
}

const ENTRY_KEYS: [&str; 5] = ["tabs", "route", "drafts", "side", "closedTabs"];

impl Entry {
    /// Reads an old entry: null reads as absent, an empty draft map or closed list as none, and
    /// closed tabs as the old list kept them (one per session, at its latest close, the newest 100).
    pub fn parse(v: &Value) -> Entry {
        let some = |k: &str| v.get(k).filter(|x| !x.is_null()).cloned();
        Entry {
            tabs: v.get("tabs").and_then(Value::as_array).map(|a| a.iter().filter_map(old_tab).collect()),
            route: some("route"),
            drafts: v["drafts"].as_object().map(|m| m.iter().filter(|(_, d)| !d.is_null()).map(|(k, d)| (k.clone(), d.clone())).collect()).filter(|m: &BTreeMap<_, _>| !m.is_empty()),
            side: some("side"),
            closed: Some(old_closed(&v["closedTabs"])).filter(|c| !c.is_empty()),
            rest: v.as_object().into_iter().flatten().filter(|(k, x)| !ENTRY_KEYS.contains(&k.as_str()) && !x.is_null()).map(|(k, x)| (k.clone(), x.clone())).collect(),
        }
    }

    /// The old entry's JSON, as the old app wrote it.
    pub fn to_json(&self) -> Value {
        let mut v = Map::new();
        if let Some(tabs) = &self.tabs {
            v.insert("tabs".into(), tabs_json(tabs));
        }
        for (k, x) in [("route", &self.route), ("side", &self.side)] {
            if let Some(x) = x {
                v.insert(k.into(), x.clone());
            }
        }
        if let Some(d) = &self.drafts {
            v.insert("drafts".into(), Value::Object(d.iter().map(|(k, x)| (k.clone(), x.clone())).collect()));
        }
        if let Some(c) = &self.closed {
            v.insert("closedTabs".into(), closed_json(c));
        }
        v.extend(self.rest.clone());
        Value::Object(v)
    }
}

fn old_tab(t: &Value) -> Option<OldTab> {
    let id = t["id"].as_str()?.to_owned();
    let directory = t["directory"].as_str().unwrap_or("").to_owned();
    // Lists saved before drafts were tabs have no kind: they're all sessions.
    let draft = t["kind"].as_str() == Some("draft");
    let title = if draft { String::new() } else { t["title"].as_str().unwrap_or("").to_owned() };
    Some(OldTab { id, draft, title, directory })
}

fn old_closed(v: &Value) -> Vec<OldClosed> {
    let mut out: Vec<OldClosed> = Vec::new();
    for e in v.as_array().into_iter().flatten() {
        let Some(id) = e["id"].as_str() else { continue };
        let entry = OldClosed {
            id: id.to_owned(),
            title: e["title"].as_str().unwrap_or("").to_owned(),
            directory: e["directory"].as_str().unwrap_or("").to_owned(),
            index: e["index"].as_u64().unwrap_or(0),
            prompted: e["prompted"].as_bool().unwrap_or(false),
        };
        out.retain(|c| c.id != entry.id);
        out.push(entry);
    }
    let over = out.len().saturating_sub(crate::servers::tabs::CLOSED_LIMIT);
    out.drain(..over);
    out
}

fn tabs_json(tabs: &[OldTab]) -> Value {
    tabs.iter().map(|t| json!({ "id": t.id, "title": t.title, "directory": t.directory, "kind": if t.draft { "draft" } else { "session" } })).collect()
}

fn closed_json(closed: &[OldClosed]) -> Value {
    closed.iter().map(|c| json!({ "index": c.index, "id": c.id, "title": c.title, "directory": c.directory, "prompted": c.prompted })).collect()
}

/// The file is still in the per-server shape: a server address at its top level, or nothing
/// saved yet (the first window then still reads the shared files).
pub fn is_legacy(file: Option<&Value>) -> bool {
    match file.and_then(Value::as_object) {
        None => true,
        Some(m) => m.is_empty() || m.keys().any(|k| is_server_url(k)),
    }
}

/// Each server's entry as the old app read it, in migration order: the window's own entry, else
/// for the first window `tabs.json`'s list and `ui.json`'s page (`windows/store.rs` `legacy`).
pub fn entries(input: &Legacy<'_>, ctx: &Context) -> Vec<(String, Entry)> {
    let keys = |v: Option<&Value>| v.and_then(Value::as_object).into_iter().flatten().map(|(k, _)| k.clone()).filter(|k| is_server_url(k)).collect::<Vec<_>>();
    let mut urls = keys(input.file);
    if input.first {
        for u in keys(input.tabs_json).into_iter().chain(keys(input.ui_json)) {
            if !urls.contains(&u) {
                urls.push(u);
            }
        }
    }
    ctx.order(urls)
        .into_iter()
        .map(|url| {
            let own = input.file.and_then(|f| f.get(&url));
            let entry = match own {
                Some(v) => Entry::parse(v),
                None => Entry::parse(&shared(input, &url)),
            };
            (url, entry)
        })
        .collect()
}

/// `tabs.json` and `ui.json` for `url`, in a window entry's shape (`windows/store.rs` `legacy`).
fn shared(input: &Legacy<'_>, url: &str) -> Value {
    let ui = input.ui_json.map(|u| &u[url]).unwrap_or(&Value::Null);
    let mut v = json!({ "route": ui["route"], "drafts": ui["drafts"], "side": ui["side"], "closedTabs": ui["closedTabs"] });
    if let Some(tabs) = input.tabs_json.and_then(|t| t.get(url)).filter(|t| t.is_array()) {
        v["tabs"] = tabs.clone();
    }
    v
}

/// Migrates one window. A file already in the new shape comes back as it is.
pub fn migrate(input: &Legacy<'_>, ctx: &Context) -> Value {
    if let Some(file) = input.file.filter(|f| !is_legacy(Some(f))) {
        return file.clone();
    }
    let entries = entries(input, ctx);
    let urls: Vec<String> = entries.iter().map(|(u, _)| u.clone()).collect();
    let active = ctx.active_of(&urls).cloned();
    let mut doc = WindowDoc::default();
    for (url, e) in &entries {
        merge(&mut doc, &ctx.key(url), e, active.as_ref() == Some(url));
    }
    if doc.last_active_url.is_none() {
        doc.last_active_url = Some(Route::Home.href());
    }
    for (url, e) in &entries {
        let key = ctx.key(url);
        let is_active = active.as_ref() == Some(url);
        let mut record = Map::new();
        record.insert("server".into(), json!(key));
        if is_active {
            record.insert("active".into(), json!(true));
        }
        let derived = derive(&doc, &key, is_active);
        overrides(&mut record, e, &derived);
        doc.legacy.insert(url.clone(), Value::Object(record));
    }
    doc.to_json()
}

/// Adds one server's entry to the window: its tabs, closed tabs and drafts join the mixed lists
/// (the first address of a server wins a tab both list), and the active one's page and panes
/// become the window's.
fn merge(doc: &mut WindowDoc, key: &ServerKey, e: &Entry, active: bool) {
    if let Some(tabs) = &e.tabs {
        let list = doc.tabs.get_or_insert_with(Vec::new);
        for t in tabs {
            let tab = if t.draft { Tab::Draft(DraftTab::new(&t.id, key.clone(), &t.directory)) } else { Tab::Session(SessionTab::new(key.clone(), &t.id)) };
            let tab_key = tab.key();
            if list.iter().any(|x| x.key() == tab_key) {
                continue;
            }
            list.push(tab);
            let info = TabInfo { title: text(&t.title), directory: if t.draft { None } else { text(&t.directory) }, prompted: None };
            if !info.is_empty() {
                doc.info.push((tab_key, info));
            }
        }
    }
    for c in e.closed.iter().flatten() {
        let tab = SessionTab::new(key.clone(), &c.id);
        doc.closed.retain(|x| x.tab != tab);
        let info = TabInfo { title: text(&c.title), directory: text(&c.directory), prompted: c.prompted.then_some(true) };
        doc.closed.push(ClosedTab { tab, index: c.index, info: (!info.is_empty()).then_some(info) });
    }
    for (k, d) in e.drafts.iter().flatten() {
        let Some(tab_key) = draft_key(key, k) else { continue };
        if !doc.drafts.iter().any(|(x, _)| *x == tab_key) {
            doc.drafts.push((tab_key, d.clone()));
        }
    }
    if active {
        doc.side = e.side.clone();
        let route = e.route.as_ref().and_then(|r| route_of(key, r));
        // The page's tab is the window's last tab (`tabs.recent`).
        doc.recent = route
            .as_ref()
            .and_then(|r| match r {
                Route::Session { server, session_id } => Some(TabKey::session(server.clone(), session_id.clone())),
                Route::Draft(id) => Some(TabKey::draft(id)),
                _ => None,
            })
            .filter(|k| doc.tabs.iter().flatten().any(|t| t.key() == *k));
        doc.last_active_url = route.map(|r| r.href());
    }
}

/// What the window doc says a server's old entry was, before the per-address overrides.
fn derive(doc: &WindowDoc, key: &ServerKey, active: bool) -> Entry {
    let info = |k: &TabKey| doc.info(k).cloned().unwrap_or_default();
    let tabs = doc.tabs.as_ref().map(|tabs| {
        tabs.iter()
            .filter(|t| t.server() == key)
            .map(|t| match t {
                Tab::Session(s) => {
                    let i = info(&s.key());
                    OldTab { id: s.session_id.clone(), draft: false, title: i.title.unwrap_or_default(), directory: i.directory.unwrap_or_default() }
                }
                Tab::Draft(d) => OldTab { id: d.draft_id.clone(), draft: true, title: String::new(), directory: d.directory.clone() },
            })
            .collect()
    });
    let closed: Vec<OldClosed> = doc
        .closed
        .iter()
        .filter(|c| c.tab.server == *key)
        .map(|c| {
            let i = c.info.clone().unwrap_or_default();
            OldClosed { id: c.tab.session_id.clone(), title: i.title.unwrap_or_default(), directory: i.directory.unwrap_or_default(), index: c.index, prompted: i.prompted.unwrap_or(false) }
        })
        .collect();
    let drafts_on = |d: &str| doc.tabs.iter().flatten().any(|t| matches!(t, Tab::Draft(x) if x.draft_id == d && x.server == *key));
    let drafts: BTreeMap<String, Value> = doc
        .drafts
        .iter()
        .filter_map(|(k, d)| match k {
            TabKey::Session { server, session_id } if server == key => Some((session_id.clone(), d.clone())),
            TabKey::Draft(id) if drafts_on(id) => Some((id.clone(), d.clone())),
            _ => None,
        })
        .collect();
    Entry {
        tabs,
        route: active.then(|| doc.route().and_then(|r| route_json(&r))).flatten(),
        drafts: Some(drafts).filter(|d| !d.is_empty()),
        side: if active { doc.side.clone() } else { None },
        closed: Some(closed).filter(|c| !c.is_empty()),
        rest: Map::new(),
    }
}

/// Records each field `derived` gets wrong, as the old entry had it (`null`: it had none).
fn overrides(record: &mut Map<String, Value>, e: &Entry, derived: &Entry) {
    let old = e.to_json();
    let field = |k: &str| old.get(k).cloned().unwrap_or(Value::Null);
    if e.tabs != derived.tabs {
        record.insert("tabs".into(), field("tabs"));
    }
    if e.route != derived.route {
        record.insert("route".into(), field("route"));
    }
    if e.drafts != derived.drafts {
        record.insert("drafts".into(), field("drafts"));
    }
    if e.side != derived.side {
        record.insert("side".into(), field("side"));
    }
    if e.closed != derived.closed {
        record.insert("closedTabs".into(), field("closedTabs"));
    }
    if !e.rest.is_empty() {
        record.insert("rest".into(), Value::Object(e.rest.clone()));
    }
}

/// Gives back each server address's old entry from a migrated window file, exactly as the old
/// app read it: the inverse of [`migrate`].
pub fn restore(doc: &Value) -> Vec<(String, Entry)> {
    let window = WindowDoc::from_json(doc);
    window
        .legacy
        .iter()
        .filter_map(|(url, record)| {
            let key = ServerKey::new(record["server"].as_str()?);
            let mut e = derive(&window, &key, record["active"] == true);
            let parsed = Entry::parse(&Value::Object(record.as_object()?.iter().filter(|(k, _)| ENTRY_KEYS.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect()));
            let set = |k: &str| record.get(k).is_some();
            if set("tabs") {
                e.tabs = parsed.tabs;
            }
            if set("route") {
                e.route = parsed.route;
            }
            if set("drafts") {
                e.drafts = parsed.drafts;
            }
            if set("side") {
                e.side = parsed.side;
            }
            if set("closedTabs") {
                e.closed = parsed.closed;
            }
            e.rest = record["rest"].as_object().cloned().unwrap_or_default();
            Some((url.clone(), e))
        })
        .collect()
}

/// Desktop Beta's rows for a window (`drafts.sqlite`'s `tabs` and `tabs.info`) are already in
/// the mixed-server shape: every tab keeps its own server, the sidecar's and every other's.
pub fn from_beta(tabs: &Value, info: &Value) -> WindowDoc {
    WindowDoc::from_json(&json!({ "tabs": tabs, "tabs.info": info }))
}

/// An old draft key on server `key`: a draft tab's own id, or a session's. The key the
/// new-session page used before drafts were tabs (`"new"`) has no tab.
fn draft_key(key: &ServerKey, k: &str) -> Option<TabKey> {
    if k == "new" {
        return None;
    }
    if k.starts_with("draft_") {
        return Some(TabKey::draft(k));
    }
    Some(TabKey::session(key.clone(), k))
}

/// An old saved page (`restore.rs` `route_from`) as a route on server `key`.
fn route_of(key: &ServerKey, v: &Value) -> Option<Route> {
    if let Some(id) = v["session"].as_str() {
        return Some(Route::Session { server: key.clone(), session_id: id.to_owned() });
    }
    if let Some(id) = v["draft"].as_str() {
        return Some(Route::Draft(id.to_owned()));
    }
    if let Some(tab) = v["settings"].as_str() {
        return Some(Route::Settings(Some(tab.to_owned())));
    }
    (v.as_str() == Some("home")).then_some(Route::Home)
}

/// The inverse of [`route_of`].
fn route_json(route: &Route) -> Option<Value> {
    Some(match route {
        Route::Home => json!("home"),
        Route::Settings(tab) => json!({ "settings": tab.clone()? }),
        Route::Draft(id) => json!({ "draft": id }),
        Route::Session { session_id, .. } => json!({ "session": session_id }),
    })
}

fn text(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_owned())
}
