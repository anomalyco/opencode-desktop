//! One attachment to an OpenCode session terminal (a persistent PTY), as Desktop's terminal pane
//! makes it (`packages/gui-plugins/src/terminal/connection-machine.ts`): the HTTP snapshot's
//! checkpoint restores the screen, the socket replays what came after, then input and size
//! frames go out at the pane's size. The server owns the grid: the emulator only takes the
//! canonical size it announces (`attached`, `resized`), and it answers terminal queries itself,
//! so the emulator's replies are dropped. An attachment never reconnects; a failure stays on
//! screen until the pane mounts a new one.

use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use opencode_kit::client::Server;
use opencode_kit::persistent_pty::attachment_id;
use rune::Waker;
use serde_json::Value;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::core::VtCore;
use crate::pty::{
    COALESCE_BUDGET, COALESCE_MAX_BYTES, RENDER_HOLD_TIMEOUT, SessionStatus, SharedState, TerminalConfig, WorkerCmd,
    apply_worker_cmd, build_core, publish_if_ready,
};
use crate::types::VtEvent;

/// Desktop waits this long after the pane's last size change before it sends the size.
const SIZE_DEBOUNCE: Duration = Duration::from_millis(100);

/// Desktop opens its terminals with a blinking bar (`cursorStyle: "bar"`); a program's own
/// DECSCUSR in the checkpoint or stream still wins.
const DEFAULT_CURSOR: &[u8] = b"\x1b[5 q";

/// Why an attachment stopped, as the pane words it (`terminal.tsx` `failureText`).
pub const DISCONNECTED: &str = "Terminal disconnected";
pub const CONNECTION_FAILED: &str = "Terminal connection failed";
pub const OUTDATED: &str = "This server is too old for terminals; update OpenCode on it";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Waiting for `attached`; the snapshot already painted the screen.
    Restoring,
    /// Attached: the replay streams until `replay_complete`.
    Replaying,
    /// Input and size frames go out at the pane's size.
    Live,
}

struct Attachment {
    id: String,
    phase: Phase,
    controller: bool,
    /// The pane's grid, once measured.
    pane: Option<(u16, u16)>,
    /// The pane's size not yet sent, and when it last changed.
    pending_size: Option<Instant>,
    /// A click or keystroke before the pane went live, which takes control once it does.
    wants_control: bool,
    held_input: Vec<Vec<u8>>,
    cell_px: (u32, u32),
    grid: (u16, u16),
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
    let mut at = Attachment {
        id: attachment_id(),
        phase: Phase::Restoring,
        controller: false,
        pane: None,
        pending_size: None,
        wants_control: false,
        held_input: Vec::new(),
        cell_px: (config.cell_w_px.max(1), config.cell_h_px.max(1)),
        grid: (config.cols.max(1), config.rows.max(1)),
    };

    let snapshot = match server.terminal_snapshot(&pty_id) {
        Ok(s) => s,
        Err(e) => return finish(&shared, &waker, gone(&e)),
    };
    resize_grid(&mut core, &mut at, snapshot.info.size);
    core.feed(DEFAULT_CURSOR);
    core.feed(&snapshot.checkpoint);
    let _ = core.take_pty_writes();
    publish(&mut core, &shared, &waker, &mut hold_started);

    let url = match server.terminal_connect_url(&pty_id, snapshot.info.tail, &at.id, observer) {
        Ok(u) => u,
        Err(e) => return finish(&shared, &waker, gone(&e)),
    };
    let mut ws = match tungstenite::connect(&url) {
        Ok((socket, _)) => socket,
        Err(_) => return finish(&shared, &waker, SessionStatus::Failed(CONNECTION_FAILED.into())),
    };
    set_read_timeout(&ws, Duration::from_millis(3));
    *shared.status.lock() = SessionStatus::Connected;
    waker();

