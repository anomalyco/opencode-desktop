//! Bounded background I/O worker pool keeping local `oc-preview://` filesystem open, seek, and
//! read syscalls strictly off CEF's single global IO thread (`notes/review-rd281.md` Blocker #3).

use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

const WORKER_COUNT: usize = 4;
const QUEUE_CAPACITY: usize = 256;

type Job = Box<dyn FnOnce() + Send + 'static>;

#[derive(Clone)]
pub struct PreviewIoPool {
    tx: SyncSender<Job>,
}

impl PreviewIoPool {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel::<Job>(QUEUE_CAPACITY);
        let shared_rx = Arc::new(Mutex::new(rx));
        for idx in 0..WORKER_COUNT {
            let rx_clone = Arc::clone(&shared_rx);
            let _ = std::thread::Builder::new()
                .name(format!("preview-io-{idx}"))
                .spawn(move || loop {
                    let job = {
                        let guard = rx_clone.lock().unwrap();
                        guard.recv()
                    };
                    match job {
                        Ok(f) => f(),
                        Err(_) => break,
                    }
                });
        }
        Self { tx }
    }

    /// Enqueues `job` without blocking the caller (CEF IO thread). Returns `false` if the queue
    /// is full or shut down so the handler cancels the request cleanly instead of stalling CEF.
    pub fn try_spawn(&self, job: impl FnOnce() + Send + 'static) -> bool {
        match self.tx.try_send(Box::new(job)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }
}
