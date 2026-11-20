//! Checking for and installing plugin package updates from Settings › Extensions
//! (`settings/providers/plugin-update-machine.ts`), and the server's plugin rows they apply to.

use std::collections::BTreeMap;

use opencode_i18n::t;
use opencode_kit::wire_error::WireError;
use serde_json::Value;

use crate::host::SettingsHost;
use crate::jobs::{SettingsJob, SettingsRequest};

/// A plugin the server lists (`Plugin.Info`), as the Plugins tab shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginRow {
    /// `pluginLabel` (`providers/catalog/plugin.ts`): its id, else its package or path.
    pub name: String,
    /// Set when it comes from a package, which can be updated.
    pub package: Option<Package>,
}

/// A package plugin's `source`, with the flags the server sets on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub target: String,
    pub outdated: bool,
    pub updating: bool,
}

/// The rows of a `/api/plugin` or `/api/plugin/check` reply, built-ins left out.
pub fn parse_rows(v: &Value) -> Vec<PluginRow> {
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let source = &p["source"];
            let kind = source["type"].as_str()?;
            if kind == "builtin" {
                return None;
            }
            let package = (kind == "package").then(|| source["target"].as_str()).flatten().map(|target| Package {
                target: target.to_owned(),
                outdated: source["outdated"].as_bool() == Some(true),
                updating: source["updating"].as_bool() == Some(true),
            });
            let name = p["id"]
                .as_str()
                .or(package.as_ref().map(|p| p.target.as_str()))
                .or(source["path"].as_str())
                .unwrap_or(kind)
                .to_owned();
            Some(PluginRow { name, package })
        })
        .collect()
}

/// Each listed package's `outdated` flag, by its target (`packageFlags` in `extensions.tsx`).
pub fn package_flags(rows: &[PluginRow]) -> BTreeMap<String, bool> {
    rows.iter().filter_map(|r| r.package.as_ref()).map(|p| (p.target.clone(), p.outdated)).collect()
}

/// One package update on its way; `id` tells it apart from an update of the same package on an
/// earlier server or an earlier visit to the page.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Running {
    id: u64,
    target: String,
}

/// What a check or update learned about one package; `at` is that operation's id, so an older
/// result never wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Finding {
    outdated: bool,
    at: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    /// A check for every package is on its way.
    Checking { attempt: u64 },
}

/// The machine. `findings` holds what checks and updates learned per package. The server reloads
/// its plugins after both return, so a list read in between still carries the old flags; a
/// finding stays until a list agrees with it. Updates run alongside a check and each other.
#[derive(Debug, Default)]
pub struct Machine {
    phase: Phase,
    next_id: u64,
    findings: BTreeMap<String, Finding>,
    updating: Vec<Running>,
}

#[derive(Debug)]
pub enum Event {
    CheckRequested,
    /// `outdated` maps every package the check covered to whether it has a newer version.
    CheckSucceeded { attempt: u64, outdated: BTreeMap<String, bool> },
    CheckFailed { attempt: u64, error: WireError },
    UpdateRequested { target: String },
    UpdateSucceeded { id: u64 },
    UpdateFailed { id: u64, error: WireError },
    /// The plugin list loaded again; `outdated` maps each listed package to its flag.
    ListLoaded { outdated: BTreeMap<String, bool> },
    /// The page shows again, for this server or another, or Settings closed: forget what earlier
    /// checks and updates learned. Desktop does this on `server.changed` and as the page unmounts
    /// (`disposed`); ids keep counting, so replies that arrive later match nothing.
    Reset,
}

#[derive(Debug)]
pub enum Effect {
    Check { attempt: u64 },
    Update { id: u64, target: String },
    /// Read the plugin list again.
    Refresh,
    Failed(WireError),
}

impl Machine {
    pub fn checking(&self) -> bool {
        matches!(self.phase, Phase::Checking { .. })
    }

    /// `pluginUpdateView`: whether `package`'s row offers Update, and whether it's updating.
    pub fn view(&self, package: &Package) -> (bool, bool) {
        let updating = package.updating || self.updating.iter().any(|r| r.target == package.target);
        let outdated = self.findings.get(&package.target).map_or(package.outdated, |f| f.outdated);
        (outdated, updating)
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

/// `transitionPluginUpdate`.
pub fn step(m: &mut Machine, event: Event) -> Vec<Effect> {
    match event {
        Event::Reset => {
            m.phase = Phase::Idle;
            m.findings.clear();
            m.updating.clear();
            Vec::new()
        }
        Event::ListLoaded { outdated } => {
            m.findings.retain(|target, f| outdated.get(target).is_some_and(|o| *o != f.outdated));
            Vec::new()
        }
        Event::CheckRequested => {
            if m.checking() {
                return Vec::new();
            }
            let attempt = m.take_id();
            m.phase = Phase::Checking { attempt };
            vec![Effect::Check { attempt }]
        }
        Event::CheckSucceeded { attempt, outdated } => {
            if m.phase != (Phase::Checking { attempt }) {
                return Vec::new();
            }
            m.phase = Phase::Idle;
            // An update that started or finished after this check began knows more about its
            // package than the check does.
            for (target, outdated) in outdated {
                let newer = m.findings.get(&target).is_some_and(|f| f.at > attempt) || m.updating.iter().any(|r| r.target == target);
                if !newer {
                    m.findings.insert(target, Finding { outdated, at: attempt });
                }
            }
            Vec::new()
        }
        Event::CheckFailed { attempt, error } => {
            if m.phase != (Phase::Checking { attempt }) {
                return Vec::new();
            }
            m.phase = Phase::Idle;
            vec![Effect::Failed(error)]
        }
        Event::UpdateRequested { target } => {
            if m.updating.iter().any(|r| r.target == target) {
                return Vec::new();
            }
            let id = m.take_id();
            m.updating.push(Running { id, target: target.clone() });
            vec![Effect::Update { id, target }]
        }
        Event::UpdateSucceeded { id } => {
            let Some(at) = m.updating.iter().position(|r| r.id == id) else { return Vec::new() };
            let running = m.updating.remove(at);
            m.findings.insert(running.target, Finding { outdated: false, at: running.id });
            vec![Effect::Refresh]
        }
        Event::UpdateFailed { id, error } => {
            let Some(at) = m.updating.iter().position(|r| r.id == id) else { return Vec::new() };
            m.updating.remove(at);
            vec![Effect::Failed(error)]
        }
    }
}

/// Steps the page's machine and runs what it asks for.
pub fn dispatch(s: &mut dyn SettingsHost, event: Event) {
    let effects = step(&mut s.settings_pages_mut().updates, event);
    for effect in effects {
        match effect {
            Effect::Check { attempt } => s.spawn_job(SettingsJob::ActiveServer(SettingsRequest::CheckPluginUpdates { attempt })),
            Effect::Update { id, target } => s.spawn_job(SettingsJob::ActiveServer(SettingsRequest::UpdatePlugin { id, target })),
            Effect::Refresh => super::extensions::load(s),
            Effect::Failed(error) => s.toast_failed_wire(t!("common.requestFailed").into(), &error),
        }
    }
}
