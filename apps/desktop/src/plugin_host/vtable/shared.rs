//! What any callback shares through the host: contracts and services, stores, icons, background
//! jobs, server sources and requests to the app.

use std::ffi::c_void;
use std::time::Duration;

use opencode_plugin::abi::*;

use super::super::instance::ServerRec;
use super::super::shared::Shared;
use super::super::{icons, tasks};
use super::{ActionCtx, Kind, Request, SetupCtx, cast, text};

/// The shared state and plugin id whichever context `ctx` is can read.
///
/// # Safety
/// As `cast`.
unsafe fn reader<'a>(ctx: *mut c_void) -> Option<(&'a Shared, &'a str)> {
    // SAFETY: as `cast`.
    unsafe { super::read(ctx) }
}

/// The shared state, plugin id, library and request queue of a setup or handler context.
///
/// # Safety
/// As `cast`.
unsafe fn writer<'a>(ctx: *mut c_void) -> Option<(&'a mut Shared, &'a str, &'a std::sync::Arc<super::LoadedLibrary>, &'a mut Vec<Request>)> {
    // SAFETY: as `cast`.
    unsafe {
        if let Some(a) = cast::<ActionCtx>(ctx, Kind::Action) {
            return Some((&mut *a.shared, a.plugin_id, a.lib, &mut a.requests));
        }
        cast::<SetupCtx>(ctx, Kind::Setup).map(|s| (&mut *s.shared, s.plugin_id, s.lib, &mut s.requests))
    }
}

pub(super) unsafe extern "C" fn provide(ctx: *mut c_void, id: AbiStr, type_hash: u64, value: usize, drop_value: Option<DropFn>) -> AbiStatus {
    // SAFETY: the plugin passes its setup context.
    let Some(s) = (unsafe { cast::<SetupCtx>(ctx, Kind::Setup) }) else { return AbiStatus::Err };
    match s.shared.contracts.provide(s.slot, s.plugin_id, s.generation, text(id), type_hash, value, drop_value, s.lib.clone()) {
        Ok(wake) => {
            s.woken |= wake;
            AbiStatus::Ok
        }
        Err(e) => {
            tasks::log(format!("plugin {}: {e}", s.plugin_id));
            AbiStatus::Err
        }
    }
}

pub(super) unsafe extern "C" fn consume(ctx: *mut c_void, id: AbiStr, type_hash: u64) -> u32 {
    // SAFETY: the plugin passes its setup context.
    let Some(s) = (unsafe { cast::<SetupCtx>(ctx, Kind::Setup) }) else { return u32::MAX };
    s.shared.contracts.consume(s.slot, text(id), type_hash)
}

pub(super) unsafe extern "C" fn live(ctx: *mut c_void, handle: u32, out: *mut LiveAbi) {
    // SAFETY: the plugin passes its context and a valid out pointer.
    if let (Some((shared, _)), Some(out)) = (unsafe { reader(ctx) }, unsafe { out.as_mut() }) {
        *out = shared.contracts.live(handle);
    }
}

pub(super) unsafe extern "C" fn intern_icon(ctx: *mut c_void, spec: *const IconSpecAbi) -> u32 {
    // SAFETY: the plugin passes its context and a spec valid for the call.
    let (Some((_, plugin)), Some(spec)) = (unsafe { reader(ctx) }, unsafe { spec.as_ref() }) else { return 0 };
    icons::intern_icon(plugin, text(spec.name), text(spec.body), spec.view)
}

pub(super) unsafe extern "C" fn image(ctx: *mut c_void, bytes: AbiStr, width: u32, height: u32) -> u32 {
    // SAFETY: the plugin passes its context and bytes valid for the call.
    let Some((shared, plugin)) = (unsafe { reader(ctx) }) else { return 0 };
    // SAFETY: as above.
    let bytes = unsafe { bytes.as_bytes() };
    shared.images.borrow_mut().add(plugin, bytes, width, height)
}

pub(super) unsafe extern "C" fn image_drop(ctx: *mut c_void, id: u32) {
    // SAFETY: the plugin passes its context.
    if let Some((shared, plugin)) = unsafe { reader(ctx) } {
        shared.images.borrow_mut().remove(plugin, id);
    }
}

