//! What a plugin asks of the system: processes the host supervises, HTTP requests, Desktop's
//! menus, and the app's own facts (`TEXT_*`).

use std::ffi::c_void;
use std::sync::Arc;
use std::time::Duration;

use opencode_plugin::abi::*;

use super::super::instance::{CallbackEnv, Scope};
use super::super::loader::LoadedLibrary;
use super::super::process::Spec;
use super::super::shared::Shared;
use super::{ActionCtx, Kind, Request, SetupCtx, WakeShared, cast, text};

/// What starting a process needs of the context: its plugin's shared state, slot, generation,
/// half, library and waker.
struct Spawner<'a> {
    shared: &'a mut Shared,
    slot: usize,
    generation: u32,
    scope: Scope,
    lib: &'a Arc<LoadedLibrary>,
    wake: &'a Arc<WakeShared>,
}

/// # Safety
/// As `cast`.
unsafe fn spawner<'a>(ctx: *mut c_void) -> Option<Spawner<'a>> {
    // SAFETY: as `cast`.
    unsafe {
        if let Some(a) = cast::<ActionCtx>(ctx, Kind::Action) {
            return Some(Spawner { shared: &mut *a.shared, slot: a.slot, generation: a.generation, scope: a.scope, lib: a.lib, wake: a.wake });
        }
        cast::<SetupCtx>(ctx, Kind::Setup).map(|s| Spawner { shared: &mut *s.shared, slot: s.slot, generation: s.generation, scope: s.scope, lib: s.lib, wake: s.wake })
    }
}

fn split(list: &str) -> Vec<String> {
    list.split('\0').filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

pub(super) unsafe extern "C" fn process_spawn(ctx: *mut c_void, spec: *const ProcessSpecAbi, events: Callback) -> u32 {
    // SAFETY: the plugin passes its setup or handler context and a spec valid for the call.
    let (Some(s), Some(spec)) = (unsafe { spawner(ctx) }, unsafe { spec.as_ref() }) else { return 0 };
    if events.func.is_none() || text(spec.program).is_empty() {
        return 0;
    }
    let env = split(text(spec.env)).into_iter().filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned()))).collect();
    let spec = Spec {
        program: text(spec.program).to_owned(),
        args: text(spec.args).split('\0').map(str::to_owned).filter(|_| !spec.args.is_empty()).collect(),
        env,
        cwd: text(spec.cwd).to_owned(),
        // SAFETY: the plugin passes bytes valid for the call.
        stdin: unsafe { spec.stdin.as_bytes() }.to_vec(),
        keep_stdin: spec.flags & PROCESS_KEEP_STDIN != 0,
        timeout: (spec.timeout_ms > 0).then(|| Duration::from_millis(spec.timeout_ms)),
    };
    let events = CallbackEnv::new(events, s.lib.clone());
    s.shared.processes.spawn(s.slot, s.generation, s.scope, spec, events, s.wake)
}

pub(super) unsafe extern "C" fn process_write(ctx: *mut c_void, handle: u32, bytes: AbiStr, close: u8) -> AbiStatus {
    // SAFETY: the plugin passes its setup or handler context.
    let Some(s) = (unsafe { spawner(ctx) }) else { return AbiStatus::Err };
    // SAFETY: the plugin passes bytes valid for the call.
    if s.shared.processes.write(s.slot, handle, unsafe { bytes.as_bytes() }, close != 0) { AbiStatus::Ok } else { AbiStatus::Err }
}

pub(super) unsafe extern "C" fn process_kill(ctx: *mut c_void, handle: u32) -> AbiStatus {
    // SAFETY: the plugin passes its setup or handler context.
    let Some(s) = (unsafe { spawner(ctx) }) else { return AbiStatus::Err };
    s.shared.processes.kill(s.slot, handle);
    AbiStatus::Ok
}

