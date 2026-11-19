//! The way into a terminal's worker: commands on a channel, and a wake for a worker that sleeps
//! on something else (the persistent PTY's socket) rather than on the channel.

use std::sync::Arc;
use std::sync::mpsc::Sender;

use crate::pty::WorkerCmd;

#[derive(Clone)]
pub(crate) struct Inbox {
    tx: Sender<WorkerCmd>,
    waker: Option<Arc<SocketWait>>,
}

impl Inbox {
    pub(crate) fn channel(tx: Sender<WorkerCmd>) -> Inbox {
        Inbox { tx, waker: None }
    }

    pub(crate) fn socket(tx: Sender<WorkerCmd>, wait: Arc<SocketWait>) -> Inbox {
        Inbox { tx, waker: Some(wait) }
    }

    pub(crate) fn send(&self, cmd: WorkerCmd) -> bool {
        let sent = self.tx.send(cmd).is_ok();
        if sent && let Some(w) = &self.waker {
            w.notify();
        }
        sent
    }

    /// Wakes the worker with nothing to do but look again (the UI read its snapshot).
    pub(crate) fn nudge(&self) {
        match &self.waker {
            Some(w) => w.notify(),
            None => {
                self.tx.send(WorkerCmd::Nudge).ok();
            }
        }
    }
}

/// A socket and the worker's wake, waited on together: the thread sleeps in the kernel until the
/// socket can be read (or written, while a frame waits to go out), a command arrives, or a
/// deadline passes. Natively it is `polling` (IOCP and AFD on Windows, epoll, kqueue); in a
/// browser, a task's wake (below).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct SocketWait {
    poller: polling::Poller,
}

#[cfg(not(target_arch = "wasm32"))]
impl SocketWait {
    const KEY: usize = 1;

    pub(crate) fn new() -> std::io::Result<SocketWait> {
        Ok(SocketWait { poller: polling::Poller::new()? })
    }

    pub(crate) fn notify(&self) {
        self.poller.notify().ok();
    }

    /// Starts watching `socket` (non-blocking from here on).
    pub(crate) fn watch(&self, socket: &std::net::TcpStream) -> std::io::Result<()> {
        socket.set_nonblocking(true)?;
        // SAFETY: the worker owns the socket for as long as it waits on it and calls `forget`
        // before the socket closes.
        unsafe { self.poller.add(socket, polling::Event::readable(Self::KEY)) }
    }

    pub(crate) fn forget(&self, socket: &std::net::TcpStream) {
        self.poller.delete(socket).ok();
    }

    /// Sleeps until the socket is readable (or writable, with `write`), a notify, or `timeout`.
    pub(crate) fn wait(&self, socket: &std::net::TcpStream, write: bool, timeout: Option<std::time::Duration>) {
        if self.poller.modify(socket, polling::Event::new(Self::KEY, true, write)).is_err() {
            return;
        }
        let mut events = polling::Events::new();
        self.poller.wait(&mut events, timeout).ok();
    }
}

/// The web's: the page's socket and the worker's commands both wake one task on the page's event
/// loop (`persistent_pty_web.rs`), which sleeps until one does or a deadline passes.
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
pub(crate) struct SocketWait {
    woken: std::sync::atomic::AtomicBool,
    task: std::sync::Mutex<Option<std::task::Waker>>,
}

#[cfg(target_arch = "wasm32")]
impl SocketWait {
    pub(crate) fn new() -> std::io::Result<SocketWait> {
        Ok(SocketWait::default())
    }

    pub(crate) fn notify(&self) {
        self.woken.store(true, std::sync::atomic::Ordering::Release);
        if let Some(task) = self.task.lock().unwrap_or_else(|e| e.into_inner()).take() {
            task.wake();
        }
    }

    /// Until a notify (a command, or the socket's listeners), or `timeout`.
    pub(crate) async fn wait(&self, timeout: Option<std::time::Duration>) {
        use std::sync::atomic::Ordering;
        use std::task::Poll;
        let mut timed = false;
        let expired = std::rc::Rc::new(std::cell::Cell::new(false));
        std::future::poll_fn(|cx| {
            if self.woken.swap(false, Ordering::AcqRel) || expired.get() {
                return Poll::Ready(());
            }
            *self.task.lock().unwrap_or_else(|e| e.into_inner()) = Some(cx.waker().clone());
            if !timed && let Some(delay) = timeout {
                timed = true;
                let (expired, task) = (expired.clone(), cx.waker().clone());
                set_timer(delay, move || {
                    expired.set(true);
                    task.wake();
                });
            }
            Poll::Pending
        })
        .await;
    }
}

/// Runs `f` once `delay` has passed, from the page's timer (freed when it fires).
#[cfg(target_arch = "wasm32")]
fn set_timer(delay: std::time::Duration, f: impl FnOnce() + 'static) {
    use wasm_bindgen::JsCast;
    let callback = wasm_bindgen::closure::Closure::once_into_js(f);
    if let Some(window) = web_sys::window() {
        window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                callback.unchecked_ref(),
                delay.as_millis().min(i32::MAX as u128) as i32,
            )
            .ok();
    }
}