pub(super) unsafe extern "C" fn store_get(ctx: *mut c_void, name: AbiStr, session: AbiStr, out: *mut AbiStr<'static>) -> u8 {
    // SAFETY: the plugin passes its context and a valid out pointer.
    let (Some((shared, plugin)), Some(out)) = (unsafe { reader(ctx) }, unsafe { out.as_mut() }) else { return 0 };
    let Some(value) = shared.stores.get(plugin, text(name), text(session)) else { return 0 };
    // The view lives until the store next changes, which no call the plugin makes before
    // copying it can do.
    *out = AbiStr { ptr: value.as_ptr(), len: value.len(), _lt: std::marker::PhantomData };
    1
}

pub(super) unsafe extern "C" fn store_set(ctx: *mut c_void, name: AbiStr, session: AbiStr, value: AbiStr) -> AbiStatus {
    // SAFETY: the plugin passes its setup or handler context.
    let Some((shared, plugin, _, _)) = (unsafe { writer(ctx) }) else { return AbiStatus::Err };
    shared.stores.set(plugin, text(name), text(session), text(value));
    AbiStatus::Ok
}

pub(super) unsafe extern "C" fn spawn(ctx: *mut c_void, job: Callback, delay_ms: u64) -> AbiStatus {
    // SAFETY: the plugin passes its setup or handler context.
    let Some((_, _, lib, _)) = (unsafe { writer(ctx) }) else { return AbiStatus::Err };
    if job.func.is_none() {
        return AbiStatus::Err;
    }
    tasks::spawn(job, Duration::from_millis(delay_ms), lib.clone());
    AbiStatus::Ok
}

pub(super) unsafe extern "C" fn set_servers(ctx: *mut c_void, source: AbiStr, entries: *const ServerEntryAbi, len: usize, flags: u32) {
    // SAFETY: the plugin passes its setup or handler context.
    let Some((_, _, _, requests)) = (unsafe { writer(ctx) }) else { return };
    let entries = if entries.is_null() || len == 0 {
        &[][..]
    } else {
        // SAFETY: the plugin passes `len` entries valid for the call.
        unsafe { std::slice::from_raw_parts(entries, len) }
    };
    let source = text(source).to_owned();
    let list = entries
        .iter()
        .map(|e| ServerRec {
            source: source.clone(),
            id: text(e.id).to_owned(),
            name: text(e.name).to_owned(),
            label: text(e.label).to_owned(),
            state: e.state,
            url: text(e.url).to_owned(),
            password: text(e.password).to_owned(),
            listed: e.flags & SERVER_LISTED != 0,
        })
        .collect();
    requests.push(Request::SetServers { source, list, ready: flags & SOURCE_READY != 0 });
}

pub(super) unsafe extern "C" fn request(ctx: *mut c_void, kind: u32, arg: AbiStr) {
    // SAFETY: the plugin passes its handler context.
    let Some(a) = (unsafe { cast::<ActionCtx>(ctx, Kind::Action) }) else { return };
    let arg = text(arg).to_owned();
    a.requests.push(match kind {
        REQUEST_RUN_COMMAND => Request::RunCommand(arg),
        REQUEST_OPEN_LINK => Request::OpenLink(arg),
        REQUEST_OPEN_SETTINGS => Request::OpenSettings(arg),
        REQUEST_OPEN_DIALOG => Request::OpenDialog(arg),
        REQUEST_CLOSE_DIALOG => Request::CloseDialog,
        REQUEST_CONNECT_SERVER => Request::ConnectServer(arg),
        REQUEST_OPEN_PANEL_INSTANCE => {
            let (panel, instance) = arg.split_once('\n').unwrap_or((&arg, ""));
            Request::OpenPanelInstance { panel: panel.to_owned(), instance: instance.to_owned() }
        }
        REQUEST_OPEN_EXTERNAL => Request::OpenExternal(arg),
        REQUEST_REVEAL => Request::Reveal(arg),
        REQUEST_TOAST_REVEAL => Request::ToastReveal(arg),
        REQUEST_DEV => Request::Dev(arg),
        REQUEST_TOGGLE_PANEL => Request::TogglePanel(arg),
        REQUEST_TOGGLE_POPOVER => {
            let (id, anchor) = arg.split_once('\n').unwrap_or((&arg, ""));
            Request::TogglePopover { id: id.to_owned(), anchor: anchor.to_owned() }
        }
        REQUEST_CLOSE_POPOVER => Request::ClosePopover,
        REQUEST_TOAST_ERROR => Request::ToastError(arg),
        REQUEST_INVALIDATE => Request::Invalidate(arg),
        REQUEST_NEW_DRAFT => {
            let (directory, prompt) = arg.split_once('\n').unwrap_or((&arg, ""));
            Request::NewDraft { directory: directory.to_owned(), prompt: prompt.to_owned() }
        }
        _ => return,
    });
}