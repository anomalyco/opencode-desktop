//! Fair, bounded background worker pool for host-side `EngineMsg::PreviewFetch` streaming
//! (`notes/review-rd281.md` PR 2 Blocker #3 & 2nd read #3):
//! - Fixed `FETCH_WORKERS = 8` worker threads + `FETCH_QUEUE_CAP = 64` global queued tasks.
//! - Per-tab active worker cap `MAX_ACTIVE_PER_TAB = 3` (`< FETCH_WORKERS`) and per-server active
//!   worker cap `MAX_ACTIVE_PER_SERVER = 4` (`< FETCH_WORKERS`), plus `MAX_QUEUED_PER_TAB = 16`
//!   so neither a single hostile tab nor a stalling server can ever starve other preview tabs or
//!   servers, while multi-asset pages load all their subresources cleanly without spurious 429s.
//! - `revoke_tab(tab_id)` immediately purges queued tasks for closed/revoked tabs, and
//!   `std::panic::catch_unwind` isolation ensures a panic never kills a worker or leaks a permit.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};

use opencode_kit::client::PreviewAbort;

const FETCH_WORKERS: usize = 8;
const FETCH_QUEUE_CAP: usize = 64;
const MAX_ACTIVE_PER_TAB: usize = 3;
const MAX_ACTIVE_PER_SERVER: usize = 4;
const MAX_QUEUED_PER_TAB: usize = 16;

struct FetchTask {
    tab_id: u64,
    server_id: String,
    alive: Arc<AtomicBool>,
    abort: PreviewAbort,
    job: Box<dyn FnOnce() + Send + 'static>,
    on_cancel: Box<dyn FnOnce() + Send + 'static>,
    on_panic: Box<dyn FnOnce() + Send + 'static>,
}

#[derive(Default)]
struct PoolState {
    queue: VecDeque<FetchTask>,
    active_by_tab: HashMap<u64, usize>,
    active_by_server: HashMap<String, usize>,
    queued_by_tab: HashMap<u64, usize>,
}

impl PoolState {
    fn dec_tab(map: &mut HashMap<u64, usize>, key: u64) {
        if let Some(v) = map.get_mut(&key) {
            if *v <= 1 {
                map.remove(&key);
            } else {
                *v -= 1;
            }
        }
    }

    fn dec_server(map: &mut HashMap<String, usize>, key: &str) {
        if let Some(v) = map.get_mut(key) {
            if *v <= 1 {
                map.remove(key);
            } else {
                *v -= 1;
            }
        }
    }

    fn drain_dead(&mut self) -> Vec<Box<dyn FnOnce() + Send + 'static>> {
        let mut canceled = Vec::new();
        let mut i = 0;
        while i < self.queue.len() {
            let dead = !self.queue[i].alive.load(Ordering::Acquire) || self.queue[i].abort.is_aborted();
            if dead {
                if let Some(task) = self.queue.remove(i) {
                    Self::dec_tab(&mut self.queued_by_tab, task.tab_id);
                    canceled.push(task.on_cancel);
                }
            } else {
                i += 1;
            }
        }
        canceled
    }

    fn pop_runnable(&mut self) -> Option<FetchTask> {
        let idx = self.queue.iter().position(|t| {
            let tab_n = self.active_by_tab.get(&t.tab_id).copied().unwrap_or(0);
            let srv_n = self.active_by_server.get(&t.server_id).copied().unwrap_or(0);
            tab_n < MAX_ACTIVE_PER_TAB && srv_n < MAX_ACTIVE_PER_SERVER
        })?;
        let task = self.queue.remove(idx)?;
        Self::dec_tab(&mut self.queued_by_tab, task.tab_id);
        *self.active_by_tab.entry(task.tab_id).or_default() += 1;
        *self.active_by_server.entry(task.server_id.clone()).or_default() += 1;
        Some(task)
    }
}

