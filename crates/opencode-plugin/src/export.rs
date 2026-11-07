//! The entry point and plugin vtable `export_plugin!` generates, with a `catch_unwind` around
//! every call.

use core::ffi::c_void;
use core::marker::PhantomData;

use crate::abi::*;
use crate::boundary::{guard, install_host, install_panic_hook, outcome, try_host, with_ctx};
use crate::cx::{ActionCx, WindowPlugin, WindowSetupCx};

/// The vtable for plugin type `P`.
pub const fn vtable<P: WindowPlugin>() -> PluginVtable {
    PluginVtable {
        window_setup: window_setup::<P>,
        drop_state: drop_state::<P>,
        poll: poll::<P>,
        save_hot_state: save_hot_state::<P>,
        load_hot_state: load_hot_state::<P>,
    }
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

unsafe extern "C" fn window_setup<P: WindowPlugin>(ctx: *mut c_void) -> AbiOutcome<*mut c_void> {
    if try_host().is_none() {
        return AbiOutcome::err("the plugin refused the host's handshake".to_owned());
    }
    match with_ctx(ctx, || guard(|| P::setup(&mut WindowSetupCx { _p: PhantomData }))) {
        Ok(Ok(state)) => AbiOutcome::ok(Box::into_raw(Box::new(state)).cast()),
        Ok(Err(e)) => AbiOutcome::err(e.0),
        Err(message) => AbiOutcome::panicked(message),
    }
}

unsafe extern "C" fn drop_state<P: WindowPlugin>(state: *mut c_void) -> AbiOutcome<u8> {
    outcome(|| {
        // SAFETY: `state` is the box `window_setup` leaked, dropped once by the host.
        drop(unsafe { Box::from_raw(state.cast::<P>()) });
        0
    })
}

unsafe extern "C" fn poll<P: WindowPlugin>(state: *mut c_void, ctx: *mut c_void) -> AbiOutcome<bool> {
    with_ctx(ctx, || {
        outcome(|| {
            let mut cx = ActionCx { changed: true, declined: false, event: AbiEvent::default() };
            // SAFETY: the host passes this instance's state.
            unsafe { &mut *state.cast::<P>() }.poll(&mut cx)
        })
    })
}

unsafe extern "C" fn save_hot_state<P: WindowPlugin>(state: *const c_void, out: *mut AbiOwnedBytes) -> AbiOutcome<u8> {
    outcome(|| {
        // SAFETY: the host passes this instance's state and a valid out pointer.
        let bytes = unsafe { &*state.cast::<P>() }.save_hot_state();
        // SAFETY: as above; the host drops what it held there first.
        unsafe { out.write(AbiOwnedBytes::from_vec(bytes)) };
        0
    })
}

unsafe extern "C" fn load_hot_state<P: WindowPlugin>(state: *mut c_void, bytes: AbiStr) -> AbiOutcome<u8> {
    outcome(|| {
        // SAFETY: the host passes this instance's state and bytes valid for the call.
        unsafe { &mut *state.cast::<P>() }.load_hot_state(unsafe { bytes.as_bytes() });
        0
    })
}

/// Exports a plugin: `opencode_plugin::export_plugin!(window = MyPlugin);`
///
/// Generates `__opencode_plugin_entry` (what a host linking the crate statically calls) and,
/// unless the crate's `static-builtin` feature is on, the `opencode_plugin_v1` symbol and a
/// `#[global_allocator]` that forwards to the host's heap. Declare
/// `[features] static-builtin = []` in the plugin's `Cargo.toml`.
#[macro_export]
macro_rules! export_plugin {
    (window = $ty:ty) => {
        static __OPENCODE_PLUGIN_VTABLE: $crate::abi::PluginVtable = $crate::export::vtable::<$ty>();
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

        #[cfg(not(feature = "static-builtin"))]
        #[doc(hidden)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn opencode_plugin_v1(host: *const $crate::abi::HostHandshake) -> *const $crate::abi::PluginDescriptor {
            // SAFETY: forwarded to the caller.
            unsafe { __opencode_plugin_entry(host) }
        }

        #[cfg(not(feature = "static-builtin"))]
        #[global_allocator]
        static __OPENCODE_PLUGIN_ALLOC: $crate::alloc::HostAllocator = $crate::alloc::HostAllocator;
    };
}
