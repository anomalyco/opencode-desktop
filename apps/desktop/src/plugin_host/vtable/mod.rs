//! The host side of the ABI: `HostApiVtable`, the host heap a plugin allocates on, and the
//! per-call contexts plugins pass back.
//!
//! Every context starts with a [`Kind`] tag; a host function checks it and does nothing when
//! called from the wrong kind of callback (a render asking to open a panel, say).
//!
//! - this file: the contexts, wakers, editors and building elements;
//! - `setup.rs`: what a setup registers (commands, panels, settings, every other contribution);
//! - `shared.rs`: what any callback shares (contracts, stores, icons, jobs, servers, requests).

mod services;
mod setup;
mod shared;
mod system;

pub use services::{HostView, Resources, panel_state};


use std::alloc::Layout;
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use opencode_plugin::abi::*;
use opencode_plugin::{EditorHandle, ElHandle};
use rune::Tx;

use super::arena::{self, Build, PluginRenderArena};
use super::editors::PluginEditors;
use super::instance::{Contributions, Scope, ServerRec};
use super::loader::LoadedLibrary;
use super::shared::Shared;

#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Setup = 0x5E70,
    Render = 0x8E4D,
    Action = 0xAC70,
    /// A query where the app is only read (listing a panel's tabs): stores, facts and icons.
    Read = 0x4EAD,
}

#[repr(C)]
pub struct ReadCtx<'a> {
    pub kind: Kind,
    pub plugin_id: &'a str,
    pub shared: &'a Shared,
}

#[repr(C)]
pub struct SetupCtx<'a> {
    pub kind: Kind,
    pub scope: Scope,
    pub plugin_id: &'a str,
    pub slot: usize,
    pub generation: u32,
    pub lib: &'a Arc<LoadedLibrary>,
    pub editors: &'a mut PluginEditors,
    /// The previous generation's editors, handed out again in creation order.
    pub reuse: &'a [EditorHandle],
    pub made: Contributions,
    pub wake: &'a Arc<WakeShared>,
    pub shared: &'a mut Shared,
    /// Consumers a publish woke, woken once setup returns.
    pub woken: u64,
    pub requests: Vec<Request>,
    /// What the app half provided: its half owns the values, every window lists them.
    pub app_services: Vec<super::app_half::Service>,
}

#[repr(C)]
pub struct RenderCtx<'a> {
    pub kind: Kind,
    pub pal: AbiPalette,
    pub build: Build<'a>,
    pub arena: &'a RefCell<PluginRenderArena>,
    /// `p:<plugin>:`: every memo this plugin makes lives under it.
    pub prefix: &'a str,
    /// Mixed into every memo version: the generation, the theme, and a count of render panics.
    pub salt: u64,
    pub panicked: RefCell<Option<String>>,
    pub shared: &'a Shared,
    /// What the render may read of the app: its sessions, the model catalog, the side region.
    pub app: &'a crate::App,
}

#[repr(C)]
pub struct ActionCtx<'a> {
    pub kind: Kind,
    /// The half the callback runs on, which the processes it starts report to.
    pub scope: Scope,
    pub slot: usize,
    pub generation: u32,
    pub wake: &'a Arc<WakeShared>,
    pub plugin_id: &'a str,
    pub lib: &'a Arc<LoadedLibrary>,
    pub editors: &'a mut PluginEditors,
    pub shared: &'a mut Shared,
    pub tx: &'a mut Tx,
    pub requests: Vec<Request>,
    pub wake_after: Option<u64>,
    /// What a handler may ask of the app's state, read before it ran.
    pub view: HostView,
}

/// What a handler asked for that needs the whole app, carried out once it returns.
pub enum Request {
    OpenPanel(String),
    Toast(String),
    ToastError(String),
    RunCommand(String),
    OpenLink(String),
    /// Settings at a host page (`servers`), or its first page.
    OpenSettings(String),
    SetServers { source: String, list: Vec<ServerRec>, ready: bool },
    TogglePanel(String),
    TogglePopover { id: String, anchor: String },
    ClosePopover,
    Invalidate(String),
    NewDraft { directory: String, prompt: String },
    ServerRequest { method: String, path: String, body: String, done: super::instance::CallbackEnv, scope: Scope },
    SaveFile { name: String, content: String, done: super::instance::CallbackEnv },
    Fetch { request: super::requests::FetchRequest, done: super::instance::CallbackEnv, scope: Scope },
    OpenMenu { anchor: String, spec: String, done: super::instance::CallbackEnv, scope: Scope },
    OpenDialog(String),
    CloseDialog,
    ConnectServer(String),
    OpenPanelInstance { panel: String, instance: String },
    OpenExternal(String),
    Reveal(String),
    /// A toast that stays, with a button that reveals a path: title, description, button, path.
    ToastReveal(String),
    /// A developer tool (`REQUEST_DEV`).
    Dev(String),
}

