//! The WSL main side's state machine (`machine.ts`), as plain data: WSL itself runs one host job
//! at a time (probe the runtime, list distros, install WSL), each distro one job (probe, install,
//! update OpenCode, start or remove its server), and every completion carries the `attempt` that
//! started it, so a stale one changes nothing. The controller runs the effects.

use std::collections::BTreeMap;

use crate::contract::*;

pub type Attempt = u64;

/// A configured distro's server while no job owns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Server {
    Stopped,
    Failed(String),
    Ready { attempt: Attempt, url: String, password: String },
}

/// The one job a distro runs. `Updating`, `Starting` and `Removing` own its server (down meanwhile).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Distro {
    Idle { server: Server },
    Probing { attempt: Attempt, server: Server },
    Installing { attempt: Attempt },
    Updating { attempt: Attempt },
    Starting { attempt: Attempt },
    Removing { attempt: Attempt },
}

impl Distro {
    fn idle() -> Distro {
        Distro::Idle { server: Server::Stopped }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostJob {
    ProbeRuntime,
    RefreshDistros,
    InstallWsl,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub runtime: Option<RuntimeCheck>,
    pub installed: Vec<InstalledDistro>,
    pub online: Vec<OnlineDistro>,
    pub distro_probes: BTreeMap<String, DistroProbe>,
    pub opencode_checks: BTreeMap<String, OpencodeCheck>,
    pub pending_restart: bool,
    pub configured: Vec<ServerConfig>,
    /// WSL's own job, while it runs.
    pub host: Option<Attempt>,
    pub distros: BTreeMap<String, Distro>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Host { attempt: Attempt, job: HostJob },
    Probe { distro: String, attempt: Attempt },
    InstallDistro { distro: String, attempt: Attempt },
    Update { distro: String, attempt: Attempt },
    Add { config: ServerConfig, attempt: Attempt },
    Start { id: String, attempt: Attempt },
    Remove { id: String, attempt: Attempt },
    RuntimeProbed { attempt: Attempt, runtime: RuntimeCheck },
    RuntimeInstalled { attempt: Attempt, runtime: RuntimeCheck },
    DistrosListed { attempt: Attempt, installed: Vec<InstalledDistro>, online: Vec<OnlineDistro> },
    HostFailed { attempt: Attempt, message: String },
    DistroProbed { distro: String, attempt: Attempt, probe: DistroProbe },
    CliInspected { distro: String, attempt: Attempt, check: OpencodeCheck },
    DistroInstalled { distro: String, attempt: Attempt, installed: Vec<InstalledDistro>, online: Vec<OnlineDistro>, probe: DistroProbe },
    CliInstalled { distro: String, attempt: Attempt, check: OpencodeCheck },
    JobFailed { distro: String, attempt: Attempt, message: String, check: Option<OpencodeCheck> },
    ServerStopped { distro: String, attempt: Attempt },
    ServerSpawned { distro: String, attempt: Attempt, url: String, password: String },
    ServerFailed { distro: String, attempt: Attempt, message: String },
    ServerExited { distro: String, attempt: Attempt, message: String },
    CliChecked { distro: String, check: OpencodeCheck },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Busy(String),
    AlreadyAdded(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Host { attempt: Attempt, job: HostJob },
    ProbeDistro { distro: String, attempt: Attempt },
    InspectCli { distro: String, attempt: Attempt },
    InstallDistro { distro: String, attempt: Attempt },
    InstallCli { distro: String, attempt: Attempt },
    /// Stops the server of start `server`, still starting or up, then reports `ServerStopped`.
    Stop { distro: String, attempt: Attempt, server: Attempt },
    Spawn { distro: String, attempt: Attempt },
    CheckCli { distro: String },
    Persist(Vec<ServerConfig>),
    Log { error: bool, message: String },
    Resolve(Attempt),
    Reject(Attempt, Failure),
}

pub fn create(configured: Vec<ServerConfig>) -> State {
    State { configured, ..State::default() }
}

/// One step: the next state and what to do.
pub fn transition(state: &State, event: Event) -> (State, Vec<Effect>) {
    let mut s = state.clone();
    let effects = step(&mut s, event);
    (s, effects)
}

fn distro_of(s: &State, distro: &str) -> Distro {
    s.distros.get(distro).cloned().unwrap_or_else(Distro::idle)
}

fn put(s: &mut State, distro: &str, job: Distro) {
    s.distros.insert(distro.to_owned(), job);
}

fn configured(s: &State, distro: &str) -> bool {
    s.configured.iter().any(|c| c.distro == distro)
}

fn needs_cli_check(s: &State, distro: &str) -> bool {
    s.distro_probes.get(distro).is_some_and(|p| p.can_execute && p.has_bash && p.has_curl) && !s.opencode_checks.contains_key(distro)
}

fn without(s: &mut State, distro: &str) {
    s.configured.retain(|c| c.distro != distro);
    s.distros.remove(distro);
    s.distro_probes.remove(distro);
    s.opencode_checks.remove(distro);
}

fn step(s: &mut State, event: Event) -> Vec<Effect> {
    match event {
        // A second host request while one runs joins it: the dialog asks again once WSL is idle.
        Event::Host { attempt, job } => {
            if s.host.is_some() {
                return vec![Effect::Resolve(attempt)];
            }
            s.host = Some(attempt);
            vec![Effect::Host { attempt, job }]
        }
        Event::RuntimeProbed { attempt, runtime } => host_settled(s, attempt, |s| {
            s.pending_restart = s.pending_restart && !runtime.available;
            s.runtime = Some(runtime);
        }),
        Event::RuntimeInstalled { attempt, runtime } => host_settled(s, attempt, |s| {
            s.pending_restart = !runtime.available;
            s.runtime = Some(runtime);
        }),
        Event::DistrosListed { attempt, installed, online } => host_settled(s, attempt, |s| {
            s.installed = installed;
            s.online = online;
        }),
        Event::HostFailed { attempt, message } => {
            if s.host != Some(attempt) {
                return vec![];
            }
            s.host = None;
            vec![Effect::Reject(attempt, Failure::Error(message))]
        }
        // The dialog probes what it shows; a distro already running a job is checked again once idle.
        Event::Probe { distro, attempt } => {
            let Distro::Idle { server } = distro_of(s, &distro) else { return vec![Effect::Resolve(attempt)] };
            if !s.distro_probes.contains_key(&distro) {
                put(s, &distro, Distro::Probing { attempt, server });
                return vec![Effect::ProbeDistro { distro, attempt }];
            }
            if needs_cli_check(s, &distro) {
                put(s, &distro, Distro::Probing { attempt, server });
                return vec![Effect::InspectCli { distro, attempt }];
            }
            vec![Effect::Resolve(attempt)]
        }
        Event::DistroProbed { distro, attempt, probe } => {
            let Distro::Probing { attempt: a, server } = distro_of(s, &distro) else { return vec![] };
            if a != attempt {
                return vec![];
            }
            s.distro_probes.insert(distro.clone(), probe);
            if needs_cli_check(s, &distro) {
                return vec![Effect::InspectCli { distro, attempt }];
            }
            put(s, &distro, Distro::Idle { server });
            vec![Effect::Resolve(attempt)]
        }
        Event::CliInspected { distro, attempt, check } => {
            let Distro::Probing { attempt: a, server } = distro_of(s, &distro) else { return vec![] };
            if a != attempt {
                return vec![];
            }
            s.opencode_checks.insert(distro.clone(), check);
            put(s, &distro, Distro::Idle { server });
            vec![Effect::Resolve(attempt)]
        }
        Event::InstallDistro { distro, attempt } => {
            let current = distro_of(s, &distro);
            if matches!(current, Distro::Installing { .. }) {
                return vec![Effect::Resolve(attempt)];
            }
            if configured(s, &distro) {
                return vec![Effect::Reject(attempt, Failure::AlreadyAdded(distro))];
            }
            if !matches!(current, Distro::Idle { .. }) {
                return vec![Effect::Reject(attempt, Failure::Busy(distro))];
            }
            put(s, &distro, Distro::Installing { attempt });
            vec![Effect::InstallDistro { distro, attempt }]
        }
        Event::DistroInstalled { distro, attempt, installed, online, probe } => {
            if distro_of(s, &distro) != (Distro::Installing { attempt }) {
                return vec![];
            }
            s.installed = installed;
            s.online = online;
            s.distro_probes.insert(distro.clone(), probe);
            put(s, &distro, Distro::idle());
            vec![Effect::Resolve(attempt)]
        }
        // Updating stops the distro's server first, even one still starting, and starts it again
        // once the CLI matches.
        Event::Update { distro, attempt } => match distro_of(s, &distro) {
            Distro::Updating { .. } => vec![Effect::Resolve(attempt)],
            Distro::Starting { attempt: starting } => {
                put(s, &distro, Distro::Updating { attempt });
                vec![Effect::Resolve(starting), Effect::Stop { distro, attempt, server: starting }]
            }
            Distro::Idle { server: Server::Ready { attempt: running, .. } } => {
                put(s, &distro, Distro::Updating { attempt });
                vec![Effect::Stop { distro, attempt, server: running }]
            }
            Distro::Idle { .. } => {
                put(s, &distro, Distro::Updating { attempt });
                vec![Effect::InstallCli { distro, attempt }]
            }
            _ => vec![Effect::Reject(attempt, Failure::Busy(distro))],
        },
        // The update's caller hears once the restarted server is up or failed.
        Event::CliInstalled { distro, attempt, check } => {
            if distro_of(s, &distro) != (Distro::Updating { attempt }) {
                return vec![];
            }
            s.opencode_checks.insert(distro.clone(), check);
            if !configured(s, &distro) {
                put(s, &distro, Distro::idle());
                return vec![Effect::Resolve(attempt)];
            }
            put(s, &distro, Distro::Starting { attempt });
            vec![Effect::Spawn { distro, attempt }]
        }
        // A failed update leaves the server stopped, so the row offers to start it again.
        Event::JobFailed { distro, attempt, message, check } => {
            let current = distro_of(s, &distro);
            let idle = match &current {
                Distro::Probing { attempt: a, server } if *a == attempt => Distro::Idle { server: server.clone() },
                Distro::Installing { attempt: a } | Distro::Updating { attempt: a } if *a == attempt => Distro::idle(),
                _ => return vec![],
            };
            if let Some(check) = check {
                s.opencode_checks.insert(distro.clone(), check);
            }
            put(s, &distro, idle);
            vec![Effect::Reject(attempt, Failure::Error(message))]
        }
        // The caller hears once the server is saved; it starts in the background.
        Event::Add { config, attempt } => {
            let distro = config.distro.clone();
            if s.configured.iter().any(|c| c.id == config.id || c.distro == distro) {
                return vec![Effect::Reject(attempt, Failure::AlreadyAdded(distro))];
            }
            if !matches!(distro_of(s, &distro), Distro::Idle { .. }) {
                return vec![Effect::Reject(attempt, Failure::Busy(distro))];
            }
            s.configured.push(config);
            put(s, &distro, Distro::Starting { attempt });
            vec![Effect::Persist(s.configured.clone()), Effect::Spawn { distro: distro.clone(), attempt }, Effect::CheckCli { distro }, Effect::Resolve(attempt)]
        }
        // A start while one is starting or an update will restart it joins that.
        Event::Start { id, attempt } => {
            let Some(distro) = s.configured.iter().find(|c| c.id == id).map(|c| c.distro.clone()) else { return vec![Effect::Resolve(attempt)] };
            match distro_of(s, &distro) {
                Distro::Starting { .. } | Distro::Updating { .. } => vec![Effect::Resolve(attempt)],
                Distro::Idle { server: Server::Ready { attempt: running, .. } } => {
                    put(s, &distro, Distro::Starting { attempt });
                    vec![Effect::Stop { distro: distro.clone(), attempt, server: running }, Effect::CheckCli { distro }]
                }
                Distro::Idle { .. } => {
                    put(s, &distro, Distro::Starting { attempt });
                    vec![Effect::Spawn { distro: distro.clone(), attempt }, Effect::CheckCli { distro }]
                }
                _ => vec![Effect::Reject(attempt, Failure::Busy(distro))],
            }
        }
        Event::Remove { id, attempt } => {
            let Some(distro) = s.configured.iter().find(|c| c.id == id).map(|c| c.distro.clone()) else { return vec![Effect::Resolve(attempt)] };
            match distro_of(s, &distro) {
                Distro::Removing { .. } => vec![Effect::Resolve(attempt)],
                Distro::Starting { attempt: starting } => {
                    put(s, &distro, Distro::Removing { attempt });
                    vec![Effect::Resolve(starting), Effect::Stop { distro, attempt, server: starting }]
                }
                Distro::Idle { server: Server::Ready { attempt: running, .. } } => {
                    put(s, &distro, Distro::Removing { attempt });
                    vec![Effect::Stop { distro, attempt, server: running }]
                }
                Distro::Idle { .. } => {
                    without(s, &distro);
                    vec![Effect::Persist(s.configured.clone()), Effect::Resolve(attempt)]
                }
                _ => vec![Effect::Reject(attempt, Failure::Busy(distro))],
            }
        }
        Event::ServerStopped { distro, attempt } => match distro_of(s, &distro) {
            Distro::Starting { attempt: a } if a == attempt => vec![Effect::Spawn { distro, attempt }],
            Distro::Updating { attempt: a } if a == attempt => vec![Effect::InstallCli { distro, attempt }],
            Distro::Removing { attempt: a } if a == attempt => {
                without(s, &distro);
                vec![Effect::Persist(s.configured.clone()), Effect::Resolve(attempt)]
            }
            _ => vec![],
        },
        // A server that comes up for a start nobody wants any more is stopped by the job that
        // superseded it.
        Event::ServerSpawned { distro, attempt, url, password } => {
            if distro_of(s, &distro) != (Distro::Starting { attempt }) {
                return vec![];
            }
            put(s, &distro, Distro::Idle { server: Server::Ready { attempt, url: url.clone(), password } });
            vec![Effect::Resolve(attempt), Effect::CheckCli { distro: distro.clone() }, Effect::Log { error: false, message: format!("wsl sidecar ready {distro} {url}") }]
        }
        // A failed start answers its callers without an error: the row shows it and offers Retry.
        Event::ServerFailed { distro, attempt, message } => {
            if distro_of(s, &distro) != (Distro::Starting { attempt }) {
                return vec![];
            }
            put(s, &distro, Distro::Idle { server: Server::Failed(message.clone()) });
            vec![Effect::Resolve(attempt), Effect::Log { error: true, message: format!("wsl sidecar failed to start {distro}: {message}") }]
        }
        // Only the live server's exit is a failure; a server a job stopped exits on purpose.
        Event::ServerExited { distro, attempt, message } => {
            let failed = Server::Failed(message);
            match distro_of(s, &distro) {
                Distro::Idle { server: Server::Ready { attempt: a, .. } } if a == attempt => put(s, &distro, Distro::Idle { server: failed }),
                Distro::Probing { attempt: p, server: Server::Ready { attempt: a, .. } } if a == attempt => put(s, &distro, Distro::Probing { attempt: p, server: failed }),
                _ => return vec![],
            }
            vec![Effect::Log { error: true, message: format!("wsl sidecar exited {distro}") }]
        }
        // Background checks never overwrite what an update or removal decides.
        Event::CliChecked { distro, check } => {
            let job = distro_of(s, &distro);
            if configured(s, &distro) && !matches!(job, Distro::Updating { .. } | Distro::Removing { .. }) {
                s.opencode_checks.insert(distro, check);
            }
            vec![]
        }
    }
}

fn host_settled(s: &mut State, attempt: Attempt, apply: impl FnOnce(&mut State)) -> Vec<Effect> {
    if s.host != Some(attempt) {
        return vec![];
    }
    apply(s);
    s.host = None;
    vec![Effect::Resolve(attempt)]
}

/// What windows see (`toServersState`).
pub fn published(s: &State) -> ServersState {
    let runtime_of = |d: Option<&Distro>| match d {
        Some(Distro::Starting { .. }) => ServerRuntime::Starting,
        Some(Distro::Idle { server } | Distro::Probing { server, .. }) => match server {
            Server::Ready { url, password, .. } => ServerRuntime::Ready { url: url.clone(), password: password.clone() },
            Server::Failed(m) => ServerRuntime::Failed(m.clone()),
            Server::Stopped => ServerRuntime::Stopped,
        },
        _ => ServerRuntime::Stopped,
    };
    ServersState {
        runtime: s.runtime.clone(),
        installed: s.installed.clone(),
        online: s.online.clone(),
        distro_probes: s.distro_probes.clone(),
        opencode_checks: s.opencode_checks.clone(),
        pending_restart: s.pending_restart,
        servers: s.configured.iter().map(|c| ServerItem { config: c.clone(), runtime: runtime_of(s.distros.get(&c.distro)) }).collect(),
        host_busy: s.host.is_some(),
        distro_jobs: s
            .distros
            .iter()
            .filter_map(|(d, j)| {
                let job = match j {
                    Distro::Idle { .. } => return None,
                    Distro::Probing { .. } => DistroJob::Probing,
                    Distro::Installing { .. } => DistroJob::Installing,
                    Distro::Updating { .. } => DistroJob::Updating,
                    Distro::Starting { .. } => DistroJob::Starting,
                    Distro::Removing { .. } => DistroJob::Removing,
                };
                Some((d.clone(), job))
            })
            .collect(),
    }
}
