//! Bounded background I/O worker pool keeping local `oc-preview://` filesystem open, seek, and
//! read syscalls strictly off CEF's single global IO thread (`notes/review-rd281.md` #3).
//!
//! Guarantees:
//! - Per-grant quota (`MAX_GRANT_IO_JOBS = 32` inside the 256-slot global queue): one abusive
//!   preview tab cannot saturate the pool or starve any other preview tab.
//! - Panic isolation (`std::panic::catch_unwind`): a panic inside any job cancels its CEF callback
//!   and releases its grant permit without killing worker threads.

use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use super::PreviewGrant;

const WORKER_COUNT: usize = 4;
const QUEUE_CAPACITY: usize = 256;
pub const MAX_GRANT_IO_JOBS: usize = 32;

struct IoPermit(Arc<PreviewGrant>);

impl Drop for IoPermit {
    fn drop(&mut self) {
        self.0.release_io_slot();
    }
}

struct Task {
    _permit: IoPermit,
    job: Box<dyn FnOnce() + Send + 'static>,
    on_panic: Box<dyn FnOnce() + Send + 'static>,
}

#[derive(Clone)]
pub struct PreviewIoPool {
    tx: SyncSender<Task>,
}

impl PreviewIoPool {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel::<Task>(QUEUE_CAPACITY);
        let shared_rx = Arc::new(Mutex::new(rx));
        for idx in 0..WORKER_COUNT {
            let rx_clone = Arc::clone(&shared_rx);
            let _ = std::thread::Builder::new()
                .name(format!("preview-io-{idx}"))
                .spawn(move || loop {
                    let task = {
                        let guard = rx_clone.lock().unwrap_or_else(|e| e.into_inner());
                        guard.recv()
                    };
                    let Ok(task) = task else { break };
                    let Task { _permit, job, on_panic } = task;
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(on_panic));
                    }
                    drop(_permit);
                });
        }
        Self { tx }
    }

    /// Enqueues `job` under `grant`'s per-grant quota without blocking the CEF IO thread.
    /// Returns `false` if `grant` reached `MAX_GRANT_IO_JOBS`, the global queue is full, or the
    /// pool shut down; `on_panic` runs if `job` unwinds so the CEF callback is always cancelled.
    pub fn try_spawn_for(
        &self,
        grant: &Arc<PreviewGrant>,
        on_panic: impl FnOnce() + Send + 'static,
        job: impl FnOnce() + Send + 'static,
    ) -> bool {
        if !grant.try_acquire_io_slot(MAX_GRANT_IO_JOBS) {
            return false;
        }
        let permit = IoPermit(Arc::clone(grant));
        let task = Task {
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