    loop {
        let mut dirty = false;
        let mut input = Vec::new();
        loop {
            match cmd_rx.try_recv() {
                Ok(WorkerCmd::Shutdown) | Err(TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    let _ = ws.flush();
                    return;
                }
                Ok(WorkerCmd::Resize { cols, rows, cell_w_px, cell_h_px }) => {
                    at.cell_px = (cell_w_px.max(1), cell_h_px.max(1));
                    core.resize(at.grid.0, at.grid.1, at.cell_px.0, at.cell_px.1);
                    let size = (cols.max(1), rows.max(1));
                    if at.pane != Some(size) {
                        at.pane = Some(size);
                        at.pending_size = Some(Instant::now());
                    }
                    dirty = true;
                }
                Ok(WorkerCmd::SetTheme(theme)) => {
                    core.set_theme(theme);
                    let _ = core.take_pty_writes();
                    dirty = true;
                }
                Ok(cmd) => {
                    let mut resized = None;
                    if !apply_worker_cmd(cmd, &mut core, &shared, &mut input, &mut resized, &mut dirty) {
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
            }
        }

        let mut frames = Vec::new();
        if at.phase == Phase::Live
            && let Some(size) = at.pane
            && !at.held_input.is_empty()
        {
            at.pending_size = None;
            frames.extend(std::mem::take(&mut at.held_input).iter().map(|data| frame(size, Some(data))));
        }
        if !input.is_empty() {
            match (at.phase, at.pane) {
                (Phase::Live, Some(size)) => frames.push(frame(size, Some(&input))),
                _ => {
                    at.wants_control = true;
                    at.held_input.push(input);
                }
            }
        }
        if at.phase == Phase::Live
            && let (Some(size), Some(t0)) = (at.pane, at.pending_size)
            && t0.elapsed() >= SIZE_DEBOUNCE
        {
            at.pending_size = None;
            if at.controller || at.wants_control {
                frames.push(frame(size, None));
            }
        }
        for f in frames {
            if ws.send(Message::Binary(f.into())).is_err() {
                return finish(&shared, &waker, SessionStatus::Failed(DISCONNECTED.into()));
            }
        }

        let burst = Instant::now();
        let mut fed = 0usize;
        loop {
            match ws.read() {
                Ok(Message::Binary(bytes)) => {
                    fed += bytes.len();
                    core.feed(&bytes);
                    let _ = core.take_pty_writes();
                    dirty = true;
                    if fed >= COALESCE_MAX_BYTES || burst.elapsed() >= COALESCE_BUDGET {
                        break;
                    }
                }
                Ok(Message::Text(text)) => {
                    let Ok(msg) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                    match on_message(&msg, &mut core, &mut at, &shared) {
                        Step::Continue(changed) => dirty |= changed,
                        Step::Send(frames) => {
                            dirty = true;
                            for f in frames {
                                let _ = ws.send(Message::Binary(f.into()));
                            }
                        }
                        Step::Stop(status) => {
                            publish(&mut core, &shared, &waker, &mut hold_started);
                            return finish(&shared, &waker, status);
                        }
                    }
                }
                Ok(Message::Close(frame)) => {
                    publish(&mut core, &shared, &waker, &mut hold_started);
                    let code = frame.as_ref().map_or(1000, |f| u16::from(f.code));
                    let status = if code == 4404 {
                        SessionStatus::Exited { code: None }
                    } else {
                        SessionStatus::Failed(DISCONNECTED.into())
                    };
                    return finish(&shared, &waker, status);
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) =>
                {
                    break;
                }
                Err(_) => {
                    publish(&mut core, &shared, &waker, &mut hold_started);
                    return finish(&shared, &waker, SessionStatus::Failed(DISCONNECTED.into()));
                }
            }
        }

        if dirty || hold_started.is_some_and(|t| t.elapsed() >= RENDER_HOLD_TIMEOUT) {
            publish(&mut core, &shared, &waker, &mut hold_started);
        }
    }
}

enum Step {
    Continue(bool),
    Send(Vec<Vec<u8>>),
    Stop(SessionStatus),
}

fn on_message(msg: &Value, core: &mut VtCore, at: &mut Attachment, shared: &SharedState) -> Step {
    match msg["type"].as_str().unwrap_or_default() {
        "attached" if at.phase == Phase::Restoring => {
            if msg["inputProtocol"].as_u64() != Some(1) {
                return Step::Stop(SessionStatus::Failed(OUTDATED.into()));
            }
            at.phase = Phase::Replaying;
            at.controller = msg["role"].as_str() == Some("controller");
            if let Some(size) = size_of(&msg["info"]["size"]) {
                resize_grid(core, at, size);
            }
            Step::Continue(true)
        }
        "replay_complete" if at.phase == Phase::Replaying => {
            at.phase = Phase::Live;
            let Some(size) = at.pane else { return Step::Continue(false) };
            at.pending_size = None;
            let held = std::mem::take(&mut at.held_input);
            if !held.is_empty() {
                return Step::Send(held.iter().map(|data| frame(size, Some(data))).collect());
            }
            if at.controller || at.wants_control {
                return Step::Send(vec![frame(size, None)]);
            }
            Step::Continue(false)
        }
        "resized" if at.phase != Phase::Restoring => {
            let Some(size) = size_of(msg) else { return Step::Continue(false) };
            resize_grid(core, at, size);
            core.feed(b"\x1bc");
            core.feed(DEFAULT_CURSOR);
            if let Some(checkpoint) = msg["checkpoint"].as_str().and_then(opencode_kit::model::base64_decode) {
                core.feed(&checkpoint);
            }
            let _ = core.take_pty_writes();
            Step::Continue(true)
        }
        "controller_changed" if at.phase != Phase::Restoring => {
            let controller = msg["attachmentID"].as_str() == Some(at.id.as_str());
            if controller == at.controller {
                return Step::Continue(false);
            }
            at.controller = controller;
            match (controller, at.phase, at.pane) {
                (true, Phase::Live, Some(size)) => Step::Send(vec![frame(size, None)]),
                _ => Step::Continue(false),
            }
        }
        "title_changed" => {
            let title = msg["title"].as_str().unwrap_or_default().to_owned();
            shared.events.lock().push(VtEvent::TitleChanged(title));
            Step::Continue(true)
        }
        "exited" => Step::Stop(SessionStatus::Exited {
            code: msg["exitCode"].as_u64().and_then(|c| u32::try_from(c).ok()),
        }),
        _ => Step::Continue(false),
    }
}

/// A frame for the server: the pane's size alone takes control; with `data`, it types.
fn frame(size: (u16, u16), data: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + data.map_or(0, <[u8]>::len));
    out.push(u8::from(data.is_some()));
    out.extend_from_slice(&size.0.to_be_bytes());
    out.extend_from_slice(&size.1.to_be_bytes());
    if let Some(d) = data {
        out.extend_from_slice(d);
    }
    out
}

fn size_of(v: &Value) -> Option<(u16, u16)> {
    let dim = |k: &str| v[k].as_u64().and_then(|n| u16::try_from(n).ok()).filter(|&n| n > 0);
    Some((dim("cols")?, dim("rows")?))
}

fn resize_grid(core: &mut VtCore, at: &mut Attachment, size: (u16, u16)) {
    at.grid = size;
    core.resize(size.0, size.1, at.cell_px.0, at.cell_px.1);
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
        SessionStatus::Failed(CONNECTION_FAILED.into())
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
