//! Server plugin inventory and skill descriptors (`packages/schema/src/plugin.ts`,
//! `packages/schema/src/skill.ts`, `packages/app/src/providers/catalog/plugin.ts`).
//!
//! Intentionally never reads the temporary `ready` field removed in `anomalyco/opencode#64524`
//! / `#64527`, and tolerates legacy flat plugin objects from older servers without dropping rows.

use serde_json::Value;

/// Where a server plugin comes from (`Plugin.Source`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginSource {
    Builtin,
    Package {
        target: String,
        version: Option<String>,
        outdated: bool,
        updating: bool,
    },
    Local {
        path: String,
    },
    Sdk,
    Unknown(String),
}

/// Capabilities declared by a server plugin (`Plugin.Features`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginFeatures {
    pub server: bool,
    pub tui: bool,
    pub rpc: bool,
    pub gui: bool,
}

/// Runtime status of a server plugin (`Plugin.State`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PluginState {
    #[default]
    Active,
    Failed {
        error: String,
        r#ref: Option<String>,
    },
    Unknown(String),
}

/// One server plugin entry returned by `GET /api/plugin` or `POST /api/plugin/check`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginInfo {
    pub id: Option<String>,
    pub source: PluginSource,
    pub features: PluginFeatures,
    pub state: PluginState,
}

impl PluginInfo {
    /// Display label matching `packages/app/src/providers/catalog/plugin.ts` (`pluginLabel`).
    pub fn label(&self) -> String {
        if let Some(id) = self.id.as_deref().filter(|s| !s.is_empty()) {
            return id.to_owned();
        }
        match &self.source {
            PluginSource::Package { target, .. } => target.clone(),
            PluginSource::Local { path } => path.clone(),
            PluginSource::Builtin => "builtin".to_owned(),
            PluginSource::Sdk => "sdk".to_owned(),
            PluginSource::Unknown(kind) => kind.clone(),
        }
    }

    pub fn is_builtin(&self) -> bool {
        matches!(self.source, PluginSource::Builtin)
    }

    /// Package target, outdated flag, and updating flag when this plugin comes from a package.
    pub fn package_source(&self) -> Option<(&str, bool, bool)> {
        match &self.source {
            PluginSource::Package { target, outdated, updating, .. } => Some((target.as_str(), *outdated, *updating)),
            _ => None,
        }
    }
}

/// Labels for non-builtin plugins (`packages/app/src/providers/catalog/plugin.ts` `pluginLabels`).
pub fn plugin_labels(plugins: &[PluginInfo]) -> Vec<String> {
    plugins.iter().filter(|p| !p.is_builtin()).map(PluginInfo::label).collect()
}

/// One skill returned by `GET /api/skill` (`packages/schema/src/skill.ts` `Skill.Info`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillInfo {
    pub id: String,
    pub name: String,
    pub path: String,
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v[key].as_str().map(str::to_owned)
}

fn list(v: &Value) -> &[Value] {
    v["data"].as_array().or_else(|| v.as_array()).map_or(&[], Vec::as_slice)
}

/// Parses one `Plugin.Info` entry leniently, preserving unknown source/state kinds and legacy
/// older-server objects (`{ name, version }`) that omit `source`.
pub fn parse_plugin(v: &Value) -> Option<PluginInfo> {
    if !v.is_object() {
        return None;
    }
    let id = str_field(v, "id")
        .or_else(|| str_field(v, "name"))
        .filter(|s| !s.is_empty());
    let source = if v["source"].is_object() {
        match v["source"]["type"].as_str() {
            Some("builtin") => PluginSource::Builtin,
            Some("package") => PluginSource::Package {
                target: str_field(&v["source"], "target").unwrap_or_default(),
                version: str_field(&v["source"], "version").filter(|s| !s.is_empty()),
                outdated: v["source"]["outdated"].as_bool() == Some(true),
                updating: v["source"]["updating"].as_bool() == Some(true),
            },
            Some("local") => PluginSource::Local {
                path: str_field(&v["source"], "path").unwrap_or_default(),
            },
            Some("sdk") => PluginSource::Sdk,
            Some(other) => PluginSource::Unknown(other.to_owned()),
            None => PluginSource::Unknown("unknown".to_owned()),
        }
    } else if id.is_some() {
        // Legacy older-server entry without structured `source`.
        PluginSource::Unknown("legacy".to_owned())
    } else {
        return None;
    };
    let f = &v["features"];
    let features = PluginFeatures {
        server: f["server"].as_bool() == Some(true),
        tui: f["tui"].as_bool() == Some(true),
        rpc: f["rpc"].as_bool() == Some(true),
        gui: f["gui"].as_bool() == Some(true),
    };
    let state = match v["state"]["status"].as_str() {
        Some("active") | None => PluginState::Active,
        Some("failed") => PluginState::Failed {
            error: str_field(&v["state"], "error").unwrap_or_default(),
            r#ref: str_field(&v["state"], "ref").filter(|s| !s.is_empty()),
        },
        Some(other) => PluginState::Unknown(other.to_owned()),
    };
    Some(PluginInfo { id, source, features, state })
}

/// Parses a `GET /api/plugin` or `POST /api/plugin/check` response (`{ data: [...] }` or `[...]`).
pub fn parse_plugins(v: &Value) -> Vec<PluginInfo> {
    list(v).iter().filter_map(parse_plugin).collect()
}

/// Parses one `Skill.Info` entry (`id`, `name`, `path` or legacy `location`).
pub fn parse_skill(v: &Value) -> Option<SkillInfo> {
    let name = str_field(v, "name").filter(|s| !s.is_empty())?;
    let id = str_field(v, "id").unwrap_or_default();
    let path = str_field(v, "path").or_else(|| str_field(v, "location")).unwrap_or_default();
    Some(SkillInfo { id, name, path })
}

/// Parses a `GET /api/skill` response (`{ data: [...] }` or `[...]`).
pub fn parse_skills(v: &Value) -> Vec<SkillInfo> {
    list(v).iter().filter_map(parse_skill).collect()
}
