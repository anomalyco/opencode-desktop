//! The app's registry of servers besides its own (Desktop's `Server` registry): the ones plugins
//! contribute (`contrib::SERVER` and Luau server sources), keyed `<plugin>:<id>`, ordered by
//! their source's `order`, checked for health once they're ready (their version, as Desktop's
//! health poll reads it); then the ones added by address (`crate::servers`), keyed by it. Every
//! window shares it: servers are added to the app, not switched to.

use opencode_plugin::abi::{SERVER_READY, contrib as kinds};
use rune::Tx;

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
        rec: ServerRec { source: "http".into(), id: s.url.clone(), name: s.label(), label: String::new(), state: SERVER_READY, url: s.url, password: s.password.unwrap_or_default(), listed: true },
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
    if !all(app).into_iter().any(|s| !s.by_address() && s.key == key && s.rec.listed && s.rec.state == SERVER_READY) {
        return;
    }
    app.focus_server(&opencode_kit::servers::ServerKey::new(key), tx);
}

/// Contributed server `c`'s connection row in Settings › Servers, as its plugin draws it.
pub fn row(app: &App, p: &crate::palette::Pal, c: &Contributed) -> Option<rune::El> {
    if c.by_address() {
        return crate::servers::list().into_iter().find(|s| s.url == c.key).map(|s| crate::settings::pages::servers::http_row(p, &s));
    }
    if c.slot == usize::MAX {
        let health = app.plugins.shared.facts.health.get(&c.key).cloned();
        return super::luau::surfaces::server_row(app, p, &c.key, &c.rec, health);
    }
    let (i, rec) = active(&app.plugins, kinds::SERVER).find(|(s, r)| *s == c.slot && r.id == c.rec.source)?;
    let render = rec.render.as_ref()?;
    let props = super::render::Props::new(opencode_plugin::abi::SURFACE_SERVER_ROW, &c.rec.id);
    super::render::surface(app, i, render, &props, p).ok().flatten()
}
