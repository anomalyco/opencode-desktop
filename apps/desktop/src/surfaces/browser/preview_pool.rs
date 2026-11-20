//! Bounded background worker pool for host-side `EngineMsg::PreviewFetch` streaming
//! (`notes/review-rd281.md` PR 2 Blocker #3):
//! - Fixed 8 worker threads + 64-slot global queue (never spawns unbounded OS threads).
//! - Per-tab RAII [`FetchPermit`] capping concurrent fetches per preview tab at `MAX_FETCHES_PER_TAB = 16`.
//! - `std::panic::catch_unwind` isolation invoking `on_panic` so a panic never kills a worker thread
//!   or leaves a CEF `ProxyResourceHandler` hanging.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

const FETCH_WORKERS: usize = 8;
const FETCH_QUEUE_CAP: usize = 64;
const MAX_FETCHES_PER_TAB: usize = 16;

struct FetchPermit(Arc<AtomicUsize>);

impl Drop for FetchPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn try_acquire_permit(counter: &Arc<AtomicUsize>, cap: usize) -> Option<FetchPermit> {
    let mut cur = counter.load(Ordering::Acquire);
    loop {
        if cur >= cap {
            return None;
        }
        match counter.compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(FetchPermit(Arc::clone(counter))),
            Err(next) => cur = next,
        }
    }
}

struct FetchTask {
    _permit: FetchPermit,
    job: Box<dyn FnOnce() + Send + 'static>,
    on_panic: Box<dyn FnOnce() + Send + 'static>,
}

pub(super) struct PreviewFetchPool {
    tx: SyncSender<FetchTask>,
}

impl PreviewFetchPool {
    pub(super) fn global() -> &'static Self {
        static POOL: OnceLock<PreviewFetchPool> = OnceLock::new();
        POOL.get_or_init(|| {
            let (tx, rx) = mpsc::sync_channel::<FetchTask>(FETCH_QUEUE_CAP);
            let shared_rx = Arc::new(Mutex::new(rx));
            for idx in 0..FETCH_WORKERS {
                let rx_clone = Arc::clone(&shared_rx);
                let _ = std::thread::Builder::new()
                    .name(format!("preview-fetch-{idx}"))
                    .spawn(move || loop {
                        let task = {
                            let guard = rx_clone.lock().unwrap_or_else(PoisonError::into_inner);
                            guard.recv()
                        };
                        let Ok(task) = task else { break };
                        let FetchTask { _permit, job, on_panic } = task;
                        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(on_panic));
                        }
                        drop(_permit);
                    });
            }
            Self { tx }
        })
    }

    pub(super) fn try_spawn(
        &self,
        tab_counter: &Arc<AtomicUsize>,
        on_panic: impl FnOnce() + Send + 'static,
        job: impl FnOnce() + Send + 'static,
    ) -> bool {
        let Some(permit) = try_acquire_permit(tab_counter, MAX_FETCHES_PER_TAB) else {
            return false;
        };
        let task = FetchTask {
            _permit: permit,
            job: Box::new(job),
            on_panic: Box::new(on_panic),
        };
        match self.tx.try_send(task) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }
}
