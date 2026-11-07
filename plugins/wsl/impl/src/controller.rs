//! The WSL main side (`servers.ts`, `runtime.ts`): runs the machine's effects as host processes
//! and fetches, reports their outcomes back to it, answers the requests windows make, saves the
//! configured distros and publishes their servers. The machine alone decides what's current.
//!
//! The app half owns it; the window half reaches it through the `wsl.servers` service. Both run
//! on the app's thread, so it lives in an `Rc<RefCell<…>>`, and every callback it hands the host
//! holds a weak reference back.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use opencode_plugin::prelude::*;
use serde_json::{Value, json};

use crate::contract::*;
use crate::machine::{self, Attempt, Effect, Event, Failure, HostJob};
use crate::runtime::{self, INSTALL_TIMEOUT_MS, TIMEOUT_MS};

/// What a request's caller hears: done, or why not.
pub type Done = Box<dyn FnOnce(&mut ActionCx, Result<(), String>)>;

pub type CoreRef = Rc<RefCell<Core>>;

/// The saved distros (`servers`, as Desktop's main store).
const SAVED: Store<String> = Store::new("servers");

pub struct Core {
    me: Weak<RefCell<Core>>,
    state: machine::State,
    /// What windows see, and a count of its changes (memo versions).
    pub published: ServersState,
    pub revision: u64,
    next: Attempt,
    waiters: HashMap<Attempt, Done>,
    /// Callers answered during the current call, told once the controller is free again.
    ready: Vec<(Done, Result<(), String>)>,
    /// Each start's server, by attempt: starting, or up.
    pub(crate) sidecars: HashMap<Attempt, crate::sidecar::Sidecar>,
    /// Wakes the plugin's polls when what windows see changes.
    pub waker: Option<PluginWaker>,
}

impl Core {
    /// The controller, with the distros saved before.
    pub fn new() -> CoreRef {
        let configured = read_saved();
        Rc::new_cyclic(|me| {
            let state = machine::create(configured);
            RefCell::new(Core { me: me.clone(), published: machine::published(&state), state, revision: 0, next: 0, waiters: HashMap::new(), ready: Vec::new(), sidecars: HashMap::new(), waker: None })
        })
    }

    pub(crate) fn weak(&self) -> Weak<RefCell<Core>> {
        self.me.clone()
    }

    fn attempt(&mut self) -> Attempt {
        self.next += 1;
        self.next
    }

    /// Runs `event`, then its effects.
    pub fn dispatch(&mut self, cx: &mut ActionCx, event: Event) {
        let (next, effects) = machine::transition(&self.state, event);
        if next != self.state {
            self.state = next;
            let published = machine::published(&self.state);
            if published != self.published {
                self.published = published;
                self.revision += 1;
                self.publish(cx);
                if let Some(w) = &self.waker {
                    w.wake();
                }
            }
        }
        for effect in effects {
            self.run(cx, effect);
        }
    }

    /// Asks the machine for `event` (made with a fresh attempt); `done` hears how it went.
    fn request(&mut self, cx: &mut ActionCx, event: impl FnOnce(Attempt) -> Event, done: Done) {
        let attempt = self.attempt();
        self.waiters.insert(attempt, done);
        self.dispatch(cx, event(attempt));
    }

    /// The servers as the app lists them (`ServerEntry`): each configured distro, joining the
    /// app's servers once up.
    pub fn publish(&self, cx: &mut ActionCx) {
        cx.set_servers(crate::SOURCE, &self.entries(), true);
    }

