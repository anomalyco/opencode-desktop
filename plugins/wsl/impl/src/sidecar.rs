//! A distro's OpenCode server (`sidecar.ts`): find its CLI, pick a free port and a password,
//! run `opencode serve` in the distro through `wsl.exe` (its script on stdin, the Windows PATH
//! left out), and poll `/api/info` until it answers. A stop aborts whatever stage the start is
//! in and reports once its process is gone, so a restart never leaves a server running.

use std::collections::VecDeque;
use std::time::Instant;

use opencode_plugin::prelude::*;

use crate::controller::{Core, with};
use crate::machine::{Attempt, Event};
use crate::runtime::{self, HEALTH_TIMEOUT_MS, TIMEOUT_MS};

/// One start's server.
pub struct Sidecar {
    distro: String,
    process: Option<Process>,
    url: String,
    password: String,
    ready: bool,
    started: Instant,
    /// The last dozen lines it wrote, for a failure's message.
    tail: VecDeque<String>,
    /// Stops waiting for it to be gone: each reports `ServerStopped` with its attempt.
    stops: Vec<Attempt>,
    /// Why the host killed it (its health check timed out).
    failure: Option<String>,
}

pub fn spawn(core: &mut Core, cx: &mut ActionCx, distro: String, attempt: Attempt) {
    opencode_plugin::system::log(false, &format!("[wsl] wsl sidecar starting {distro}"));
    core.sidecars.insert(attempt, Sidecar { distro: distro.clone(), process: None, url: String::new(), password: String::new(), ready: false, started: Instant::now(), tail: VecDeque::new(), stops: Vec::new(), failure: None });
    core.wsl(cx, runtime::sh(runtime::DISCOVER_SCRIPT, &distro), TIMEOUT_MS, move |core, cx, out| {
        if stopped_early(core, cx, attempt) {
            return;
        }
        let path = match out {
            Ok(o) => runtime::first_line(&o.stdout),
            Err(e) => return fail(core, cx, attempt, e),
        };
        let Some(path) = path else { return fail(core, cx, attempt, format!("OpenCode is not installed in {distro}")) };
        let Some(port) = free_port() else { return fail(core, cx, attempt, "Failed to get port".into()) };
        let password = password();
        let script = runtime::sidecar_script(&path, &password, port, !cfg!(debug_assertions));
        let spec = ProcessSpec::new(runtime::program()).args(runtime::in_distro(&["bash", "-se"], Some(&distro))).stdin(script);
        let me = core.weak();
        let process = cx.spawn_process::<()>(&spec, move |_, cx, event| {
            let Some(core) = me.upgrade() else { return };
            with(&core, cx, |core, cx| on_event(core, cx, attempt, event));
        });
        let Some(side) = core.sidecars.get_mut(&attempt) else { return };
        side.process = process;
        side.url = format!("http://127.0.0.1:{port}");
        side.password = password;
        if process.is_none() {
            return fail(core, cx, attempt, "the host refused to start wsl.exe".into());
        }
        check(core, cx, attempt, 0);
    });
}

/// Stops start `server`'s server; `ServerStopped` (with `attempt`) follows once it's gone.
pub fn stop(core: &mut Core, cx: &mut ActionCx, distro: String, attempt: Attempt, server: Attempt) {
    let Some(side) = core.sidecars.get_mut(&server) else { return core.dispatch(cx, Event::ServerStopped { distro, attempt }) };
    side.stops.push(attempt);
    if let Some(p) = side.process {
        cx.kill_process(p);
    }
}

/// A stop came while the start had no process yet: it's gone now.
fn stopped_early(core: &mut Core, cx: &mut ActionCx, attempt: Attempt) -> bool {
    if core.sidecars.get(&attempt).is_some_and(|s| s.stops.is_empty()) {
        return false;
    }
    let Some(side) = core.sidecars.remove(&attempt) else { return true };
    for stop in side.stops {
        core.dispatch(cx, Event::ServerStopped { distro: side.distro.clone(), attempt: stop });
    }
    true
}

