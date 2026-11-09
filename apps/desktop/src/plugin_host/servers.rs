//! The servers plugins contribute (`contrib::SERVER`, Desktop's `Server` registry), as the app
//! sees them: keyed `<plugin>:<id>`, ordered by their source's `order`, checked for health once
//! they're ready (their version, as Desktop's health poll reads it), and connected to on request.

use opencode_plugin::abi::{SERVER_READY, contrib as kinds};
use rune::Tx;

use super::contrib::active;
use super::instance::ServerRec;
use crate::App;

/// A contributed server with what the app knows of it.
#[derive(Clone, Debug)]
pub struct Contributed {
    /// `<plugin>:<id>`.
    pub key: String,
    pub slot: usize,
    pub rec: ServerRec,
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
    // Luau server sources' entries, shared by every window; their plugin draws no row yet.
    out.extend(super::luau::servers().into_iter().map(|(plugin, rec)| (0, Contributed { key: format!("{plugin}:{}", rec.id), slot: usize::MAX, rec })));
    out.sort_by_key(|(order, _)| *order);
    out.into_iter().map(|(_, c)| c).collect()
}

/// What sources list changed: checks the health of servers that became ready, and forgets it
/// for the ones that aren't any more.
pub fn changed(app: &mut App, tx: &mut Tx) {
    tx.instant();
    let servers = all(app);
    let facts = &mut app.plugins.shared.facts;
    facts.health.retain(|key, _| servers.iter().any(|s| &s.key == key && s.rec.state == SERVER_READY));
    let fresh: Vec<Contributed> = servers.into_iter().filter(|s| s.rec.state == SERVER_READY && !s.rec.url.is_empty() && !app.plugins.checking.contains(&s.key)).filter(|s| !app.plugins.shared.facts.health.contains_key(&s.key)).collect();
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
                    app.plugins.wake.wake(super::vtable::slot_bit(s.slot), false);
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

/// Connects the window to contributed server `key` once it's ready and listed.
pub fn connect(app: &mut App, key: &str, tx: &mut Tx) {
    let Some(s) = all(app).into_iter().find(|s| s.key == key && s.rec.listed && s.rec.state == SERVER_READY) else { return };
    let server = opencode_kit::client::Server::new(&s.rec.url).with_password(&s.rec.password).with_media_refs().with_remote_files();
    app.switch_server(server, Some(key.to_owned()), tx);
}

/// Contributed server `c`'s connection row in Settings › Servers, as its plugin draws it.
pub fn row(app: &App, p: &crate::palette::Pal, c: &Contributed) -> Option<rune::El> {
    let (i, rec) = active(&app.plugins, kinds::SERVER).find(|(s, r)| *s == c.slot && r.id == c.rec.source)?;
    let render = rec.render.as_ref()?;
    let props = super::render::Props::new(opencode_plugin::abi::SURFACE_SERVER_ROW, &c.rec.id);
    super::render::surface(app, i, render, &props, p).ok().flatten()
}
