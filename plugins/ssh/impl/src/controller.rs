//! The SSH main side (`controller.ts`, `bootstrap.ts`): runs the machine's effects. A connect
//! parses the target, opens the sign-in listener, resolves where `ssh` goes (`ssh -G`), finds or
//! installs and starts the host's OpenCode service and reads its registration, forwards a free
//! local port to it through an `ssh -L` tunnel, and waits for `/api/info` to answer. Every step
//! is a process the host supervises; a stop kills them and closes the listener.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Instant;

use opencode_plugin::prelude::*;
use serde_json::{Value, json};

use crate::askpass::{Askpass, Note};
use crate::command::{self, Code, Failure, Target};
use crate::machine::{self, Attempt, Config, Effect, Event, Http};

pub type CoreRef = Rc<RefCell<Core>>;

/// The saved hosts (`servers`, as Desktop's main store).
const SAVED: Store<String> = Store::new("servers");
/// How long a started service has to answer, from the tunnel's start or the last prompt.
const READY_MS: u128 = 30_000;

/// One connect attempt's running parts.
struct Conn {
    id: String,
    askpass: Option<Askpass>,
    processes: Vec<Process>,
    tunnel: Option<Process>,
    tail: String,
    deadline: Instant,
}

pub struct Core {
    me: Weak<RefCell<Core>>,
    pub state: machine::State,
    pub revision: u64,
    conns: HashMap<Attempt, Conn>,
    retries: Vec<(String, Attempt, Instant)>,
    pub waker: Option<PluginWaker>,
}

impl Core {
    pub fn new() -> CoreRef {
        opencode_plugin::spawn_after(5_000, command::Log::sweep);
        let configs = read_saved();
        Rc::new_cyclic(|me| RefCell::new(Core { me: me.clone(), state: machine::create(configs), revision: 0, conns: HashMap::new(), retries: Vec::new(), waker: None }))
    }

    pub fn dispatch(&mut self, cx: &mut ActionCx, event: Event) {
        let before = self.state.clone();
        let effects = machine::transition(&mut self.state, event);
        if before != self.state {
            self.revision += 1;
            self.publish(cx);
            if let Some(w) = &self.waker {
                w.wake();
            }
        }
        for effect in effects {
            self.run(cx, effect);
        }
    }

    /// The saved hosts as the app lists them; each joins the app's servers once its tunnel is up.
    pub fn publish(&self, cx: &mut ActionCx) {
        let entries: Vec<ServerEntry> = self
            .state
            .servers
            .iter()
            .filter(|r| r.saved.is_some())
            .map(|r| {
                let stage = r.stage();
                let state = match stage {
                    machine::Stage::Connecting => ServerState::Starting,
                    machine::Stage::Authentication => ServerState::Auth,
                    machine::Stage::Ready => ServerState::Ready,
                    machine::Stage::Failed => ServerState::Failed,
                    machine::Stage::Incompatible => ServerState::Incompatible,
                    machine::Stage::Disconnected => ServerState::Stopped,
                };
                let http = r.http().cloned().unwrap_or(Http { url: String::new(), password: String::new() });
                ServerEntry { id: r.config.id.clone(), name: crate::name(&r.config), label: "SSH".into(), state, url: http.url, password: http.password, listed: state == ServerState::Ready }
            })
            .collect();
        cx.set_servers(crate::SOURCE, &entries, true);
    }

    /// Restores the saved hosts in the background, once at launch.
    pub fn restore(&mut self, cx: &mut ActionCx) {
        let saved: Vec<Config> = self.state.servers.iter().filter(|r| r.saved.is_some() && r.stage() == machine::Stage::Disconnected).map(|r| r.config.clone()).collect();
        for config in saved {
            self.dispatch(cx, Event::Start { config, replace: false, background: true });
        }
    }

    /// Runs the retries that came due; the next one's delay, if any.
    pub fn due(&mut self, cx: &mut ActionCx) -> Option<u64> {
        let now = Instant::now();
        let (due, later): (Vec<_>, Vec<_>) = self.retries.drain(..).partition(|r| r.2 <= now);
        self.retries = later;
        for (id, attempt, _) in due {
            self.dispatch(cx, Event::RetryDue { id, attempt });
        }
        self.retries.iter().map(|r| r.2.saturating_duration_since(now).as_millis() as u64).min()
    }