    pub fn entries(&self) -> Vec<ServerEntry> {
        self.published
            .servers
            .iter()
            .map(|item| {
                let (state, url, password) = match &item.runtime {
                    ServerRuntime::Ready { url, password } => (ServerState::Ready, url.clone(), password.clone()),
                    ServerRuntime::Starting => (ServerState::Starting, String::new(), String::new()),
                    ServerRuntime::Failed(_) => (ServerState::Failed, String::new(), String::new()),
                    ServerRuntime::Stopped => (ServerState::Stopped, String::new(), String::new()),
                };
                ServerEntry { id: item.config.distro.clone(), name: item.config.distro.clone(), label: "WSL".into(), state, url, password, listed: state == ServerState::Ready }
            })
            .collect()
    }

    // Requests windows make (`Wsl` IPC methods).

    pub fn probe_runtime(&mut self, cx: &mut ActionCx, done: Done) {
        self.request(cx, |attempt| Event::Host { attempt, job: HostJob::ProbeRuntime }, done);
    }

    pub fn refresh_distros(&mut self, cx: &mut ActionCx, done: Done) {
        self.request(cx, |attempt| Event::Host { attempt, job: HostJob::RefreshDistros }, done);
    }

    pub fn install_wsl(&mut self, cx: &mut ActionCx, done: Done) {
        self.request(cx, |attempt| Event::Host { attempt, job: HostJob::InstallWsl }, done);
    }

    pub fn install_distro(&mut self, cx: &mut ActionCx, name: &str, done: Done) {
        let distro = name.to_owned();
        self.request(cx, |attempt| Event::InstallDistro { distro, attempt }, done);
    }

    /// Probes each distro the dialog can add; `done` once all answered.
    pub fn probe_addable(&mut self, cx: &mut ActionCx, distros: &[String], done: Done) {
        let mut unique: Vec<&String> = distros.iter().collect();
        unique.dedup();
        if unique.is_empty() {
            return done(cx, Ok(()));
        }
        let join = Rc::new(RefCell::new((unique.len(), Some(done), None::<String>)));
        for distro in unique {
            let (join, distro) = (join.clone(), distro.clone());
            self.request(
                cx,
                |attempt| Event::Probe { distro, attempt },
                Box::new(move |cx, result| {
                    let mut j = join.borrow_mut();
                    j.0 -= 1;
                    if let Err(e) = result {
                        j.2.get_or_insert(e);
                    }
                    if j.0 == 0
                        && let Some(done) = j.1.take()
                    {
                        let error = j.2.take();
                        drop(j);
                        done(cx, error.map_or(Ok(()), Err));
                    }
                }),
            );
        }
    }

    pub fn install_opencode(&mut self, cx: &mut ActionCx, name: &str, done: Done) {
        let distro = name.to_owned();
        self.request(cx, |attempt| Event::Update { distro, attempt }, done);
    }

    pub fn add_server(&mut self, cx: &mut ActionCx, distro: &str, done: Done) {
        let config = ServerConfig { id: server_id_for_distro(distro), distro: distro.to_owned() };
        self.request(cx, |attempt| Event::Add { config, attempt }, done);
    }

    pub fn remove_server(&mut self, cx: &mut ActionCx, id: &str, done: Done) {
        let id = id.to_owned();
        self.request(cx, |attempt| Event::Remove { id, attempt }, done);
    }

    pub fn start_server(&mut self, cx: &mut ActionCx, id: &str, done: Done) {
        let id = id.to_owned();
        self.request(cx, |attempt| Event::Start { id, attempt }, done);
    }

    /// Starts the saved distros' servers, at launch; nobody waits on them.
    pub fn start_configured(&mut self, cx: &mut ActionCx) {
        let ids: Vec<String> = self.published.servers.iter().map(|s| s.config.id.clone()).collect();
        for id in ids {
            let attempt = self.attempt();
            self.dispatch(cx, Event::Start { id, attempt });
        }
    }

