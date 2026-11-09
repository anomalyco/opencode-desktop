//! The host signals that follow app state rather than an event: `Sessions.Changed`,
//! `Sessions.RoutedChanged`, `Models.Changed`, `Theme.Changed`, `I18n.Changed`,
//! `App.LocaleChanged`, `Server.Connected`, `Servers.Changed`, each store's and contract's
//! `Changed`. Each poll compares what a signal follows with what it last saw and fires on a
//! change; a plugin that made none of them costs one map lookup.

use std::hash::{Hash, Hasher};

use rune_luau::Data;

use super::store::{contract_generation, store_version};
use super::{map, session_version};
use crate::App;

fn hash(v: impl Hash) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// Fires plugin `i`'s watched signals whose state moved. True when any fired.
pub fn fire_watched(app: &App, i: usize, vm: u64) -> bool {
    let half = app.plugins.luau.plugins[i].half.clone();
    let names: Vec<String> = half.signals.borrow().keys().filter(|(v, _)| *v == vm).map(|(_, n)| n.clone()).collect();
    if names.is_empty() {
        return false;
    }
    let mut fired = false;
    for name in names {
        let plugin = &half.id;
        let now: Option<(u64, Data)> = match name.as_str() {
            "Sessions.RoutedChanged" => {
                let id = app.active_session_id();
                Some((hash(id), id.map_or(Data::Nil, |id| session_data(app, id))))
            }
            "Models.Changed" => Some((hash((app.pickers.models.len(), app.pickers.limits.len(), app.pickers.paid.len())), Data::Nil)),
            "Theme.Changed" => {
                let p = crate::palette::pal(app.plugins.luau.dark.get());
                Some((hash((p.theme, p.generation, p.dark)), map([("scheme", Data::String(if p.dark { "dark" } else { "light" }.into()))])))
            }
            "I18n.Changed" | "App.LocaleChanged" => Some((hash(opencode_i18n::locale().id()), Data::String(opencode_i18n::locale().id().to_owned()))),
            "Server.Connected" => Some((hash(&app.server_key), Data::Bool(true))),
            "Servers.Changed" => Some((super::servers_version(app), Data::Nil)),
            "Sessions.Changed" => {
                for s in &app.sessions {
                    let key = format!("Sessions.Changed:{}", s.id);
                    let v = session_version(app, &s.id);
                    let was = half.seen.borrow_mut().insert(key, v);
                    if was.is_some_and(|w| w != v) {
                        fired |= super::super::fire_named(app, i, &name, session_data(app, &s.id));
                    }
                }
                None
            }
            n => {
                if let Some(store) = n.strip_prefix("Store.").and_then(|s| s.strip_suffix(".Changed")) {
                    Some((store_version(plugin, store), Data::Nil))
                } else if let Some(session) = n.strip_prefix("Diff.").and_then(|s| s.strip_suffix(".Changed")) {
                    Some((hash(super::super::native::diff::version(app, session)), Data::Nil))
                } else {
                    n.strip_prefix("Contracts.").and_then(|s| s.strip_suffix(".Changed")).map(|id| (contract_generation(id), Data::Nil))
                }
            }
        };
        let Some((v, payload)) = now else { continue };
        let was = half.seen.borrow_mut().insert(name.clone(), v);
        if was.is_some_and(|w| w != v) {
            fired |= super::super::fire_named(app, i, &name, payload);
        }
    }
    fired
}

/// A session as `Data`, the record `Sessions:Get` returns.
pub fn session_data(app: &App, id: &str) -> Data {
    let Some(info) = app.session(id) else { return Data::Nil };
    map([
        ("id", Data::String(info.id.clone())),
        ("directory", Data::String(info.directory.clone())),
        ("title", Data::String(info.title.clone())),
        ("busy", Data::Bool(app.subtree_running(&info.id))),
        ("version", Data::Integer((session_version(app, id) >> 11) as i64)),
        ("cost", Data::Number(info.cost)),
    ])
}
