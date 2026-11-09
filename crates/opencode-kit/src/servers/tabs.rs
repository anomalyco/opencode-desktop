//! A window's tabs from every server, in the shape Desktop keeps them in its window file
//! (`shell/tabs/schema.ts`, `tabs.tsx:104-112`, `desktop/src/renderer/window/route-storage.ts`):
//! one flat, mixed-server list whose tabs each carry their server, beside the saved titles
//! (`tabs.info`), the closed tabs (`tabs.closed`), the last tab (`tabs.recent`) and the page
//! (`lastActiveUrl`). Rune also keeps the composer drafts and side panes there.

use serde_json::{Map, Value, json};

use super::key::{Route, ServerKey, TabKey};

/// A session tab (`TabStorage.Session`, `schema.ts:9-15`). The route is kept only when it
/// shows another session than the tab's own (a subagent), and its parent only with it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionTab {
    pub server: ServerKey,
    pub session_id: String,
    pub route_session_id: Option<String>,
    pub route_parent_id: Option<String>,
}

impl SessionTab {
    pub fn new(server: ServerKey, session_id: impl Into<String>) -> SessionTab {
        SessionTab { server, session_id: session_id.into(), route_session_id: None, route_parent_id: None }
    }

    /// `withRoute` (`schema.ts:72-81`).
    pub fn with_route(mut self, route_session_id: Option<String>, route_parent_id: Option<String>) -> SessionTab {
        match route_session_id.filter(|r| !r.is_empty() && *r != self.session_id) {
            Some(r) => {
                self.route_session_id = Some(r);
                self.route_parent_id = route_parent_id.filter(|p| !p.is_empty());
            }
            None => {
                self.route_session_id = None;
                self.route_parent_id = None;
            }
        }
        self
    }

    pub fn key(&self) -> TabKey {
        TabKey::session(self.server.clone(), self.session_id.clone())
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({ "type": "session", "server": self.server, "sessionId": self.session_id });
        if let Some(r) = &self.route_session_id {
            v["routeSessionId"] = json!(r);
            if let Some(p) = &self.route_parent_id {
                v["routeParentId"] = json!(p);
            }
        }
        v
    }

    pub fn from_json(v: &Value) -> Option<SessionTab> {
        if v["type"] != "session" {
            return None;
        }
        let text = |k: &str| v[k].as_str().map(str::to_owned);
        Some(SessionTab::new(ServerKey::new(v["server"].as_str()?), text("sessionId")?).with_route(text("routeSessionId"), text("routeParentId")))
    }
}

/// A new session's draft tab (`TabStorage.Draft`, `schema.ts:17-25`): where its session will start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftTab {
    pub draft_id: String,
    pub server: ServerKey,
    pub directory: String,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    /// The MCP toggles chosen for it, as Desktop keeps them (`{ target, states }`).
    pub mcp: Option<Value>,
}

impl DraftTab {
    pub fn new(draft_id: impl Into<String>, server: ServerKey, directory: impl Into<String>) -> DraftTab {
        DraftTab { draft_id: draft_id.into(), server, directory: directory.into(), worktree: None, branch: None, mcp: None }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({ "type": "draft", "draftID": self.draft_id, "server": self.server, "directory": self.directory });
        for (k, val) in [("worktree", &self.worktree), ("branch", &self.branch)] {
            if let Some(x) = val {
                v[k] = json!(x);
            }
        }
        if let Some(m) = &self.mcp {
            v["mcp"] = m.clone();
        }
        v
    }

    pub fn from_json(v: &Value) -> Option<DraftTab> {
        if v["type"] != "draft" {
            return None;
        }
        let text = |k: &str| v[k].as_str().map(str::to_owned);
        Some(DraftTab {
            draft_id: text("draftID")?,
            server: ServerKey::new(v["server"].as_str()?),
            directory: text("directory")?,
            worktree: text("worktree"),
            branch: text("branch"),
            mcp: v.get("mcp").filter(|m| m.is_object()).cloned(),
        })
    }
}

/// One tab of a window (`TabStorage.Tab`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tab {
    Session(SessionTab),
    Draft(DraftTab),
}

impl Tab {
    pub fn key(&self) -> TabKey {
        match self {
            Tab::Session(t) => t.key(),
            Tab::Draft(d) => TabKey::draft(&d.draft_id),
        }
    }

    pub fn server(&self) -> &ServerKey {
        match self {
            Tab::Session(t) => &t.server,
            Tab::Draft(d) => &d.server,
        }
    }

    /// `tabHref` (`tabs.tsx:67-68`): the page it shows.
    pub fn route(&self) -> Route {
        match self {
            Tab::Session(t) => Route::Session { server: t.server.clone(), session_id: t.route_session_id.clone().unwrap_or_else(|| t.session_id.clone()) },
            Tab::Draft(d) => Route::Draft(d.draft_id.clone()),
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            Tab::Session(t) => t.to_json(),
            Tab::Draft(d) => d.to_json(),
        }
    }

    pub fn from_json(v: &Value) -> Option<Tab> {
        SessionTab::from_json(v).map(Tab::Session).or_else(|| DraftTab::from_json(v).map(Tab::Draft))
    }
}

/// What a window saved of a tab, so it paints before its server answers (`TabStorage.Info`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TabInfo {
    pub title: Option<String>,
    pub directory: Option<String>,
    pub prompted: Option<bool>,
}