/// Wakes the event loop for plugins: one bit per slot in `dirty`, which `PluginHost::poll`
/// swaps once a frame; at rest it reads zero and calls nothing.
#[derive(Default)]
pub struct WakeShared {
    pub dirty: AtomicU64,
    /// Slots whose library changed on disk and is loaded, ready to swap in (hot reload).
    pub hot: AtomicU64,
    pub waker: OnceLock<rune::Waker>,
}

impl WakeShared {
    pub fn wake(&self, bits: u64, hot: bool) {
        if bits == 0 {
            return;
        }
        if hot {
            self.hot.fetch_or(bits, Ordering::AcqRel);
        } else {
            self.dirty.fetch_or(bits, Ordering::AcqRel);
        }
        if let Some(w) = self.waker.get() {
            w();
        }
    }
}

pub fn slot_bit(slot: usize) -> u64 {
    1 << slot.min(63)
}

struct SlotWaker {
    shared: Arc<WakeShared>,
    bit: u64,
}

unsafe extern "C" fn waker_wake(data: *const c_void) {
    // SAFETY: `data` is a live `Arc<SlotWaker>` the plugin holds a reference to.
    let w = unsafe { &*data.cast::<SlotWaker>() };
    w.shared.wake(w.bit, false);
}

unsafe extern "C" fn waker_clone(data: *const c_void) -> *const c_void {
    // SAFETY: as `waker_wake`; adds the clone's reference.
    unsafe { Arc::increment_strong_count(data.cast::<SlotWaker>()) };
    data
}

unsafe extern "C" fn waker_drop(data: *const c_void) {
    // SAFETY: releases the reference this waker held.
    unsafe { Arc::decrement_strong_count(data.cast::<SlotWaker>()) };
}

/// The context `ctx` points at, when it is of kind `kind`.
///
/// # Safety
/// `ctx` must be null or point at one of the contexts above, alive for `'a`.
unsafe fn cast<'a, T>(ctx: *mut c_void, kind: Kind) -> Option<&'a mut T> {
    // SAFETY: every context is `repr(C)` and starts with its `Kind`.
    (!ctx.is_null() && unsafe { *ctx.cast::<u32>() } == kind as u32).then(|| unsafe { &mut *ctx.cast::<T>() })
}

/// A render context, shared: memo builds re-enter while an outer call holds one.
///
/// # Safety
/// As `cast`.
unsafe fn render<'a>(ctx: *mut c_void) -> Option<&'a RenderCtx<'a>> {
    // SAFETY: as `cast`.
    (!ctx.is_null() && unsafe { *ctx.cast::<u32>() } == Kind::Render as u32).then(|| unsafe { &*ctx.cast::<RenderCtx>() })
}

/// The shared state and plugin id whichever context `ctx` is can read.
///
/// # Safety
/// As `cast`.
unsafe fn read<'a>(ctx: *mut c_void) -> Option<(&'a Shared, &'a str)> {
    // SAFETY: as `cast`.
    unsafe {
        if let Some(r) = render(ctx) {
            return Some((r.shared, r.build.plugin));
        }
        if let Some(a) = cast::<ActionCtx>(ctx, Kind::Action) {
            return Some((&*a.shared, a.plugin_id));
        }
        if let Some(r) = cast::<ReadCtx>(ctx, Kind::Read) {
            return Some((r.shared, r.plugin_id));
        }
        cast::<SetupCtx>(ctx, Kind::Setup).map(|s| (&*s.shared, s.plugin_id))
    }
}

/// The editors whichever context `ctx` is can read.
///
/// # Safety
/// As `cast`.
unsafe fn editors<'a>(ctx: *mut c_void) -> Option<&'a PluginEditors> {
    // SAFETY: as `cast`.
    unsafe {
        if let Some(r) = render(ctx) {
            return Some(r.build.editors);
        }
        if let Some(a) = cast::<ActionCtx>(ctx, Kind::Action) {
            return Some(&*a.editors);
        }
        cast::<SetupCtx>(ctx, Kind::Setup).map(|s| &*s.editors)
    }
}

/// A string a plugin passed, valid for the call it made.
fn text(v: AbiStr<'_>) -> &str {
    // SAFETY: a plugin passes strings valid for the call it makes.
    unsafe { v.as_str() }
}

unsafe extern "C" fn log(level: u32, message: AbiStr) {
    let level = match level {
        LOG_ERROR => "error",
        LOG_WARN => "warn",
        LOG_INFO => "info",
        _ => "debug",
    };
    super::tasks::log(format!("plugin {level}: {}", text(message)));
}

