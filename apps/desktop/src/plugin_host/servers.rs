//! The app's registry of servers besides its own (Desktop's `Server` registry): the ones plugins
//! contribute (`contrib::SERVER` and Luau server sources), keyed `<plugin>:<id>`, ordered by
//! their source's `order`, checked for health once they're ready (their version, as Desktop's
//! health poll reads it); then the ones added by address (`crate::servers`), keyed by it. Every
//! window shares it: servers are added to the app, not switched to.

use std::cell::RefCell;
use std::collections::HashMap;

use opencode_kit::servers::HttpBase;
use opencode_kit::servers::plugin_server::{self, PluginEntry, PluginStage};
use opencode_plugin::abi::{SERVER_READY, contrib as kinds};
use rune::Tx;
use rune::machine::Attempt;

use super::contrib::active;
use super::instance::ServerRec;
use crate::App;

/// The slot of a server added by address (Luau sources' use `usize::MAX`).
pub const BY_ADDRESS: usize = usize::MAX - 1;

/// A server in the registry with what the app knows of it.
#[derive(Clone, Debug)]
pub struct Contributed {
    /// `<plugin>:<id>`, or the address of one added by address.
    pub key: String,
    pub slot: usize,
    pub rec: ServerRec,
}

impl Contributed {
    /// Added by address, rather than by a plugin.
    pub fn by_address(&self) -> bool {
        self.slot == BY_ADDRESS
    }
}

/// Every contributed server, its source's `order` first, then as listed.
pub fn all(app: &App) -> Vec<Contributed> {
    let host = &app.plugins;
    let mut out: Vec<(i32, Contributed)> = Vec::new();
    for (i, slot) in host.slots.iter().enumerate() {
        let Some(inst) = slot.active() else { continue };
        for rec in &inst.servers {
            let order = active(host, kinds::SERVER).find(|(s, c)| *s == i && c.id == rec.source).map_or(0, |(_, c)| c.order);
            let key = format!("{}:{}", slot.manifest.id, rec.id);
            out.push((order, Contributed { key, slot: i, rec: rec.clone() }));
        }
    }
    // Luau server sources' entries, shared by every window; a window half draws their rows.
    out.extend(super::luau::servers().into_iter().map(|(plugin, order, rec)| (order as i32, Contributed { key: format!("{plugin}:{}", rec.id), slot: usize::MAX, rec })));
    out.sort_by_key(|(order, _)| *order);
    // Then the servers added by address, as Desktop lists its stored ones after those provided.
    let by_address = crate::servers::list().into_iter().map(|s| Contributed {
        key: s.url.clone(),
        slot: BY_ADDRESS,
        rec: ServerRec { source: "http".into(), id: s.url.clone(), name: s.label(), label: String::new(), state: SERVER_READY, url: s.url, password: s.password.unwrap_or_default(), listed: true, managed: false },
    });
    out.into_iter().map(|(_, c)| c).chain(by_address).collect()
}

/// What sources list changed: checks the health of servers that became ready, and forgets it
/// for the ones that aren't any more (a Luau source whose list isn't complete keeps it).
pub fn changed(app: &mut App, tx: &mut Tx) {
    tx.instant();
    let servers = all(app);
    let pending = super::luau::servers::pending_sources();
    let facts = &mut app.plugins.shared.facts;
    facts.health.retain(|key, _| servers.iter().any(|s| &s.key == key && s.rec.state == SERVER_READY) || pending.iter().any(|(plugin, _)| key.starts_with(&format!("{plugin}:"))));
    let fresh: Vec<Contributed> = servers.into_iter().filter(|s| !s.by_address() && s.rec.state == SERVER_READY && !s.rec.url.is_empty() && !app.plugins.checking.contains(&s.key)).filter(|s| !app.plugins.shared.facts.health.contains_key(&s.key)).collect();
    for s in fresh {
        app.plugins.checking.insert(s.key.clone());
        let deliver = app.deliverer();
        super::tasks::run(move || {
            let server = opencode_kit::client::Server::new(&s.rec.url).with_password(&s.rec.password);
            let version = server.get("/api/info").ok().and_then(|v| v["version"].as_str().map(str::to_owned));
            deliver(Box::new(move |app: &mut App, tx: &mut Tx| {
                app.plugins.checking.remove(&s.key);
                let still = all(app).into_iter().any(|c| c.key == s.key && c.rec.url == s.rec.url && c.rec.state == SERVER_READY);
                if let (Some(v), true) = (version, still) {
                    app.plugins.shared.facts.health.insert(s.key.clone(), v);
                    if s.slot == usize::MAX {
                        super::luau::bump();
                    } else {
                        app.plugins.wake.wake(super::vtable::slot_bit(s.slot), false);
                    }
                    tx.instant();
                }
            }));
        });
    }
}

