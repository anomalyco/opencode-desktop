//! What every window shared per server (`ui.json`'s sidebar, inbox and organized-sidebar state,
//! `tabs.json`'s zoom and used projects, `servers.json`'s list) to the `opencode.global.dat` keys
//! Desktop keeps them under (`server`, `layout`, `sidebar.inbox`, `sidebar.organize`), and back.

use opencode_kit::servers::key::ServerKey;
use serde_json::{Map, Value, json};

use super::{Context, is_server_url};
use crate::registry::{ServerConn, http_from_json, http_to_json};

/// The shared files as the old app read them.
#[derive(Clone, Copy, Debug, Default)]
pub struct Legacy<'a> {
    pub tabs_json: Option<&'a Value>,
    pub ui_json: Option<&'a Value>,
    pub servers_json: Option<&'a Value>,
}

/// The inbox's disclosures (`restore.rs` `InboxUi`), with its defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inbox {
    pub working: bool,
    pub recent: bool,
    pub home_automations: bool,
}

impl Inbox {
    fn parse(v: &Value) -> Inbox {
        Inbox {
            working: v["workingExpanded"].as_bool().unwrap_or(true),
            recent: v["recentExpanded"].as_bool().unwrap_or(false),
            home_automations: v["homeAutomationsExpanded"].as_bool().unwrap_or(false),
        }
    }

    fn to_json(self) -> Value {
        json!({ "workingExpanded": self.working, "recentExpanded": self.recent, "homeAutomationsExpanded": self.home_automations })
    }
}

/// The organized sidebar (`restore.rs` `restore_organize`): Projects shut, the open projects (a
/// set, so sorted here) and the projects' order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Organize {
    pub collapsed: bool,
    pub expanded: Vec<String>,
    pub order: Vec<String>,
}

impl Organize {
    fn parse(v: &Value) -> Organize {
        let strings = |v: &Value| v.as_array().into_iter().flatten().filter_map(|s| Some(s.as_str()?.to_owned())).collect::<Vec<_>>();
        let mut expanded = strings(&v["expanded"]);
        expanded.sort();
        expanded.dedup();
        Organize { collapsed: v["projectsCollapsed"].as_bool().unwrap_or(false), expanded, order: strings(&v["order"]) }
    }

    fn to_json(&self) -> Value {
        json!({ "projectsCollapsed": self.collapsed, "expanded": self.expanded, "order": self.order })
    }
}

/// One server's shared state as the old app read it: what [`restore`] must give back.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub sidebar: Option<bool>,
    pub sidebar_width: Option<f64>,
    pub inbox: Inbox,
    pub organize: Organize,
    /// `tabs.json`'s `projects:<url>`.
    pub projects: Option<Vec<String>>,
    /// `ui.json`'s other keys for the server, beside the window keys the window migration reads.
    pub rest: Map<String, Value>,
}

const UI_KEYS: [&str; 8] = ["route", "drafts", "side", "closedTabs", "sidebar", "sidebarWidth", "inbox", "organize"];

impl Entry {
    fn read(input: &Legacy<'_>, url: &str) -> Entry {
        let ui = input.ui_json.map(|u| &u[url]).unwrap_or(&Value::Null);
        Entry {
            sidebar: ui["sidebar"].as_bool(),
            sidebar_width: ui["sidebarWidth"].as_f64(),
            inbox: Inbox::parse(&ui["inbox"]),
            organize: Organize::parse(&ui["organize"]),
            projects: input.tabs_json.and_then(|t| t.get(format!("projects:{url}"))).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()),
            rest: ui.as_object().into_iter().flatten().filter(|(k, x)| !UI_KEYS.contains(&k.as_str()) && !x.is_null()).map(|(k, x)| (k.clone(), x.clone())).collect(),
        }
    }
}