unsafe extern "C" fn create_editor(ctx: *mut c_void, initial: AbiStr) -> EditorHandle {
    // SAFETY: the plugin passes its setup context.
    let Some(s) = (unsafe { cast::<SetupCtx>(ctx, Kind::Setup) }) else { return EditorHandle::NONE };
    let h = match s.reuse.get(s.made.editors.len()) {
        Some(&h) => h,
        None => s.editors.create(text(initial)),
    };
    s.made.editors.push(h);
    h
}

unsafe extern "C" fn waker(ctx: *mut c_void) -> WakerAbi {
    // SAFETY: the plugin passes its setup context.
    let Some(s) = (unsafe { cast::<SetupCtx>(ctx, Kind::Setup) }) else {
        return WakerAbi { data: std::ptr::null(), wake: None, clone: None, drop: None };
    };
    let data = Arc::into_raw(Arc::new(SlotWaker { shared: s.wake.clone(), bit: slot_bit(s.slot) })).cast();
    WakerAbi { data, wake: Some(waker_wake), clone: Some(waker_clone), drop: Some(waker_drop) }
}

unsafe extern "C" fn editor_text(ctx: *mut c_void, editor: EditorHandle, out: *mut AbiStr<'static>) -> u8 {
    // SAFETY: the plugin passes its context and a valid out pointer.
    let (Some(editors), Some(out)) = (unsafe { editors(ctx) }, unsafe { out.as_mut() }) else { return 0 };
    let Some(edit) = editors.get(editor) else { return 0 };
    // The view lives until the field next changes, which no call the plugin makes before
    // copying it can do.
    *out = AbiStr { ptr: edit.text.as_ptr(), len: edit.text.len(), _lt: std::marker::PhantomData };
    1
}

unsafe extern "C" fn editor_version(ctx: *mut c_void, editor: EditorHandle) -> u64 {
    // SAFETY: the plugin passes its context.
    unsafe { editors(ctx) }.map_or(0, |e| e.version(editor))
}

unsafe extern "C" fn editor_set_text(ctx: *mut c_void, editor: EditorHandle, value: AbiStr) {
    // SAFETY: the plugin passes its handler context.
    if let Some(a) = unsafe { cast::<ActionCtx>(ctx, Kind::Action) } {
        a.editors.get_mut(editor).replace_text(text(value));
    }
}

unsafe extern "C" fn instant(ctx: *mut c_void) {
    // SAFETY: the plugin passes its handler context.
    if let Some(a) = unsafe { cast::<ActionCtx>(ctx, Kind::Action) } {
        a.tx.instant();
    }
}

unsafe extern "C" fn wake_after(ctx: *mut c_void, millis: u64) {
    // SAFETY: the plugin passes its handler context.
    if let Some(a) = unsafe { cast::<ActionCtx>(ctx, Kind::Action) } {
        a.wake_after = Some(a.wake_after.map_or(millis, |m| m.min(millis)));
    }
}

unsafe extern "C" fn copy_text(ctx: *mut c_void, value: AbiStr) {
    // SAFETY: the plugin passes its handler context.
    if let Some(a) = unsafe { cast::<ActionCtx>(ctx, Kind::Action) } {
        a.tx.copy(text(value));
    }
}

unsafe extern "C" fn open_panel(ctx: *mut c_void, panel: AbiStr) {
    // SAFETY: the plugin passes its handler context.
    if let Some(a) = unsafe { cast::<ActionCtx>(ctx, Kind::Action) } {
        a.requests.push(Request::OpenPanel(text(panel).to_owned()));
    }
}

unsafe extern "C" fn toast(ctx: *mut c_void, value: AbiStr) {
    // SAFETY: the plugin passes its handler context.
    if let Some(a) = unsafe { cast::<ActionCtx>(ctx, Kind::Action) } {
        a.requests.push(Request::Toast(text(value).to_owned()));
    }
}

unsafe extern "C" fn palette(ctx: *mut c_void, out: *mut AbiPalette) {
    // SAFETY: the plugin passes its render context and a valid out pointer.
    if let (Some(r), Some(out)) = (unsafe { render(ctx) }, unsafe { out.as_mut() }) {
        *out = r.pal;
    }
}

unsafe extern "C" fn el_build(ctx: *mut c_void, spec: *const ElSpec) -> ElHandle {
    // SAFETY: the plugin passes its render context and a spec valid for the call.
    let (Some(r), Some(spec)) = (unsafe { render(ctx) }, unsafe { spec.as_ref() }) else { return ElHandle::NONE };
    // SAFETY: as above.
    let el = unsafe { arena::build(&r.build, spec) };
    let handlers = arena::handlers(&r.build, spec);
    let compose = super::widgets::compose(&r.build, spec);
    r.arena.borrow_mut().push_with(el, handlers, compose)
}