    fn run(&mut self, cx: &mut ActionCx, effect: Effect) {
        match effect {
            Effect::Resolve(attempt) => self.settle(cx, attempt, Ok(())),
            Effect::Reject(attempt, failure) => self.settle(cx, attempt, Err(failure_text(&failure))),
            Effect::Persist(servers) => {
                let list: Vec<Value> = servers.iter().map(|s| json!({ "id": s.id, "distro": s.distro })).collect();
                SAVED.set(&json!({ "servers": list }).to_string());
            }
            Effect::Log { error, message } => opencode_plugin::system::log(error, &format!("[wsl] {message}")),
            Effect::Spawn { distro, attempt } => crate::sidecar::spawn(self, cx, distro, attempt),
            Effect::Stop { distro, attempt, server } => crate::sidecar::stop(self, cx, distro, attempt, server),
            Effect::CheckCli { distro } => self.inspect_cli(cx, distro.clone(), move |core, cx, check| match check {
                Ok(check) => core.dispatch(cx, Event::CliChecked { distro, check }),
                Err(e) => opencode_plugin::system::log(true, &format!("[wsl] wsl CLI check failed {distro}: {e}")),
            }),
            Effect::Host { attempt, job } => self.host_job(cx, attempt, job),
            Effect::ProbeDistro { distro, attempt } => self.probe_distro(cx, distro.clone(), move |core, cx, probe| core.dispatch(cx, Event::DistroProbed { distro, attempt, probe })),
            Effect::InspectCli { distro, attempt } => self.inspect_cli(cx, distro.clone(), move |core, cx, check| {
                let event = match check {
                    Ok(check) => Event::CliInspected { distro, attempt, check },
                    Err(message) => Event::JobFailed { distro, attempt, message, check: None },
                };
                core.dispatch(cx, event);
            }),
            Effect::InstallDistro { distro, attempt } => self.install_distro_job(cx, distro, attempt),
            Effect::InstallCli { distro, attempt } => self.install_cli_job(cx, distro, attempt),
        }
    }

    fn settle(&mut self, _: &mut ActionCx, attempt: Attempt, result: Result<(), String>) {
        if let Some(done) = self.waiters.remove(&attempt) {
            self.ready.push((done, result));
        }
    }

    /// Runs `wsl.exe args`, then `done` with its output or why it didn't run.
    pub(crate) fn wsl(&self, cx: &mut ActionCx, args: Vec<String>, timeout: u64, done: impl FnOnce(&mut Core, &mut ActionCx, Result<ProcessOutput, String>) + 'static) {
        let me = self.weak();
        let program = runtime::program();
        let spec = ProcessSpec::new(program.clone()).args(args.clone()).timeout(timeout);
        let started = cx.run_process::<()>(&spec, move |_, cx, out| {
            let Some(core) = me.upgrade() else { return };
            let result = if out.exit.timed_out {
                Err(format!("{program} {} timed out after {timeout}ms", args.join(" ")))
            } else if let Some(e) = out.exit.error.clone() {
                Err(e)
            } else {
                Ok(out)
            };
            with(&core, cx, |core, cx| done(core, cx, result));
        });
        if started.is_none() {
            opencode_plugin::system::log(true, "[wsl] the host refused to start wsl.exe");
        }
    }