    /// Hands the sign-in listeners' prompts to the machine.
    pub fn notes(&mut self, cx: &mut ActionCx) {
        let mut all = Vec::new();
        for (attempt, conn) in &self.conns {
            if let Some(a) = &conn.askpass {
                all.extend(a.take_notes().into_iter().map(|n| (*attempt, conn.id.clone(), n)));
            }
        }
        for (attempt, id, note) in all {
            match note {
                Note::Prompt(prompt) => {
                    if let Some(c) = self.conns.get_mut(&attempt) {
                        c.deadline = Instant::now();
                    }
                    self.dispatch(cx, Event::PromptReceived { id, attempt, prompt });
                }
                Note::Cleared(prompt) => {
                    if let Some(c) = self.conns.get_mut(&attempt) {
                        c.deadline = Instant::now();
                    }
                    self.dispatch(cx, Event::PromptCleared { id, attempt, prompt });
                }
            }
        }
    }

    fn run(&mut self, cx: &mut ActionCx, effect: Effect) {
        match effect {
            Effect::Save(configs) => {
                let list: Vec<Value> = configs.iter().map(|c| json!({ "id": c.id, "target": c.target, "name": c.name })).collect();
                SAVED.set(&Value::Array(list).to_string());
            }
            Effect::AttemptStop(attempt) => {
                self.retries.retain(|r| r.1 != attempt);
                if let Some(conn) = self.conns.remove(&attempt) {
                    for p in conn.processes.iter().chain(conn.tunnel.iter()) {
                        cx.kill_process(*p);
                    }
                }
            }
            Effect::AttemptRespond { attempt, prompt, value } => {
                if let Some(a) = self.conns.get(&attempt).and_then(|c| c.askpass.as_ref()) {
                    a.respond(&prompt, &value);
                }
            }
            Effect::RetrySchedule { id, attempt } => {
                self.retries.push((id, attempt, Instant::now() + std::time::Duration::from_millis(machine::RETRY_DELAY_MS)));
                cx.wake_after(machine::RETRY_DELAY_MS);
            }
            Effect::AttemptStart { id, attempt, config, replace } => self.connect(cx, id, attempt, config, replace),
        }
    }

    fn fail(&mut self, cx: &mut ActionCx, attempt: Attempt, failure: Failure) {
        let Some(conn) = self.conns.remove(&attempt) else { return };
        for p in conn.processes.iter().chain(conn.tunnel.iter()) {
            cx.kill_process(*p);
        }
        opencode_plugin::system::log(true, &format!("[ssh] connect failed ({:?}): {}", failure.code, failure.detail.trim()));
        self.dispatch(cx, Event::Failed { id: conn.id, attempt, code: failure.code, detail: failure.detail });
    }