/// Each server address's shared state, in migration order.
pub fn entries(input: &Legacy<'_>, ctx: &Context) -> Vec<(String, Entry)> {
    let mut urls: Vec<String> = input.ui_json.and_then(Value::as_object).into_iter().flatten().map(|(k, _)| k.clone()).filter(|k| is_server_url(k)).collect();
    for k in input.tabs_json.and_then(Value::as_object).into_iter().flatten().map(|(k, _)| k) {
        if let Some(url) = k.strip_prefix("projects:").filter(|u| !urls.iter().any(|x| x == u)) {
            urls.push(url.to_owned());
        }
    }
    ctx.order(urls).into_iter().map(|u| (u.clone(), Entry::read(input, &u))).collect()
}

/// Builds the global keys from the shared files. An `existing` global file that already has a
/// server registry comes back as it is; otherwise its other keys stay beside the new ones.
pub fn migrate(input: &Legacy<'_>, ctx: &Context, existing: Option<&Value>) -> Value {
    if let Some(done) = existing.filter(|e| e.get("server").is_some()) {
        return done.clone();
    }
    let entries = entries(input, ctx);
    let urls: Vec<String> = entries.iter().map(|(u, _)| u.clone()).collect();
    let active = ctx.active_of(&urls).cloned();
    let list: Vec<Value> = input.servers_json.and_then(|s| s["list"].as_array()).into_iter().flatten().filter_map(http_from_json).filter_map(|c| http_to_json(&c)).collect();
    let mut used: Map<String, Value> = Map::new();
    let (mut expanded, mut order) = (Map::new(), Map::new());
    let mut sidebar = Map::new();
    let mut inbox = None;
    let mut collapsed = false;
    for (url, e) in &entries {
        let key = ctx.key(url);
        if let Some(ids) = &e.projects {
            let slot = used.entry(key.scope().to_string()).or_insert_with(|| json!([]));
            let Some(list) = slot.as_array_mut() else { continue };
            for id in ids {
                if !list.iter().any(|x| x == id) {
                    list.push(json!(id));
                }
            }
        }
        for id in &e.organize.expanded {
            expanded.insert(format!("{key}:{id}"), json!(true));
        }
        if !e.organize.order.is_empty() && !order.contains_key(key.as_str()) {
            order.insert(key.to_string(), json!(e.organize.order));
        }
        if active.as_ref() == Some(url) {
            if let Some(b) = e.sidebar {
                sidebar.insert("opened".into(), json!(b));
            }
            if let Some(w) = e.sidebar_width {
                sidebar.insert("width".into(), json!(w));
            }
            inbox = Some(e.inbox);
            collapsed = e.organize.collapsed;
        }
    }
    let mut doc = existing.and_then(Value::as_object).cloned().unwrap_or_default();
    let mut put = |k: &str, v: Value| {
        doc.entry(k.to_owned()).or_insert(v);
    };
    put("server", json!({ "list": list, "hidden": {}, "lastProjectID": {}, "usedProjectIDs": used, "hostnames": {}, "projects": {} }));
    if !sidebar.is_empty() {
        put("layout", json!({ "sidebar": sidebar }));
    }
    if let Some(i) = inbox {
        put("sidebar.inbox", i.to_json());
    }
    put("sidebar.organize", json!({ "projectsCollapsed": collapsed, "expanded": expanded, "order": order }));
    if let Some(z) = input.tabs_json.and_then(|t| t.get("zoom")).filter(|z| z.is_number()) {
        put("zoom", z.clone());
    }
    let built = Value::Object(doc.clone());
    let mut legacy = Map::new();
    for (url, e) in &entries {
        let key = ctx.key(url);
        let is_active = active.as_ref() == Some(url);
        let mut record = Map::new();
        record.insert("server".into(), json!(key));
        if is_active {
            record.insert("active".into(), json!(true));
        }
        let derived = derive(&built, &key, is_active);
        if e.sidebar != derived.sidebar {
            record.insert("sidebar".into(), json!(e.sidebar));
        }
        if e.sidebar_width != derived.sidebar_width {
            record.insert("sidebarWidth".into(), json!(e.sidebar_width));
        }
        if e.inbox != derived.inbox {
            record.insert("inbox".into(), e.inbox.to_json());
        }
        if e.organize != derived.organize {
            record.insert("organize".into(), e.organize.to_json());
        }
        if e.projects != derived.projects {
            record.insert("projects".into(), json!(e.projects));
        }
        if !e.rest.is_empty() {
            record.insert("rest".into(), Value::Object(e.rest.clone()));
        }
        legacy.insert(url.clone(), Value::Object(record));
    }
    let leftovers = |v: Option<&Value>, keep: &dyn Fn(&str) -> bool| -> Map<String, Value> { v.and_then(Value::as_object).into_iter().flatten().filter(|(k, _)| keep(k)).map(|(k, x)| (k.clone(), x.clone())).collect() };
    let tabs_rest = leftovers(input.tabs_json, &|k| !is_server_url(k) && !k.starts_with("projects:") && k != "zoom");
    let ui_rest = leftovers(input.ui_json, &|k| !is_server_url(k));
    let servers_rest = leftovers(input.servers_json, &|k| k != "list");
    for (name, rest) in [("tabs.json", tabs_rest), ("ui.json", ui_rest), ("servers.json", servers_rest)] {
        if !rest.is_empty() {
            legacy.insert(name.into(), Value::Object(rest));
        }
    }
    if !legacy.is_empty() {
        doc.insert("legacy".into(), Value::Object(legacy));
    }
    Value::Object(doc)
}