/// Reads the app's own server's version (`/api/info`) for plugins that compare against it.
pub fn read_own_version(app: &mut App) {
    let crate::Source::Live { server, .. } = &app.source else { return };
    let (server, deliver) = (server.clone(), app.deliverer());
    super::tasks::run(move || {
        let info = server.get("/api/info").ok();
        let field = |k: &str| info.as_ref().and_then(|v| v[k].as_str().map(str::to_owned));
        let version = field("version");
        deliver(Box::new(move |app: &mut App, _: &mut Tx| {
            if let Some(v) = version {
                app.plugins.shared.facts.server_version = Some(v);
                let all = (0..app.plugins.slots.len()).fold(0, |bits, i| bits | super::vtable::slot_bit(i));
                app.plugins.wake.wake(all, false);
            }
        }));
    });
}

/// Home shows plugin server `key` once it's ready and listed (`Servers:Connect`, kept as the
/// design's deprecated alias): every listed server is live at once (`servers::runtime`), so
/// nothing connects or switches.
pub fn connect(app: &mut App, key: &str, tx: &mut Tx) {
    // One that waits for a sign-in starts it, as Home's Authenticate does.
    sync_actions(app);
    if can_authenticate(key) {
        return authenticate(app, key, tx);
    }
    if !all(app).into_iter().any(|s| !s.by_address() && s.key == key && s.rec.listed && s.rec.state == SERVER_READY) {
        return;
    }
    app.focus_server(&opencode_kit::servers::ServerKey::new(key), tx);
}

/// A plugin server's stage (`SERVER_*` but ready).
pub fn stage(state: u32) -> PluginStage {
    match state {
        opencode_plugin::abi::SERVER_STARTING => PluginStage::Starting,
        opencode_plugin::abi::SERVER_AUTH => PluginStage::Auth,
        opencode_plugin::abi::SERVER_FAILED => PluginStage::Failed,
        opencode_plugin::abi::SERVER_INCOMPATIBLE => PluginStage::Incompatible,
        _ => PluginStage::Stopped,
    }
}

/// What the app reads of plugin server `c`'s entry (`PluginEntrySnapshot`).
fn entry(c: &Contributed) -> PluginEntry {
    if c.rec.state != SERVER_READY {
        return PluginEntry::staged(stage(c.rec.state));
    }
    if c.rec.url.is_empty() {
        return PluginEntry { ready: true, stage: PluginStage::Starting, http: None };
    }
    let http = HttpBase::new(&c.rec.url);
    PluginEntry::ready(if c.rec.password.is_empty() { http } else { http.with_password(&c.rec.password) })
}

/// One plugin server's sign-in (`PluginServerState`, [`plugin_server`]) and its timeout, and
/// whether its managed stream was last seen up.
#[derive(Clone, Debug, PartialEq)]
struct Action {
    machine: plugin_server::State,
    timeout: Timeout,
    link: Link,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Timeout {
    Off,
    Due { attempt: Attempt, at: f64 },
}

/// A managed server's reach, as its health says: a drop runs the plugin's `reconnect` once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Link {
    Unknown,
    Up,
    Down,
}

