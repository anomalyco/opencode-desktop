//! The servers Settings lists (`settings/servers/inventory.ts`): every server reported by
//! [`SettingsHost::servers_inventory`], with its name, connection, health, and availability.

use opencode_kit::servers::settings::{self, Input};
use opencode_kit::servers::{HttpBase, ServerConn, ServerKey};

pub use crate::host::{ServerEntry as Server, ServerKind as Kind};
use crate::host::SettingsHost;

/// Fallback entry representing the local sidecar server when a minimal host lists none.
pub(crate) fn fallback_own() -> Server {
    let conn = ServerConn::Sidecar { http: HttpBase::new(""), display_name: None, label: None };
    let availability = settings::availability(Input {
        connection: Some(&conn),
        source: None,
        health: None,
        signed_out: false,
    });
    Server {
        key: ServerKey::sidecar(),
        name: opencode_i18n::t!("server.thisComputer").to_owned(),
        kind: Kind::Own { address: String::new() },
        connection: Some(conn),
        health: None,
        availability,
    }
}

/// Every server in the host's Settings inventory.
pub fn list(s: &dyn SettingsHost) -> Vec<Server> {
    let mut items = s.servers_inventory();
    if items.is_empty() {
        items.push(fallback_own());
    }
    items
}

/// The server per-server pages show: Settings' own pick while it's listed, else the first server.
pub fn selected(s: &dyn SettingsHost) -> Server {
    let all = list(s);
    let key = &s.settings_pages().server.key;
    all.iter()
        .find(|x| x.key == *key)
        .or_else(|| all.first())
        .cloned()
        .unwrap_or_else(fallback_own)
}
