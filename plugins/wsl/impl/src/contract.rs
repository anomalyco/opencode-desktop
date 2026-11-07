//! What the WSL plugin's main side publishes (`contract.ts`): WSL itself, its distros, what
//! each distro was found to have, the servers it runs, and the jobs running now.

use std::collections::BTreeMap;

/// A distro whose server the plugin runs; the id is `wsl:<distro>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    pub id: String,
    pub distro: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerRuntime {
    Starting,
    Ready { url: String, password: String },
    Failed(String),
    Stopped,
}

/// The one job a distro runs; a distro missing from `distro_jobs` runs none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistroJob {
    Probing,
    Installing,
    Updating,
    Starting,
    Removing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeCheck {
    pub available: bool,
    pub version: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledDistro {
    pub name: String,
    pub version: Option<u32>,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnlineDistro {
    pub name: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DistroProbe {
    pub name: String,
    pub can_execute: bool,
    pub has_bash: bool,
    pub has_curl: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpencodeCheck {
    pub distro: String,
    pub resolved_path: Option<String>,
    pub version: Option<String>,
    pub expected_version: Option<String>,
    pub matches_desktop: Option<bool>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerItem {
    pub config: ServerConfig,
    pub runtime: ServerRuntime,
}

/// `WslServersState`, as windows see it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServersState {
    pub runtime: Option<RuntimeCheck>,
    pub installed: Vec<InstalledDistro>,
    pub online: Vec<OnlineDistro>,
    pub distro_probes: BTreeMap<String, DistroProbe>,
    pub opencode_checks: BTreeMap<String, OpencodeCheck>,
    pub pending_restart: bool,
    pub servers: Vec<ServerItem>,
    pub host_busy: bool,
    pub distro_jobs: BTreeMap<String, DistroJob>,
}

pub fn server_id_for_distro(distro: &str) -> String {
    format!("wsl:{distro}")
}