thread_local! {
    static ACTIONS: RefCell<HashMap<String, Action>> = RefCell::default();
    /// `connect` tasks that returned, as `(key, attempt, ready)`, for the next poll.
    static SETTLED: RefCell<Vec<(String, Attempt, bool)>> = const { RefCell::new(Vec::new()) };
}

/// Signs in to plugin server `key`, which waits for it (Home's Authenticate, a signed-out tab's
/// Sign in, `plugins.authenticate`, `Servers:Connect`): its source's `connect` runs once, a
/// second press joins it, and 120 s bound the wait.
pub fn authenticate(app: &mut App, key: &str, tx: &mut Tx) {
    sync_actions(app);
    let effects = ACTIONS.with_borrow_mut(|a| a.get_mut(key).map(|slot| plugin_server::step(&mut slot.machine, plugin_server::Event::Authenticate { on_connected: None })));
    run_actions(app, key, effects.unwrap_or_default(), tx);
}

/// Whether plugin server `key` waits for a sign-in it can start, or one is on its way.
pub fn can_authenticate(key: &str) -> bool {
    ACTIONS.with_borrow(|a| a.get(key).is_some_and(|s| s.machine.can_authenticate()))
}

/// Whether a sign-in of plugin server `key` is on its way.
pub fn authenticating(key: &str) -> bool {
    ACTIONS.with_borrow(|a| a.get(key).is_some_and(|s| matches!(s.machine, plugin_server::State::Authenticating { .. })))
}

/// `poll`: each plugin server's machine follows its entry, a sign-in that ran out of time ends,
/// and a managed server whose health dropped runs its plugin's `reconnect`.
pub fn poll_actions(app: &mut App, tx: &mut Tx) {
    sync_actions(app);
    for (key, attempt, ready) in SETTLED.take() {
        let effects = ACTIONS.with_borrow_mut(|a| a.get_mut(&key).map(|slot| plugin_server::step(&mut slot.machine, plugin_server::Event::ConnectSettled { attempt, ready })));
        run_actions(app, &key, effects.unwrap_or_default(), tx);
    }
    let now = app.clock;
    let due: Vec<(String, Attempt)> = ACTIONS.with_borrow(|a| a.iter().filter_map(|(k, s)| match s.timeout {
        Timeout::Due { attempt, at } if now >= at => Some((k.clone(), attempt)),
        _ => None,
    }).collect());
    for (key, attempt) in due {
        let effects = ACTIONS.with_borrow_mut(|a| {
            let slot = a.get_mut(&key)?;
            slot.timeout = Timeout::Off;
            Some(plugin_server::step(&mut slot.machine, plugin_server::Event::ConnectTimedOut { attempt }))
        });
        run_actions(app, &key, effects.unwrap_or_default(), tx);
    }
    let managed: Vec<String> = all(app).into_iter().filter(|c| !c.by_address() && c.rec.managed && c.rec.state == SERVER_READY).map(|c| c.key).collect();
    let dropped: Vec<String> = ACTIONS.with_borrow_mut(|a| {
        a.iter_mut()
            .filter_map(|(key, slot)| {
                let managed = managed.contains(key);
                let health = crate::servers::health::get(&opencode_kit::servers::ServerKey::new(key));
                let now = match health {
                    Some(opencode_kit::servers::health::Health::Probe(opencode_kit::servers::health::ProbeResult::Healthy { .. })) => Link::Up,
                    Some(opencode_kit::servers::health::Health::Probe(opencode_kit::servers::health::ProbeResult::Unreachable)) => Link::Down,
                    _ => slot.link,
                };
                let was = std::mem::replace(&mut slot.link, now);
                (managed && was == Link::Up && now == Link::Down).then(|| key.clone())
            })
            .collect()
    });
    for key in dropped {
        super::luau::servers::call(app, &key, "reconnect", None, tx);
    }
    if let Some(at) = ACTIONS.with_borrow(|a| a.values().filter_map(|s| match s.timeout {
        Timeout::Due { at, .. } => Some(at),
        Timeout::Off => None,
    }).reduce(f64::min)) {
        tx.wake_at(at);
    }
}

