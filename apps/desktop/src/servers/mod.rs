//! The servers you add by address (Desktop's `ServerConnection.Http` list, `runtime/server/
//! registry.tsx`): each one's address, optional password and name, saved in `servers.json`, and
//! its health (`/api/info`, as Desktop's health check reads it). They join the app's registry of
//! servers beside the plugins' sources (`plugin_host::servers::all`), keyed by address as Desktop
//! keys them, one list every window shares: a server is added to the app, not switched to by a
//! window. Add server and Edit server are [`dialog`], over [`machine`].

pub mod dialog;
pub mod machine;
pub mod pairing;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
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
            r.list = if cfg!(test) { Vec::new() } else { load() };
        }
        f(r)
    })
}

/// Something in the registry changed: every window and plugin reading it draws again.
fn changed() {
    VERSION.set(VERSION.get().wrapping_add(1));
    crate::plugin_host::luau::bump();
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

fn file() -> Option<PathBuf> {
    crate::dirs::data().map(|d| d.join("servers.json"))
}

/// The saved list. Desktop's own stores each as `{type: "http", http: {url, password}, displayName}`.
fn load() -> Vec<Server> {
    let Some(v) = file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { return Vec::new() };
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
    if cfg!(test) {
        return;
    }
    let Some(path) = file() else { return };
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
    let body = json!({ "list": list }).to_string();
    crate::disk::later("servers", move || {
        crate::disk::replace(&path, body.as_bytes());
    });
}

/// `checkServerHealth`: `/api/info` with the password, if any. Blocks: off the app's thread only.
pub fn check(url: &str, password: Option<&str>, timeout: Duration) -> Health {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).build().into();
    let mut req = agent.get(format!("{url}/api/info"));
    if let Some(p) = password {
        use base64::Engine as _;
        req = req.header("authorization", format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("opencode:{p}"))));
    }
    match req.call() {
        Ok(resp) if resp.status().as_u16() == 401 => Health::Unauthorized,
        Ok(resp) if resp.status().is_success() => Health::Healthy,
        _ => Health::Unreachable,
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
pub fn remove(url: &str) {
    with(|r| {
        r.list.retain(|x| x.url != url);
        r.health.remove(url);
        save(&r.list);
    });
    changed();
}

/// Checks each server that has no check yet.
pub fn refresh(s: &App) {
    if !s.live() || cfg!(test) {
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
                    if r.list.iter().any(|x| *x == server) {
                        r.health.insert(server.url.clone(), health);
                    }
                });
                changed();
            }) as Apply);
        });
    }
}
