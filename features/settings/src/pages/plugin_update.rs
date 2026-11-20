//! Explicit state machine for checking and installing server plugin package updates in
//! Settings › Extensions (`packages/app/src/settings/providers/plugin-update-machine.ts`,
//! `anomalyco/opencode#64511`, `#64525`, `#64527`).
//!
//! - `findings` holds what checks and updates learned per package target. Because the server
//!   reloads plugins asynchronously after check/update returns, an intermediate `GET /api/plugin`
//!   can still report old `outdated` flags; each finding stays until a settled list read agrees
//!   with it (`#64525`).
//! - Never reads the removed `ready` flag (`#64527`).
//! - Older OpenCode servers (`v2.2.6`, `v2.3.0`) returning `404`/`405`/`501` transition cleanly
//!   to [`State::Unsupported`] with zero error toasts, hiding update controls quietly until the
//!   selected server changes or reconnects.
//! - Nothing ever checks or installs without an explicit user click (`Event::CheckRequested` /
//!   `Event::UpdateRequested`).

use std::collections::HashMap;

use opencode_kit::model::PluginInfo;

/// One package update in flight; `id` distinguishes it from updates on earlier servers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningUpdate {
    pub id: u64,
    pub target: String,
}

/// What a check or update learned about one package; `at` is that operation's monotonic id so an
/// older result never overrides a newer check or update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Finding {
    pub outdated: bool,
    pub at: u64,
}

/// What the Plugins tab renders for one package plugin row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowView {
    pub outdated: bool,
    pub updating: bool,
}

/// Explicit state of plugin update checking/installing for the active Settings server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Idle {
        next_id: u64,
        findings: HashMap<String, Finding>,
        updating: Vec<RunningUpdate>,
    },
    Checking {
        attempt: u64,
        next_id: u64,
        findings: HashMap<String, Finding>,
        updating: Vec<RunningUpdate>,
    },
    /// Server answered `404`/`405`/`501` on `/api/plugin/check` or `/api/plugin/update`.
    /// Update controls stay quietly hidden with no error toast.
    Unsupported {
        next_id: u64,
    },
    Disposed,
}

impl Default for State {
    fn default() -> Self {
        Self::Idle {
            next_id: 0,
            findings: HashMap::new(),
            updating: Vec::new(),
        }
    }
}

/// Inputs to [`step`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    CheckRequested,
    /// `outdated` maps every package target the check covered to whether it has a newer version.
    CheckSucceeded {
        attempt: u64,
        outdated: HashMap<String, bool>,
    },
    CheckFailed {
        attempt: u64,
        message: String,
    },
    /// `/api/plugin/check` returned 404/405/501 on an older server.
    CheckUnsupported {
        attempt: u64,
    },
    UpdateRequested {
        target: String,
    },
    UpdateSucceeded {
        id: u64,
    },
    UpdateFailed {
        id: u64,
        message: String,
    },
    /// `/api/plugin/update` returned 404/405/501 on an older server.
    UpdateUnsupported {
        id: u64,
    },
    /// `GET /api/plugin` loaded again; `outdated` maps every listed package target to its flag.
    ListLoaded {
        outdated: HashMap<String, bool>,
    },
    /// Settings switched to another server (or reconnected to the current one).
    ServerChanged {
        unsupported: bool,
    },
    Disposed,
}

/// Side effects returned by [`step`] for the caller to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Check { attempt: u64 },
    Update { id: u64, target: String },
    Refresh,
    Failed { message: String },
    MarkServerUnsupported,
}