/// Every plugin server has a machine following its entry; one no longer listed is disposed.
fn sync_actions(app: &App) {
    let servers: Vec<(String, PluginEntry)> = all(app).into_iter().filter(|c| !c.by_address()).map(|c| (c.key.clone(), entry(&c))).collect();
    ACTIONS.with_borrow_mut(|a| {
        a.retain(|key, _| servers.iter().any(|(k, _)| k == key));
        for (key, e) in servers {
            match a.get_mut(&key) {
                Some(slot) => {
                    let fx = plugin_server::step(&mut slot.machine, plugin_server::Event::EntryUpdated(e));
                    if fx.into_iter().any(|f| matches!(f, plugin_server::Effect::TimeoutCancel)) {
                        slot.timeout = Timeout::Off;
                    }
                }
                None => {
                    a.insert(key, Action { machine: plugin_server::State::new(&e), timeout: Timeout::Off, link: Link::Unknown });
                }
            }
        }
    });
}

fn run_actions(app: &mut App, key: &str, effects: rune::machine::Effects<plugin_server::Effect>, tx: &mut Tx) {
    for effect in effects {
        match effect {
            plugin_server::Effect::Connect { attempt, timeout_ms } => {
                let at = app.clock + timeout_ms as f64 / 1000.0;
                ACTIONS.with_borrow_mut(|a| {
                    if let Some(slot) = a.get_mut(key) {
                        slot.timeout = Timeout::Due { attempt, at };
                    }
                });
                tx.wake_at(at);
                let answer = key.to_owned();
                let settle: super::luau::servers::Settle = Box::new(move |ready| SETTLED.with_borrow_mut(|q| q.push((answer, attempt, ready))));
                // A source without `connect` can't sign in: the attempt ends at once.
                if !super::luau::servers::call(app, key, "connect", Some(settle), tx) {
                    SETTLED.with_borrow_mut(|q| q.push((key.to_owned(), attempt, false)));
                }
            }
            plugin_server::Effect::TimeoutCancel => ACTIONS.with_borrow_mut(|a| {
                if let Some(slot) = a.get_mut(key) {
                    slot.timeout = Timeout::Off;
                }
            }),
            plugin_server::Effect::NotifyConnected(_) => {}
        }
    }
    tx.instant();
}

/// The plugin cover over the routed session or draft tab (`PluginServerCover`): drawn by the
/// window half of the plugin whose server the tab is on while that server isn't ready and its
/// source has a `cover`. None otherwise; the tab stays mounted under it.
pub fn cover(app: &App, p: &crate::palette::Pal) -> Option<rune::El> {
    let (server, tab) = match &app.route {
        crate::Route::Session { server, id } => (server.clone(), format!("{}:{id}", server.as_str())),
        crate::Route::Draft(id) => (app.tabs.iter().find(|t| t.id == *id)?.server.clone(), id.clone()),
        crate::Route::Home => return None,
    };
    let c = all(app).into_iter().find(|c| !c.by_address() && c.key == server.as_str() && c.rec.state != SERVER_READY)?;
    super::luau::surfaces::server_cover(app, p, &c.key, &c.rec, &tab)
}

/// Contributed server `c`'s connection row in Settings › Servers, as its plugin draws it.
pub fn row(app: &App, p: &crate::palette::Pal, c: &Contributed) -> Option<rune::El> {
    if c.by_address() {
        return crate::servers::list().into_iter().find(|s| s.url == c.key).map(|s| crate::settings::pages::servers::http_row(p, &s));
    }
    if c.slot == usize::MAX {
        return super::luau::surfaces::server_row(app, p, &c.key, &c.rec);
    }
    let (i, rec) = active(&app.plugins, kinds::SERVER).find(|(s, r)| *s == c.slot && r.id == c.rec.source)?;
    let render = rec.render.as_ref()?;
    let props = super::render::Props::new(opencode_plugin::abi::SURFACE_SERVER_ROW, &c.rec.id);
    super::render::surface(app, i, render, &props, p).ok().flatten()
}