    fn host_job(&mut self, cx: &mut ActionCx, attempt: Attempt, job: HostJob) {
        match job {
            HostJob::ProbeRuntime => self.probe_runtime_job(cx, move |core, cx, runtime| core.dispatch(cx, Event::RuntimeProbed { attempt, runtime })),
            HostJob::RefreshDistros => self.list_distros(cx, move |core, cx, lists| {
                let event = match lists {
                    Ok((installed, online)) => Event::DistrosListed { attempt, installed, online },
                    Err(message) => Event::HostFailed { attempt, message },
                };
                core.dispatch(cx, event);
            }),
            HostJob::InstallWsl => {
                let me = self.weak();
                let script = "$ErrorActionPreference = 'Stop'; $process = Start-Process -FilePath 'wsl.exe' -Verb RunAs -ArgumentList @('--install','--no-distribution') -Wait -PassThru; if ($null -ne $process.ExitCode) { exit $process.ExitCode }";
                let spec = ProcessSpec::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]).timeout(INSTALL_TIMEOUT_MS);
                cx.run_process::<()>(&spec, move |_, cx, out| {
                    let Some(core) = me.upgrade() else { return };
                    with(&core, cx, |core, cx| {
                        if !out.ok() {
                            let message = Some(runtime::summarize(if out.stderr.trim().is_empty() { &out.stdout } else { &out.stderr })).filter(|m| !m.is_empty()).unwrap_or_else(|| "WSL installation failed".into());
                            return core.dispatch(cx, Event::HostFailed { attempt, message });
                        }
                        core.probe_runtime_job(cx, move |core, cx, runtime| core.dispatch(cx, Event::RuntimeInstalled { attempt, runtime }));
                    });
                });
            }
        }
    }

    fn probe_runtime_job(&self, cx: &mut ActionCx, done: impl FnOnce(&mut Core, &mut ActionCx, RuntimeCheck) + 'static) {
        self.wsl(cx, vec!["--version".into()], TIMEOUT_MS, move |core, cx, out| {
            let check = match out {
                Ok(o) if o.ok() => RuntimeCheck { available: true, version: runtime::first_line(&o.stdout), error: None },
                Ok(o) => RuntimeCheck { available: false, version: None, error: Some(Some(runtime::summarize(if o.stderr.trim().is_empty() { &o.stdout } else { &o.stderr })).filter(|m| !m.is_empty()).unwrap_or_else(|| "WSL is unavailable".into())) },
                Err(e) => RuntimeCheck { available: false, version: None, error: Some(e) },
            };
            done(core, cx, check);
        });
    }

    /// `--list --verbose` and `--list --online`, both.
    fn list_distros(&self, cx: &mut ActionCx, done: impl FnOnce(&mut Core, &mut ActionCx, Result<(Vec<InstalledDistro>, Vec<OnlineDistro>), String>) + 'static) {
        type Lists = (Option<Result<Vec<InstalledDistro>, String>>, Option<Result<Vec<OnlineDistro>, String>>);
        let join: Rc<RefCell<Lists>> = Rc::new(RefCell::new((None, None)));
        let done = Rc::new(RefCell::new(Some(done)));
        let finish = move |core: &mut Core, cx: &mut ActionCx, join: &Rc<RefCell<Lists>>| {
            let mut j = join.borrow_mut();
            if j.0.is_none() || j.1.is_none() {
                return;
            }
            let (Some(installed), Some(online)) = (j.0.take(), j.1.take()) else { return };
            drop(j);
            if let Some(done) = done.borrow_mut().take() {
                done(core, cx, installed.and_then(|i| online.map(|o| (i, o))));
            }
        };
        let finish = Rc::new(finish);
        let (j1, f1) = (join.clone(), finish.clone());
        self.wsl(cx, vec!["--list".into(), "--verbose".into()], TIMEOUT_MS, move |core, cx, out| {
            j1.borrow_mut().0 = Some(listed(out, "Failed to list installed WSL distros").map(|o| runtime::parse_installed(&o)));
            f1(core, cx, &j1);
        });
        let (j2, f2) = (join, finish);
        self.wsl(cx, vec!["--list".into(), "--online".into()], TIMEOUT_MS, move |core, cx, out| {
            j2.borrow_mut().1 = Some(listed(out, "Failed to list online WSL distros").map(|o| runtime::parse_online(&o)));
            f2(core, cx, &j2);
        });
    }

    /// Can the distro run commands, and does it have bash and curl.
    pub(crate) fn probe_distro(&self, cx: &mut ActionCx, name: String, done: impl FnOnce(&mut Core, &mut ActionCx, DistroProbe) + 'static) {
        let args = runtime::in_distro(&["/bin/true"], Some(&name));
        self.wsl(cx, args, TIMEOUT_MS, move |core, cx, out| {
            let failure = match &out {
                Ok(o) if o.ok() => None,
                Ok(o) => Some(Some(runtime::summarize(if o.stderr.trim().is_empty() { &o.stdout } else { &o.stderr })).filter(|m| !m.is_empty()).unwrap_or_else(|| "Cannot execute commands in distro".into())),
                Err(e) => Some(e.clone()),
            };
            if let Some(error) = failure {
                return done(core, cx, DistroProbe { name, can_execute: false, has_bash: false, has_curl: false, error: Some(error) });
            }
            let join = Rc::new(RefCell::new((None::<bool>, None::<bool>, Some(done))));
            for (tool, first) in [("bash", true), ("curl", false)] {
                let (join, name) = (join.clone(), name.clone());
                let script = format!("command -v {tool} >/dev/null && printf yes || printf no");
                core.wsl(cx, runtime::sh(&script, &name), TIMEOUT_MS, move |core, cx, out| {
                    let yes = out.is_ok_and(|o| o.ok() && runtime::summarize(&o.stdout) == "yes");
                    let mut j = join.borrow_mut();
                    if first { j.0 = Some(yes) } else { j.1 = Some(yes) }
                    if let (Some(has_bash), Some(has_curl)) = (j.0, j.1)
                        && let Some(done) = j.2.take()
                    {
                        drop(j);
                        done(core, cx, DistroProbe { name, can_execute: true, has_bash, has_curl, error: None });
                    }
                });
            }
        });
    }

    /// Where the distro's OpenCode is and what version it runs, against the app's server's.
    pub(crate) fn inspect_cli(&self, cx: &mut ActionCx, distro: String, done: impl FnOnce(&mut Core, &mut ActionCx, Result<OpencodeCheck, String>) + 'static) {
        self.wsl(cx, runtime::sh(runtime::DISCOVER_SCRIPT, &distro), TIMEOUT_MS, move |core, cx, out| {
            let resolved = match out {
                Ok(o) => runtime::first_line(&o.stdout),
                Err(e) => return done(core, cx, Err(e)),
            };
            let expected = opencode_plugin::system::server_version();
            let Some(path) = resolved else {
                return done(core, cx, Ok(OpencodeCheck { distro, resolved_path: None, version: None, expected_version: expected, matches_desktop: None, error: Some("opencode is not installed in this distro".into()) }));
            };
            let script = runtime::version_script(&runtime::quote(&path));
            core.wsl(cx, runtime::sh(&script, &distro), TIMEOUT_MS, move |core, cx, out| {
                let version = out.ok().and_then(|o| runtime::parse_version(&o.stdout));
                let check = match version {
                    None => OpencodeCheck { distro, resolved_path: Some(path), version: None, expected_version: expected, matches_desktop: None, error: Some("opencode is installed but could not run".into()) },
                    Some(v) => {
                        let matches = expected.as_ref().map(|e| *e == v);
                        OpencodeCheck { distro, resolved_path: Some(path), version: Some(v), expected_version: expected, matches_desktop: matches, error: None }
                    }
                };
                done(core, cx, Ok(check));
            });
        });
    }

    fn install_distro_job(&self, cx: &mut ActionCx, distro: String, attempt: Attempt) {
        let args = vec!["--install".into(), "-d".into(), distro.clone(), "--web-download".into(), "--no-launch".into()];
        self.wsl(cx, args, INSTALL_TIMEOUT_MS, move |core, cx, out| {
            if let Some(message) = failed(&out, &format!("Failed to install distro: {distro}")) {
                return core.dispatch(cx, Event::JobFailed { distro, attempt, message, check: None });
            }
            core.list_distros(cx, move |core, cx, lists| match lists {
                Err(message) => core.dispatch(cx, Event::JobFailed { distro, attempt, message, check: None }),
                Ok((installed, online)) => {
                    let name = distro.clone();
                    core.probe_distro(cx, name, move |core, cx, probe| core.dispatch(cx, Event::DistroInstalled { distro, attempt, installed, online, probe }));
                }
            });
        });
    }

    fn install_cli_job(&mut self, cx: &mut ActionCx, distro: String, attempt: Attempt) {
        let expected = opencode_plugin::system::server_version().unwrap_or_default();
        let Some(script) = runtime::install_script(&expected) else {
            let message = format!("OpenCode installation failed: this app's server reports no version to install ({expected})");
            return self.dispatch(cx, Event::JobFailed { distro, attempt, message, check: None });
        };
        self.wsl(cx, runtime::in_distro(&["bash", "-lc", &script], Some(&distro)), INSTALL_TIMEOUT_MS, move |core, cx, out| {
            if let Some(message) = failed(&out, "OpenCode installation failed") {
                return core.dispatch(cx, Event::JobFailed { distro, attempt, message, check: None });
            }
            let name = distro.clone();
            core.inspect_cli(cx, name, move |core, cx, check| {
                let event = match check {
                    Ok(check) if check.version.as_deref() == Some(expected.as_str()) => Event::CliInstalled { distro, attempt, check },
                    Ok(check) => {
                        let installed = check.version.clone().unwrap_or_else(|| "no version".into());
                        let message = format!("OpenCode update finished but {distro} still reports {installed}; expected {expected}");
                        Event::JobFailed { distro, attempt, message, check: Some(check) }
                    }
                    Err(message) => Event::JobFailed { distro, attempt, message, check: None },
                };
                core.dispatch(cx, event);
            });
        });
    }
}