#[allow(clippy::too_many_arguments)]
pub(super) unsafe extern "C" fn fetch(ctx: *mut c_void, method: AbiStr, url: AbiStr, headers: AbiStr, body: AbiStr, timeout_ms: u64, delay_ms: u64, done: Callback) -> AbiStatus {
    // SAFETY: the plugin passes its handler context.
    let Some(a) = (unsafe { cast::<ActionCtx>(ctx, Kind::Action) }) else { return AbiStatus::Err };
    if done.func.is_none() || text(url).is_empty() {
        return AbiStatus::Err;
    }
    let headers = text(headers).lines().filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))).collect();
    let request = super::super::requests::FetchRequest {
        method: text(method).to_ascii_uppercase(),
        url: text(url).to_owned(),
        headers,
        body: text(body).to_owned(),
        timeout: Duration::from_millis(if timeout_ms == 0 { 15_000 } else { timeout_ms }),
        delay: Duration::from_millis(delay_ms),
    };
    let done = CallbackEnv::new(done, a.lib.clone());
    a.requests.push(Request::Fetch { request, done, scope: a.scope });
    AbiStatus::Ok
}

pub(super) unsafe extern "C" fn host_text(ctx: *mut c_void, what: u32, arg: AbiStr, out: *mut AbiOwnedBytes) -> u8 {
    // SAFETY: the plugin passes a valid out pointer holding nothing it still needs.
    let Some(out) = (unsafe { out.as_mut() }) else { return 0 };
    let answer = match what {
        TEXT_ARG => crate::arg(&format!("--{}", text(arg).trim_start_matches('-'))),
        TEXT_EXE => std::env::current_exe().ok().map(|p| p.display().to_string()),
        TEXT_CHANNEL => Some(crate::updater::build_channel().to_owned()),
        TEXT_LOGS_DIR => crate::dirs::motion_log().and_then(|f| f.parent().map(|d| d.display().to_string())),
        TEXT_DOWNLOADS_DIR => crate::dirs::downloads().map(|d| d.display().to_string()),
        TEXT_MEMORY => Some(super::super::memory::report()),
        TEXT_ACTIVE_SESSION => {
            // SAFETY: the plugin passes its context.
            if let Some(r) = unsafe { super::render(ctx) } {
                super::services::active_session(r.app)
            } else {
                // SAFETY: as above.
                unsafe { super::cast::<super::ActionCtx>(ctx, super::Kind::Action) }.and_then(|a| a.view.session.clone())
            }
        }
        TEXT_EXPERIMENT | TEXT_DEV => {
            // SAFETY: the plugin passes its context.
            let Some((shared, _)) = (unsafe { reader(ctx) }) else { return 0 };
            let facts = if what == TEXT_DEV { &shared.facts.dev } else { &shared.facts.experiments };
            facts.get(text(arg)).cloned()
        }
        TEXT_SERVER_VERSION | TEXT_SERVER_HEALTH => {
            // SAFETY: the plugin passes its context.
            let Some((shared, plugin)) = (unsafe { reader(ctx) }) else { return 0 };
            match what {
                TEXT_SERVER_VERSION => shared.facts.server_version.clone(),
                _ => shared.facts.health.get(&format!("{plugin}:{}", text(arg))).cloned(),
            }
        }
        _ => None,
    };
    match answer {
        Some(v) => {
            *out = AbiOwnedBytes::from_string(v);
            1
        }
        None => 0,
    }
}

/// # Safety
/// As `cast`.
unsafe fn reader<'a>(ctx: *mut c_void) -> Option<(&'a Shared, &'a str)> {
    // SAFETY: as `cast`.
    unsafe { super::read(ctx) }
}

pub(super) unsafe extern "C" fn open_menu(ctx: *mut c_void, anchor: AbiStr, spec: AbiStr, done: Callback) -> AbiStatus {
    // SAFETY: the plugin passes its handler context.
    let Some(a) = (unsafe { cast::<ActionCtx>(ctx, Kind::Action) }) else { return AbiStatus::Err };
    if done.func.is_none() || text(anchor).is_empty() {
        return AbiStatus::Err;
    }
    let done = CallbackEnv::new(done, a.lib.clone());
    a.requests.push(Request::OpenMenu { anchor: text(anchor).to_owned(), spec: text(spec).to_owned(), done, scope: a.scope });
    AbiStatus::Ok
}
