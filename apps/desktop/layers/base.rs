// The allocator of a build whose code is several libraries (layers.txt). The standard library is
// linked into this lowest library, and Rust takes the allocator from the file that holds the
// standard library: defined in the exe, as `main.rs` does for a one-file build, it would serve the
// exe alone while every library allocated from the system heap, and memory would be freed by the
// wrong one. The same allocator as `main.rs`: mimalloc, counted per thread, with the app told when
// an allocation fails.
use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicUsize, Ordering};

static FAILED: AtomicUsize = AtomicUsize::new(0);

/// The app's `OomGuard` hook: called with the size of a heap allocation that returned null.
pub fn on_alloc_failure(hook: fn(usize)) {
    FAILED.store(hook as usize, Ordering::Relaxed);
}

pub struct Guard<A>(pub A);

#[cold]
#[inline(never)]
fn failed(requested: usize) {
    let hook = FAILED.load(Ordering::Relaxed);
    if hook != 0 {
        // SAFETY: the only value ever stored is a `fn(usize)`.
        let hook: fn(usize) = unsafe { std::mem::transmute(hook) };
        hook(requested);
    }
}

// SAFETY: delegates every operation unchanged to `A`; `failed` never touches the heap.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Guard<A> {
    #[inline(always)]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc` for `layout`.
        let p = unsafe { self.0.alloc(layout) };
        if p.is_null() {
            failed(layout.size());
        }
        p
    }

    #[inline(always)]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed` for `layout`.
        let p = unsafe { self.0.alloc_zeroed(layout) };
        if p.is_null() {
            failed(layout.size());
        }
        p
    }

    #[inline(always)]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `GlobalAlloc::dealloc` for `ptr` and `layout`.
        unsafe { self.0.dealloc(ptr, layout) };
    }

    #[inline(always)]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::realloc` for `ptr`, `layout` and `new_size`.
        let p = unsafe { self.0.realloc(ptr, layout, new_size) };
        if p.is_null() {
            failed(new_size);
        }
        p
    }
}

#[global_allocator]
static ALLOC: procwatch::alloc::Counting<Guard<mimalloc::MiMalloc>> = procwatch::alloc::Counting::new(Guard(mimalloc::MiMalloc));
