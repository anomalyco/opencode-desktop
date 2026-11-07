//! The plugin's side of the boundary: the host vtable, the per-call context, panic capture and
//! closure packing.

use core::cell::Cell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, Ordering};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, Once};

use crate::abi::{AbiOutcome, DropFn, HostApiVtable};

static HOST: AtomicPtr<HostApiVtable> = AtomicPtr::new(core::ptr::null_mut());

pub(crate) fn install_host(vtable: *const HostApiVtable) {
    HOST.store(vtable.cast_mut(), Ordering::Release);
}

/// The host's vtable, once the handshake accepted it.
pub(crate) fn try_host() -> Option<&'static HostApiVtable> {
    let ptr = HOST.load(Ordering::Acquire);
    // SAFETY: the host's vtable is a static that outlives the library.
    (!ptr.is_null()).then(|| unsafe { &*ptr })
}

pub(crate) fn host() -> &'static HostApiVtable {
    let Some(host) = try_host() else { panic!("opencode-plugin: host vtable used before the handshake") };
    host
}

thread_local! {
    static CTX: Cell<*mut c_void> = const { Cell::new(core::ptr::null_mut()) };
}

/// The context of the host call running on this thread (null outside one).
pub(crate) fn ctx() -> *mut c_void {
    CTX.get()
}

/// Runs `f` with `ctx` as the current host context, restoring the previous one afterwards (a
/// memo build runs inside a render), on unwind too.
pub(crate) fn with_ctx<R>(ctx: *mut c_void, f: impl FnOnce() -> R) -> R {
    struct Restore(*mut c_void);
    impl Drop for Restore {
        fn drop(&mut self) {
            CTX.set(self.0);
        }
    }
    let _restore = Restore(CTX.replace(ctx));
    f()
}

/// The last panic's message and location. A plain static rather than a thread-local, so the
/// library registers no TLS destructor that could outlive it.
static LAST_PANIC: Mutex<Option<String>> = Mutex::new(None);

/// Chains a hook that records each panic's message and `file:line:col` ahead of the previous one.
pub(crate) fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".to_owned());
            let at = info.location().map(|l| format!(" at {}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
            if let Ok(mut last) = LAST_PANIC.lock() {
                *last = Some(format!("{message}{at}"));
            }
            previous(info);
        }));
    });
}

/// Runs `f`, turning a panic into its message.
pub(crate) fn guard<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|payload| {
        LAST_PANIC.lock().ok().and_then(|mut l| l.take()).unwrap_or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".to_owned())
        })
    })
}

/// [`guard`] as an outcome.
pub(crate) fn outcome<T>(f: impl FnOnce() -> T) -> AbiOutcome<T> {
    match guard(f) {
        Ok(v) => AbiOutcome::ok(v),
        Err(message) => AbiOutcome::panicked(message),
    }
}

const fn fits_inline<F>() -> bool {
    size_of::<F>() <= size_of::<usize>() && align_of::<F>() <= align_of::<usize>() && !core::mem::needs_drop::<F>()
}

/// Packs a closure into a callback's `data`: nothing for a zero-sized one, the bytes themselves
/// when they fit in a `usize` and need no drop, else a box with its destructor.
pub(crate) fn pack<F: 'static>(f: F) -> (usize, Option<DropFn>) {
    if size_of::<F>() == 0 {
        core::mem::forget(f);
        return (0, None);
    }
    if fits_inline::<F>() {
        let mut data = 0usize;
        // SAFETY: `F` fits in and is no more aligned than a `usize`, and needs no drop.
        unsafe { core::ptr::write((&raw mut data).cast::<F>(), f) };
        return (data, None);
    }
    (Box::into_raw(Box::new(f)) as usize, Some(drop_boxed::<F>))
}

unsafe extern "C" fn drop_boxed<F>(data: usize) {
    guard(|| {
        // SAFETY: `data` is the `Box<F>` `pack` leaked, dropped once by the host.
        drop(unsafe { Box::from_raw(data as *mut F) });
    })
    .unwrap_or(());
}

/// The closure `pack` packed into `data`.
///
/// # Safety
/// `data` must come from `pack::<F>` and not have been dropped.
pub(crate) unsafe fn unpack<F>(data: &usize) -> &F {
    if size_of::<F>() == 0 {
        // SAFETY: any aligned non-null pointer is a valid reference to a zero-sized value.
        return unsafe { &*core::ptr::NonNull::<F>::dangling().as_ptr() };
    }
    if fits_inline::<F>() {
        // SAFETY: `pack` wrote an `F` into these bytes.
        return unsafe { &*(data as *const usize).cast::<F>() };
    }
    // SAFETY: `pack` boxed an `F` at this address.
    unsafe { &*(*data as *const F) }
}

/// Frees a packed closure the host never took.
pub(crate) fn drop_packed(data: usize, drop_data: Option<DropFn>) {
    if let Some(drop_data) = drop_data {
        // SAFETY: `drop_data` is the destructor `pack` chose for this `data`.
        unsafe { drop_data(data) };
    }
}