/// What the global keys say a server's shared state was, before the per-address overrides.
fn derive(doc: &Value, key: &ServerKey, active: bool) -> Entry {
    let organize = &doc["sidebar.organize"];
    let prefix = format!("{key}:");
    let mut expanded: Vec<String> = organize["expanded"].as_object().into_iter().flatten().filter(|(_, v)| **v == true).filter_map(|(k, _)| k.strip_prefix(&prefix).map(str::to_owned)).collect();
    expanded.sort();
    let order = organize["order"][key.as_str()].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_owned)).collect();
    let projects = doc["server"]["usedProjectIDs"].get(key.scope().as_str()).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect());
    Entry {
        sidebar: if active { doc["layout"]["sidebar"]["opened"].as_bool() } else { None },
        sidebar_width: if active { doc["layout"]["sidebar"]["width"].as_f64() } else { None },
        inbox: Inbox::parse(if active { &doc["sidebar.inbox"] } else { &Value::Null }),
        organize: Organize { collapsed: active && organize["projectsCollapsed"].as_bool().unwrap_or(false), expanded, order },
        projects,
        rest: Map::new(),
    }
}

/// What a migrated global file gives back: each address's shared state exactly as the old app
/// read it, the zoom, and the saved servers. The inverse of [`migrate`].
pub fn restore(doc: &Value) -> (Vec<(String, Entry)>, Option<f64>, Vec<ServerConn>) {
    let entries = doc["legacy"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(url, _)| is_server_url(url))
        .filter_map(|(url, record)| {
            let key = ServerKey::new(record["server"].as_str()?);
            let mut e = derive(doc, &key, record["active"] == true);
            if let Some(v) = record.get("sidebar") {
                e.sidebar = v.as_bool();
            }
            if let Some(v) = record.get("sidebarWidth") {
                e.sidebar_width = v.as_f64();
            }
            if let Some(v) = record.get("inbox") {
                e.inbox = Inbox::parse(v);
            }
            if let Some(v) = record.get("organize") {
                e.organize = Organize::parse(v);
            }
            if let Some(v) = record.get("projects") {
                e.projects = v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect());
            }
            e.rest = record["rest"].as_object().cloned().unwrap_or_default();
            Some((url.clone(), e))
        })
        .collect();
    let servers = doc["server"]["list"].as_array().into_iter().flatten().filter_map(http_from_json).collect();
    (entries, doc["zoom"].as_f64(), servers)
}

/// The saved servers as the old app read them (`servers.json`).
pub fn saved_servers(input: &Legacy<'_>) -> Vec<ServerConn> {
    input.servers_json.and_then(|s| s["list"].as_array()).into_iter().flatten().filter_map(http_from_json).collect()
}
