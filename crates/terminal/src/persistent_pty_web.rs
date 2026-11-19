//! A persistent PTY attachment from a page: the same worker as natively (`Worker::step`, the
//! connection and size machines), on a task of the page's event loop instead of a thread. The
//! snapshot and the connect ticket are HTTP reads, made on `rune::task` work where a request
//! waits without holding the page; the socket is the page's own `WebSocket`, whose listeners
//! queue what arrives and wake the task (`SocketWait`).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use opencode_kit::client::Server;
use opencode_kit::persistent_pty::attachment_id;
use rune::Waker;
use tungstenite::Message;
use tungstenite::protocol::CloseFrame;
use tungstenite::protocol::frame::coding::CloseCode;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use super::{Connection, Failure, Flow, Link, Sizing, Worker, errored, failed, fetch, finish, restore};
use crate::inbox::SocketWait;
use crate::pty::{SessionStatus, SharedState, TerminalConfig, WorkerCmd};

/// Starts the attachment's task; it ends with the attachment, as the native thread does.
#[allow(clippy::too_many_arguments)]
pub(crate) fn start(
    server: Server,
    pty_id: String,
    config: TerminalConfig,
    observer: bool,
    cmd_rx: Receiver<WorkerCmd>,
    wait: Arc<SocketWait>,
    shared: Arc<SharedState>,
    waker: Waker,
) {
    wasm_bindgen_futures::spawn_local(async move {
        let id = attachment_id();
        // The reads, on task work; their answer comes back with a wake.
        let (tx, rx) = channel();
        let (read_id, read_wait) = (id.clone(), wait.clone());
        let started = rune::task::spawn("terminal-snapshot", move || {
            tx.send(fetch(&server, &pty_id, &read_id, observer)).ok();
            read_wait.notify();
        });
        if started.is_err() {
            return finish(&shared, &waker, failed(Failure::Failed));
        }
        let fetched = loop {
            match rx.try_recv() {
                Ok(fetched) => break fetched,
                Err(TryRecvError::Disconnected) => return finish(&shared, &waker, failed(Failure::Failed)),
                Err(TryRecvError::Empty) => wait.wait(None).await,
            }
        };
        let (snapshot, url) = match fetched {
            Ok(start) => start,
            Err(status) => return finish(&shared, &waker, status),
        };
        let Some((mut screen, mut grid)) = restore(&config, &snapshot, &shared, &waker) else { return };
        let mut conn = Connection::new();
        let mut sizing = Sizing::Unmeasured;
        let Some(mut socket) = PageSocket::open(&url, wait.clone()) else { return errored(&mut conn, &shared, &waker) };
        while !socket.state.borrow().open {
            if socket.state.borrow().closed {
                return errored(&mut conn, &shared, &waker);
            }
            wait.wait(None).await;
        }
        shared.set_status(SessionStatus::Connected);
        waker();

        let console = opencode_kit::path::windows(&snapshot.info.cwd);
        let mut worker = Worker { conn: &mut conn, sizing: &mut sizing, grid: &mut grid, screen: &mut screen, shared: &shared, waker: &waker, id: &id, console, unflushed: false };
        let end = loop {
            let (flow, more) = worker.step(&cmd_rx, &mut socket);
            if let Flow::End(status) = flow {
                break status;
            }
            // Until the socket has data, a command or a read of the snapshot wakes the task, or
            // a deadline passes; output left over from the last budget, at once.
            let timeout = if more { Some(std::time::Duration::ZERO) } else { worker.next_deadline() };
            wait.wait(timeout).await;
        };
        socket.close();
        if let Some(status) = end {
            finish(&shared, &waker, status);
        }
    });
}

/// What the socket's listeners have handed the task.
#[derive(Default)]
struct State {
    open: bool,
    closed: bool,
    messages: VecDeque<Message>,
}

/// The page's `WebSocket`, read as tungstenite's is: messages in arrival order, then the close.
struct PageSocket {
    ws: web_sys::WebSocket,
    state: Rc<RefCell<State>>,
    _listeners: [Closure<dyn FnMut(web_sys::Event)>; 4],
}

impl PageSocket {
    fn open(url: &str, wait: Arc<SocketWait>) -> Option<PageSocket> {
        let ws = web_sys::WebSocket::new(url).ok()?;
        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let state = Rc::new(RefCell::new(State::default()));
        let listener = |on: fn(&mut State, web_sys::Event)| {
            let (state, wait) = (state.clone(), wait.clone());
            Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                on(&mut state.borrow_mut(), event);
                wait.notify();
            })
        };
        let listeners = [
            listener(|s, _| s.open = true),
            listener(|s, event| {
                let Some(m) = event.dyn_ref::<web_sys::MessageEvent>() else { return };
                let data = m.data();
                let message = match data.as_string() {
                    Some(text) => Message::text(text),
                    None => Message::binary(js_sys::Uint8Array::new(&data).to_vec()),
                };
                s.messages.push_back(message);
            }),
            // A close after an error says the rest (a dropped connection closes with 1006).
            listener(|_, _| {}),
            listener(|s, event| {
                if s.closed {
                    return;
                }
                s.closed = true;
                let frame = event.dyn_ref::<web_sys::CloseEvent>().map(|c| CloseFrame { code: CloseCode::from(c.code()), reason: c.reason().into() });
                s.messages.push_back(Message::Close(frame));
            }),
        ];
        ws.set_onopen(Some(listeners[0].as_ref().unchecked_ref()));
        ws.set_onmessage(Some(listeners[1].as_ref().unchecked_ref()));
        ws.set_onerror(Some(listeners[2].as_ref().unchecked_ref()));
        ws.set_onclose(Some(listeners[3].as_ref().unchecked_ref()));
        Some(PageSocket { ws, state, _listeners: listeners })
    }

    fn close(&self) {
        self.ws.set_onopen(None);
        self.ws.set_onmessage(None);
        self.ws.set_onerror(None);
        self.ws.set_onclose(None);
        self.ws.close_with_code(1000).ok();
    }
}

impl Link for PageSocket {
    fn read(&mut self) -> tungstenite::Result<Message> {
        let mut state = self.state.borrow_mut();
        match state.messages.pop_front() {
            Some(message) => Ok(message),
            None if state.closed => Err(tungstenite::Error::AlreadyClosed),
            None => Err(tungstenite::Error::Io(ErrorKind::WouldBlock.into())),
        }
    }

    fn send(&mut self, message: Message) -> tungstenite::Result<()> {
        let sent = match &message {
            Message::Binary(bytes) => self.ws.send_with_u8_array(&bytes[..]),
            Message::Text(text) => self.ws.send_with_str(text.as_str()),
            _ => Ok(()),
        };
        sent.map_err(|_| tungstenite::Error::AlreadyClosed)
    }

    /// The browser sends what it was given on its own.
    fn flush(&mut self) -> tungstenite::Result<()> {
        Ok(())
    }
}
