//! Server requests that run off the main thread and land back on it: a job reads or writes
//! through the live server and hands back what to do with the answer, which the next poll
//! applies. A headless run asks inline, so a snapshot sees the answer on its first frame; it
//! never writes (see [`App::can_write`]) and skips delayed jobs (polls of a sign-in).

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use opencode_kit::client::Server;
use rune::Tx;

use crate::{App, Source};

/// What a finished job does to the app.
pub type Apply = Box<dyn FnOnce(&mut App, &mut Tx) + Send>;

pub struct Jobs {
    tx: Sender<Apply>,
    rx: Receiver<Apply>,
}

impl Default for Jobs {
    fn default() -> Self {
        let (tx, rx) = channel();
        Jobs { tx, rx }
    }
}

impl App {
    /// Runs `work` against the live server; nothing in a fixture run.
    pub(crate) fn job(&self, work: impl FnOnce(&Server) -> Apply + Send + 'static) {
        self.job_after(Duration::ZERO, work);
    }

    /// [`App::job`] after `delay`, as a sign-in's next status poll.
    pub(crate) fn job_after(&self, delay: Duration, work: impl FnOnce(&Server) -> Apply + Send + 'static) {
        let Source::Live { server, .. } = &self.source else { return };
        if crate::headless() {
            if delay.is_zero() {
                let _ = self.jobs.tx.send(work(server));
            }
            return;
        }
        let (server, out, wake) = (server.clone(), self.jobs.tx.clone(), self.waker.clone());
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            let _ = out.send(work(&server));
            if let Some(w) = wake {
                w();
            }
        });
    }

    /// Where a thread of the app's own (not a server's) hands back what to apply.
    pub(crate) fn jobs_sender(&self) -> Sender<Apply> {
        self.jobs.tx.clone()
    }

    /// Applies `apply` on the next poll rather than now, so an answer known at once still lands
    /// after the transition that asked for it.
    pub(crate) fn later(&self, apply: Apply) {
        let _ = self.jobs.tx.send(apply);
        if let Some(w) = &self.waker {
            w();
        }
    }

    /// Hands what a worker thread finished back to the app's thread: the next poll applies it.
    pub(crate) fn deliverer(&self) -> impl Fn(Apply) + Send + 'static {
        let (out, wake) = (self.jobs.tx.clone(), self.waker.clone());
        move |apply| {
            let _ = out.send(apply);
            if let Some(w) = &wake {
                w();
            }
        }
    }

    /// Whether this window may change the server: live, with a person at it, not read-only.
    pub(crate) fn can_write(&self) -> bool {
        matches!(self.source, Source::Live { allow_send: true, .. }) && self.stage.is_none()
    }

    /// Lands every finished job.
    pub(crate) fn poll_jobs(&mut self, tx: &mut Tx) -> bool {
        let done: Vec<Apply> = self.jobs.rx.try_iter().collect();
        let any = !done.is_empty();
        for apply in done {
            apply(self, tx);
        }
        any
    }
}

/// What a write says in a window that can't write.
pub const READ_ONLY: &str = "This window is read-only. Relaunch without --read-only to act.";
