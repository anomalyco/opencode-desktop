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
                let _ = self.tx.send(WorkerCmd::Nudge);
            }
        }
    }
}

/// A socket and the worker's wake, waited on together: the thread sleeps in the kernel until the
/// socket can be read (or written, while a frame waits to go out), a command arrives, or a
/// deadline passes. Natively it is `polling` (IOCP and AFD on Windows, epoll, kqueue); the web
/// build has no sockets or threads for it.
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
        let _ = self.poller.notify();
    }

    /// Starts watching `socket` (non-blocking from here on).
    pub(crate) fn watch(&self, socket: &std::net::TcpStream) -> std::io::Result<()> {
        socket.set_nonblocking(true)?;
        // SAFETY: the worker owns the socket for as long as it waits on it and calls `forget`
        // before the socket closes.
        unsafe { self.poller.add(socket, polling::Event::readable(Self::KEY)) }
    }

    pub(crate) fn forget(&self, socket: &std::net::TcpStream) {
        let _ = self.poller.delete(socket);
    }

    /// Sleeps until the socket is readable (or writable, with `write`), a notify, or `timeout`.
    pub(crate) fn wait(&self, socket: &std::net::TcpStream, write: bool, timeout: Option<std::time::Duration>) {
        if self.poller.modify(socket, polling::Event::new(Self::KEY, true, write)).is_err() {
            return;
        }
        let mut events = polling::Events::new();
        let _ = self.poller.wait(&mut events, timeout);
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct SocketWait;

#[cfg(target_arch = "wasm32")]
impl SocketWait {
    pub(crate) fn new() -> std::io::Result<SocketWait> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "no sockets in the browser build"))
    }

    pub(crate) fn notify(&self) {}

    pub(crate) fn watch(&self, _: &std::net::TcpStream) -> std::io::Result<()> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "no sockets in the browser build"))
    }

    pub(crate) fn forget(&self, _: &std::net::TcpStream) {}

    pub(crate) fn wait(&self, _: &std::net::TcpStream, _: bool, _: Option<std::time::Duration>) {}
}