/// Runs  on the controller, then tells the callers it answered meanwhile, once it's free:
/// they may call it again.
pub fn with<R>(core: &CoreRef, cx: &mut ActionCx, f: impl FnOnce(&mut Core, &mut ActionCx) -> R) -> R {
    let (r, ready) = {
        let mut c = core.borrow_mut();
        let r = f(&mut c, cx);
        (r, std::mem::take(&mut c.ready))
    };
    for (done, result) in ready {
        done(cx, result);
    }
    r
}

fn listed(out: Result<ProcessOutput, String>, fallback: &str) -> Result<String, String> {
    match out {
        Ok(o) if o.ok() => Ok(o.stdout),
        Ok(o) => Err(Some(runtime::summarize(if o.stderr.trim().is_empty() { &o.stdout } else { &o.stderr })).filter(|m| !m.is_empty()).unwrap_or_else(|| fallback.into())),
        Err(e) => Err(e),
    }
}

/// Why a command failed (`requireSuccess`), or nothing when it ran fine.
fn failed(out: &Result<ProcessOutput, String>, fallback: &str) -> Option<String> {
    match out {
        Ok(o) if o.ok() => None,
        Ok(o) => Some(Some(runtime::summarize(if o.stderr.trim().is_empty() { &o.stdout } else { &o.stderr })).filter(|m| !m.is_empty()).unwrap_or_else(|| fallback.into())),
        Err(e) => Some(e.clone()),
    }
}

fn failure_text(f: &Failure) -> String {
    match f {
        Failure::Busy(d) => format!("{d} is busy with another task. Try again when it finishes."),
        Failure::AlreadyAdded(d) => format!("{d} is already added"),
        Failure::Error(m) => m.clone(),
    }
}

/// The saved distros, read leniently: a record without a distro is skipped, one without an id
/// gets `wsl:<distro>`.
fn read_saved() -> Vec<ServerConfig> {
    let text = SAVED.get().unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    v["servers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let distro = s["distro"].as_str().filter(|d| !d.is_empty())?.to_owned();
            let id = s["id"].as_str().filter(|i| !i.is_empty()).map_or_else(|| server_id_for_distro(&distro), str::to_owned);
            Some(ServerConfig { id, distro })
        })
        .collect()
}