unsafe extern "C" fn el_apply(ctx: *mut c_void, h: ElHandle, spec: *const ElSpec) {
    // SAFETY: as `el_build`.
    let (Some(r), Some(spec)) = (unsafe { render(ctx) }, unsafe { spec.as_ref() }) else { return };
    // SAFETY: as above.
    let el = unsafe { arena::build(&r.build, spec) };
    let handlers = arena::handlers(&r.build, spec);
    let compose = super::widgets::compose(&r.build, spec);
    r.arena.borrow_mut().replace(h, el, handlers, compose);
}

unsafe extern "C" fn el_child(ctx: *mut c_void, parent: ElHandle, child: ElHandle) {
    // SAFETY: the plugin passes its render context.
    if let Some(r) = unsafe { render(ctx) } {
        r.arena.borrow_mut().link(parent, child);
    }
}

unsafe extern "C" fn memo(ctx: *mut c_void, key: AbiStr, version: u64, build: BuildFn, data: *mut c_void) -> ElHandle {
    // SAFETY: the plugin passes its render context.
    let Some(r) = (unsafe { render(ctx) }) else { return ElHandle::NONE };
    let id = format!("{}{}", r.prefix, text(key));
    let el = rune::memo::memo_with_hash(id, version ^ r.salt, || {
        // SAFETY: the plugin's own builder, with the data it passed, during this call.
        match unsafe { build(data, ctx) }.into_result() {
            Ok((_, h)) => r.arena.borrow_mut().take(h),
            Err((_, message)) => {
                r.panicked.borrow_mut().get_or_insert(message);
                rune::el()
            }
        }
    });
    r.arena.borrow_mut().push(el, (None, None))
}

pub static HOST_VTABLE: HostApiVtable = HostApiVtable {
    log,
    add_command: setup::add_command,
    add_panel: setup::add_panel,
    add_setting: setup::add_setting,
    create_editor,
    waker,
    editor_text,
    editor_version,
    editor_set_text,
    instant,
    wake_after,
    copy_text,
    open_panel,
    toast,
    palette,
    el_build,
    el_apply,
    el_child,
    memo,
    add_contribution: setup::add_contribution,
    provide: shared::provide,
    consume: shared::consume,
    intern_icon: shared::intern_icon,
    live: shared::live,
    store_get: shared::store_get,
    store_set: shared::store_set,
    spawn: shared::spawn,
    set_servers: shared::set_servers,
    request: shared::request,
    session: services::session,
    model: services::model,
    resource: services::resource,
    token: services::token,
    host_state: services::host_state,
    server_request: services::server_request,
    save_file: services::save_file,
    process_spawn: system::process_spawn,
    process_write: system::process_write,
    process_kill: system::process_kill,
    fetch: system::fetch,
    host_text: system::host_text,
    open_menu: system::open_menu,
    image: shared::image,
    image_drop: shared::image_drop,
};

unsafe extern "C" fn host_alloc(size: usize, align: usize) -> *mut u8 {
    // SAFETY: a plugin's `GlobalAlloc::alloc` forwards a valid non-zero layout.
    Layout::from_size_align(size, align).map_or(std::ptr::null_mut(), |l| unsafe { std::alloc::alloc(l) })
}

unsafe extern "C" fn host_alloc_zeroed(size: usize, align: usize) -> *mut u8 {
    // SAFETY: as `host_alloc`.
    Layout::from_size_align(size, align).map_or(std::ptr::null_mut(), |l| unsafe { std::alloc::alloc_zeroed(l) })
}

unsafe extern "C" fn host_dealloc(ptr: *mut u8, size: usize, align: usize) {
    if let Ok(l) = Layout::from_size_align(size, align) {
        // SAFETY: `ptr` was allocated on this heap (or by `System`, which it wraps) with this layout.
        unsafe { std::alloc::dealloc(ptr, l) };
    }
}

unsafe extern "C" fn host_realloc(ptr: *mut u8, size: usize, align: usize, new_size: usize) -> *mut u8 {
    // SAFETY: as `host_dealloc`.
    Layout::from_size_align(size, align).map_or(std::ptr::null_mut(), |l| unsafe { std::alloc::realloc(ptr, l, new_size) })
}

pub static HOST_ALLOC: HostAllocVtable =
    HostAllocVtable { alloc: host_alloc, alloc_zeroed: host_alloc_zeroed, dealloc: host_dealloc, realloc: host_realloc };