fn fail(core: &mut Core, cx: &mut ActionCx, attempt: Attempt, message: String) {
    let Some(side) = core.sidecars.remove(&attempt) else { return };
    if !side.stops.is_empty() {
        for stop in side.stops {
            core.dispatch(cx, Event::ServerStopped { distro: side.distro.clone(), attempt: stop });
        }
        return;
    }
    core.dispatch(cx, Event::ServerFailed { distro: side.distro, attempt, message });
}

/// Polls `/api/info` until it answers, the start is stopped, or 30 s pass.
fn check(core: &mut Core, cx: &mut ActionCx, attempt: Attempt, delay: u64) {
    let Some(side) = core.sidecars.get(&attempt) else { return };
    let request = Fetch::get(format!("{}/api/info", side.url)).basic_auth("opencode", &side.password).timeout(3000).delay(delay);
    let me = core.weak();
    cx.fetch::<()>(&request, move |_, cx, answer| {
        let Some(core) = me.upgrade() else { return };
        with(&core, cx, |core, cx| {
            let Some(side) = core.sidecars.get_mut(&attempt) else { return };
            if !side.stops.is_empty() || side.process.is_none() || side.failure.is_some() {
                return;
            }
            if answer.ok() {
                side.ready = true;
                let (distro, url, password) = (side.distro.clone(), side.url.clone(), side.password.clone());
                return core.dispatch(cx, Event::ServerSpawned { distro, attempt, url, password });
            }
            if side.started.elapsed().as_millis() as u64 >= HEALTH_TIMEOUT_MS {
                side.failure = Some(format!("Sidecar for {} health check timed out after {HEALTH_TIMEOUT_MS}ms", side.distro));
                if let Some(p) = side.process {
                    cx.kill_process(p);
                }
                return;
            }
            check(core, cx, attempt, 100);
        });
    });
}

fn on_event(core: &mut Core, cx: &mut ActionCx, attempt: Attempt, event: ProcessEvent) {
    match event {
        ProcessEvent::Output { stream, text, .. } => {
            let Some(side) = core.sidecars.get_mut(&attempt) else { return };
            let tag = if stream == Stream::Stderr { "stderr" } else { "stdout" };
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                side.tail.push_back(format!("[{tag}] {line}"));
                if side.tail.len() > 12 {
                    side.tail.pop_front();
                }
                opencode_plugin::system::log(false, &format!("[wsl] wsl sidecar {} {tag}: {line}", side.distro));
            }
        }
        ProcessEvent::Exit { exit, .. } => {
            let Some(side) = core.sidecars.remove(&attempt) else { return };
            let code = exit.code.map_or_else(|| "null".to_owned(), |c| c.to_string());
            if !side.stops.is_empty() {
                for stop in side.stops {
                    core.dispatch(cx, Event::ServerStopped { distro: side.distro.clone(), attempt: stop });
                }
                return;
            }
            if side.ready {
                let message = format!("WSL server exited after startup (code={code} signal=null)");
                return core.dispatch(cx, Event::ServerExited { distro: side.distro, attempt, message });
            }
            let message = side.failure.unwrap_or_else(|| {
                let output = if side.tail.is_empty() { String::new() } else { format!("\n{}", side.tail.iter().cloned().collect::<Vec<_>>().join("\n")) };
                let output = match exit.error {
                    Some(e) => format!("{output}\n{e}"),
                    None => output,
                };
                format!("WSL server exited before becoming healthy (code={code} signal=null){output}")
            });
            core.dispatch(cx, Event::ServerFailed { distro: side.distro, attempt, message });
        }
    }
}

/// A port nothing listens on now, on the loopback address.
fn free_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0").ok()?.local_addr().ok().map(|a| a.port())
}

/// A random UUID-shaped password (`randomUUID`).
fn password() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 16];
    for (i, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(i);
        h.write_u128(Instant::now().elapsed().as_nanos());
        chunk.copy_from_slice(&h.finish().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}
