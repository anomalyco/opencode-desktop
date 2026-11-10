//! One attachment to an OpenCode session terminal (a persistent PTY), as Desktop's terminal pane
//! makes it: the HTTP snapshot's checkpoint restores the screen, the socket replays what came
//! after, then input and size frames go out at the pane's size. The server owns the grid: the
//! emulator only takes the canonical size it announces (`attached`, `resized`), and it answers
//! terminal queries itself, so the emulator's replies are dropped. An attachment never
//! reconnects; a failure stays on screen until the pane mounts a new one.
//!
//! The attachment and the pane's size are explicit machines (`connection.rs`:
//! `connection-machine.ts`, `pane-machine.ts`); this worker feeds them and carries out what
//! they return.

use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use opencode_kit::client::Server;
use opencode_kit::persistent_pty::attachment_id;
use rune::Waker;
use rune::machine::Effects;
use serde_json::Value;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::connection::{Connection, Effect, Event, Failure, Size, Sizing};
use crate::core::VtCore;
use crate::pty::{
    COALESCE_BUDGET, COALESCE_MAX_BYTES, RENDER_HOLD_TIMEOUT, SessionStatus, SharedState, TerminalConfig, WorkerCmd,
    apply_worker_cmd, build_core, publish_if_ready,
};
use crate::types::VtEvent;

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

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_persistent_worker(
    server: Server,
    pty_id: String,
    config: TerminalConfig,
    observer: bool,
    cmd_rx: Receiver<WorkerCmd>,
    shared: Arc<SharedState>,
    waker: Waker,
) {
    let Ok(mut core) = build_core(&config, None) else {
        return;
    };
    let mut hold_started: Option<Instant> = None;
    let id = attachment_id();
    let mut conn = Connection::new();
    let mut sizing = Sizing::Unmeasured;
    let mut grid = Grid { cell_px: (config.cell_w_px.max(1), config.cell_h_px.max(1)), size: (config.cols.max(1), config.rows.max(1)) };

    let snapshot = match server.terminal_snapshot(&pty_id) {
        Ok(s) => s,
        Err(e) => return finish(&shared, &waker, gone(&e)),
    };
    grid.resize(&mut core, snapshot.info.size);
    core.feed(DEFAULT_CURSOR);
    core.feed(&snapshot.checkpoint);
    let _ = core.take_pty_writes();
    publish(&mut core, &shared, &waker, &mut hold_started);

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
    set_read_timeout(&ws, Duration::from_millis(3));
    *shared.status.lock() = SessionStatus::Connected;
    waker();

    loop {
        let mut dirty = false;
        let mut events = Vec::new();
        let now = Instant::now();
        loop {
            match cmd_rx.try_recv() {
                Ok(WorkerCmd::Shutdown) | Err(TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    let _ = ws.flush();
                    return;
                }
                Ok(WorkerCmd::Resize { cols, rows, cell_w_px, cell_h_px }) => {
                    grid.cell_px = (cell_w_px.max(1), cell_h_px.max(1));
                    let size = grid.size;
                    grid.resize(&mut core, size);
                    events.extend(sizing.sampled((cols.max(1), rows.max(1)), now).map(Event::SizeChanged));
                    dirty = true;
                }
                Ok(WorkerCmd::SetTheme(theme)) => {
                    core.set_theme(theme);
                    let _ = core.take_pty_writes();
                    dirty = true;
                }
                Ok(WorkerCmd::Interact) => events.push(Event::Interacted),
                Ok(cmd) => {
                    let (mut input, mut resized) = (Vec::new(), None);
                    if !apply_worker_cmd(cmd, &mut core, &shared, &mut input, &mut resized, &mut dirty) {
                        return;
                    }
                    if !input.is_empty() {
                        events.push(Event::Input(input));
                    }
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        events.extend(sizing.due(now).map(Event::SizeChanged));
        for event in events {
            let effects = conn.step(event);
            if let Some(status) = carry_out(effects, &mut core, &mut grid, &mut ws) {
                publish(&mut core, &shared, &waker, &mut hold_started);
                return finish(&shared, &waker, status);
            }
        }

        let burst = Instant::now();
        let mut fed = 0usize;
        loop {
            let event = match ws.read() {
                Ok(Message::Binary(bytes)) if conn.accepts_output() => {
                    fed += bytes.len();
                    core.feed(&bytes);
                    let _ = core.take_pty_writes();
                    dirty = true;
                    if fed >= COALESCE_MAX_BYTES || burst.elapsed() >= COALESCE_BUDGET {
                        break;
                    }
                    continue;
                }
                Ok(Message::Text(text)) => {
                    let Ok(msg) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                    if msg["type"].as_str() == Some("title_changed") {
                        let title = msg["title"].as_str().unwrap_or_default().to_owned();
                        shared.events.lock().push(VtEvent::TitleChanged(title));
                        dirty = true;
                        continue;
                    }
                    let Some(event) = message(&msg, &id) else { continue };
                    event
                }
                // The server closes with 4404 when the terminal is gone: it exited.
                Ok(Message::Close(frame)) if frame.as_ref().is_some_and(|f| u16::from(f.code) == 4404) => Event::Exited { code: None },
                Ok(Message::Close(_)) => Event::SocketClosed,
                Ok(_) => continue,
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) =>
                {
                    break;
                }
                // A dropped connection (the server went away) is a close, as Chromium reports it:
                // Desktop says "Terminal disconnected".
                Err(_) => Event::SocketClosed,
            };
            let ended = matches!(event, Event::Exited { .. } | Event::SocketClosed);
            let effects = conn.step(event);
            dirty = true;
            if let Some(status) = carry_out(effects, &mut core, &mut grid, &mut ws) {
                publish(&mut core, &shared, &waker, &mut hold_started);
                return finish(&shared, &waker, status);
            }
            if let (true, Connection::Exited { code }) = (ended, &conn) {
                publish(&mut core, &shared, &waker, &mut hold_started);
                return finish(&shared, &waker, SessionStatus::Exited { code: *code });
            }
        }

        if dirty || hold_started.is_some_and(|t| t.elapsed() >= RENDER_HOLD_TIMEOUT) {
            publish(&mut core, &shared, &waker, &mut hold_started);
        }
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

/// Carries out the connection's effects; returns the status a failure ends the worker with.
fn carry_out(
    effects: Effects<Effect>,
    core: &mut VtCore,
    grid: &mut Grid,
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
) -> Option<SessionStatus> {
    for effect in effects {
        match effect {
            Effect::Resize { size, checkpoint, reset } => {
                grid.resize(core, size);
                if reset {
                    core.feed(b"\x1bc");
                    core.feed(DEFAULT_CURSOR);
                }
                if let Some(checkpoint) = checkpoint {
                    core.feed(&checkpoint);
                }
                let _ = core.take_pty_writes();
            }
            Effect::Send { size, input } => {
                if ws.send(Message::Binary(frame(size, input.as_deref()).into())).is_err() {
                    return Some(failed(Failure::Disconnected));
                }
            }
            Effect::Fail(failure) => return Some(failed(failure)),
        }
    }
    None
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

/// Publishes the screen; the server's `title_changed` names the tab, so the emulator's own
/// title events are dropped.
fn publish(core: &mut VtCore, shared: &SharedState, waker: &Waker, hold: &mut Option<Instant>) {
    let events: Vec<VtEvent> =
        core.take_events().into_iter().filter(|e| !matches!(e, VtEvent::TitleChanged(_))).collect();
    if !events.is_empty() {
        shared.events.lock().extend(events);
    }
    publish_if_ready(core, shared, waker, hold);
}

fn gone(err: &str) -> SessionStatus {
    if err.contains("404") {
        SessionStatus::Exited { code: None }
    } else {
        failed(Failure::Failed)
    }
}

fn finish(shared: &SharedState, waker: &Waker, status: SessionStatus) {
    *shared.status.lock() = status;
    waker();
}

fn set_read_timeout(ws: &WebSocket<MaybeTlsStream<TcpStream>>, dur: Duration) {
    match ws.get_ref() {
        MaybeTlsStream::Plain(tcp) => {
            let _ = tcp.set_read_timeout(Some(dur));
        }
        MaybeTlsStream::Rustls(tls) => {
            let _ = tls.get_ref().set_read_timeout(Some(dur));
        }
        _ => {}
    }
}
