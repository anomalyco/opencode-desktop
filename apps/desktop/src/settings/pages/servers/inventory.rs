//! The servers Settings lists (`settings/servers/inventory.ts`): every server in the app's
//! registry ([`crate::servers::registry`], the app's own, those added by address and the plugins'
//! listed ones, ready or not), then the plugin servers kept out of it (a WSL distro that isn't
//! running); each with its name, its connection while it's ready, its health, and whether its
//! per-server pages can load ([`opencode_kit::servers::settings::availability`]).

use opencode_kit::servers::health::Health;
use opencode_kit::servers::plugin_server::{PluginStage, PluginStatus};
use opencode_kit::servers::settings::{self, Availability};
use opencode_kit::servers::{ServerConn, ServerKey};

use crate::App;
use crate::plugin_host::instance::ServerRec;

/// Where a server comes from.
#[derive(Clone, Debug)]
pub enum Kind {
    /// The app's own server (`"sidecar"`).
    Own,
    /// Added by address (`servers.json`).
    Http(crate::servers::Server),
    /// Listed by a plugin's server source (`<plugin>:<id>`).
    Plugin(ServerRec),
}

/// One server as Settings shows it (`SettingsServer`).
#[derive(Clone, Debug)]
pub struct Server {
    pub key: ServerKey,
    pub name: String,
    pub kind: Kind,
    /// Its listed connection; a plugin's has one only while its plugin reports it ready.
    pub connection: Option<ServerConn>,
    pub health: Option<Health>,
    pub availability: Availability,
}

impl Server {
    /// The address its row shows: the connection's, or what the app's own server answers on.
    pub fn address(&self) -> String {
        match (&self.kind, &self.connection) {
            (Kind::Own, _) => crate::servers::runtime::own_server().map(|s| s.base).unwrap_or_default(),
            (_, Some(c)) => c.http().map(|h| h.url.clone()).unwrap_or_default(),
            (Kind::Http(s), None) => s.url.clone(),
            (Kind::Plugin(_), None) => String::new(),
        }
    }
}

/// Every server, in the registry's order, then the plugin servers it doesn't list
/// (`settingsServers`).
pub fn list(s: &App) -> Vec<Server> {
    let contributed = crate::plugin_host::contributed_servers(s);
    let saved = crate::servers::list();
    let mut out: Vec<Server> = crate::servers::registry::list(s)
        .into_iter()
        .filter_map(|e| {
            let key = e.key();
            let name = crate::servers::registry::label(&e.conn);
            let kind = match &e.conn {
                ServerConn::Sidecar { .. } => Kind::Own,
                ServerConn::Http { .. } => Kind::Http(saved.iter().find(|x| ServerConn::http_server(&x.url).map(|c| c.key()) == Some(key.clone()))?.clone()),
                ServerConn::Plugin { .. } => Kind::Plugin(contributed.iter().find(|c| c.key == key.as_str())?.rec.clone()),
            };
            // A sign-in on its way shows as starting (`selectPluginStatus`).
            let conn = match e.conn {
                ServerConn::Plugin { key: k, plugin, managed, display_name, label, .. } if crate::plugin_host::server_authenticating(k.as_str()) => {
                    ServerConn::Plugin { key: k, plugin, status: PluginStatus::Stage(PluginStage::Starting), managed, display_name, label }
                }
                conn => conn,
            };
            Some(server(key, name, kind, Some(conn), None))
        })
        .collect();
    for c in contributed.iter().filter(|c| !c.by_address() && !c.rec.listed) {
        let key = ServerKey::new(&c.key);
        let stage = (c.rec.state != opencode_plugin::abi::SERVER_READY).then(|| crate::plugin_host::servers_stage(c.rec.state));
        out.push(server(key, c.rec.name.clone(), Kind::Plugin(c.rec.clone()), None, Some(stage)));
    }
    out
}

/// The server per-server pages show: Settings' own pick while it's listed, else the app's own.
pub fn selected(s: &App) -> Server {
    let all = list(s);
    let key = &s.settings_pages.server.key;
    all.iter().find(|x| x.key == *key).cloned().unwrap_or_else(|| all[0].clone())
}

/// `source`: the plugin entry of a server the registry doesn't list (its stage, None when ready).
fn server(key: ServerKey, name: String, kind: Kind, connection: Option<ServerConn>, source: Option<Option<PluginStage>>) -> Server {
    let health = crate::servers::health::get(&key);
    let availability = settings::availability(settings::Input { connection: connection.as_ref(), source, health: health.as_ref(), signed_out: false });
    // A plugin's server has its connection only while it's ready.
    let connection = connection.filter(|c| !matches!(c, ServerConn::Plugin { status: PluginStatus::Stage(_), .. }));
    let name = if name.is_empty() { key.as_str().to_owned() } else { name };
    Server { key, name, kind, connection, health, availability }
}
