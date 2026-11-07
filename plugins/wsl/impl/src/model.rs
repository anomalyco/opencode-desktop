//! What the Add WSL server dialog shows (`model.ts`, `row-view.ts`), from the main side's state
//! and the dialog's own: which distros it lists and how each reads, the primary button, the
//! catalog of distros to install, and the probe the dialog needs next.

use crate::contract::*;
use crate::dialog::{DialogState, View};

/// A distro row's status line and its tone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub label: String,
    pub tone: Tone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tone {
    Success,
    Warning,
    Muted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    InstallOpencode,
    Add,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Primary {
    pub contrast: bool,
    pub label: String,
    pub disabled: bool,
    pub action: Option<Action>,
    pub loading: bool,
    pub width: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Loading,
    PendingRestart,
    Checking,
    Unavailable,
    Ready,
}

/// The probe the dialog needs next: WSL itself, its distros, or the addable distros.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    Runtime,
    Distros,
    Addable { key: String, distros: Vec<String> },
}

impl Plan {
    pub fn key(&self) -> String {
        match self {
            Plan::Runtime => "auto:runtime".into(),
            Plan::Distros => "auto:distros".into(),
            Plan::Addable { key, .. } => format!("addable:{key}"),
        }
    }
}

pub struct Model {
    pub view: View,
    pub adding: bool,
    pub busy: bool,
    pub runtime: RuntimeState,
    pub visible_installed: Vec<InstalledDistro>,
    pub addable: Vec<InstalledDistro>,
    pub selected: Option<String>,
    pub statuses: Vec<(String, Option<Status>)>,
    pub primary: Primary,
    pub installable: Vec<OnlineDistro>,
    pub filtered: Vec<OnlineDistro>,
    pub catalog_target: Option<String>,
    pub installing_catalog: bool,
}

fn hidden(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "docker-desktop" || n == "docker-desktop-data"
}

/// Jobs the dialog waits for: WSL's, and those of distros without a server (an added server's
/// jobs belong to its row).
pub fn dialog_job_running(state: &ServersState) -> bool {
    state.host_busy || state.distro_jobs.keys().any(|d| !state.servers.iter().any(|s| &s.config.distro == d))
}

fn ready(state: &ServersState, name: &str) -> bool {
    let (Some(installed), Some(probe)) = (state.installed.iter().find(|i| i.name == name), state.distro_probes.get(name)) else { return false };
    installed.version != Some(1) && probe.can_execute && probe.has_bash && probe.has_curl
}

pub fn build(state: Option<&ServersState>, dialog: &DialogState, search: &str, probing_addable: bool) -> Model {
    let empty = ServersState::default();
    let s = state.unwrap_or(&empty);
    let visible_installed: Vec<InstalledDistro> = s.installed.iter().filter(|i| !hidden(&i.name)).cloned().collect();
    let visible_online: Vec<OnlineDistro> = s.online.iter().filter(|i| !hidden(&i.name)).cloned().collect();
    let addable: Vec<InstalledDistro> = visible_installed.iter().filter(|i| !s.servers.iter().any(|x| x.config.distro == i.name)).cloned().collect();
    let selected = selected_distro(dialog.selected(), &visible_installed, &addable);
    let check = selected.as_ref().and_then(|d| s.opencode_checks.get(d));
    let installable = installable(&visible_installed, &visible_online);
    let filtered = filter(&installable, search);
    let catalog_target = dialog.catalog_target().filter(|t| filtered.iter().any(|d| &d.name == t)).or_else(|| filtered.first().map(|d| d.name.clone()));
    let working = (state.is_some() && dialog_job_running(s)) || dialog.busy();
    let statuses = addable.iter().map(|i| (i.name.clone(), status(state, &i.name, probing_addable))).collect();
    let primary = primary(state, selected.as_deref(), check, working, dialog.installing_opencode(selected.as_deref()), dialog.adding(), probing_addable);
    Model {
        view: dialog.view(),
        adding: dialog.adding(),
        busy: working || probing_addable,
        runtime: runtime_state(state),
        visible_installed,
        addable,
        selected,
        statuses,
        primary,
        installable,
        filtered,
        catalog_target,
        installing_catalog: dialog.installing_catalog() || s.distro_jobs.values().any(|j| *j == DistroJob::Installing),
    }
}

