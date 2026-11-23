//! The post rate's queue and the GPU work that waits in it (`widgets-live.md`, Post rate;
//! `widgets-gpu.md`): counted posts out as tokens come, GPU batches behind them in order, and the
//! run's GPU canvases.

use std::collections::VecDeque;
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use psychoscript::HostValue;

use super::super::Post;
use super::super::gpu::RunGpu;
use super::{Link, clock};

/// Host work inside a program's call, until dropped (`Watch::host`).
pub(super) struct HostWork<'a>(pub(super) &'a Link);

impl Drop for HostWork<'_> {
    fn drop(&mut self) {
        let mut st = self.0.lock();
        st.watch.host = st.watch.host.saturating_sub(1);
        st.watch.last_stop = Instant::now();
    }
}

/// What waits for the post rate (`widgets-live.md`, Post rate).
pub(in crate::live) enum Wait {
    /// A `render` (no key) or canvas key's `draw`, for a token.
    Counted(Option<String>, Post),
    /// A GPU batch, its bytes copied: it takes no token, but never overtakes a waiting post.
    /// `frame`: it rode a frame request, which goes out after it.
    Batch { ops: Vec<widget_gpu::Arg>, bytes: Vec<u8>, frame: bool },
    /// A batch whose shape failed the host checks, with why (and its ops, when a list).
    Refused { ops: Option<Vec<widget_gpu::Arg>>, message: String, frame: bool },
    /// A `canvas` size the program heard, for the GPU after the batches ahead of it.
    Resize(String, u32, u32),
}

impl Link {
    /// The program posted a render (no `key`) or canvas `key`'s draw, counted toward the post rate
    /// (`widgets-live.md`, Post rate): with nothing waiting and a token, it takes the token and
    /// goes to the client at once. Otherwise it waits, in place of the waiting render or the
    /// waiting draw of its canvas, or last; the clock lets the waiting posts out as tokens come.
    pub fn counted(&self, post: Post, key: Option<String>) {
        let mut st = self.lock();
        st.refill(Instant::now(), self.per_second);
        if st.waiting.is_empty() && st.token() {
            st.bucket.0 -= 1.0;
            let first = st.out(post);
            drop(st);
            if first {
                self.signal();
            }
            return;
        }
        let render = matches!(post, Post::Render(_));
        let same = st.waiting.iter_mut().find(|w| match w {
            Wait::Counted(_, Post::Render(_)) => render,
            Wait::Counted(k, Post::Draw { .. }) => !render && *k == key,
            _ => false,
        });
        match same {
            Some(waiting) => *waiting = Wait::Counted(key, post),
            None => {
                let first = st.waiting.is_empty();
                st.waiting.push_back(Wait::Counted(key, post));
                drop(st);
                if first {
                    clock().poke();
                }
            }
        }
    }

    /// The clock's look at `now` for the post rate: waiting posts go to the client, in order, one
    /// per token. When the next one's token comes.
    pub(super) fn release_due(&self, now: Instant) -> Option<Instant> {
        if self.lock().waiting.is_empty() {
            return None;
        }
        let _order = self.order.lock().unwrap_or_else(PoisonError::into_inner);
        let mut first = false;
        loop {
            let mut st = self.lock();
            st.refill(now, self.per_second);
            // A GPU batch waiting among the posts takes no token.
            let next = match st.waiting.front() {
                Some(Wait::Counted(..)) if !st.token() => break,
                Some(_) => st.waiting.pop_front(),
                None => break,
            };
            match next {
                Some(Wait::Counted(_, post)) => {
                    st.bucket.0 -= 1.0;
                    first |= st.out(post);
                }
                Some(w) => {
                    drop(st);
                    self.apply(w);
                }
                None => break,
            }
        }
        let st = self.lock();
        let next = (!st.waiting.is_empty()).then(|| now + Duration::from_secs_f64((1.0 - st.bucket.0).max(0.0) / self.per_second));
        drop(st);
        if first {
            self.signal();
        }
        next
    }

