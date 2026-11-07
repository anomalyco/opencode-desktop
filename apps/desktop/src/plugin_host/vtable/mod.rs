//! The host side of the ABI: `HostApiVtable`, the host heap a plugin allocates on, and the
//! per-call contexts plugins pass back.
//!
//! Every context starts with a [`Kind`] tag; a host function checks it and does nothing when
//! called from the wrong kind of callback (a render asking to open a panel, say).

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
use super::instance::{CallbackEnv, CommandRec, Contributions, PanelRec, RenderEnv, SettingRec};
use super::intern;
use super::loader::LoadedLibrary;

#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Setup = 0x5E70,
    Render = 0x8E4D,
    Action = 0xAC70,
}

#[repr(C)]
pub struct SetupCtx<'a> {
    pub kind: Kind,
    pub plugin_id: &'a str,
    pub slot: usize,
    pub lib: &'a Arc<LoadedLibrary>,
    pub editors: &'a mut PluginEditors,
    /// The previous generation's editors, handed out again in creation order.
    pub reuse: &'a [EditorHandle],
    pub made: Contributions,
    pub wake: &'a Arc<WakeShared>,
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
}

#[repr(C)]
pub struct ActionCtx<'a> {
    pub kind: Kind,
    pub editors: &'a mut PluginEditors,
    pub tx: &'a mut Tx,
    pub requests: Vec<Request>,
    pub wake_after: Option<u64>,
}

/// What a handler asked for that needs the whole app, carried out once it returns.
pub enum Request {
    OpenPanel(String),
    Toast(String),
}

/// Wakes the event loop for plugins: one bit per slot in `dirty`, which `PluginHost::poll`
/// swaps once a frame; at rest it reads zero and calls nothing.
#[derive(Default)]
pub struct WakeShared {
    pub dirty: AtomicU64,
    /// Slots whose library changed on disk (hot reload).
    pub hot: AtomicU64,
    pub waker: OnceLock<rune::Waker>,
}