impl TabInfo {
    pub fn is_empty(&self) -> bool {
        *self == TabInfo::default()
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({});
        if let Some(t) = &self.title {
            v["title"] = json!(t);
        }
        if let Some(d) = &self.directory {
            v["directory"] = json!(d);
        }
        if let Some(p) = self.prompted {
            v["prompted"] = json!(p);
        }
        v
    }

    pub fn from_json(v: &Value) -> TabInfo {
        TabInfo { title: v["title"].as_str().map(str::to_owned), directory: v["directory"].as_str().map(str::to_owned), prompted: v["prompted"].as_bool() }
    }
}

/// A closed session tab and the place it had (`TabStorage.ClosedTab`, `schema.ts:64-68`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClosedTab {
    pub tab: SessionTab,
    pub index: u64,
    pub info: Option<TabInfo>,
}

impl ClosedTab {
    pub fn to_json(&self) -> Value {
        let mut v = json!({ "tab": self.tab.to_json(), "index": self.index });
        if let Some(i) = &self.info {
            v["info"] = i.to_json();
        }
        v
    }

    pub fn from_json(v: &Value) -> Option<ClosedTab> {
        Some(ClosedTab { tab: SessionTab::from_json(&v["tab"])?, index: v["index"].as_u64()?, info: v.get("info").filter(|i| i.is_object()).map(TabInfo::from_json) })
    }
}

/// How many closed tabs a window keeps (`CLOSED_TAB_LIMIT`, `shell/tabs/closed.ts`).
pub const CLOSED_LIMIT: usize = 100;

/// A window file in the mixed-server shape. `rest` keeps every key this doesn't know, and
/// `legacy` what a migration kept of the per-server shape that has no place here.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowDoc {
    /// None when the window never saved a list (it then opens on recent sessions).
    pub tabs: Option<Vec<Tab>>,
    pub info: Vec<(TabKey, TabInfo)>,
    pub closed: Vec<ClosedTab>,
    pub recent: Option<TabKey>,
    pub last_active_url: Option<String>,
    /// Composer text per tab key: a string, or `{ text, mode }` for one in shell mode.
    pub drafts: Vec<(TabKey, Value)>,
    pub side: Option<Value>,
    pub legacy: Map<String, Value>,
    pub rest: Map<String, Value>,
}

const DOC_KEYS: [&str; 8] = ["tabs", "tabs.info", "tabs.closed", "tabs.recent", "lastActiveUrl", "drafts", "side", "legacy"];

impl WindowDoc {
    pub fn info(&self, key: &TabKey) -> Option<&TabInfo> {
        self.info.iter().find(|(k, _)| k == key).map(|(_, i)| i)
    }

    pub fn route(&self) -> Option<Route> {
        Route::parse(self.last_active_url.as_deref()?)
    }

    pub fn to_json(&self) -> Value {
        let mut v = Map::new();
        if let Some(tabs) = &self.tabs {
            v.insert("tabs".into(), tabs.iter().map(Tab::to_json).collect());
        }
        if !self.info.is_empty() {
            v.insert("tabs.info".into(), Value::Object(self.info.iter().map(|(k, i)| (k.encode(), i.to_json())).collect()));
        }
        if !self.closed.is_empty() {
            v.insert("tabs.closed".into(), self.closed.iter().map(ClosedTab::to_json).collect());
        }
        if let Some(r) = &self.recent {
            v.insert("tabs.recent".into(), json!({ "key": r.encode() }));
        }
        if let Some(u) = &self.last_active_url {
            v.insert("lastActiveUrl".into(), json!(u));
        }
        if !self.drafts.is_empty() {
            v.insert("drafts".into(), Value::Object(self.drafts.iter().map(|(k, d)| (k.encode(), d.clone())).collect()));
        }
        if let Some(s) = &self.side {
            v.insert("side".into(), s.clone());
        }
        v.extend(self.rest.clone());
        if !self.legacy.is_empty() {
            v.insert("legacy".into(), Value::Object(self.legacy.clone()));
        }
        Value::Object(v)
    }

    /// Reads a window file in this shape; entries that don't decode are dropped, as Desktop's
    /// schema drops them.
    pub fn from_json(v: &Value) -> WindowDoc {
        let obj = |k: &str| v[k].as_object().into_iter().flatten();
        WindowDoc {
            tabs: v["tabs"].as_array().map(|a| a.iter().filter_map(Tab::from_json).collect()),
            info: obj("tabs.info").filter_map(|(k, i)| Some((TabKey::parse(k)?, TabInfo::from_json(i)))).collect(),
            closed: v["tabs.closed"].as_array().into_iter().flatten().filter_map(ClosedTab::from_json).collect(),
            recent: v["tabs.recent"]["key"].as_str().and_then(TabKey::parse),
            last_active_url: v["lastActiveUrl"].as_str().map(str::to_owned),
            drafts: obj("drafts").filter_map(|(k, d)| Some((TabKey::parse(k)?, d.clone()))).collect(),
            side: v.get("side").filter(|s| !s.is_null()).cloned(),
            legacy: v["legacy"].as_object().cloned().unwrap_or_default(),
            rest: v.as_object().into_iter().flatten().filter(|(k, _)| !DOC_KEYS.contains(&k.as_str())).map(|(k, x)| (k.clone(), x.clone())).collect(),
        }
    }
}
