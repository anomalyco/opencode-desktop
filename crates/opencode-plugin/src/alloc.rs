//! [`HostAllocator`]: the `#[global_allocator]` `export_plugin!` installs in a plugin library.
//!
//! It forwards every allocation to the host's heap through the [`HostAllocVtable`] the
//! handshake carries, so a plugin's `Vec`s and `String`s are counted by the host's allocator
//! like its own. Before the handshake (statics initialised by the loader) it falls back to
//! `std::alloc::System`, which the host's counting allocator wraps without headers, so either
//! side may free the other's blocks.

use core::sync::atomic::{AtomicPtr, Ordering};
use std::alloc::{GlobalAlloc, Layout, System};

use crate::abi::HostAllocVtable;

static HOST_ALLOC: AtomicPtr<HostAllocVtable> = AtomicPtr::new(core::ptr::null_mut());

/// Routes this library's heap to the host's from now on. Called first thing in the entry point.
pub(crate) fn install(vtable: *const HostAllocVtable) {
    HOST_ALLOC.store(vtable.cast_mut(), Ordering::Release);
}

fn host() -> Option<&'static HostAllocVtable> {
    let ptr = HOST_ALLOC.load(Ordering::Acquire);
    // SAFETY: the host's vtable is a static that outlives every library it loads.
    (!ptr.is_null()).then(|| unsafe { &*ptr })
}

pub struct HostAllocator;

// SAFETY: every method forwards to a `GlobalAlloc`-conforming allocator (the host's, or `System`).
unsafe impl GlobalAlloc for HostAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match host() {
            // SAFETY: forwarded with the caller's layout.
            Some(h) => unsafe { (h.alloc)(layout.size(), layout.align()) },
            // SAFETY: forwarded with the caller's layout.
            None => unsafe { System.alloc(layout) },
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        match host() {
            // SAFETY: forwarded with the caller's layout.
            Some(h) => unsafe { (h.alloc_zeroed)(layout.size(), layout.align()) },
            // SAFETY: forwarded with the caller's layout.
            None => unsafe { System.alloc_zeroed(layout) },
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        match host() {
            // SAFETY: `ptr` came from this allocator, or from `System`, which the host's wraps.
            Some(h) => unsafe { (h.dealloc)(ptr, layout.size(), layout.align()) },
            // SAFETY: as above.
            None => unsafe { System.dealloc(ptr, layout) },
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        match host() {
            // SAFETY: as `dealloc`.
            Some(h) => unsafe { (h.realloc)(ptr, layout.size(), layout.align(), new_size) },
            // SAFETY: as `dealloc`.
            None => unsafe { System.realloc(ptr, layout, new_size) },
        }
    }
}