struct ActiveGuard {
    shared: Arc<(Mutex<PoolState>, Condvar)>,
    tab_id: u64,
    server_id: String,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let mut st = self.shared.0.lock().unwrap_or_else(PoisonError::into_inner);
        PoolState::dec_tab(&mut st.active_by_tab, self.tab_id);
        PoolState::dec_server(&mut st.active_by_server, &self.server_id);
        drop(st);
        self.shared.1.notify_all();
    }
}

pub(super) struct PreviewFetchPool {
    shared: Arc<(Mutex<PoolState>, Condvar)>,
}

impl PreviewFetchPool {
    pub(super) fn global() -> &'static Self {
        static POOL: OnceLock<PreviewFetchPool> = OnceLock::new();
        POOL.get_or_init(|| {
            let shared = Arc::new((Mutex::new(PoolState::default()), Condvar::new()));
            for idx in 0..FETCH_WORKERS {
                let s = Arc::clone(&shared);
                let _ = std::thread::Builder::new()
                    .name(format!("preview-fetch-{idx}"))
                    .spawn(move || loop {
                        let (task, canceled) = {
                            let mut st = s.0.lock().unwrap_or_else(PoisonError::into_inner);
                            loop {
                                let dead = st.drain_dead();
                                if let Some(task) = st.pop_runnable() {
                                    break (task, dead);
                                }
                                if !dead.is_empty() {
                                    drop(st);
                                    for cb in dead {
                                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(cb));
                                    }
                                    st = s.0.lock().unwrap_or_else(PoisonError::into_inner);
                                    continue;
                                }
                                st = s.1.wait(st).unwrap_or_else(PoisonError::into_inner);
                            }
                        };
                        for cb in canceled {
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(cb));
                        }
                        let guard = ActiveGuard {
                            shared: Arc::clone(&s),
                            tab_id: task.tab_id,
                            server_id: task.server_id,
                        };
                        let FetchTask { job, on_panic, .. } = task;
                        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(on_panic));
                        }
                        drop(guard);
                    });
            }
            Self { shared }
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_spawn(
        &self,
        tab_id: u64,
        server_id: String,
        alive: Arc<AtomicBool>,
        abort: PreviewAbort,
        on_cancel: impl FnOnce() + Send + 'static,
        on_panic: impl FnOnce() + Send + 'static,
        job: impl FnOnce() + Send + 'static,
    ) -> bool {
        let mut st = self.shared.0.lock().unwrap_or_else(PoisonError::into_inner);
        let dead = st.drain_dead();
        let tab_total = st.queued_by_tab.get(&tab_id).copied().unwrap_or(0)
            + st.active_by_tab.get(&tab_id).copied().unwrap_or(0);
        if st.queue.len() >= FETCH_QUEUE_CAP || tab_total >= MAX_QUEUED_PER_TAB {
            drop(st);
            for cb in dead {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(cb));
            }
            return false;
        }
        *st.queued_by_tab.entry(tab_id).or_default() += 1;
        st.queue.push_back(FetchTask {
            tab_id,
            server_id,
            alive,
            abort,
            job: Box::new(job),
            on_cancel: Box::new(on_cancel),
            on_panic: Box::new(on_panic),
        });
        drop(st);
        self.shared.1.notify_one();
        for cb in dead {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(cb));
        }
        true
    }

    pub(super) fn revoke_tab(&self, tab_id: u64) {
        let canceled: Vec<Box<dyn FnOnce() + Send + 'static>> = {
            let mut st = self.shared.0.lock().unwrap_or_else(PoisonError::into_inner);
            let mut out = Vec::new();
            let mut i = 0;
            while i < st.queue.len() {
                if st.queue[i].tab_id == tab_id {
                    if let Some(task) = st.queue.remove(i) {
                        PoolState::dec_tab(&mut st.queued_by_tab, task.tab_id);
                        out.push(task.on_cancel);
                    }
                } else {
                    i += 1;
                }
            }
            out
        };
        if !canceled.is_empty() {
            self.shared.1.notify_all();
            for cb in canceled {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(cb));
            }
        }
    }
}