fn selected_distro(selected: Option<&str>, visible: &[InstalledDistro], addable: &[InstalledDistro]) -> Option<String> {
    if let Some(sel) = selected
        && addable.iter().any(|i| i.name == sel && i.version != Some(1))
    {
        return Some(sel.to_owned());
    }
    if let Some(d) = visible.iter().find(|i| i.is_default)
        && d.version != Some(1)
        && addable.iter().any(|i| i.name == d.name)
    {
        return Some(d.name.clone());
    }
    addable.iter().find(|i| i.version != Some(1)).map(|i| i.name.clone())
}

pub fn runtime_state(state: Option<&ServersState>) -> RuntimeState {
    let Some(s) = state else { return RuntimeState::Loading };
    match &s.runtime {
        _ if s.pending_restart => RuntimeState::PendingRestart,
        None => RuntimeState::Checking,
        Some(r) if !r.available => RuntimeState::Unavailable,
        Some(_) => RuntimeState::Ready,
    }
}

fn status(state: Option<&ServersState>, name: &str, probing_addable: bool) -> Option<Status> {
    let s = state?;
    let tone = |label: String, tone: Tone| Some(Status { label, tone });
    let installed = s.installed.iter().find(|i| i.name == name);
    if installed.is_some_and(|i| i.version == Some(1)) {
        return tone("Unsupported · Use WSL 2".into(), Tone::Muted);
    }
    let probing = probing_addable || s.distro_jobs.get(name) == Some(&DistroJob::Probing);
    let checking = || if probing { tone("Checking…".into(), Tone::Muted) } else { None };
    let Some(probe) = s.distro_probes.get(name) else { return checking() };
    if !probe.can_execute {
        return match installed {
            None => tone(format!("{name} is not installed yet."), Tone::Warning),
            Some(_) => tone(format!("Open {name} once to finish setup."), Tone::Warning),
        };
    }
    if !probe.has_bash || !probe.has_curl {
        return tone("Missing bash, curl".into(), Tone::Warning);
    }
    let Some(check) = s.opencode_checks.get(name) else { return checking() };
    if check.matches_desktop == Some(false) {
        return tone("Update OpenCode".into(), Tone::Warning);
    }
    if check.resolved_path.is_none() {
        return tone("OpenCode not installed".into(), Tone::Warning);
    }
    if check.error.is_some() {
        return tone("Install OpenCode".into(), Tone::Warning);
    }
    tone("Ready".into(), Tone::Success)
}

fn primary(state: Option<&ServersState>, selected: Option<&str>, check: Option<&OpencodeCheck>, busy: bool, installing: bool, adding: bool, probing_addable: bool) -> Primary {
    let ready = selected.is_some_and(|d| state.is_some_and(|s| ready(s, d)));
    let probing_selected = probing_addable && !settled(state, selected);
    let job = selected.and_then(|d| state.and_then(|s| s.distro_jobs.get(d)));
    let probing_opencode = probing_selected || (ready && (check.is_none() || job == Some(&DistroJob::Probing)));
    let installing_opencode = job == Some(&DistroJob::Updating) || installing;
    if !ready || probing_opencode {
        return Primary {
            contrast: true,
            label: if probing_selected { "Checking…".into() } else { "Add WSL server".into() },
            disabled: true,
            action: None,
            loading: probing_selected,
            width: None,
        };
    }
    let opencode_ready = check.is_some_and(|c| c.resolved_path.is_some() && c.matches_desktop != Some(false) && c.error.is_none());
    if !opencode_ready {
        let update = check.is_some_and(|c| c.resolved_path.is_some() && c.matches_desktop == Some(false));
        let label = if installing_opencode { "Updating OpenCode…" } else if update { "Update OpenCode" } else { "Install OpenCode" };
        return Primary { contrast: false, label: label.into(), disabled: busy, action: Some(Action::InstallOpencode), loading: installing_opencode, width: Some(if update { 138.0 } else { 129.0 }) };
    }
    Primary { contrast: true, label: if adding { "Adding…".into() } else { "Add WSL server".into() }, disabled: busy, action: Some(Action::Add), loading: adding, width: None }
}

fn settled(state: Option<&ServersState>, selected: Option<&str>) -> bool {
    let (Some(s), Some(d)) = (state, selected) else { return false };
    if s.installed.iter().find(|i| i.name == d).is_some_and(|i| i.version == Some(1)) || !s.distro_probes.contains_key(d) {
        return false;
    }
    !ready(s, d) || s.opencode_checks.contains_key(d)
}