    /// What waited goes out at once, in order: posts to the client, GPU work replayed.
    pub(super) fn flush(&self, pending: VecDeque<Wait>) {
        if pending.is_empty() {
            return;
        }
        let _order = self.order.lock().unwrap_or_else(PoisonError::into_inner);
        for w in pending {
            match w {
                Wait::Counted(_, post) => {
                    let mut st = self.lock();
                    let first = st.out(post);
                    drop(st);
                    if first {
                        self.signal();
                    }
                }
                w => self.apply(w),
            }
        }
    }

    /// GPU work that waited, now (`order` held).
    fn apply(&self, w: Wait) {
        let Some(gpu) = self.gpu() else { return };
        match w {
            Wait::Counted(..) => {}
            Wait::Batch { ops, bytes, frame } => self.replayed(&gpu, gpu.apply(&ops, &bytes), frame),
            Wait::Refused { ops, message, frame } => self.replayed(&gpu, gpu.refuse(ops.as_deref(), message), frame),
            Wait::Resize(key, w, h) => gpu.resize(&key, w, h),
        }
    }

    /// A batch replayed (or refused): its error to the client, then the frame request it rode.
    fn replayed(&self, gpu: &RunGpu, error: Option<widget_gpu::GpuError>, frame: bool) {
        if let Some(e) = error {
            self.post(Post::Gpu(e));
        }
        if frame {
            gpu.end_frame();
            self.post(Post::Frame);
        }
    }

    /// The run's GPU canvases, once it started.
    pub fn gpu(&self) -> Option<Arc<RunGpu>> {
        self.gpu.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The program posted a GPU batch (`{ ops, bytes }`; `frame`: in a frame request, which goes
    /// out after it): replayed now with its bytes in place, or, behind posts waiting for the rate,
    /// queued with them.
    pub fn gpu_batch(&self, batch: &HostValue, frame: bool) {
        let Some(gpu) = self.gpu() else {
            if frame {
                self.post(Post::Frame);
            }
            return;
        };
        let _order = self.order.lock().unwrap_or_else(PoisonError::into_inner);
        let waits = {
            let mut st = self.lock();
            st.watch.host += 1;
            !st.waiting.is_empty()
        };
        // Replaying is the host's work, not the program's: it doesn't count toward the 50 ms.
        let _host = HostWork(self);
        crate::live::gpu::tapped(Some(batch), frame);
        let queued = match crate::live::gpu::with_batch(batch, |ops, bytes| {
            if waits {
                return Some(Wait::Batch { ops: ops.to_vec(), bytes: bytes.to_vec(), frame });
            }
            self.replayed(&gpu, gpu.apply(ops, bytes), frame);
            None
        }) {
            Ok(queued) => queued,
            Err((ops, message)) if waits => Some(Wait::Refused { ops, message, frame }),
            Err((ops, message)) => {
                self.replayed(&gpu, gpu.refuse(ops.as_deref(), message), frame);
                None
            }
        };
        if let Some(w) = queued {
            self.lock().waiting.push_back(w);
        }
    }

    /// A frame request without a batch: it goes at once, and the GPU's frame limits count again.
    pub fn bare_frame(&self) {
        crate::live::gpu::tapped(None, true);
        if let Some(gpu) = self.gpu() {
            gpu.end_frame();
        }
        self.post(Post::Frame);
    }

    /// The program heard canvas `key`'s size: the GPU takes it after the batches posted before.
    pub fn resized(&self, key: String, width: u32, height: u32) {
        let Some(gpu) = self.gpu() else { return };
        let _order = self.order.lock().unwrap_or_else(PoisonError::into_inner);
        let mut st = self.lock();
        if st.waiting.iter().any(|w| matches!(w, Wait::Batch { .. } | Wait::Refused { .. })) {
            st.waiting.push_back(Wait::Resize(key, width, height));
            return;
        }
        drop(st);
        gpu.resize(&key, width, height);
    }

    /// A run from `main` starts its GPU canvases afresh, on its own thread; a loss tells it.
    pub(super) fn start_gpu(&self) {
        let me = self.me.clone();
        let gpu = RunGpu::new(
            move |lost| {
                if let Some(link) = me.upgrade() {
                    link.deliver(lost);
                }
            },
            self.setup.notify.clone(),
        );
        *self.gpu.lock().unwrap_or_else(PoisonError::into_inner) = Some(gpu);
    }
}
