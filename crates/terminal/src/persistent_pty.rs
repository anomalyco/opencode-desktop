//! One attachment to an OpenCode session terminal (a persistent PTY), as Desktop's terminal pane
//! makes it: the HTTP snapshot's checkpoint restores the screen, the socket replays what came
//! after, then input and size frames go out at the pane's size. The server owns the grid: the
//! emulator only takes the canonical size it announces (`attached`, `resized`), and it answers
//! terminal queries itself, so the emulator's replies are dropped. An attachment never
//! reconnects; a failure stays on screen until the pane mounts a new one.
//!
//! The attachment and the pane's size are explicit machines (`connection.rs`:
//! `connection-machine.ts`, `pane-machine.ts`), and so is what the UI is owed (`publish.rs`);
//! this worker feeds them and carries out what they return. It sleeps on the socket and its
//! commands together (`inbox.rs`): an idle terminal's thread never runs.

use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use rune::time::Instant;

use opencode_kit::client::Server;
use opencode_kit::persistent_pty::attachment_id;
use rune::Waker;
use rune::machine::Effects;
use serde_json::Value;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::connection::{Connection, Effect, Event, Failure, Size, Sizing};
use crate::core::VtCore;
use crate::inbox::SocketWait;
use crate::pty::{
    COALESCE_BUDGET, COALESCE_MAX_BYTES, Screen, SessionStatus, SharedState, TerminalConfig, WorkerCmd,
    apply_worker_cmd, build_core,
};
use crate::types::VtEvent;

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

/// Desktop opens its terminals with a blinking bar (`cursorStyle: "bar"`); a program's own
/// DECSCUSR in the checkpoint or stream still wins.
const DEFAULT_CURSOR: &[u8] = b"\x1b[5 q";

/// Why an attachment stopped, as the pane words it (`terminal.tsx` `failureText`).
pub const DISCONNECTED: &str = "Terminal disconnected";
pub const CONNECTION_FAILED: &str = "Terminal connection failed";
pub const OUTDATED: &str = "This server is too old for terminals; update OpenCode on it";

/// The emulator's grid: the server's canonical size, at the pane's cell size.
struct Grid {
    cell_px: (u32, u32),
    size: Size,
}

impl Grid {
    fn resize(&mut self, core: &mut VtCore, size: Size) {
        self.size = size;
        core.resize(size.0, size.1, self.cell_px.0, self.cell_px.1);
    }
}

/// How a step of the worker ended.
enum Flow {
    Continue,
    /// The worker ends with this status (`None`: a shutdown, nothing to say).
    End(Option<SessionStatus>),
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_persistent_worker(
    server: Server,
    pty_id: String,
    config: TerminalConfig,
    observer: bool,
    cmd_rx: Receiver<WorkerCmd>,
    wait: Arc<SocketWait>,
    shared: Arc<SharedState>,
    waker: Waker,
) {
    let Ok(core) = build_core(&config, None) else {
        return;
    };
    let mut screen = Screen::new(core);
    let id = attachment_id();
    let mut conn = Connection::new();
    let mut sizing = Sizing::Unmeasured;
    let mut grid = Grid { cell_px: (config.cell_w_px.max(1), config.cell_h_px.max(1)), size: (config.cols.max(1), config.rows.max(1)) };

    let snapshot = match server.terminal_snapshot(&pty_id) {
        Ok(s) => s,
        Err(e) => return finish(&shared, &waker, gone(&e)),
    };
    grid.resize(&mut screen.core, snapshot.info.size);
    screen.core.feed(DEFAULT_CURSOR);
    screen.core.feed(&snapshot.checkpoint);
    let _ = screen.core.take_pty_writes();
    publish(&mut screen, &shared, &waker);

    let url = match server.terminal_connect_url(&pty_id, snapshot.info.tail, &id, observer) {
        Ok(u) => u,
        Err(e) => return finish(&shared, &waker, gone(&e)),
    };
    let mut ws = match tungstenite::connect(&url) {
        Ok((socket, _)) => socket,
        // Setting the socket up failed (`socket.errored`).
        Err(_) => {
            for effect in conn.step(Event::SocketErrored) {
                if let Effect::Fail(failure) = effect {
                    return finish(&shared, &waker, failed(failure));
                }
            }
            return;
        }
    };
    if wait.watch(tcp(&ws)).is_err() {
        return finish(&shared, &waker, failed(Failure::Failed));
    }
    shared.set_status(SessionStatus::Connected);
    waker();

    let mut worker = Worker { conn: &mut conn, sizing: &mut sizing, grid: &mut grid, screen: &mut screen, shared: &shared, waker: &waker, id: &id, unflushed: false };
    let end = loop {
        let (flow, more) = worker.step(&cmd_rx, &mut ws);
        if let Flow::End(status) = flow {
            break status;
        }
        // Asleep in the kernel until the socket has data (or room, while a frame waits to go
        // out), a command or a read of the snapshot wakes it, or a deadline passes.
        let now = Instant::now();
        let deadline = [worker.screen.publish.deadline(), worker.sizing.deadline()].into_iter().flatten().min();
        let timeout = if more { Some(std::time::Duration::ZERO) } else { deadline.map(|at| at.saturating_duration_since(now)) };
        wait.wait(tcp(&ws), worker.unflushed, timeout);
    };
    wait.forget(tcp(&ws));
    match end {
        Some(status) => finish(&shared, &waker, status),
        None => {
            let _ = tcp(&ws).set_nonblocking(false);
            let _ = ws.close(None);
            let _ = ws.flush();
        }
    }
}

struct Worker<'a> {
    conn: &'a mut Connection,
    sizing: &'a mut Sizing,
    grid: &'a mut Grid,
    screen: &'a mut Screen,
    shared: &'a SharedState,
    waker: &'a Waker,
    id: &'a str,
    /// A frame (input, a pong) is queued in the socket: wait for room to send it.
    unflushed: bool,
}