fn installable(installed: &[InstalledDistro], online: &[OnlineDistro]) -> Vec<OnlineDistro> {
    let versioned_ubuntu = online.iter().any(|o| o.name.starts_with("Ubuntu-") && o.name[7..].starts_with(|c: char| c.is_ascii_digit()));
    online.iter().filter(|o| !installed.iter().any(|i| i.name == o.name)).filter(|o| o.name != "Ubuntu" || !versioned_ubuntu).cloned().collect()
}

/// `fuzzysort` over label and name: every query character in order, the tighter and earlier
/// the match the higher it ranks.
fn filter(distros: &[OnlineDistro], search: &str) -> Vec<OnlineDistro> {
    let q = search.trim().to_lowercase();
    if q.is_empty() {
        return distros.to_vec();
    }
    let score = |text: &str| -> Option<i64> {
        let t = text.to_lowercase();
        let mut at = 0usize;
        let (mut first, mut last) = (None, 0usize);
        for c in q.chars() {
            let found = t[at..].find(c)? + at;
            first.get_or_insert(found);
            last = found;
            at = found + c.len_utf8();
        }
        Some(-((last - first.unwrap_or(0)) as i64) * 10 - first.unwrap_or(0) as i64)
    };
    let mut scored: Vec<(i64, &OnlineDistro)> = distros.iter().filter_map(|d| score(&d.label).max(score(&d.name)).map(|s| (s, d))).collect();
    scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    scored.into_iter().map(|(_, d)| d.clone()).collect()
}

/// The probe the dialog needs next (`addServerProbePlan`).
pub fn plan(state: Option<&ServersState>, view: View, busy: bool, selected: Option<&str>, addable: &[InstalledDistro]) -> Option<Plan> {
    let s = state?;
    if !busy && !s.pending_restart {
        match &s.runtime {
            None => return Some(Plan::Runtime),
            Some(r) if r.available && s.installed.is_empty() && s.online.is_empty() => return Some(Plan::Distros),
            _ => {}
        }
    }
    if !s.runtime.as_ref().is_some_and(|r| r.available) || s.pending_restart || view != View::Main || busy || dialog_job_running(s) {
        return None;
    }
    let ordered: Vec<&InstalledDistro> = addable.iter().filter(|i| Some(i.name.as_str()) == selected).chain(addable.iter().filter(|i| Some(i.name.as_str()) != selected)).collect();
    let pending: Vec<String> = ordered
        .iter()
        .filter(|i| i.version != Some(1))
        .filter_map(|i| {
            if !s.distro_probes.contains_key(&i.name) {
                return Some(format!("distro:{}", i.name));
            }
            (ready(s, &i.name) && !s.opencode_checks.contains_key(&i.name)).then(|| format!("opencode:{}", i.name))
        })
        .collect();
    if pending.is_empty() {
        return None;
    }
    Some(Plan::Addable { key: pending.join("|"), distros: ordered.iter().filter(|i| i.version != Some(1)).map(|i| i.name.clone()).collect() })
}

/// A configured distro's row (`wslRowView`): its OpenCode version, its install or update
/// button, whether Retry start and Remove are offered.
pub struct RowView {
    pub version: Option<String>,
    pub button: Option<(String, bool)>,
    pub retry: bool,
    pub removable: bool,
}

pub fn row_view(state: &ServersState, distro: &str) -> RowView {
    let job = state.distro_jobs.get(distro).copied();
    let check = state.opencode_checks.get(distro);
    let action = check.and_then(|c| {
        if c.resolved_path.is_none() {
            return Some("Install OpenCode");
        }
        (c.matches_desktop == Some(false)).then_some("Update OpenCode")
    });
    let runtime = state.servers.iter().find(|s| s.config.distro == distro).map(|s| &s.runtime);
    let free = job.is_none() || job == Some(DistroJob::Starting);
    RowView {
        version: check.and_then(|c| c.version.clone()),
        button: action.map(|a| (if job == Some(DistroJob::Updating) { "Updating…".to_owned() } else { a.to_owned() }, !free)),
        retry: job.is_none() && matches!(runtime, Some(ServerRuntime::Failed(_) | ServerRuntime::Stopped)),
        removable: free,
    }
}
