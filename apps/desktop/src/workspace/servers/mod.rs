//! The servers you add by address (Desktop's `ServerConnection.Http` list, `runtime/server/
//! registry.tsx`): each one's address, optional password and name, saved in the app's database (Desktop's global `server.list`), and
//! its health (`/api/info`, as Desktop's health check reads it). They join the app's registry of
//! servers beside the plugins' sources (`plugin_host::servers::all`), keyed by address as Desktop
//! keys them, one list every window shares: a server is added to the app, not switched to by a
//! window. Add server and Edit server are [`dialog`], over [`machine`].

pub mod dialog;
pub mod health;
pub mod indicator;
pub mod machine;
pub mod pairing;
pub mod registry;
pub mod runtime;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use rune::Tx;
use serde_json::{Value, json};

use crate::App;
use crate::jobs::Apply;

/// A server added by address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    pub url: String,
    pub password: Option<String>,
    pub name: Option<String>,
}

impl Server {
    /// `serverName`: its name, else its address without the scheme.
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.url.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/').to_owned())
    }
}

/// What a check found (`ServerProbeResult`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Health {
    Healthy,
    /// The server rejects the saved credentials.
    Unauthorized,
    Unreachable,
}

/// The list and each server's last check, shared by every window (they all run on the app's
/// thread).
#[derive(Default)]
struct Registry {
    loaded: bool,
    list: Vec<Server>,
    health: HashMap<String, Health>,
    /// Checks in flight, by address.
    checking: HashSet<String>,
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
    static VERSION: Cell<u64> = const { Cell::new(0) };
}

fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    REGISTRY.with_borrow_mut(|r| {
        if !r.loaded {
            r.loaded = true;
            r.list = load();
        }
        f(r)
    })
}

/// Something in the registry changed: every window and plugin reading it draws again.
fn changed() {
    VERSION.set(VERSION.get().wrapping_add(1));
    crate::plugin_host::bump();
}

/// Changes with every change to the list or a server's health.
pub fn version() -> u64 {
    VERSION.get()
}

/// The servers added by address, in the order added.
pub fn list() -> Vec<Server> {
    with(|r| r.list.clone())
}

/// Server `url`'s last check; None while it has none.
pub fn health(url: &str) -> Option<Health> {
    with(|r| r.health.get(url).copied())
}

/// The saved list: Desktop's global `server.list`, each `{type: "http", http: {url, password}, displayName}`.
fn load() -> Vec<Server> {
    let Some(v) = crate::storage::json("opencode.global.dat", "server") else { return Vec::new() };
    v["list"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let url = pairing::normalize(s["http"]["url"].as_str()?)?;
            let text = |v: &Value| v.as_str().filter(|t| !t.is_empty()).map(str::to_owned);
            Some(Server { url, password: text(&s["http"]["password"]), name: text(&s["displayName"]) })
        })
        .collect()
}

fn save(list: &[Server]) {
    let list: Vec<Value> = list
        .iter()
        .map(|s| {
            let mut http = json!({ "url": s.url });
            if let Some(p) = &s.password {
                http["password"] = json!(p);
            }
            let mut v = json!({ "type": "http", "http": http });
            if let Some(n) = &s.name {
                v["displayName"] = json!(n);
            }
            v
        })
        .collect();
    crate::storage::update("opencode.global.dat", "server", |m| {
        m.insert("list".into(), Value::Array(list));
    });
}

/// `checkServerHealth`: `/api/info` with the password, if any. Healthy only on OpenCode's own
/// answer: a sign-in page a captive portal redirects to isn't a server. Blocks: off the app's
/// thread only.
pub fn check(url: &str, password: Option<&str>, timeout: Duration) -> Health {
    use opencode_kit::servers::probe::{RequestError, info};
    let http = opencode_kit::servers::registry::HttpBase { url: url.to_owned(), password: password.map(str::to_owned) };
    match info(&http, timeout) {
        Ok(_) => Health::Healthy,
        Err(RequestError::Status(401)) => Health::Unauthorized,
        Err(_) => Health::Unreachable,
    }
}

/// `servers.add`: by address, replacing the one saved there; its health is checked afresh.
pub fn add(s: &mut App, server: machine::Saved) {
    let Some(url) = pairing::normalize(&server.http.url) else { return };
    let entry = Server { url: url.clone(), password: server.http.password, name: server.name };
    with(|r| {
        match r.list.iter_mut().find(|x| x.url == url) {
            Some(x) => *x = entry,
            None => r.list.push(entry),
        }
        r.health.remove(&url);
        save(&r.list);
    });
    changed();
    refresh(s);
}

/// `servers.remove`.
pub fn remove(url: &str) -> Option<(Server, usize)> {
    let gone = with(|r| {
        let at = r.list.iter().position(|x| x.url == url)?;
        let server = r.list.remove(at);
        r.health.remove(url);
        save(&r.list);
        Some((server, at))
    });
    changed();
    gone
}

/// A removed server's Undo: listed again at `index`, password and name as they were.
pub fn put_back(server: Server, index: usize) {
    with(|r| {
        r.list.retain(|x| x.url != server.url);
        let at = index.min(r.list.len());
        r.list.insert(at, server);
        save(&r.list);
    });
    changed();
}

/// Removes `url` and offers it back (`crate::undo`).
pub fn remove_offering(s: &mut App, url: &str) {
    if let Some((server, index)) = remove(url) {
        crate::undo::offer(s, opencode_i18n::t!("rune.undo.serverRemoved"), crate::undo::Undoable::Server { server, index });
    }
}

/// Checks each server that has no check yet.
pub fn refresh(s: &App) {
    if !s.live() {
        return;
    }
    let due: Vec<Server> = with(|r| {
        let due: Vec<Server> = r.list.iter().filter(|x| !r.health.contains_key(&x.url) && !r.checking.contains(&x.url)).cloned().collect();
        r.checking.extend(due.iter().map(|x| x.url.clone()));
        due
    });
    for server in due {
        let deliver = s.deliverer();
        crate::plugin_host::tasks::run(move || {
            let health = check(&server.url, server.password.as_deref(), Duration::from_secs(30));
            deliver(Box::new(move |_: &mut App, tx: &mut Tx| {
                tx.instant();
                with(|r| {
                    r.checking.remove(&server.url);
                    if r.list.contains(&server) {
                        r.health.insert(server.url.clone(), health);
                    }
                });
                changed();
            }) as Apply);
        });
    }
}