impl WakeShared {
    pub fn wake(&self, bits: u64, hot: bool) {
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

// SAFETY (for every `as_str` below): a plugin passes strings valid for the call it makes.

unsafe extern "C" fn log(level: u32, message: AbiStr) {
    let level = match level {
        LOG_ERROR => "error",
        LOG_WARN => "warn",
        LOG_INFO => "info",
        _ => "debug",
    };
    // SAFETY: see above.
    eprintln!("plugin {level}: {}", unsafe { message.as_str() });
}

unsafe extern "C" fn add_command(ctx: *mut c_void, spec: *const CommandSpecAbi) -> AbiStatus {
    // SAFETY: the plugin passes its setup context and a spec valid for the call.
    let (Some(s), Some(spec)) = (unsafe { cast::<SetupCtx>(ctx, Kind::Setup) }, unsafe { spec.as_ref() }) else { return AbiStatus::Err };
    fn text(v: AbiStr<'_>) -> &str {
        // SAFETY: see above.
        unsafe { v.as_str() }
    }
    if text(spec.id).is_empty() || spec.run.func.is_none() {
        return AbiStatus::Err;
    }
    let bind = if cfg!(target_os = "windows") {
        spec.bind_windows
    } else if cfg!(target_os = "macos") {
        spec.bind_macos
    } else {
        spec.bind_linux
    };
    let keybind = text(bind).parse::<rune::Keybind>().ok().filter(|k| !k.chords().is_empty());
    s.made.commands.push(CommandRec {
        id: intern(&format!("{}.{}", s.plugin_id, text(spec.id))),
        title: intern(text(spec.title)),
        description: intern(text(spec.description)),
        keybind,
        editable: spec.flags & COMMAND_EDITABLE != 0,
        hidden: spec.flags & COMMAND_HIDDEN != 0,
        run: CallbackEnv::new(spec.run, s.lib.clone()),
    });
    AbiStatus::Ok
}

unsafe extern "C" fn add_panel(ctx: *mut c_void, spec: *const PanelSpecAbi) -> AbiStatus {
    // SAFETY: as `add_command`.
    let (Some(s), Some(spec)) = (unsafe { cast::<SetupCtx>(ctx, Kind::Setup) }, unsafe { spec.as_ref() }) else { return AbiStatus::Err };
    // SAFETY: see above.
    let text = |v: AbiStr| unsafe { v.as_str() }.to_owned();
    let id = text(spec.id);
    if id.is_empty() || id.contains(':') || spec.render.func.is_none() {
        return AbiStatus::Err;
    }
    s.made.panels.push(PanelRec {
        tab: format!("{}.{id}", s.plugin_id),
        id,
        title: text(spec.title),
        icon: text(spec.icon),
        order: spec.order,
        render: RenderEnv::new(spec.render, s.lib.clone()),
    });
    AbiStatus::Ok
}

unsafe extern "C" fn add_setting(ctx: *mut c_void, spec: *const SettingSpecAbi) -> AbiStatus {
    // SAFETY: as `add_command`.
    let (Some(s), Some(spec)) = (unsafe { cast::<SetupCtx>(ctx, Kind::Setup) }, unsafe { spec.as_ref() }) else { return AbiStatus::Err };
    // SAFETY: see above.
    let text = |v: AbiStr| unsafe { v.as_str() }.to_owned();
    if spec.id.is_empty() || spec.render.func.is_none() {
        return AbiStatus::Err;
    }
    s.made.settings.push(SettingRec {
        id: text(spec.id),
        title: text(spec.title),
        description: text(spec.description),
        render: RenderEnv::new(spec.render, s.lib.clone()),
    });
    AbiStatus::Ok
}

unsafe extern "C" fn create_editor(ctx: *mut c_void, initial: AbiStr) -> EditorHandle {
    // SAFETY: the plugin passes its setup context.
    let Some(s) = (unsafe { cast::<SetupCtx>(ctx, Kind::Setup) }) else { return EditorHandle::NONE };
    let h = match s.reuse.get(s.made.editors.len()) {
        Some(&h) => h,
        // SAFETY: see above.
        None => s.editors.create(unsafe { initial.as_str() }),
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

unsafe extern "C" fn editor_set_text(ctx: *mut c_void, editor: EditorHandle, text: AbiStr) {
    // SAFETY: the plugin passes its handler context.
    if let Some(a) = unsafe { cast::<ActionCtx>(ctx, Kind::Action) } {
        // SAFETY: see above.
        a.editors.get_mut(editor).replace_text(unsafe { text.as_str() });
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

unsafe extern "C" fn copy_text(ctx: *mut c_void, text: AbiStr) {
    // SAFETY: the plugin passes its handler context.
    if let Some(a) = unsafe { cast::<ActionCtx>(ctx, Kind::Action) } {
        // SAFETY: see above.
        a.tx.copy(unsafe { text.as_str() });
    }
}

unsafe extern "C" fn open_panel(ctx: *mut c_void, panel: AbiStr) {
    // SAFETY: the plugin passes its handler context.
    if let Some(a) = unsafe { cast::<ActionCtx>(ctx, Kind::Action) } {
        // SAFETY: see above.
        a.requests.push(Request::OpenPanel(unsafe { panel.as_str() }.to_owned()));
    }
}

unsafe extern "C" fn toast(ctx: *mut c_void, text: AbiStr) {
    // SAFETY: the plugin passes its handler context.
    if let Some(a) = unsafe { cast::<ActionCtx>(ctx, Kind::Action) } {
        // SAFETY: see above.
        a.requests.push(Request::Toast(unsafe { text.as_str() }.to_owned()));
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
    let press = arena::press(&r.build, spec);
    r.arena.borrow_mut().push(el, press)
}

unsafe extern "C" fn el_apply(ctx: *mut c_void, h: ElHandle, spec: *const ElSpec) {
    // SAFETY: as `el_build`.
    let (Some(r), Some(spec)) = (unsafe { render(ctx) }, unsafe { spec.as_ref() }) else { return };
    // SAFETY: as above.
    let el = unsafe { arena::build(&r.build, spec) };
    let press = arena::press(&r.build, spec);
    r.arena.borrow_mut().replace(h, el, press);
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
    // SAFETY: see above.
    let id = format!("{}{}", r.prefix, unsafe { key.as_str() });
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
    r.arena.borrow_mut().push(el, None)
}

pub static HOST_VTABLE: HostApiVtable = HostApiVtable {
    log,
    add_command,
    add_panel,
    add_setting,
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