    fn connect(&mut self, cx: &mut ActionCx, id: String, attempt: Attempt, config: Config, replace: bool) {
        let askpass = self.waker.clone().and_then(Askpass::start);
        self.conns.insert(attempt, Conn { id: id.clone(), askpass, processes: Vec::new(), tunnel: None, tail: String::new(), deadline: Instant::now() });
        let target = match command::parse_target(&config.target) {
            Ok(t) => t,
            Err(f) => return self.fail(cx, attempt, f),
        };
        let mut args = command::ssh_args(&target);
        args.extend(["-G".into(), target.host.clone()]);
        self.ssh(cx, attempt, args, "", 10_000, move |core, cx, out| {
            if let Ok(text) = out {
                let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k} ")).map(str::to_owned));
                if let Some(host) = field("hostname") {
                    let destination = format!("{}@{host}:{}", field("user").unwrap_or_default(), field("port").unwrap_or_else(|| "22".into()));
                    core.dispatch(cx, Event::DestinationResolved { id: id.clone(), attempt, destination });
                }
            }
            core.bootstrap(cx, attempt, target, replace);
        });
    }

    /// Runs `ssh args` (with `stdin`) for attempt `attempt`, then `done` with its stdout or why it
    /// failed. The attempt's sign-in listener answers its prompts.
    fn ssh(&mut self, cx: &mut ActionCx, attempt: Attempt, args: Vec<String>, stdin: &str, timeout: u64, done: impl FnOnce(&mut Core, &mut ActionCx, Result<String, Failure>) + 'static) {
        let Some(conn) = self.conns.get_mut(&attempt) else { return };
        let log = command::Log::new();
        let mut spec = ProcessSpec::new(command::ssh()).args(log.args()).args(args).stdin(stdin).timeout(timeout);
        for (k, v) in conn.askpass.as_ref().map(Askpass::env).unwrap_or_default() {
            spec = spec.env(k, v);
        }
        let me = self.me.clone();
        let process = cx.run_process::<()>(&spec, move |_, cx, mut out| {
            let Some(core) = me.upgrade() else { return };
            out.stderr = [log.take(), std::mem::take(&mut out.stderr)].concat();
            with(&core, cx, |core, cx| {
                // A stopped attempt's steps end unheard.
                if !core.conns.contains_key(&attempt) {
                    return;
                }
                let result = if let Some(e) = out.exit.error.clone() {
                    Err(Failure::new(Code::SshMissing, e))
                } else if out.exit.timed_out {
                    Err(Failure::new(Code::Connection, "timed out"))
                } else if out.ok() {
                    Ok(out.stdout)
                } else {
                    Err(Failure::new(Code::Connection, command::failure_detail(out.exit.code, &out.stdout, &out.stderr)))
                };
                done(core, cx, result);
            });
        });
        if let Some(p) = process {
            conn.processes.push(p);
        }
    }

    /// `bootstrap.ts`: the host's registered service, or the CLI installed and its service started.
    fn bootstrap(&mut self, cx: &mut ActionCx, attempt: Attempt, target: Target, replace: bool) {
        let version = opencode_plugin::system::server_version().unwrap_or_default();
        let development = cfg!(debug_assertions);
        let run_args = { let mut a = command::ssh_args(&target); a.extend([target.host.clone(), "sh -l -s".into()]); a };
        let args = run_args.clone();
        self.ssh(cx, attempt, args, &command::discover_script(), 600_000, move |core, cx, out| {
            let out = match out {
                Ok(o) => o,
                Err(f) => return core.fail(cx, attempt, f),
            };
            if let Some(r) = command::parse_registration(&out) {
                if development || r.version == version {
                    return core.tunnel(cx, attempt, target, r.url, r.password);
                }
                if !replace {
                    return core.fail(cx, attempt, Failure::new(Code::Version, r.version));
                }
            }
            if !command::valid_version(&version) {
                return core.fail(cx, attempt, Failure::new(Code::Version, version));
            }
            let check = command::version_script(&format!("\"{}\"", command::binary_path(&version)));
            let (args, version2) = (run_args.clone(), version.clone());
            core.ssh(cx, attempt, run_args.clone(), &check, 600_000, move |core, cx, out| {
                let staged = out.ok().and_then(|o| command::parse_version(&o)).as_deref() == Some(version2.as_str());
                let version4 = version2.clone();
                let start = move |core: &mut Core, cx: &mut ActionCx| {
                    let (args3, target3, version3) = (args.clone(), target.clone(), version2.clone());
                    core.ssh(cx, attempt, args3, &command::start_script(&version3, replace), 600_000, move |core, cx, out| {
                        let registration = match out {
                            Ok(o) => command::parse_registration(&o),
                            Err(f) => return core.fail(cx, attempt, f),
                        };
                        match registration {
                            None => core.fail(cx, attempt, Failure::new(Code::Service, "")),
                            Some(r) if !development && r.version != version3 => core.fail(cx, attempt, Failure::new(Code::Version, r.version)),
                            Some(r) => core.tunnel(cx, attempt, target3, r.url, r.password),
                        }
                    });
                };
                if staged {
                    return start(core, cx);
                }
                let args4 = run_args.clone();
                core.ssh(cx, attempt, run_args.clone(), command::PROBE_SCRIPT, 600_000, move |core, cx, out| {
                    let platform = out.ok().and_then(|o| o.lines().rev().find_map(|l| l.strip_prefix("OPENCODE_REMOTE_TARGET=").map(str::to_owned)));
                    let Some(url) = platform.as_deref().and_then(|p| command::archive_url(p, &version4)) else {
                        return core.fail(cx, attempt, Failure::new(Code::Platform, platform.unwrap_or_default()));
                    };
                    let checked = url.clone();
                    core.ssh(cx, attempt, args4, &command::install_script(&version4, &url), 600_000, move |core, cx, out| match out {
                        Ok(_) => start(core, cx),
                        // The host couldn't fetch it: a version never published reads as such.
                        Err(f) => {
                            let me = core.me.clone();
                            cx.fetch::<()>(&Fetch::get(checked).method("HEAD").timeout(30_000), move |_, cx, answer| {
                                let Some(core) = me.upgrade() else { return };
                                let code = if answer.status == 404 { Code::Unpublished } else { Code::Install };
                                with(&core, cx, |core, cx| core.fail(cx, attempt, Failure::new(code, f.detail)));
                            });
                        }
                    });
                });
            });
        });
    }

    /// Forwards a free local port to the service and waits for it to answer.
    fn tunnel(&mut self, cx: &mut ActionCx, attempt: Attempt, target: Target, url: String, password: String) {
        let Some((host, port)) = command::remote_address(&url) else { return self.fail(cx, attempt, Failure::new(Code::Service, "")) };
        let Some(local) = std::net::TcpListener::bind("127.0.0.1:0").ok().and_then(|l| l.local_addr().ok()).map(|a| a.port()) else {
            return self.fail(cx, attempt, Failure::new(Code::Connection, ""));
        };
        let Some(conn) = self.conns.get_mut(&attempt) else { return };
        let log = command::Log::new();
        let mut spec = ProcessSpec::new(command::ssh()).args(log.args()).args(command::tunnel_args(&target, local, &host, port)).keep_stdin();
        for (k, v) in conn.askpass.as_ref().map(Askpass::env).unwrap_or_default() {
            spec = spec.env(k, v);
        }
        let me = self.me.clone();
        conn.tunnel = cx.spawn_process::<()>(&spec, move |_, cx, event| {
            let Some(core) = me.upgrade() else { return };
            with(&core, cx, |core, cx| match event {
                ProcessEvent::Output { stream: Stream::Stderr, text, .. } => {
                    if let Some(c) = core.conns.get_mut(&attempt) {
                        c.tail.push_str(text);
                        let mut cut = c.tail.len().saturating_sub(8192);
                        while !c.tail.is_char_boundary(cut) {
                            cut += 1;
                        }
                        c.tail.drain(..cut);
                    }
                }
                ProcessEvent::Output { .. } => {}
                // The tunnel closing fails the attempt, which the machine reads as a ready server
                // that dropped.
                ProcessEvent::Exit { exit, .. } => {
                    let logged = log.take();
                    let detail = core.conns.get(&attempt).map(|c| format!("{logged}{}", c.tail).trim().to_owned()).filter(|t| !t.is_empty());
                    let detail = detail.unwrap_or_else(|| format!("{{\"exitCode\":{}}}", exit.code.map_or("null".into(), |c| c.to_string())));
                    core.fail(cx, attempt, Failure::new(Code::Connection, detail));
                }
            });
        });
        conn.deadline = Instant::now();
        let http = Http { url: format!("http://127.0.0.1:{local}"), password };
        self.health(cx, attempt, http, 0);
    }

    fn health(&mut self, cx: &mut ActionCx, attempt: Attempt, http: Http, delay: u64) {
        let request = Fetch::get(format!("{}/api/info", http.url)).basic_auth("opencode", &http.password).timeout(2000).delay(delay);
        let me = self.me.clone();
        cx.fetch::<()>(&request, move |_, cx, answer| {
            let Some(core) = me.upgrade() else { return };
            with(&core, cx, |core, cx| {
                let Some(conn) = core.conns.get(&attempt) else { return };
                if answer.ok() {
                    let id = conn.id.clone();
                    // Prompts only come while signing in: the listener closes once the tunnel is up.
                    if let Some(c) = core.conns.get_mut(&attempt) {
                        c.askpass = None;
                    }
                    return core.dispatch(cx, Event::Ready { id, attempt, http });
                }
                let authenticating = core.state.servers.iter().any(|r| matches!(&r.phase, machine::Phase::Authenticating { attempt: a, .. } if *a == attempt));
                if !authenticating && conn.deadline.elapsed().as_millis() >= READY_MS {
                    let detail = conn.tail.clone();
                    return core.fail(cx, attempt, Failure::new(Code::Service, detail));
                }
                core.health(cx, attempt, http, 100);
            });
        });
    }
}

/// Runs `f` on the controller.
pub fn with<R>(core: &CoreRef, cx: &mut ActionCx, f: impl FnOnce(&mut Core, &mut ActionCx) -> R) -> R {
    f(&mut core.borrow_mut(), cx)
}

fn read_saved() -> Vec<Config> {
    let v: Value = serde_json::from_str(&SAVED.get().unwrap_or_default()).unwrap_or(Value::Null);
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let s = |k: &str| c[k].as_str().map(str::to_owned);
            Some(Config { id: s("id")?, target: s("target")?, name: s("name").unwrap_or_default() })
        })
        .collect()
}
