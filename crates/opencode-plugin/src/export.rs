//! The entry point and plugin vtable the export macros generate, with a `catch_unwind` around
//! every call.
//!
//! - [`export_plugin!`]: a plugin library. Emits `__opencode_plugin_entry`, the exported
//!   `opencode_plugin_v1` symbol and a `#[global_allocator]` on the host's heap. Always: a
//!   library build never loses its entry point to a feature some other crate turned on.
//! - [`declare_plugin!`] + [`export_entry!`]: the same, split for built-ins the app can also
//!   link in (`--features static-builtins`). The plugin crate (an `rlib`) declares; a one-line
//!   `cdylib` crate beside it exports. The app links the `rlib` alone, so it never meets a second
//!   allocator or a duplicate `opencode_plugin_v1`.

use core::ffi::c_void;
use core::marker::PhantomData;

use crate::abi::*;
use crate::app::{AppPlugin, AppSetupCx};
use crate::boundary::{guard, install_host, install_panic_hook, outcome, try_host, with_ctx};
use crate::cx::{ActionCx, WindowPlugin, WindowSetupCx};

/// The vtable of a plugin with a window half only.
pub const fn window_vtable<W: WindowPlugin>() -> PluginVtable {
    PluginVtable {
        app_setup: None,
        app_drop_state: None,
        app_poll: None,
        window_setup: Some(window_setup::<W>),
        drop_state: Some(drop_state::<W>),
        poll: Some(window_poll::<W>),
        save_hot_state: Some(save_hot_state::<W>),
        load_hot_state: Some(load_hot_state::<W>),
    }
}

/// The vtable of a plugin with an app half only.
pub const fn app_vtable<A: AppPlugin>() -> PluginVtable {
    PluginVtable {
        app_setup: Some(app_setup::<A>),
        app_drop_state: Some(drop_state::<A>),
        app_poll: Some(app_poll::<A>),
        window_setup: None,
        drop_state: None,
        poll: None,
        save_hot_state: None,
        load_hot_state: None,
    }
}

/// The vtable of a plugin with both halves.
pub const fn vtable<A: AppPlugin, W: WindowPlugin>() -> PluginVtable {
    PluginVtable { app_setup: Some(app_setup::<A>), app_drop_state: Some(drop_state::<A>), app_poll: Some(app_poll::<A>), ..window_vtable::<W>() }
}

/// The descriptor pointing at `vtable`.
pub const fn descriptor(vtable: &'static PluginVtable) -> PluginDescriptor {
    PluginDescriptor {
        magic: ABI_MAGIC,
        endian: ABI_ENDIAN,
        abi_epoch: ABI_EPOCH,
        abi_minor: ABI_MINOR,
        abi_layout_hash: ABI_LAYOUT_HASH,
        sdk_version: AbiStr::new(SDK_VERSION),
        plugin_vtable_size: size_of::<PluginVtable>() as u32,
        plugin_vtable: vtable,
    }
}

/// The body of `opencode_plugin_v1`. When the handshake matches this SDK, routes the library's
/// heap to the host's (first, before anything allocates), keeps the host vtable and installs
/// the panic recorder. Either way it answers with `descriptor`, so a host can say what didn't
/// match; a refused plugin's setup fails without touching the host.
///
/// # Safety
/// `host` must be null or point at a valid handshake for the call.
pub unsafe fn entry(host: *const HostHandshake, descriptor: &'static PluginDescriptor) -> *const PluginDescriptor {
    // SAFETY: the caller passes null or a valid handshake.
    let Some(h) = (unsafe { host.as_ref() }) else { return descriptor };
    let accepted = h.magic == ABI_MAGIC
        && h.endian == ABI_ENDIAN
        && h.abi_epoch == ABI_EPOCH
        && h.abi_layout_hash == ABI_LAYOUT_HASH
        && h.pointer_width as usize == size_of::<usize>()
        && h.os == HOST_OS
        && h.arch == HOST_ARCH
        && h.host_vtable_size as usize >= size_of::<HostApiVtable>()
        && !h.host_vtable.is_null();
    if accepted {
        if !h.host_alloc.is_null() {
            crate::alloc::install(h.host_alloc);
        }
        install_host(h.host_vtable);
        install_panic_hook();
    }
    descriptor
}

fn refused<T>() -> Option<AbiOutcome<T>> {
    try_host().is_none().then(|| AbiOutcome::err("the plugin refused the host's handshake".to_owned()))
}

unsafe extern "C" fn window_setup<W: WindowPlugin>(ctx: *mut c_void) -> AbiOutcome<*mut c_void> {
    if let Some(no) = refused() {
        return no;
    }
    match with_ctx(ctx, || guard(|| W::setup(&mut WindowSetupCx { _p: PhantomData }))) {
        Ok(Ok(state)) => AbiOutcome::ok(Box::into_raw(Box::new(state)).cast()),
        Ok(Err(e)) => AbiOutcome::err(e.0),
        Err(message) => AbiOutcome::panicked(message),
    }
}