impl Worker<'_> {
    /// Takes every command, then the socket's output up to the coalescing budget; `more` says
    /// the socket still had output when the budget ran out.
    fn step(&mut self, cmd_rx: &Receiver<WorkerCmd>, ws: &mut Socket) -> (Flow, bool) {
        let mut dirty = false;
        let mut events = Vec::new();
        let mut show = Vec::new();
        let now = Instant::now();
        loop {
            match cmd_rx.try_recv() {
                Ok(WorkerCmd::Shutdown) | Err(TryRecvError::Disconnected) => return (Flow::End(None), false),
                Ok(WorkerCmd::Resize { cols, rows, cell_w_px, cell_h_px }) => {
                    self.grid.cell_px = (cell_w_px.max(1), cell_h_px.max(1));
                    let size = self.grid.size;
                    self.grid.resize(&mut self.screen.core, size);
                    events.extend(self.sizing.sampled((cols.max(1), rows.max(1)), now).map(Event::SizeChanged));
                    dirty = true;
                }
                Ok(WorkerCmd::SetTheme(theme)) => {
                    self.screen.core.set_theme(theme);
                    let _ = self.screen.core.take_pty_writes();
                    dirty = true;
                }
                Ok(WorkerCmd::Interact) => events.push(Event::Interacted),
                Ok(WorkerCmd::Show(reply)) => show.push(reply),
                Ok(cmd) => {
                    let (mut input, mut resized) = (Vec::new(), None);
                    if !apply_worker_cmd(cmd, &mut self.screen.core, self.shared, &mut input, &mut resized, &mut dirty) {
                        return (Flow::End(None), false);
                    }
                    if !input.is_empty() {
                        events.push(Event::Input(input));
                    }
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        events.extend(self.sizing.due(now).map(Event::SizeChanged));
        for event in events {
            let effects = self.conn.step(event);
            if let Some(flow) = self.carry_out(effects, ws) {
                return (flow, false);
            }
        }

        let burst = Instant::now();
        let mut fed = 0usize;
        let mut more = false;
        loop {
            if fed >= COALESCE_MAX_BYTES || burst.elapsed() >= COALESCE_BUDGET {
                more = true;
                break;
            }
            let event = match ws.read() {
                Ok(Message::Binary(bytes)) if self.conn.accepts_output() => {
                    fed += bytes.len();
                    self.screen.core.feed(&bytes);
                    let _ = self.screen.core.take_pty_writes();
                    dirty = true;
                    continue;
                }
                Ok(Message::Text(text)) => {
                    let Ok(msg) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                    if msg["type"].as_str() == Some("title_changed") {
                        let title = msg["title"].as_str().unwrap_or_default().to_owned();
                        self.shared.events.lock().push(VtEvent::TitleChanged(title));
                        dirty = true;
                        continue;
                    }
                    let Some(event) = message(&msg, self.id) else { continue };
                    event
                }
                // The server closes with 4404 when the terminal is gone: it exited.
                Ok(Message::Close(frame)) if frame.as_ref().is_some_and(|f| u16::from(f.code) == 4404) => Event::Exited { code: None },
                Ok(Message::Close(_)) => Event::SocketClosed,
                Ok(_) => continue,
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => break,
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::Interrupted => continue,
                // A dropped connection (the server went away) is a close, as Chromium reports it:
                // Desktop says "Terminal disconnected".
                Err(_) => Event::SocketClosed,
            };
            let ended = matches!(event, Event::Exited { .. } | Event::SocketClosed);
            let effects = self.conn.step(event);
            dirty = true;
            if let Some(flow) = self.carry_out(effects, ws) {
                return (flow, false);
            }
            if let (true, Connection::Exited { code }) = (ended, &*self.conn) {
                let code = *code;
                publish_last(self.screen, self.shared, self.waker);
                return (Flow::End(Some(SessionStatus::Exited { code })), false);
            }
        }

        if self.unflushed {
            self.unflushed = match ws.flush() {
                Ok(()) => false,
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => true,
                Err(_) => return (Flow::End(Some(failed(Failure::Disconnected))), false),
            };
        }

        if dirty {
            let events: Vec<VtEvent> =
                self.screen.core.take_events().into_iter().filter(|e| !matches!(e, VtEvent::TitleChanged(_))).collect();
            if !events.is_empty() {
                self.shared.events.lock().extend(events);
            }
            self.screen.changed(self.shared, self.waker);
        } else {
            self.screen.woke(self.shared, self.waker);
        }
        for reply in show {
            self.screen.show(self.shared, self.waker, reply);
        }
        (Flow::Continue, more)
    }

    /// Carries out the connection's effects; returns how a failure ends the worker.
    fn carry_out(&mut self, effects: Effects<Effect>, ws: &mut Socket) -> Option<Flow> {
        for effect in effects {
            match effect {
                Effect::Resize { size, checkpoint, reset } => {
                    let core = &mut self.screen.core;
                    self.grid.resize(core, size);
                    if reset {
                        core.feed(b"\x1bc");
                        core.feed(DEFAULT_CURSOR);
                    }
                    if let Some(checkpoint) = checkpoint {
                        core.feed(&checkpoint);
                    }
                    let _ = core.take_pty_writes();
                }
                Effect::Send { size, input } => match ws.send(Message::Binary(frame(size, input.as_deref()).into())) {
                    Ok(()) => {}
                    // Queued in the socket; it goes out once there's room.
                    Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => self.unflushed = true,
                    Err(_) => {
                        publish_last(self.screen, self.shared, self.waker);
                        return Some(Flow::End(Some(failed(Failure::Disconnected))));
                    }
                },
                Effect::Fail(failure) => {
                    publish_last(self.screen, self.shared, self.waker);
                    return Some(Flow::End(Some(failed(failure))));
                }
            }
        }
        None
    }
}

fn tcp(ws: &Socket) -> &TcpStream {
    match ws.get_ref() {
        MaybeTlsStream::Plain(tcp) => tcp,
        #[cfg(not(target_arch = "wasm32"))]
        MaybeTlsStream::Rustls(tls) => tls.get_ref(),
        _ => unreachable!("tungstenite connects plain or rustls streams"),
    }
}

/// A socket message as the connection's event, or `None` for one it doesn't take.
fn message(msg: &Value, id: &str) -> Option<Event> {
    Some(match msg["type"].as_str()? {
        "attached" => Event::Attached {
            controller: msg["role"].as_str() == Some("controller"),
            protocol: msg["inputProtocol"].as_u64(),
            size: size_of(&msg["info"]["size"]),
        },
        "resized" => Event::Resized {
            size: size_of(msg)?,
            checkpoint: msg["checkpoint"].as_str().and_then(opencode_kit::model::base64_decode),
        },
        "replay_complete" => Event::ReplayComplete,
        "controller_changed" => Event::ControllerChanged { ours: msg["attachmentID"].as_str() == Some(id) },
        "exited" => Event::Exited { code: msg["exitCode"].as_u64().and_then(|c| u32::try_from(c).ok()) },
        _ => return None,
    })
}

fn failed(failure: Failure) -> SessionStatus {
    SessionStatus::Failed(
        match failure {
            Failure::Disconnected => DISCONNECTED,
            Failure::Failed => CONNECTION_FAILED,
            Failure::Outdated => OUTDATED,
        }
        .into(),
    )
}

/// A frame for the server: the pane's size alone takes control; with `data`, it types.
fn frame(size: Size, data: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + data.map_or(0, <[u8]>::len));
    out.push(u8::from(data.is_some()));
    out.extend_from_slice(&size.0.to_be_bytes());
    out.extend_from_slice(&size.1.to_be_bytes());
    if let Some(d) = data {
        out.extend_from_slice(d);
    }
    out
}

fn size_of(v: &Value) -> Option<Size> {
    let dim = |k: &str| v[k].as_u64().and_then(|n| u16::try_from(n).ok()).filter(|&n| n > 0);
    Some((dim("cols")?, dim("rows")?))
}

/// Publishes the screen as it is (the restored checkpoint, a final screen); the server's
/// `title_changed` names the tab, so the emulator's own title events are dropped.
fn publish(screen: &mut Screen, shared: &SharedState, waker: &Waker) {
    let events: Vec<VtEvent> =
        screen.core.take_events().into_iter().filter(|e| !matches!(e, VtEvent::TitleChanged(_))).collect();
    if !events.is_empty() {
        shared.events.lock().extend(events);
    }
    screen.changed(shared, waker);
}

/// [publish] for a worker that is ending: its last screen goes whatever the UI had read.
fn publish_last(screen: &mut Screen, shared: &SharedState, waker: &Waker) {
    screen.core.take_events().into_iter().filter(|e| !matches!(e, VtEvent::TitleChanged(_))).for_each(|e| shared.events.lock().push(e));
    screen.last(shared, waker);
}

fn gone(err: &str) -> SessionStatus {
    if err.contains("404") {
        SessionStatus::Exited { code: None }
    } else {
        failed(Failure::Failed)
    }
}

fn finish(shared: &SharedState, waker: &Waker, status: SessionStatus) {
    shared.set_status(status);
    waker();
}
