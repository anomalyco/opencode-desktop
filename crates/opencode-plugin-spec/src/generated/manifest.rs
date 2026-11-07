//! plugin.toml, decoded with any serde TOML parser.
//! Generated from crates/opencode-plugin-spec/contract.json by `cargo run -p opencode-plugin-spec -- --write`. Do not edit.

use serde::{Deserialize, Serialize};

use super::*;

/// plugin.toml: a plugin folder's manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Manifest {
    /// The plugin's id: lowercase letters, digits and hyphens. It prefixes commands (`<id>.<command>`) and contracts.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The plugin's own version, as semver.
    pub version: String,
    /// The contract version the plugin targets, such as `"1.0"`.
    pub contract: String,
    /// One sentence about what the plugin adds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The window half's module, relative to the plugin folder.
    #[serde(default = "default_manifest_entry")]
    pub entry: String,
    /// The app half's module, such as `"app.luau"`. Omit it for a plugin with no app half.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// The platforms it runs on. Omit it for all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub os: Vec<Platform>,
    /// The app channels it loads in. Omit it for all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<Channel>,
    /// Its authors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// Its homepage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// An SPDX license expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Stores to load before setup, so their first read has the saved value.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stores: Vec<ManifestStore>,
    /// Settings search entries found without loading the plugin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings_search: Vec<ManifestSettingsSearch>,
}

fn default_manifest_entry() -> String {
    "init.luau".to_owned()
}

/// Stores to load before setup, so their first read has the saved value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ManifestStore {
    /// The name passed to `Store:Value` or `Store:SessionValue`.
    pub name: String,
    /// One value for the app, or one per session.
    #[serde(default)]
    pub scope: StoreScope,
}

/// Settings search entries found without loading the plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ManifestSettingsSearch {
    /// The `ui.settingsRow` id search reveals.
    pub id: String,
    /// The page: `plugins`, a host page, or one of this plugin's settings pages.
    pub page: String,
    /// The title in search results.
    pub title: String,
    /// Its second line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// More words search matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
}