unsafe extern "C" fn app_setup<A: AppPlugin>(ctx: *mut c_void) -> AbiOutcome<*mut c_void> {
    if let Some(no) = refused() {
        return no;
    }
    match with_ctx(ctx, || guard(|| A::setup(&mut AppSetupCx { _a: PhantomData }))) {
        Ok(Ok(state)) => AbiOutcome::ok(Box::into_raw(Box::new(state)).cast()),
        Ok(Err(e)) => AbiOutcome::err(e.0),
        Err(message) => AbiOutcome::panicked(message),
    }
}

unsafe extern "C" fn drop_state<S>(state: *mut c_void) -> AbiOutcome<u8> {
    outcome(|| {
        // SAFETY: `state` is the box a setup leaked, dropped once by the host.
        drop(unsafe { Box::from_raw(state.cast::<S>()) });
        0
    })
}

unsafe extern "C" fn window_poll<W: WindowPlugin>(state: *mut c_void, ctx: *mut c_void) -> AbiOutcome<bool> {
    with_ctx(ctx, || {
        outcome(|| {
            let mut cx = ActionCx { changed: true, declined: false, event: AbiEvent::default() };
            // SAFETY: the host passes this instance's state.
            unsafe { &mut *state.cast::<W>() }.poll(&mut cx)
        })
    })
}

unsafe extern "C" fn app_poll<A: AppPlugin>(state: *mut c_void, ctx: *mut c_void) -> AbiOutcome<bool> {
    with_ctx(ctx, || {
        outcome(|| {
            let mut cx = ActionCx { changed: true, declined: false, event: AbiEvent::default() };
            // SAFETY: the host passes this instance's app state.
            unsafe { &mut *state.cast::<A>() }.poll(&mut cx)
        })
    })
}

unsafe extern "C" fn save_hot_state<W: WindowPlugin>(state: *const c_void, out: *mut AbiOwnedBytes) -> AbiOutcome<u8> {
    outcome(|| {
        // SAFETY: the host passes this instance's state and a valid out pointer.
        let bytes = unsafe { &*state.cast::<W>() }.save_hot_state();
        // SAFETY: as above; the host drops what it held there first.
        unsafe { out.write(AbiOwnedBytes::from_vec(bytes)) };
        0
    })
}

unsafe extern "C" fn load_hot_state<W: WindowPlugin>(state: *mut c_void, bytes: AbiStr) -> AbiOutcome<u8> {
    outcome(|| {
        // SAFETY: the host passes this instance's state and bytes valid for the call.
        unsafe { &mut *state.cast::<W>() }.load_hot_state(unsafe { bytes.as_bytes() });
        0
    })
}

/// Declares a plugin without exporting it: generates `__opencode_plugin_entry`, the function
/// the app calls when it links the crate in, and [`export_entry!`] exports from a `cdylib`.
///
/// `declare_plugin!(window = W)`, `declare_plugin!(app = A)` or `declare_plugin!(app = A, window = W)`.
#[macro_export]
macro_rules! declare_plugin {
    (window = $w:ty) => {
        $crate::__declare_plugin!($crate::export::window_vtable::<$w>());
    };
    (app = $a:ty) => {
        $crate::__declare_plugin!($crate::export::app_vtable::<$a>());
    };
    (app = $a:ty, window = $w:ty) => {
        $crate::__declare_plugin!($crate::export::vtable::<$a, $w>());
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __declare_plugin {
    ($vtable:expr) => {
        static __OPENCODE_PLUGIN_VTABLE: $crate::abi::PluginVtable = $vtable;
        static __OPENCODE_PLUGIN_DESCRIPTOR: $crate::abi::PluginDescriptor = $crate::export::descriptor(&__OPENCODE_PLUGIN_VTABLE);

        /// The plugin's entry point, as `opencode_plugin_v1` exports it.
        ///
        /// # Safety
        /// `host` must be null or point at a valid handshake for the call.
        #[doc(hidden)]
        pub unsafe extern "C" fn __opencode_plugin_entry(host: *const $crate::abi::HostHandshake) -> *const $crate::abi::PluginDescriptor {
            // SAFETY: forwarded to the caller.
            unsafe { $crate::export::entry(host, &__OPENCODE_PLUGIN_DESCRIPTOR) }
        }
    };
}

/// Exports `entry` (a declared plugin's `__opencode_plugin_entry`) as `opencode_plugin_v1`, with
/// a `#[global_allocator]` on the host's heap. Only ever in a `cdylib` crate.
#[macro_export]
macro_rules! export_entry {
    ($entry:path) => {
        #[doc(hidden)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn opencode_plugin_v1(host: *const $crate::abi::HostHandshake) -> *const $crate::abi::PluginDescriptor {
            // SAFETY: forwarded to the caller.
            unsafe { $entry(host) }
        }

        #[global_allocator]
        static __OPENCODE_PLUGIN_ALLOC: $crate::alloc::HostAllocator = $crate::alloc::HostAllocator;
    };
}

/// Exports a plugin library: `opencode_plugin::export_plugin!(window = MyPlugin);` (or
/// `app = A`, or `app = A, window = W`). The crate is a `cdylib`.
#[macro_export]
macro_rules! export_plugin {
    ($($spec:tt)*) => {
        $crate::declare_plugin!($($spec)*);
        $crate::export_entry!(__opencode_plugin_entry);
    };
}