/// Pure transition function (`transitionPluginUpdate` in `plugin-update-machine.ts`).
pub fn step(state: &mut State, event: Event) -> Vec<Effect> {
    if matches!(state, State::Disposed) {
        return Vec::new();
    }
    match event {
        Event::Disposed => {
            *state = State::Disposed;
            Vec::new()
        }
        Event::ServerChanged { unsupported } => {
            let next_id = next_id_of(state);
            *state = if unsupported {
                State::Unsupported { next_id }
            } else {
                State::Idle {
                    next_id,
                    findings: HashMap::new(),
                    updating: Vec::new(),
                }
            };
            Vec::new()
        }
        Event::ListLoaded { outdated } => {
            let Some(findings) = findings_mut(state) else { return Vec::new() };
            // Keep a finding until a settled list either drops the target or agrees with its flag.
            findings.retain(|target, finding| {
                outdated
                    .get(target)
                    .is_some_and(|listed| *listed != finding.outdated)
            });
            Vec::new()
        }
        Event::CheckRequested => match state {
            State::Idle { next_id, findings, updating } => {
                let attempt = *next_id;
                let next = attempt + 1;
                let findings = std::mem::take(findings);
                let updating = std::mem::take(updating);
                *state = State::Checking { attempt, next_id: next, findings, updating };
                vec![Effect::Check { attempt }]
            }
            State::Checking { .. } | State::Unsupported { .. } | State::Disposed => Vec::new(),
        },
        Event::CheckSucceeded { attempt, outdated } => {
            let State::Checking { attempt: cur, next_id, findings, updating } = state else {
                return Vec::new();
            };
            if *cur != attempt {
                return Vec::new();
            }
            let next = *next_id;
            let mut findings = std::mem::take(findings);
            let updating = std::mem::take(updating);
            // An update that started or finished after this check began knows more about its package.
            for (target, is_outdated) in outdated {
                let newer = findings.get(&target).is_some_and(|f| f.at > attempt)
                    || updating.iter().any(|item| item.target == target);
                if !newer {
                    findings.insert(target, Finding { outdated: is_outdated, at: attempt });
                }
            }
            *state = State::Idle { next_id: next, findings, updating };
            Vec::new()
        }
        Event::CheckFailed { attempt, message } => {
            let State::Checking { attempt: cur, next_id, findings, updating } = state else {
                return Vec::new();
            };
            if *cur != attempt {
                return Vec::new();
            }
            let next = *next_id;
            let findings = std::mem::take(findings);
            let updating = std::mem::take(updating);
            *state = State::Idle { next_id: next, findings, updating };
            vec![Effect::Failed { message }]
        }
        Event::CheckUnsupported { attempt } => {
            let State::Checking { attempt: cur, next_id, .. } = state else {
                return Vec::new();
            };
            if *cur != attempt {
                return Vec::new();
            }
            let next = *next_id;
            *state = State::Unsupported { next_id: next };
            vec![Effect::MarkServerUnsupported]
        }
        Event::UpdateRequested { target } => {
            let Some((next_id, updating)) = next_and_updating_mut(state) else {
                return Vec::new();
            };
            if updating.iter().any(|item| item.target == target) {
                return Vec::new();
            }
            let id = *next_id;
            *next_id = id + 1;
            updating.push(RunningUpdate { id, target: target.clone() });
            vec![Effect::Update { id, target }]
        }
        Event::UpdateSucceeded { id } => {
            let Some((findings, updating)) = findings_and_updating_mut(state) else {
                return Vec::new();
            };
            let Some(pos) = updating.iter().position(|item| item.id == id) else {
                return Vec::new();
            };
            let running = updating.remove(pos);
            findings.insert(running.target, Finding { outdated: false, at: running.id });
            vec![Effect::Refresh]
        }
        Event::UpdateFailed { id, message } => {
            let Some((_, updating)) = findings_and_updating_mut(state) else {
                return Vec::new();
            };
            let Some(pos) = updating.iter().position(|item| item.id == id) else {
                return Vec::new();
            };
            updating.remove(pos);
            vec![Effect::Failed { message }]
        }
        Event::UpdateUnsupported { id } => {
            let matches_running = match state {
                State::Idle { updating, .. } | State::Checking { updating, .. } => {
                    updating.iter().any(|item| item.id == id)
                }
                State::Unsupported { .. } | State::Disposed => false,
            };
            if !matches_running {
                return Vec::new();
            }
            let next = next_id_of(state);
            *state = State::Unsupported { next_id: next };
            vec![Effect::MarkServerUnsupported]
        }
    }
}

/// What the Plugins tab shows for one package source (`pluginUpdateView`).
pub fn view(state: &State, target: &str, listed_outdated: bool, listed_updating: bool) -> RowView {
    let (findings, updating) = match state {
        State::Idle { findings, updating, .. } | State::Checking { findings, updating, .. } => (findings, updating),
        State::Unsupported { .. } | State::Disposed => return RowView::default(),
    };
    let is_updating = listed_updating || updating.iter().any(|item| item.target == target);
    let is_outdated = findings.get(target).map_or(listed_outdated, |f| f.outdated);
    RowView { outdated: is_outdated, updating: is_updating }
}

pub fn checking(state: &State) -> bool {
    matches!(state, State::Checking { .. })
}

pub fn supported(state: &State) -> bool {
    !matches!(state, State::Unsupported { .. } | State::Disposed)
}

/// Extracts `target -> outdated` flags from a plugin inventory (`packageFlags` in `extensions.tsx`).
pub fn package_flags(plugins: &[PluginInfo]) -> HashMap<String, bool> {
    plugins
        .iter()
        .filter_map(|p| {
            let (target, outdated, _) = p.package_source()?;
            (!target.is_empty()).then(|| (target.to_owned(), outdated))
        })
        .collect()
}

fn next_id_of(state: &State) -> u64 {
    match state {
        State::Idle { next_id, .. } | State::Checking { next_id, .. } | State::Unsupported { next_id } => *next_id,
        State::Disposed => 0,
    }
}

fn findings_mut(state: &mut State) -> Option<&mut HashMap<String, Finding>> {
    match state {
        State::Idle { findings, .. } | State::Checking { findings, .. } => Some(findings),
        State::Unsupported { .. } | State::Disposed => None,
    }
}

fn next_and_updating_mut(state: &mut State) -> Option<(&mut u64, &mut Vec<RunningUpdate>)> {
    match state {
        State::Idle { next_id, updating, .. } | State::Checking { next_id, updating, .. } => Some((next_id, updating)),
        State::Unsupported { .. } | State::Disposed => None,
    }
}

fn findings_and_updating_mut(state: &mut State) -> Option<(&mut HashMap<String, Finding>, &mut Vec<RunningUpdate>)> {
    match state {
        State::Idle { findings, updating, .. } | State::Checking { findings, updating, .. } => Some((findings, updating)),
        State::Unsupported { .. } | State::Disposed => None,
    }
}
