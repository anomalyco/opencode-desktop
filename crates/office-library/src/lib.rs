//! The office viewer's engines as a library the app loads at the first Word, Excel or PowerPoint
//! file (`features/office` with its `library` feature), instead of code and fonts in the app's exe.
//! On every install and update the system reads a new build before its first run. On macOS it
//! reads every Mach-O file of the bundle side by side and the launch waits for the largest one
//! (5.5 ms per MiB on an M4), so what leaves the exe for a second library is no longer waited
//! for: with the engines here a build the system had never seen first ran in 1,004 ms against
//! 1,225 (thin LTO) and 911 against 1,057 (fat), 10 of 10 rounds. The engines are a quarter of
//! the exe and a launch runs none of them.
//!
//! It exports one function, `opencode_office_v1` (`office_preview::engine::Entry`). Rust values
//! cross the line (a `Start`, `Job`s, `Reply`s with Vello scenes), so the library and the app must
//! be one build's: the entry answers null to any other. It allocates with the app's allocator, so
//! each side may drop what the other made. It is never unloaded: scenes hold font and image bytes
//! whose drop code lives here.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicPtr, Ordering};

use office_preview::engine::{self, Abi, Api, Host, Job, Start, Worker};

static HOST: AtomicPtr<Host> = AtomicPtr::new(std::ptr::null_mut());

/// The app's allocator once the entry has it. Nothing allocates before the entry runs (a library
/// has no start-up code of its own), so no block is made by one allocator and freed by the other.
struct HostAllocator;

fn host() -> Option<&'static Host> {
    // SAFETY: the entry stores the app's `Host`, a static of the app's that outlives the library's use.
    unsafe { HOST.load(Ordering::Relaxed).as_ref() }
}

// SAFETY: each call goes to the app's `GlobalAlloc` with the same layout (`features/office`
// `library.rs`), or to `System` before the app handed it over.
unsafe impl GlobalAlloc for HostAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match host() {
            // SAFETY: the caller's layout, passed on.
            Some(host) => unsafe { (host.alloc)(layout.size(), layout.align()) },
            // SAFETY: the caller's layout, passed on.
            None => unsafe { System.alloc(layout) },
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        match host() {
            // SAFETY: the caller's layout, passed on.
            Some(host) => unsafe { (host.alloc_zeroed)(layout.size(), layout.align()) },
            // SAFETY: the caller's layout, passed on.
            None => unsafe { System.alloc_zeroed(layout) },
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        match host() {
            // SAFETY: the caller's block and layout, passed on.
            Some(host) => unsafe { (host.dealloc)(ptr, layout.size(), layout.align()) },
            // SAFETY: the caller's block and layout, passed on.
            None => unsafe { System.dealloc(ptr, layout) },
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        match host() {
            // SAFETY: the caller's block, layout and size, passed on.
            Some(host) => unsafe { (host.realloc)(ptr, layout.size(), layout.align(), new_size) },
            // SAFETY: the caller's block, layout and size, passed on.
            None => unsafe { System.realloc(ptr, layout, new_size) },
        }
    }
}

#[global_allocator]
static ALLOCATOR: HostAllocator = HostAllocator;

static API: Api = Api { start, send, close };

/// `office_preview::engine::Entry`: null, with nothing touched, unless `abi` is this build's.
///
/// # Safety
/// `host` must point at a `Host` that lives as long as the process, `abi` at the caller's `Abi`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn opencode_office_v1(host: *const Host, abi: *const Abi) -> *const Api {
    if host.is_null() || abi.is_null() {
        return std::ptr::null();
    }
    // SAFETY: `abi` is the caller's, alive for this call; the comparison allocates nothing.
    if !unsafe { engine::abi().matches(&*abi) } {
        return std::ptr::null();
    }
    HOST.store(host.cast_mut(), Ordering::Release);
    &API
}

unsafe extern "C" fn start(start: *mut Start) -> *mut Worker {
    // SAFETY: the app made the box with the allocator this library allocates with, and gives it up.
    let start = unsafe { Box::from_raw(start) };
    match std::panic::catch_unwind(AssertUnwindSafe(|| engine::start(*start))) {
        Ok(Some(worker)) => Box::into_raw(Box::new(worker)),
        _ => std::ptr::null_mut(),
    }
}

unsafe extern "C" fn send(worker: *const Worker, job: *mut Job) -> bool {
    // SAFETY: `worker` is a `start` answer the app has not closed; the job's box is the app's to give.
    let (worker, job) = unsafe { (&*worker, Box::from_raw(job)) };
    std::panic::catch_unwind(AssertUnwindSafe(|| worker.send(*job))).unwrap_or(false)
}

unsafe extern "C" fn close(worker: *mut Worker) {
    // SAFETY: a `start` answer, closed once.
    drop(unsafe { Box::from_raw(worker) });
}
