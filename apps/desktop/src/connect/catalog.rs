//! The integrations and providers the connect dialog and Settings read, by location (Desktop's
//! `useIntegrations` and `useProviders`): `/api/integration` and `/api/provider` per folder, the
//! server's default one under "". A sync reads them again with the model list, which the
//! composer's pickers take too.

use std::collections::HashMap;

use opencode_kit::integration::{Integration, location_query};
use rune::Tx;
use serde_json::Value;

use crate::App;
use crate::jobs::Apply;

/// A provider the server runs (`/api/provider`).
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    /// The integration that connects it, where that isn't its own id (Console's providers).
    pub integration: Option<String>,
}

#[derive(Default)]
pub struct Catalogs {
    integrations: HashMap<String, Vec<Integration>>,
    providers: HashMap<String, Vec<ProviderInfo>>,
}

fn slot(dir: Option<&str>) -> String {
    dir.unwrap_or_default().to_owned()
}

fn parse_providers(v: &Value) -> Vec<ProviderInfo> {
    v["data"].as_array().or_else(|| v.as_array()).into_iter().flatten().map(|p| ProviderInfo {
        id: p["id"].as_str().unwrap_or_default().to_owned(),
        name: p["name"].as_str().unwrap_or_default().to_owned(),
        integration: p["integrationID"].as_str().map(str::to_owned),
    }).collect()
}

/// What one sync read; a part the server didn't answer stays as it was.
struct Read {
    integrations: Option<Vec<Integration>>,
    providers: Option<Value>,
    models: Option<(Value, Value)>,
}

impl App {
    /// The integrations at `dir` (the server's default when none), once read.
    pub(crate) fn integrations(&self, dir: Option<&str>) -> Option<&[Integration]> {
        self.catalogs.integrations.get(&slot(dir)).map(Vec::as_slice)
    }

    pub(crate) fn integration_of(&self, dir: Option<&str>, id: &str) -> Option<&Integration> {
        self.integrations(dir)?.iter().find(|i| i.id == id)
    }

    /// The providers the server runs at `dir`, once read.
    pub(crate) fn provider_infos(&self, dir: Option<&str>) -> Option<&[ProviderInfo]> {
        self.catalogs.providers.get(&slot(dir)).map(Vec::as_slice)
    }

    /// Reads `dir`'s integrations and providers and the model list again, then calls `then`
    /// with whether every read answered.
    pub(crate) fn sync_catalogs(&mut self, dir: Option<String>, then: impl FnOnce(&mut App, &mut Tx, bool) + Send + 'static) {
        self.job(move |server| {
            let q = location_query(dir.as_deref());
            let read = Read {
                integrations: server.integrations(dir.as_deref()).ok(),
                providers: server.get(&format!("/api/provider{q}")).ok(),
                models: server.get("/api/model").ok().map(|m| (m, server.get("/api/agent").unwrap_or_default())),
            };
            Box::new(move |s: &mut App, tx: &mut Tx| {
                let key = slot(dir.as_deref());
                let ok = read.integrations.is_some() && read.providers.is_some() && read.models.is_some();
                if let Some(list) = read.integrations {
                    s.catalogs.integrations.insert(key.clone(), list);
                }
                if let Some(p) = &read.providers {
                    s.catalogs.providers.insert(key, parse_providers(p));
                    s.pickers.load_providers(p.get("data").unwrap_or(p));
                }
                if let Some((m, agents)) = &read.models {
                    s.pickers.load_live(m.get("data").unwrap_or(m), agents.get("data").unwrap_or(agents));
                }
                tx.instant();
                s.catalogs_changed(tx);
                then(s, tx, ok);
            }) as Apply
        });
    }
}
